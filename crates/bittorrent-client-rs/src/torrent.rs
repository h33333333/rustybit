use crate::{
    handle_peer,
    state::{
        event::{PeerEvent, TorrentManagerReq},
        torrent::PieceState,
    },
    stats::{Stats, DOWNLOADED_BYTES, DOWNLOADED_PIECES},
    storage::{FileInfo, StorageOp},
    torrent_meta::TorrentMeta,
    tracker, Error, FileStorage, PieceHashVerifier, Result, Storage, StorageManager, Torrent, TorrentFileMetadata,
    TorrentSharedState,
};
use std::{
    net::SocketAddrV4,
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
};

use anyhow::Context;
use leechy_dht::DhtRequester;
use tokio::{
    sync::{
        mpsc::{self, unbounded_channel},
        oneshot, RwLock,
    },
    task::{JoinHandle, JoinSet},
};
use tracing::{instrument, Instrument as _};
use url::Url;

use crate::{parser::MetaInfo, util::generate_peer_id};

#[derive(Debug)]
pub struct TorrentManager {
    next_torrent_idx: usize,
    http_client: reqwest::Client,
    base_download_path: PathBuf,
    peer_id: String,
}

impl TorrentManager {
    pub fn new(download_path: PathBuf) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .gzip(true)
            .build()
            .context("building reqwest client")?;
        Ok(TorrentManager {
            next_torrent_idx: 0,
            http_client: client,
            base_download_path: download_path,
            peer_id: generate_peer_id(),
        })
    }

    #[instrument(err(level = "debug"), skip_all, fields(torrent_id = self.next_torrent_idx))]
    pub async fn add_new_torrent<'a>(
        &mut self,
        mut meta_info: MetaInfo<'a>,
    ) -> anyhow::Result<Option<JoinHandle<anyhow::Result<()>>>> {
        let splitted_piece_hashes = meta_info
            .info
            .pieces
            .chunks(20)
            .map(|item| try_into!(item, [u8; 20]))
            .collect::<Result<Vec<[u8; 20]>>>()?;

        let file_metadata = {
            let mut base_path = self.base_download_path.clone();
            // Multi-file mode: add directory name
            if meta_info.info.files.is_some() {
                base_path.push(&*meta_info.info.name);
            }
            TorrentFileMetadata::new(&mut meta_info.info, &base_path).context("bug: bad metadata")?
        };

        let length = meta_info
            .info
            .files
            .as_ref()
            .map_or_else(
                || {
                    meta_info
                        .info
                        .length
                        .map(|len| try_into!(len, usize).ok())
                        .ok_or(Error::InternalError(
                            "Malformed torrent file: both 'files' and 'length' fields are missing",
                        ))
                },
                |files| {
                    Ok(try_into!(
                        files.iter().fold(0, |mut acc, file| {
                            acc += file.length;
                            acc
                        }),
                        usize
                    )
                    .ok())
                },
            )?
            .context("converting total torrent length to usize")?;

        let mut storage =
            FileStorage::new(&file_metadata.file_infos).context("error while creating a file-based storage")?;
        let mut piece_hash_verifier = PieceHashVerifier::new(meta_info.info.piece_length);

        let (verified_pieces, downloaded, left, piece_states) = match self.do_initial_checksum_verification(
            &mut piece_hash_verifier,
            &mut storage,
            &file_metadata.file_infos,
            &splitted_piece_hashes,
            length,
            meta_info.info.piece_length,
        )? {
            Some((verified_pieces, downloaded, left, piece_states)) => {
                (verified_pieces, downloaded, left, piece_states)
            }
            None => return Ok(None),
        };
        DOWNLOADED_BYTES.store(downloaded, Ordering::Relaxed);
        DOWNLOADED_PIECES.store(verified_pieces, Ordering::Relaxed);

        let info_hash = meta_info.info.hash()?;
        let n_of_pieces = splitted_piece_hashes.len();

        let torrent_meta =
            TorrentMeta::new(info_hash, meta_info.info.piece_length, length, n_of_pieces).context("TorrentMeta")?;

        let piece_length = try_into!(meta_info.info.piece_length, u64)?;
        let (storage_task, storage_tx, hash_check_rx) = spawn_storage_task(
            storage,
            file_metadata,
            piece_length,
            piece_hash_verifier,
            n_of_pieces,
            length,
        )
        .context("spawning a storage manager")?;

        let torrent_state = Arc::new(RwLock::new(TorrentSharedState::new(piece_states, n_of_pieces)?));
        let (torrent_task, peer_event_tx, new_peer_tx) = spawn_torrent_task(
            torrent_meta.clone(),
            torrent_state.clone(),
            splitted_piece_hashes,
            storage_tx,
            hash_check_rx,
        );

        let (stats_task, stats_cancel_tx) = spawn_stats_task(length, downloaded, left);

        let peers = self
            .get_peers_from_tracker(
                try_into!(length, u64)?,
                try_into!(downloaded, u64)?,
                &meta_info.announce,
                info_hash,
            )
            .await?;
        let peer_id: [u8; 20] = self.peer_id.as_bytes().try_into()?;

        let root_handle = tokio::spawn(async move {
            let (peer_manager_cancel_tx, mut peer_manager_cancel_rx) = oneshot::channel::<()>();
            let (peer_queue_tx, mut peer_queue_rx) = mpsc::channel(10);
            let peer_manager_task = tokio::spawn(
                async move {
                    let spawn_peer = move |set: &mut JoinSet<anyhow::Result<()>>, peer_addr: SocketAddrV4| {
                        let peer_state = torrent_state.clone();
                        let peer_event_tx = peer_event_tx.clone();

                        set.spawn(handle_peer(
                            peer_addr,
                            torrent_meta.clone(),
                            peer_id,
                            peer_state,
                            peer_event_tx,
                            new_peer_tx.clone(),
                        ));
                    };

                    let mut n_of_peers = 0;
                    let mut peer_handler_tasks = JoinSet::new();
                    // Process all initial peers
                    for peer_address in peers.into_iter() {
                        spawn_peer(&mut peer_handler_tasks, peer_address);
                        n_of_peers += 1;
                    }

                    loop {
                        tokio::select! {
                            Some(next_peer) = peer_queue_rx.recv(), if n_of_peers < 90 => {
                                spawn_peer(&mut peer_handler_tasks, next_peer);
                                n_of_peers +=1;
                            },
                            Some(peer_handler_result) = peer_handler_tasks.join_next() => {
                                    n_of_peers -= 1;
                                    if let Err(e) = peer_handler_result.context("peer handler task")? {
                                        tracing::debug!("an error happened in a peer: {:#}", e);
                                    }
                            }
                            _ = &mut peer_manager_cancel_rx => {
                                tracing::debug!("cancellation requested, aborting peer tasks");
                                peer_handler_tasks.abort_all();
                                // Wait for all tasks to exit
                                while let Some(_) = peer_handler_tasks.join_next().await {}
                                tracing::debug!("all peers were aborted successfully, shutting down the manager");
                                break;
                            }
                            else => {
                                tracing::debug!("peer queue sender and all peers exited, shutting down the peer manager");
                                break;
                            }
                        }
                    }

                    Ok::<(), anyhow::Error>(())
                }
                .instrument(tracing::error_span!("peer manager task")),
            );

            let (dht_cancel_tx, dht_cancel_rx) = oneshot::channel();

            let mut dht_requester = DhtRequester::new(None, info_hash).context("creating DhtRequester")?;
            let dht_requester_task =
                tokio::spawn(async move { dht_requester.process_dht_nodes(dht_cancel_rx, peer_queue_tx).await });

            torrent_task.await.context("torrent task")??;

            // Finish all other tasks
            let _ = dht_cancel_tx.send(());
            let _ = peer_manager_cancel_tx.send(());
            peer_manager_task
                .await
                .context("peer manager task panicked?")?
                .context("peer manager task")?;
            dht_requester_task.await.context("dht requester task")??;
            storage_task.await.context("storage task")??;
            stats_cancel_tx
                .send(())
                .map_err(|_| anyhow::anyhow!("bug: stats collector exited before cancellation?"))?;
            stats_task.await.context("stats task")?;

            Ok::<(), anyhow::Error>(())
        });

        Ok(Some(root_handle))
    }

    async fn get_peers_from_tracker(
        &self,
        total_length: u64,
        downloaded: u64,
        annouce_url: &str,
        info_hash: [u8; 20],
    ) -> anyhow::Result<Vec<SocketAddrV4>> {
        let request = tracker::TrackerRequest::new(
            &self.peer_id,
            info_hash,
            &[None, Some(downloaded), Some(total_length)],
            Some(tracker::EventType::Started),
        );

        let tracker_announce_url = params(annouce_url, request)?;

        let response = self
            .http_client
            .request(reqwest::Method::GET, tracker_announce_url)
            .header("User-Agent", "RustyBitTorrent")
            .send()
            .await
            .context("sending request to the tracker")?;

        let resp: tracker::TrackerResponse = serde_bencode::from_bytes(
            &response
                .bytes()
                .await
                .context("unable to get the tracker's response body")?,
        )
        .context("error while parsing the tracker's response")?;

        match resp.get_peers() {
            Some(peers) => Ok(peers?),
            None => anyhow::bail!("peers are missing in the tracker response"),
        }
    }

    fn do_initial_checksum_verification(
        &self,
        verifier: &mut PieceHashVerifier,
        storage: &mut dyn Storage,
        file_infos: &[FileInfo],
        piece_hashes: &[[u8; 20]],
        total_length: usize,
        piece_length: usize,
    ) -> anyhow::Result<Option<(usize, usize, usize, Vec<PieceState>)>> {
        tracing::info!("Starting initial checksum verification");
        let (verified_pieces, piece_states) = verifier
            .check_all_pieces(storage, file_infos, piece_hashes, total_length)
            .context("error while doing initial checksums verification")?;

        if verified_pieces == piece_hashes.len() {
            tracing::info!(%verified_pieces, "Finished initial checksum verification: already downloaded all pieces, exiting...");
            return Ok(None);
        } else {
            tracing::info!(%verified_pieces, "Finished initial checksum verification: already downloaded and verified some of the pieces");
        }

        let (downloaded, left) = {
            let downloaded_bytes = verified_pieces * piece_length;
            let bytes_left = total_length - downloaded_bytes;
            (downloaded_bytes, bytes_left)
        };

        Ok(Some((verified_pieces, downloaded, left, piece_states)))
    }
}

fn spawn_storage_task(
    mut storage: FileStorage,
    metadata: TorrentFileMetadata,
    piece_length: u64,
    hash_verifier: PieceHashVerifier,
    n_of_pieces: usize,
    total_length: usize,
) -> anyhow::Result<(
    JoinHandle<anyhow::Result<()>>,
    mpsc::Sender<StorageOp>,
    mpsc::Receiver<(SocketAddrV4, u32, bool)>,
)> {
    let (storage_tx, storage_rx) = mpsc::channel(200);
    let (hash_check_tx, hash_check_rx) = mpsc::channel(200);
    let storage_mgr_task = tokio::task::spawn(async move {
        let mut storage_manager = StorageManager::new(
            &mut storage as &mut dyn Storage,
            metadata,
            piece_length,
            hash_verifier,
            n_of_pieces,
            total_length,
        )
        .context("error while creating a storage manager")?;
        storage_manager.listen_for_blocks(storage_rx, hash_check_tx).await
    });
    Ok((storage_mgr_task, storage_tx, hash_check_rx))
}

fn spawn_torrent_task(
    meta: TorrentMeta,
    state: Arc<RwLock<TorrentSharedState>>,
    piece_hashes: Vec<[u8; 20]>,
    storage_tx: mpsc::Sender<StorageOp>,
    hash_check_rx: mpsc::Receiver<(SocketAddrV4, u32, bool)>,
) -> (
    JoinHandle<anyhow::Result<()>>,
    mpsc::UnboundedSender<(SocketAddrV4, PeerEvent)>,
    mpsc::UnboundedSender<(SocketAddrV4, mpsc::Sender<TorrentManagerReq>)>,
) {
    let (peer_event_tx, peer_event_rx) = unbounded_channel();
    let (new_peer_tx, new_peer_rx) = unbounded_channel();
    let torrent_task = tokio::spawn(async move {
        // TODO: this struct will most likely need peer_id in order to finish downloading
        // torrents and send keep alives
        let mut torrent = Torrent::new(
            meta,
            state,
            piece_hashes,
            peer_event_rx,
            new_peer_rx,
            storage_tx,
            hash_check_rx,
        );
        torrent.handle().await?;
        Ok::<(), anyhow::Error>(())
    });
    (torrent_task, peer_event_tx, new_peer_tx)
}

fn spawn_stats_task(length: usize, downloaded: usize, left: usize) -> (JoinHandle<()>, oneshot::Sender<()>) {
    let (stats_cancel_tx, stats_cancel_rx) = oneshot::channel();
    let stats_task = {
        let mut stats = Stats::new(downloaded, left, length);
        tokio::spawn(async move { stats.collect_stats(stats_cancel_rx).await })
    };
    (stats_task, stats_cancel_tx)
}

fn params(url: &str, request: tracker::TrackerRequest<'_>) -> anyhow::Result<Url> {
    let mut url = Url::parse(url).context("tracker announce URL parsing")?;

    let mut query = request.into_query_params()?;

    // NOTE: Some trackers include additional query params in the announce URL.
    // Some even require them to be the first ones in the query, so we have
    // to reorder things a bit in order to be sure that this supports as much
    // torrent trackers as possible
    if let Some(existing_query) = url.query() {
        query.insert_str(0, existing_query);
        query.insert(existing_query.len(), '&');
    }

    url.set_query(Some(&query));

    Ok(url)
}
