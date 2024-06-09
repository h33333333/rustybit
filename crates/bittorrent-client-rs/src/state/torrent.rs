use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use bitvec::bitvec;
use bitvec::order::Msb0;
use bitvec::{slice::BitSlice, vec::BitVec};
use tokio::fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::{broadcast, RwLock};

use crate::parser::Info;
use crate::{Error, Result};

use super::event::PeerEvent;
use super::event::SystemEvent;
use super::util::{calculate_piece_hash, write_to_file};

#[derive(Debug)]
pub struct TorrentFileInfo<'a> {
    path: PathBuf,
    length: u64,
    md5_sum: Option<&'a str>,
}

impl<'a> TorrentFileInfo<'a> {
    pub fn new(path: PathBuf, length: u64, md5_sum: Option<&'a str>) -> Self {
        TorrentFileInfo { path, length, md5_sum }
    }
}

#[derive(Debug)]
pub enum TorrentMode<'a> {
    SingleFile(TorrentFileInfo<'a>),
    MultiFile(Vec<TorrentFileInfo<'a>>),
}

impl<'a> TorrentMode<'a> {
    pub async fn new(info: &'a Info, base_path: &Path) -> Result<Self> {
        if let Some(files) = info.files.as_deref() {
            let mut torrent_file_infos = Vec::with_capacity(files.len());
            for file in files.iter() {
                let mut path = base_path.to_path_buf();
                file.path.iter().for_each(|path_part| {
                    path.push(path_part);
                });

                // Create all required directories
                fs::create_dir_all(
                    &path
                        .parent()
                        .expect("bug: file is being downloaded into the root folder"),
                )
                .await?;

                torrent_file_infos.push(TorrentFileInfo::new(path, file.length, file.md5sum.as_deref()));
            }

            Ok(TorrentMode::MultiFile(torrent_file_infos))
        } else {
            let mut path = base_path.to_path_buf();
            path.push(&info.name);

            // Create all required directories
            fs::create_dir_all(
                &path
                    .parent()
                    .expect("bug: file is being downloaded into the root folder"),
            )
            .await?;

            let length = info.length.ok_or_else(|| {
                Error::InternalError(
                    "Error while starting up a torrent: the `length` field is missing a single-file download mode",
                )
            })?;

            let torrent_info = TorrentFileInfo::new(path, length, info.md5sum.as_deref());

            Ok(TorrentMode::SingleFile(torrent_info))
        }
    }
}

/// State that each peer keeps a reference to
#[derive(Debug)]
pub struct TorrentSharedState {
    // Should have length equal to the total number of pieces
    last_piece_idx: u32,
    downloaded_pieces: BitVec,
    piece_queue: Vec<u32>,
    failed_pieces: Vec<u32>,
}

impl TorrentSharedState {
    pub fn new(number_of_pieces: usize) -> Result<Self> {
        let last_piece_idx = try_into!(number_of_pieces - 1, u32)?;

        Ok(TorrentSharedState {
            last_piece_idx,
            downloaded_pieces: bitvec![0; number_of_pieces],
            // TODO: how to fill a piece queue?
            piece_queue: (0..=last_piece_idx).collect(),
            failed_pieces: Vec::new(),
        })
    }
}

impl TorrentSharedState {
    /// Returns either a piece that we failed to download earlier or one that we didn't try yet.
    /// Also returns a `bool` that indicates whether this piece is the last one
    pub fn get_next_missing_piece_idx(&mut self, peer_available_pieces: &BitSlice<u8, Msb0>) -> Option<(u32, bool)> {
        let piece_idx = if !self.failed_pieces.is_empty() {
            self.failed_pieces
                .iter()
                .map(|&idx| try_into!(idx, usize))
                .enumerate()
                .find(|(_, idx)| idx.as_ref().is_ok_and(|&idx| peer_available_pieces[idx]))
                .map(|(queue_idx, _)| self.failed_pieces.remove(queue_idx))
        } else if !self.piece_queue.is_empty() {
            self.piece_queue
                .iter()
                .map(|&idx| try_into!(idx, usize))
                .enumerate()
                .find(|(_, idx)| idx.as_ref().is_ok_and(|&idx| peer_available_pieces[idx]))
                .map(|(queue_idx, _)| self.piece_queue.remove(queue_idx))
        } else {
            None
        };

        let is_last = piece_idx.is_some_and(|idx| idx == self.last_piece_idx);

        piece_idx.map(|piece_idx| (piece_idx, is_last))
    }

    /// Checks whether the current torrent was fully downloaded
    pub fn finished_downloading(&self) -> bool {
        self.downloaded_pieces.all()
    }

    /// Returns the total number of pieces for the current torrent
    pub fn get_number_of_pieces(&self) -> usize {
        self.downloaded_pieces.len()
    }

    pub fn add_downloaded_piece(&mut self, index: usize) {
        self.downloaded_pieces.set(index, true);
    }
}

#[derive(Debug)]
pub struct Torrent<'a> {
    shared_state: Arc<RwLock<TorrentSharedState>>,
    info_hash: [u8; 20],
    piece_hashes: Vec<[u8; 20]>,
    mode: TorrentMode<'a>,
    /// Single piece length
    piece_length: u64,
    /// Channels for sending peer-level events to peers
    peer_channels: HashMap<Ipv4Addr, mpsc::Sender<PeerEvent>>,
    /// Channel for receiving peer-level events from peers
    rx: UnboundedReceiver<(Ipv4Addr, PeerEvent)>,
    /// Channel for sending system-wide events to the peers
    tx: broadcast::Sender<SystemEvent>,
    /// Channel for receiving peer-event channels for new peers
    peer_rx: UnboundedReceiver<(Ipv4Addr, mpsc::Sender<PeerEvent>)>,
}

impl<'a> Torrent<'a> {
    pub fn new(
        state: Arc<RwLock<TorrentSharedState>>,
        info_hash: [u8; 20],
        mode: TorrentMode<'a>,
        piece_hashes: Vec<[u8; 20]>,
        piece_length: u64,
        rx: UnboundedReceiver<(Ipv4Addr, PeerEvent)>,
        tx: broadcast::Sender<SystemEvent>,
        peer_rx: UnboundedReceiver<(Ipv4Addr, mpsc::Sender<PeerEvent>)>,
    ) -> Self {
        Torrent {
            shared_state: state,
            info_hash,
            mode,
            piece_hashes,
            piece_length,
            peer_channels: HashMap::new(),
            rx,
            tx,
            peer_rx,
        }
    }

    #[tracing::instrument(err, skip(self))]
    pub async fn handle(&mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                result = self.rx.recv() => {
                    if let Some((peer_ip, event)) = result {
                        match event {
                            PeerEvent::PieceDownloaded(piece_idx, piece) => {
                                match self.add_piece(piece_idx, piece).await {
                                    Ok(()) =>  {
                                        if self.shared_state.read().await.finished_downloading() {
                                            self.tx.send(SystemEvent::DownloadFinished)
                                                .context("Error while broadcasting a system event")?;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::debug!(error = %e, "Error while adding a piece");
                                        // TODO: track how many bad pieces a peer sent and
                                        // disconnect him?
                                        if let Error::InternalError(message) = e {
                                            anyhow::bail!("An unrecoverable error happened while adding a piece: {}", message);
                                        }
                                        self.tx.send(SystemEvent::PieceFailed(piece_idx))
                                            .context("Error while broadcasting a system event")?;
                                    }
                                }
                            }
                            PeerEvent::Disconnected => {
                                self.peer_channels.remove(&peer_ip);
                                if self.peer_channels.is_empty() {
                                    todo!("Last peer disconnected, shutdown")
                                }
                            }
                        };
                    } else {
                        todo!("Channel closed, shutdown")
                    }
                },
                result = self.peer_rx.recv() => {
                    if let Some((peer_ip, peer_tx_channel)) = result {
                        self.peer_channels.insert(peer_ip, peer_tx_channel);
                    };
                }
            }
        }
    }

    /// Adds a new piece with the provided index.
    ///
    /// Sends [crate::p2p::messages::BittorrentP2pMessage::Have] message to all peers with the
    /// provided piece index.
    #[tracing::instrument(err, skip(self, piece))]
    async fn add_piece(&mut self, index: u32, piece: Vec<u8>) -> Result<()> {
        let piece_hash = calculate_piece_hash(piece.as_ref());

        let Some(expected_piece_hash) = self.get_piece_hash(try_into!(index, usize)?) else {
            return Err(Error::WrongPieceIndex(
                index,
                try_into!(self.shared_state.read().await.get_number_of_pieces(), u32)?,
            ));
        };

        if piece_hash.as_ref() != expected_piece_hash {
            self.shared_state.write().await.failed_pieces.push(index);
            tracing::warn!(expected = ?*expected_piece_hash, received = ?piece_hash, "Piece hash mismatch");

            // TODO: I probably should keep track of which peers send bad pieces and disconnect from them
            // after certain number of bad pieces
            return Err(Error::PieceHashMismatch(piece_hash, *expected_piece_hash));
        }

        match &self.mode {
            TorrentMode::SingleFile(file_info) => {
                // TODO: opening a file each time seems wasteful.
                // Can I buffer say 5 pieces and then write them together to the file?
                write_to_file(&file_info.path, Into::<u64>::into(index), &piece).await?;
            }
            TorrentMode::MultiFile(file_infos) => {
                let global_piece_offset = Into::<u64>::into(index) * self.piece_length;

                let Some((file_idx, offset, bytes_to_write)) = file_infos
                    .iter()
                    .enumerate()
                    .scan(0, |state, (file_idx, file_info)| {
                        // We need this to calculate an in-file offset
                        let prev_length = *state;

                        // Update the length
                        *state += file_info.length;

                        // Look for a matching file
                        if global_piece_offset < *state {
                            // Check if we cross a file boundary
                            let lfile_piece_length = if global_piece_offset + self.piece_length < *state {
                                // We don't cross the file boundary
                                self.piece_length
                            } else {
                                // We cross the file boundary and have to do two writes
                                let rfile_write_length = global_piece_offset + self.piece_length - *state;
                                self.piece_length - rfile_write_length
                            };

                            let offset_into_file = global_piece_offset - prev_length;

                            Some(Some((file_idx, offset_into_file, lfile_piece_length)))
                        } else {
                            Some(None)
                        }
                    })
                    .find(|el| el.is_some())
                    // We use nested Option to make scan traverse the collection as long as we need
                    // instead of stopping at the first None. The first one is always going be
                    // Some.
                    .expect("bug: scan returned None?")
                else {
                    return Err(Error::InternalError("bug: failed to find a matching file for a piece?"));
                };

                let (current_file_bytes, next_file_bytes) = piece.split_at(try_into!(bytes_to_write, usize)?);

                let path = &file_infos
                    .get(file_idx)
                    .ok_or_else(|| Error::InternalError("bug: scan found an out-of-bounds index?"))?
                    .path;

                write_to_file(path, offset, current_file_bytes).await?;

                // Piece that crosses the file bondary
                if !next_file_bytes.is_empty() {
                    let path = &file_infos
                        .get(file_idx + 1)
                        .ok_or_else(|| {
                            Error::InternalError(
                                "bug: a piece that crosses the file boundary doesn't have the `next` file?",
                            )
                        })?
                        .path;

                    write_to_file(path, 0, next_file_bytes).await?;
                }
            }
        }

        self.shared_state
            .write()
            .await
            .add_downloaded_piece(try_into!(index, usize)?);

        self.tx
            .send(SystemEvent::NewPieceAdded(index))
            .map_err(|_| Error::InternalError("Failed to send a system event"))?;

        tracing::debug!("added new piece");

        Ok(())
    }

    fn get_peer_event_sender(&self, peer_ip: &Ipv4Addr) -> Option<mpsc::Sender<PeerEvent>> {
        self.peer_channels.get(peer_ip).cloned()
    }

    fn get_piece_hash(&self, index: usize) -> Option<&[u8; 20]> {
        self.piece_hashes.get(index)
    }
}
