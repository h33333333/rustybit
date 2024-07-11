use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use bittorrent_peer_protocol::Block;
use bitvec::order::Msb0;
use bitvec::slice::BitSlice;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::{oneshot, RwLock};

use super::event::PeerEvent;
use crate::stats::{DOWNLOADED_BYTES, NUMBER_OF_PEERS};
use crate::storage::StorageOp;
use crate::torrent_meta::TorrentMeta;
use crate::util::piece_size_from_idx;
use crate::Result;

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
}

impl TorrentSharedState {
    pub fn new(number_of_pieces: usize) -> Result<Self> {
        Ok(TorrentSharedState {
            peer_download_stats: HashMap::new(),
            pieces: vec![PieceState::Queued; number_of_pieces],
            piece_download_progress: HashMap::with_capacity(number_of_pieces),
        })
    }
}

impl TorrentSharedState {
    /// Returns either a piece that we failed to download earlier or one that we didn't try yet.
    pub fn get_next_missing_piece_indexes(
        &mut self,
        peer_addr: SocketAddrV4,
        peer_available_pieces: &BitSlice<u8, Msb0>,
        number_of_pieces: usize,
    ) -> anyhow::Result<Option<Vec<(u32, Option<SocketAddrV4>)>>> {
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
                            None
                        // Compare elapsed time to requesting peer's average piece downloading time
                        } else if start.elapsed().as_secs_f64()
                            > (requesting_peer_stats.0 / requesting_peer_stats.1) * 10.
                        {
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
            .collect::<Result<Vec<(u32, Option<SocketAddrV4>)>>>()
            .context("bug: converting piece index to u32 failed - too many pieces?")
            .map(|vec| if vec.is_empty() { None } else { Some(vec) })?;

        if let Some(pieces) = pieces.as_ref() {
            for (piece_idx, stolen_from) in pieces.iter().filter(|(_, stolen_from)| stolen_from.is_some()) {
                // TODO: send cancellation requests
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
}

#[derive(Debug)]
pub struct Torrent {
    torrent_meta: TorrentMeta,
    shared_state: Arc<RwLock<TorrentSharedState>>,
    piece_hashes: Vec<[u8; 20]>,
    piece_download_progress: HashMap<u32, usize>,
    /// Channels for sending peer-level events to peers
    peer_cancellation_txs: HashMap<SocketAddrV4, oneshot::Sender<()>>,
    /// Channel for receiving peer-level events from peers
    rx: UnboundedReceiver<(SocketAddrV4, PeerEvent)>,
    /// Channel for receiving cancellation channels for new peers
    peer_cancel_rx: UnboundedReceiver<(SocketAddrV4, oneshot::Sender<()>)>,
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
        peer_cancel_rx: UnboundedReceiver<(SocketAddrV4, oneshot::Sender<()>)>,
        storage_tx: mpsc::Sender<StorageOp>,
        storage_rx: mpsc::Receiver<(SocketAddrV4, u32, bool)>,
    ) -> Self {
        Torrent {
            torrent_meta,
            shared_state: state,
            piece_download_progress: HashMap::with_capacity(piece_hashes.len()),
            peer_cancellation_txs: HashMap::new(),
            piece_hashes,
            rx,
            peer_cancel_rx,
            storage_tx,
            storage_rx,
        }
    }

    #[tracing::instrument(err, skip(self))]
    pub async fn handle(&mut self) -> anyhow::Result<()> {
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
                                    self.add_block(&mut state, peer_addr, block).await?;
                                }
                            }
                            PeerEvent::Disconnected => { NUMBER_OF_PEERS.fetch_sub(1, Ordering::Relaxed);
                                // Drop peer cancellation tx
                                self.get_peer_cancellation_tx(&peer_addr);

                                if self.peer_cancellation_txs.is_empty() && !self.shared_state.read().await.finished_downloading() {
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

                    let mut shared_state = self.shared_state.write().await;
                    let piece_state = shared_state.pieces.get_mut(try_into!(piece_idx, usize)?).context("bug: downloaded a ghost piece?")?;

                    if !is_correct {
                        tracing::error!(
                            %peer_addr,
                            %piece_idx,
                            "piece hash verification failed: disconnecting the peer"
                        );

                        *piece_state = PieceState::Queued;
                        // Reset download progress for the failed piece
                        shared_state.piece_download_progress.insert(piece_idx, 0);

                        drop(shared_state);
                        if let Some(cancel_tx) = self.get_peer_cancellation_tx(&peer_addr) {
                            if cancel_tx.send(()).is_err() {
                                tracing::error!(
                                    %peer_addr,
                                    "error while shutting down a peer: peer already dropped the receiver"
                                );
                            }
                        };
                    } else {
                        *piece_state = PieceState::Verified;

                        let downloaded_bytes = shared_state.piece_download_progress.get(&piece_idx).context("bug: downloaded a piece but didn't track its bytes?")?;
                        DOWNLOADED_BYTES.fetch_add(*downloaded_bytes, Ordering::Relaxed);

                        if shared_state.finished_downloading() {
                            tracing::debug!("all pieces were downloaded; shutting down peers");
                            self.peer_cancellation_txs.drain().for_each(|(peer_addr, cancel_tx)| {
                                if cancel_tx.send(()).is_err() {
                                    tracing::error!(
                                        %peer_addr,
                                        "error while shutting down a peer: peer already dropped the receiver"
                                    );
                                }
                            });
                            break;
                        }
                    }
                },
                result = self.peer_cancel_rx.recv() => {
                    if let Some((peer_addr, peer_tx_channel)) = result {
                        NUMBER_OF_PEERS.fetch_add(1, Ordering::Relaxed);
                        self.peer_cancellation_txs.insert(peer_addr, peer_tx_channel);
                    };
                }
            }
        }

        Ok(())
    }

    #[tracing::instrument(err, skip(self, begin, block))]
    async fn add_block(
        &self,
        state: &mut TorrentSharedState,
        peer_addr: SocketAddrV4,
        Block { index, begin, block }: Block,
    ) -> anyhow::Result<()> {
        let downloaded_bytes = state.piece_download_progress.entry(index).or_insert(0);

        let expected_piece_size = piece_size_from_idx(
            self.torrent_meta.number_of_pieces,
            self.torrent_meta.total_length,
            self.torrent_meta.piece_size,
            index,
        )?;

        tracing::trace!(index, begin, block_len = block.len(), "block info");

        if *downloaded_bytes + block.len() > expected_piece_size {
            // TODO: do not bail, disconnect only one peer
            anyhow::bail!(
                "piece is larger than expected: {} vs {}",
                *downloaded_bytes + block.len(),
                expected_piece_size,
            );
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
                anyhow::bail!(format!(
                    "Wrong piece index: index {}, total pieces: {}",
                    piece_idx,
                    self.shared_state.read().await.get_number_of_pieces()
                ));
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

        Ok(())
    }

    fn get_peer_cancellation_tx(&mut self, peer_addr: &SocketAddrV4) -> Option<oneshot::Sender<()>> {
        self.peer_cancellation_txs.remove(peer_addr)
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
}
