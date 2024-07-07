use std::fs;
use std::io::Read;
use std::sync::Arc;

use anyhow::{bail, Context};
use bittorrent_client_rs::logging::setup_logger;
use bittorrent_client_rs::stats::stats;
use bittorrent_client_rs::tracker::TrackerRequest;
use bittorrent_client_rs::util::generate_peer_id;
use bittorrent_client_rs::{args, handle_peer, parser, torrent_meta::TorrentMeta, tracker, try_into, StorageManager};
use bittorrent_client_rs::{Error, Result};
use bittorrent_client_rs::{Torrent, TorrentSharedState};
use clap::Parser;
use tokio::sync::mpsc::{self, unbounded_channel};
use tokio::sync::{broadcast, oneshot, RwLock};
use url::Url;

fn params(url: &str, request: TrackerRequest<'_>) -> anyhow::Result<Url> {
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

// TODO: use exponential moving average to estimate speed and download time
// FIXME: stats printer

// TODO: global code refactoring/restructuring

#[tokio::main]
#[tracing::instrument(err)]
async fn main() -> anyhow::Result<()> {
    setup_logger();

    let args = args::Arguments::parse();
    let torrent_file = read_torrent_file(&args.file)?;
    let mut meta_info: parser::MetaInfo = serde_bencode::from_bytes(&torrent_file)?;
    let peer_id = generate_peer_id();

    let length = meta_info.info.files.as_ref().map_or_else(
        || {
            meta_info.info.length.ok_or(Error::InternalError(
                "Malformed torrent file: both 'files' and 'length' fields are missing",
            ))
        },
        |files| {
            Ok(files.iter().fold(0, |mut acc, file| {
                acc += file.length;
                acc
            }))
        },
    )?;

    // TODO: calculate how many bytes we already downloaded
    let request = tracker::TrackerRequest::new(
        &peer_id,
        meta_info.info.hash()?,
        &[None, None, Some(length)],
        Some(tracker::EventType::Started),
    );

    let tracker_announce_url = params(&meta_info.announce, request)?;

    let client = reqwest::Client::builder()
        .gzip(true)
        .build()
        .context("building reqwest client")?;

    let response = client
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

    let peers = match resp.get_peers() {
        Some(peers) => peers?,
        None => bail!("peers are missing in the tracker response"),
    };

    let peer_id = try_into!(peer_id.as_bytes(), [u8; 20])?;
    let root_handle = tokio::spawn(async move {
        let pieces_total = meta_info.info.pieces.len() / 20;
        let piece_length = meta_info.info.piece_length;

        let info_hash = meta_info.info.hash()?;

        let torrent_state = Arc::new(RwLock::new(TorrentSharedState::new(pieces_total)?));

        let (peer_event_tx, peer_event_rx) = unbounded_channel();
        // TODO: capacity
        let (broadcast_tx, _) = broadcast::channel(200);

        let (new_peer_tx, new_peer_rx) = unbounded_channel();

        let (stats_tx, stats_rx) = tokio::sync::mpsc::unbounded_channel();
        let stats_task = tokio::spawn(async move {
            let length = try_into!(length, usize).context("starting a stats task")?;
            stats(0, length, length, stats_rx).await
        });

        let torrent_meta = TorrentMeta::new(
            info_hash,
            try_into!(piece_length, usize)?,
            try_into!(length, usize)?,
            pieces_total,
        )
        .context("TorrentMeta")?;

        let (torrent_task, storage_task) = {
            let mut base_path = fs::canonicalize(".")?;
            // TODO: make this a CLI arg
            base_path.push("downloads");

            // Multi-file mode: add directory name
            if meta_info.info.files.is_some() {
                base_path.push(&meta_info.info.name);
            }

            let (storage_tx, storage_rx) = mpsc::channel(200);
            let mut storage_manager = StorageManager::new(&mut meta_info.info, &base_path)
                .context("error while creating a storage manager")?;

            let (hash_check_tx, hash_check_rx) = mpsc::channel(200);

            let storage_handle =
                tokio::spawn(async move { storage_manager.listen_for_blocks(storage_rx, hash_check_tx).await });

            let torrent_state = torrent_state.clone();
            let broadcast_tx = broadcast_tx.clone();

            let torrent_meta = torrent_meta.clone();
            let torrent_handle = tokio::spawn(async move {
                let splitted_piece_hashes: Result<_> = meta_info
                    .info
                    .pieces
                    .chunks(20)
                    .map(|item| try_into!(item, [u8; 20]))
                    .collect();

                // TODO: this struct will most likely need peer_id in order to finish downloading
                // torrents and send keep alives
                let mut torrent = Torrent::new(
                    torrent_meta,
                    torrent_state,
                    splitted_piece_hashes?,
                    peer_event_rx,
                    broadcast_tx,
                    new_peer_rx,
                    stats_tx,
                    storage_tx,
                    hash_check_rx,
                );

                torrent.handle().await?;

                Ok::<(), anyhow::Error>(())
            });

            (torrent_handle, storage_handle)
        };

        let mut handles_with_cancel = Vec::with_capacity(peers.len());
        for peer_address in peers.into_iter() {
            let peer_broadcast_rx = broadcast_tx.subscribe();
            let peer_state = torrent_state.clone();
            let peer_event_tx = peer_event_tx.clone();
            let (cancellation_tx, cancellation_rx) = oneshot::channel();

            let task_handle = tokio::spawn(handle_peer(
                peer_address,
                torrent_meta.clone(),
                peer_id,
                peer_state,
                peer_broadcast_rx,
                peer_event_tx,
                cancellation_rx,
            ));

            handles_with_cancel.push((task_handle, cancellation_tx, peer_address));
        }

        drop(peer_event_tx);

        for (handle, cancellation_tx, peer_addr) in handles_with_cancel {
            let connection_result = handle.await.context("peer connection task")?;
            if let Ok(peer_handler_task) = connection_result {
                if new_peer_tx.send((peer_addr, cancellation_tx)).is_ok() {
                    peer_handler_task.await.context("peer handler task")??;
                };
            } else {
                tracing::trace!(%peer_addr, "Peer connection failed: {}", connection_result.unwrap_err());
            }
        }

        drop(new_peer_tx);

        torrent_task.await.context("torrent task")??;

        storage_task.await.context("storage task")??;

        stats_task.await.context("stats task")??;

        Ok::<(), anyhow::Error>(())
    });

    root_handle
        .await
        .context("failed to execute the root task")?
        .context("error while executing the root task")
}

fn read_torrent_file(path: &str) -> Result<Vec<u8>> {
    let mut buffer = vec![];
    let mut file = fs::File::open(path)?;
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}
