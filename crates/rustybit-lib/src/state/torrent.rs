use std::collections::{HashMap, VecDeque};
use std::net::SocketAddrV4;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use bittorrent_peer_protocol::Block;
use bitvec::order::Msb0;
use bitvec::slice::BitSlice;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::RwLock;
use tokio::time;

use super::event::{PeerEvent, TorrentManagerReq};
use crate::stats::{DOWNLOADED_BYTES, DOWNLOADED_PIECES, NUMBER_OF_PEERS};
use crate::storage::StorageOp;
use crate::torrent_meta::TorrentMeta;
use crate::util::piece_size_from_idx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PieceState {
    Queued,
    Downloading { peer: SocketAddrV4, start: Instant },
    Downloaded,
    Verified,
}

/// State that each peer keeps a reference to
#[derive(Debug)]
pub struct TorrentSharedState {
    peer_download_stats: HashMap<SocketAddrV4, (f64, f64)>,
    pieces: Vec<PieceState>,
    piece_download_progress: HashMap<u32, usize>,
    cancellation_req_queue: VecDeque<(SocketAddrV4, u32)>,
}

impl TorrentSharedState {
    pub fn new(piece_states: Vec<PieceState>, number_of_pieces: usize) -> Self {
        TorrentSharedState {
            peer_download_stats: HashMap::new(),
            pieces: piece_states,
            piece_download_progress: HashMap::with_capacity(number_of_pieces),
            cancellation_req_queue: VecDeque::new(),
        }
    }
}

impl TorrentSharedState {
    fn get_piece_steal_coeff(&self) -> f64 {
        let total_pieces = self.pieces.len() as f64;
        let downloaded_pieces = DOWNLOADED_PIECES.load(Ordering::Relaxed) as f64;
        if downloaded_pieces / total_pieces >= 0.8 {
            3.
        } else {
            10.
        }
    }

    /// Returns either a piece that we failed to download earlier or one that we didn't try yet.
    pub fn get_next_missing_piece_indexes(
        &mut self,
        peer_addr: SocketAddrV4,
        peer_available_pieces: &BitSlice<u8, Msb0>,
        number_of_pieces: usize,
    ) -> anyhow::Result<Option<Vec<(u32, Option<SocketAddrV4>)>>> {
        let piece_steal_coeff = self.get_piece_steal_coeff();
        let pieces = self
            .pieces
            .iter_mut()
            .enumerate()
            .filter_map(|(idx, status)| {
                if !peer_available_pieces[idx] {
                    return None;
                }
                match status {
                    PieceState::Queued => {
                        *status = PieceState::Downloading {
                            peer: peer_addr,
                            start: Instant::now(),
                        };
                        Some(try_into!(idx, u32).map(|idx| (idx, None)))
                    }
                    PieceState::Downloading { peer, start } => {
                        let peer = *peer;
                        if peer == peer_addr {
                            return None;
                        }

                        let requesting_peer_stats = self.peer_download_stats.entry(peer_addr).or_insert((0., 0.));
                        if requesting_peer_stats.1 == 0. {
                            return None;
                        }
                        let requesting_peer_avg_time = requesting_peer_stats.0 / requesting_peer_stats.1;

                        let elapsed_secs = start.elapsed().as_secs_f64();
                        // Compare elapsed time to requesting peer's average piece download time
                        if elapsed_secs > requesting_peer_avg_time * piece_steal_coeff {
                            tracing::debug!(
                                %peer_addr,
                                stolen_from=%peer,
                                piece=%idx,
                                "stole a piece: elapsed time {}, my avg piece time: {}",
                                elapsed_secs,
                                requesting_peer_avg_time
                            );

                            // Update the current peer's download stats
                            let (ref mut cur_peer_piece_download_times_sum, _) =
                                self.peer_download_stats.entry(peer).or_insert((0., 0.));
                            *cur_peer_piece_download_times_sum += elapsed_secs;

                            *status = PieceState::Downloading {
                                peer: peer_addr,
                                start: Instant::now(),
                            };
                            Some(try_into!(idx, u32).map(|idx| (idx, Some(peer))))
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            })
            .take(number_of_pieces)
            .collect::<anyhow::Result<Vec<(u32, Option<SocketAddrV4>)>>>()
            .context("bug: converting piece index to u32 failed - too many pieces?")
            .map(|vec| if vec.is_empty() { None } else { Some(vec) })?;

        if let Some(pieces) = pieces.as_ref() {
            for (piece_idx, stolen_from) in pieces.iter().filter(|(_, stolen_from)| stolen_from.is_some()) {
                self.cancellation_req_queue
                    .push_back((stolen_from.unwrap(), *piece_idx));

                // Reset piece download progress
                self.piece_download_progress.insert(*piece_idx, 0);
            }
        }

        Ok(pieces)
    }

    pub fn get_piece_status(&self, idx: usize) -> Option<&PieceState> {
        self.pieces.get(idx)
    }

    /// Checks whether the current torrent was fully downloaded
    pub fn finished_downloading(&self) -> bool {
        self.pieces.iter().all(|state| state == &PieceState::Verified)
    }

    /// Returns the total number of pieces for the current torrent
    pub fn get_number_of_pieces(&self) -> usize {
        self.pieces.len()
    }

    pub fn mark_piece_as_verified(&mut self, piece_idx: usize) {
        if let Some(piece_state) = self.pieces.get_mut(piece_idx) {
            *piece_state = PieceState::Verified
        };
    }

    pub fn on_peer_disconnect(&mut self, dead_peer_addr: &SocketAddrV4) {
        self.pieces.iter_mut().for_each(|piece| match piece {
            PieceState::Downloading { peer, .. } if peer == dead_peer_addr => {
                *piece = PieceState::Queued;
            }
            _ => {}
        })
    }
}

#[derive(Debug)]
pub struct Torrent {
    torrent_meta: TorrentMeta,
    shared_state: Arc<RwLock<TorrentSharedState>>,
    piece_hashes: Vec<[u8; 20]>,
    /// Channels for sending peer-level events to peers
    peer_req_txs: HashMap<SocketAddrV4, mpsc::Sender<TorrentManagerReq>>,
    /// Channel for receiving peer-level events from peers
    rx: UnboundedReceiver<(SocketAddrV4, PeerEvent)>,
    /// Channel for receiving request channels for new peers
    new_peer_req_rx: UnboundedReceiver<(SocketAddrV4, mpsc::Sender<TorrentManagerReq>)>,
    /// Channel for communicating with the storage backend
    storage_tx: mpsc::Sender<StorageOp>,
    /// Channel for receiving the result of checking piece hashes and storage-related errors
    storage_rx: mpsc::Receiver<(SocketAddrV4, u32, bool)>,
}

impl Torrent {
    pub fn new(
        torrent_meta: TorrentMeta,
        state: Arc<RwLock<TorrentSharedState>>,
        piece_hashes: Vec<[u8; 20]>,
        rx: UnboundedReceiver<(SocketAddrV4, PeerEvent)>,
        new_peer_req_rx: UnboundedReceiver<(SocketAddrV4, mpsc::Sender<TorrentManagerReq>)>,
        storage_tx: mpsc::Sender<StorageOp>,
        storage_rx: mpsc::Receiver<(SocketAddrV4, u32, bool)>,
    ) -> Self {
        Torrent {
            torrent_meta,
            shared_state: state,
            peer_req_txs: HashMap::new(),
            piece_hashes,
            rx,
            new_peer_req_rx,
            storage_tx,
            storage_rx,
        }
    }

    #[tracing::instrument(level = "debug", err, skip(self))]
    pub async fn handle(&mut self) -> anyhow::Result<()> {
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                result = self.rx.recv() => {
                    if let Some((peer_addr, event)) = result {
                        match event {
                            PeerEvent::BlockDownloaded(block) => {
                                // We need to own the state all this time to avoid some other peer
                                // stealing the piece
                                let mut state = self.shared_state.write().await;
                                if Torrent::verify_piece_not_stolen(&state, peer_addr, try_into!(block.index, usize)?).await? {
                                    if !self.add_block(&mut state, peer_addr, block).await? {
                                        drop(state);
                                        self.disconnect_peer(&peer_addr, "error while adding a block", true).await?;
                                    };
                                }
                            }
                            PeerEvent::Disconnected => {
                                tracing::debug!(%peer_addr, "peer exited unexpectedly");
                                // Drop peer cancellation tx
                                self.remove_peer_req_tx(&peer_addr);
                                self.disconnect_peer(&peer_addr, "peer exited", false).await?;

                                if self.peer_req_txs.is_empty() && !self.shared_state.read().await.finished_downloading() {
                                    anyhow::bail!("All peers exited before finishing the torrent, the download is incomplete");
                                }
                            }
                        };
                    } else {
                        // This can only happen if all peers panicked somehow and didn't disconnect
                        // properly
                        anyhow::bail!("bug: all peers exited unexepectedly?");
                    }
                },
                result = self.storage_rx.recv() => {
                    let Some((peer_addr, piece_idx, is_correct)) = result else {
                        anyhow::bail!("bug: storage backend exited before torrent manager?");
                    };

                    if !is_correct {
                        tracing::debug!(
                            %peer_addr,
                            %piece_idx,
                            "piece hash verification failed: disconnecting the peer"
                        );

                        self.disconnect_peer(&peer_addr, "piece hash verification failed", true).await?;
                    } else {
                        let mut shared_state = self.shared_state.write().await;
                        let piece_state = shared_state.pieces.get_mut(try_into!(piece_idx, usize)?).context("bug: downloaded a ghost piece?")?;

                        *piece_state = PieceState::Verified;

                        let downloaded_bytes = shared_state.piece_download_progress.get(&piece_idx).context("bug: downloaded a piece but didn't track its bytes?")?;
                        DOWNLOADED_BYTES.fetch_add(*downloaded_bytes, Ordering::Relaxed);
                        DOWNLOADED_PIECES.fetch_add(1, Ordering::Relaxed);

                        if shared_state.finished_downloading() {
                            tracing::info!("Successfully finished downloading the torrent");
                            tracing::debug!("shutting down peers");
                            for (peer_addr, req_tx) in self.peer_req_txs.drain() {
                                if req_tx.send(TorrentManagerReq::Disconnect("finished downloading")).await.is_err() {
                                    tracing::debug!(
                                        %peer_addr,
                                        "error while shutting down a peer: peer already dropped the receiver"
                                    );
                                }
                            };
                            break;
                        }
                    }
                },
                result = self.new_peer_req_rx.recv() => {
                    if let Some((peer_addr, peer_tx_channel)) = result {
                        NUMBER_OF_PEERS.fetch_add(1, Ordering::Relaxed);
                        self.peer_req_txs.insert(peer_addr, peer_tx_channel);
                    };
                }
                _ = interval.tick() => {}
            }
            let mut state = self.shared_state.write().await;
            while let Some((peer_addr, piece_idx)) = state.cancellation_req_queue.pop_front() {
                if let Some(sender) = self.get_peer_req_tx(&peer_addr) {
                    if let Err(e) = sender.send(TorrentManagerReq::CancelPiece(piece_idx)).await {
                        tracing::debug!(
                            %peer_addr,
                            "error while sending a cancellation request: {}", e
                        );
                    }
                }
            }
        }

        Ok(())
    }

    #[tracing::instrument(err, skip(self, state, begin, block))]
    async fn add_block(
        &self,
        state: &mut TorrentSharedState,
        peer_addr: SocketAddrV4,
        Block { index, begin, block }: Block,
    ) -> anyhow::Result<bool> {
        let downloaded_bytes = state.piece_download_progress.entry(index).or_insert(0);

        let expected_piece_size = piece_size_from_idx(
            self.torrent_meta.number_of_pieces,
            self.torrent_meta.total_length,
            self.torrent_meta.piece_size,
            try_into!(index, usize)?,
        );

        if *downloaded_bytes + block.len() > expected_piece_size {
            tracing::debug!(
                "piece is larger than expected: {} vs {}",
                *downloaded_bytes + block.len(),
                expected_piece_size,
            );
            return Ok(false);
        }

        *downloaded_bytes += block.len();

        self.storage_tx
            .send(StorageOp::AddBlock(Block { index, begin, block }))
            .await
            .with_context(|| {
                format!(
                    "Failed to send a block to the storage backend: index {}, in-piece offset: {}",
                    index, begin
                )
            })?;

        if *downloaded_bytes == expected_piece_size {
            let piece_idx = try_into!(index, usize)?;
            let Some(expected_piece_hash) = self.get_piece_hash(piece_idx) else {
                tracing::debug!(
                    "Wrong piece index: index {}, total pieces: {}",
                    piece_idx,
                    self.shared_state.read().await.get_number_of_pieces()
                );
                return Ok(false);
            };

            let piece_state = state
                .pieces
                .get_mut(piece_idx)
                .with_context(|| format!("bug: missing piece state for piece #{}", piece_idx))?;

            let PieceState::Downloading { start, .. } = piece_state else {
                anyhow::bail!("bug: how did we even get here?");
            };

            // Update peer download stats
            let elapsed_secs = start.elapsed().as_secs_f64();
            let (ref mut peer_piece_download_times_sum, ref mut peer_downloaded_pieces) =
                state.peer_download_stats.entry(peer_addr).or_insert((0., 0.));
            *peer_piece_download_times_sum += elapsed_secs;
            *peer_downloaded_pieces += 1.;

            // Mark piece as downloaded
            *piece_state = PieceState::Downloaded;

            self.storage_tx
                .send(StorageOp::CheckPieceHash((
                    peer_addr.to_owned(),
                    index,
                    expected_piece_hash.to_owned(),
                )))
                .await
                .with_context(|| {
                    format!(
                        "Failed to send a 'check piece hash' request to the storage backend: index {}",
                        index
                    )
                })?;
        }

        Ok(true)
    }

    fn get_peer_req_tx(&self, peer_addr: &SocketAddrV4) -> Option<&mpsc::Sender<TorrentManagerReq>> {
        self.peer_req_txs.get(peer_addr)
    }

    fn remove_peer_req_tx(&mut self, peer_addr: &SocketAddrV4) -> Option<mpsc::Sender<TorrentManagerReq>> {
        self.peer_req_txs.remove(peer_addr)
    }

    fn get_piece_hash(&self, index: usize) -> Option<&[u8; 20]> {
        self.piece_hashes.get(index)
    }

    async fn verify_piece_not_stolen(
        state: &TorrentSharedState,
        peer_addr: SocketAddrV4,
        piece_idx: usize,
    ) -> anyhow::Result<bool> {
        let piece_status = state.get_piece_status(piece_idx).context("bug: bad piece index?")?;
        match piece_status {
            PieceState::Downloading { peer, .. } if *peer == peer_addr => Ok(true),
            _ => Ok(false),
        }
    }

    async fn disconnect_peer(
        &mut self,
        peer_addr: &SocketAddrV4,
        disconnect_reason: &'static str,
        send_disconnect: bool,
    ) -> anyhow::Result<()> {
        if send_disconnect {
            if let Some(req_tx) = self.remove_peer_req_tx(&peer_addr) {
                if req_tx
                    .send(TorrentManagerReq::Disconnect(disconnect_reason))
                    .await
                    .is_err()
                {
                    tracing::debug!(
                        %peer_addr,
                        "error while shutting down a peer: peer already dropped the receiver"
                    );
                }
            };
        }

        let mut state = self.shared_state.write().await;
        let reset_pieces = state
            .pieces
            .iter_mut()
            .enumerate()
            .filter_map(|(idx, piece_state)| match piece_state {
                PieceState::Downloading { peer, .. } if peer == peer_addr => {
                    *piece_state = PieceState::Queued;
                    Some(idx)
                }
                _ => None,
            })
            .collect::<Vec<usize>>();
        for piece in reset_pieces.into_iter() {
            let piece_idx = try_into!(piece, u32)?;
            state.piece_download_progress.insert(piece_idx, 0);
        }

        NUMBER_OF_PEERS.fetch_sub(1, Ordering::Relaxed);

        Ok(())
    }
}
