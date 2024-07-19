use std::fs;
use std::io::Read;
use std::sync::Arc;

use anyhow::{bail, Context};
use bittorrent_client_rs::logging::setup_logger;
use bittorrent_client_rs::stats::Stats;
use bittorrent_client_rs::tracker::TrackerRequest;
use bittorrent_client_rs::util::generate_peer_id;
use bittorrent_client_rs::{args, handle_peer, parser, torrent_meta::TorrentMeta, tracker, try_into, StorageManager};
use bittorrent_client_rs::{Error, Result};
use bittorrent_client_rs::{Torrent, TorrentSharedState};
use clap::Parser;
use tokio::sync::mpsc::{self, unbounded_channel};
use tokio::sync::{oneshot, RwLock};
use tokio::task::JoinSet;
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

// TODO: implement DHT?
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

        let (peer_event_tx, peer_event_rx) = unbounded_channel();

        let (new_peer_tx, new_peer_rx) = unbounded_channel();

        let torrent_meta = TorrentMeta::new(
            info_hash,
            try_into!(piece_length, usize)?,
            try_into!(length, usize)?,
            pieces_total,
        )
        .context("TorrentMeta")?;

        let mut base_path = fs::canonicalize(".")?;
        // TODO: make this a CLI arg
        base_path.push("downloads");

        // Multi-file mode: add directory name
        if meta_info.info.files.is_some() {
            base_path.push(&meta_info.info.name);
        }

        let (storage_tx, storage_rx) = mpsc::channel(200);
        let mut storage_manager = StorageManager::new(&mut meta_info.info, &base_path, length)
            .context("error while creating a storage manager")?;

        let splitted_piece_hashes = meta_info
            .info
            .pieces
            .chunks(20)
            .map(|item| try_into!(item, [u8; 20]))
            .collect::<Result<Vec<[u8; 20]>>>()?;

        // TODO: update download stats if we continue downloading from a certain piece
        let starting_piece = match storage_manager
            // TODO: make this a CLI arg
            .checksum_verification(&splitted_piece_hashes)
            .await
            .context("error while doing initial checksums verification")?
        {
            Some(piece_idx) => piece_idx,
            None => {
                tracing::info!("All files were downloaded already, exiting...");
                return Ok(());
            }
        };

        dbg!(starting_piece);

        let torrent_state = Arc::new(RwLock::new(TorrentSharedState::new(pieces_total, starting_piece)?));

        let (torrent_task, storage_task) = {
            let (hash_check_tx, hash_check_rx) = mpsc::channel(200);

            let storage_handle =
                tokio::spawn(async move { storage_manager.listen_for_blocks(storage_rx, hash_check_tx).await });

            let torrent_state = torrent_state.clone();

            let torrent_meta = torrent_meta.clone();
            let torrent_handle = tokio::spawn(async move {
                // TODO: this struct will most likely need peer_id in order to finish downloading
                // torrents and send keep alives
                let mut torrent = Torrent::new(
                    torrent_meta,
                    torrent_state,
                    splitted_piece_hashes,
                    peer_event_rx,
                    new_peer_rx,
                    storage_tx,
                    hash_check_rx,
                );

                torrent.handle().await?;

                Ok::<(), anyhow::Error>(())
            });

            (torrent_handle, storage_handle)
        };

        let (stats_cancel_tx, stats_cancel_rx) = oneshot::channel();
        let stats_task = {
            let length = try_into!(length, usize).context("starting a stats task")?;
            let mut stats = Stats::new(0, length, length);
            tokio::spawn(async move { stats.collect_stats(stats_cancel_rx).await })
        };

        let mut peer_handler_tasks = JoinSet::new();
        for peer_address in peers.into_iter() {
            let peer_state = torrent_state.clone();
            let peer_event_tx = peer_event_tx.clone();

            peer_handler_tasks.spawn(handle_peer(
                peer_address,
                torrent_meta.clone(),
                peer_id,
                peer_state,
                peer_event_tx,
                new_peer_tx.clone(),
            ));
        }

        drop(new_peer_tx);
        drop(peer_event_tx);

        while let Some(peer_handler_result) = peer_handler_tasks.join_next().await {
            if let Err(e) = peer_handler_result.context("peer handler task")? {
                tracing::error!("an error happened in the peer: {:#}", e);
            }
        }

        torrent_task.await.context("torrent task")??;

        storage_task.await.context("storage task")??;

        stats_cancel_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("bug: stats collector exited before cancellation?"))?;

        stats_task.await.context("stats task")?;

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
