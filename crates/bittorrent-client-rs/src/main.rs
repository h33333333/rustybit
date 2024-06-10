use std::borrow::Cow;
use std::fs;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use bittorrent_client_rs::logging::setup_logger;
use bittorrent_client_rs::stats::stats;
use bittorrent_client_rs::tracker::TrackerRequest;
use bittorrent_client_rs::util::generate_peer_id;
use bittorrent_client_rs::{args, parser, tracker, try_into, Peer, TorrentMode};
use bittorrent_client_rs::{Error, Result};
use bittorrent_client_rs::{Torrent, TorrentSharedState};
use bittorrent_peer_protocol::{Encode, Handshake};
use clap::Parser;
use tokio::io::BufReader;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::{broadcast, RwLock};
use tracing::Instrument;
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

// TODO: send KeepAlive to peers

// TODO: torrents can stall if a piece had failed the checksum check and we try to get it from the same peer over and
// over, receiving the same bad piece

// TODO: download speed is slow (~0.7 MiB using 1 GiB ethernet). How can I make it at least 5-10 times faster?

// TODO: add normal logging/error handling (use anyhow?)

// TODO: global code refactoring/restructuring

// TODO: improve shutdown mechanisms


#[tokio::main]
#[tracing::instrument(err)]
async fn main() -> anyhow::Result<()> {
    setup_logger();

    let args = args::Arguments::parse();
    let torrent_file = read_torrent_file(&args.file)?;
    let meta_info: parser::MetaInfo = serde_bencode::from_bytes(&torrent_file)?;
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

    let mut handles = vec![];

    let root_handle = tokio::spawn(async move {
        // Arc is needed to share a single instance between all futures
        let handshake_message = Arc::new(Handshake {
            pstr: Cow::Borrowed("BitTorrent protocol"),
            extension_bytes: 0,
            info_hash: meta_info.info.hash()?,
            peer_id: Cow::Owned(peer_id.as_bytes().to_vec()),
        });

        let pieces_num = meta_info.info.pieces.len() / 20;
        let piece_length = meta_info.info.piece_length;

        // TODO: conversions
        let last_piece_size = if (length / meta_info.info.piece_length) as usize == pieces_num {
            meta_info.info.piece_length as usize
        } else {
            (length - (length / meta_info.info.piece_length) * meta_info.info.piece_length) as usize
        };

        let info_hash = meta_info.info.hash()?;

        let torrent_state = Arc::new(RwLock::new(TorrentSharedState::new(pieces_num)?));

        let (peer_event_tx, peer_event_rx) = unbounded_channel();
        // TODO: capacity
        let (broadcast_tx, _) = broadcast::channel(30);

        let (new_peer_tx, new_peer_rx) = unbounded_channel();

        let (stats_tx, stats_rx) = tokio::sync::mpsc::unbounded_channel();
        let stats_task = tokio::spawn(async move {
            let length = try_into!(length, usize).context("starting a stats task")?;
            stats(0, length, length, stats_rx).await
        });

        let torrent_task = {
            let mut base_path = fs::canonicalize(".")?;
            // TODO: make this a CLI arg
            base_path.push("downloads");

            // Multi-file mode: add directory name
            if meta_info.info.files.is_some() {
                base_path.push(&meta_info.info.name);
            }

            let torrent_state = torrent_state.clone();
            let broadcast_tx = broadcast_tx.clone();

            tokio::spawn(async move {
                let mode = TorrentMode::new(&meta_info.info, &base_path).await?;

                let split_piece_hashes: Result<_> = meta_info
                    .info
                    .pieces
                    .chunks(20)
                    .map(|item| try_into!(item, [u8; 20]))
                    .collect();

                // TODO: this struct will most likely need peer_id in order to finish downloading
                // torrents and send keep alives
                let mut torrent = Torrent::new(
                    torrent_state,
                    info_hash,
                    mode,
                    split_piece_hashes?,
                    piece_length,
                    peer_event_rx,
                    broadcast_tx,
                    new_peer_rx,
                    stats_tx,
                );

                torrent.handle().await?;

                Ok::<(), anyhow::Error>(())
            })
        };

        let mut peer_connect_tasks = tokio::task::JoinSet::new();
        for peer_address in peers {
            let peer_broadcast_rx = broadcast_tx.subscribe();
            let peer_state = torrent_state.clone();
            let peer_event_tx = peer_event_tx.clone();
            let new_peer_tx = new_peer_tx.clone();
            let peer_handshake = handshake_message.clone();

            peer_connect_tasks.spawn(
                async move {
                    tracing::trace!("Trying a new peer");

                    let socket = match tokio::time::timeout(
                        Duration::from_secs(10),
                        tokio::net::TcpStream::connect(peer_address),
                    )
                    .await
                    {
                        Ok(ok) => ok,
                        Err(e) => {
                            tracing::debug!(
                                error = %e,
                                "Peer connection timeout"
                            );
                            return Ok(None);
                        }
                    };

                    let mut socket = match socket {
                        Ok(sock) => sock,
                        Err(e) => {
                            tracing::debug!(
                                error = %e,
                                "Error while establishing connection"
                            );
                            return Ok(None);
                        }
                    };

                    tracing::info!("Connected");

                    peer_handshake.encode(&mut socket).await?;

                    let peer_ip = peer_address.ip().to_owned();

                    let (mut peer, peer_tx) = Peer::new(
                        BufReader::new(socket),
                        peer_state,
                        peer_broadcast_rx,
                        peer_event_tx,
                        peer_ip,
                        Some(info_hash),
                        (piece_length as usize, last_piece_size),
                    );

                    new_peer_tx
                        .send((peer_ip, peer_tx))
                        .context("failed sending new peer's event sender")?;

                    Ok::<_, anyhow::Error>(Some(tokio::spawn(async move { peer.handle().await })))
                }
                .instrument(tracing::info_span!("Peer connect task", addr = ?peer_address)),
            );
        }

        while let Some(result) = peer_connect_tasks.join_next().await {
            if let Some(handle) = result
                .context("a peer connect task has failed to execute")?
                .context("establishing connection with peer failed")?
            {
                handles.push(handle);
            }
        }

        drop(new_peer_tx);
        drop(peer_event_tx);

        for handle in handles {
            handle.await.context("error in peer handler")??;
        }

        torrent_task.await.context("error in torrent handler")??;

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
