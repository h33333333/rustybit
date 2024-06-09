use std::io::Write;
use std::net::Ipv4Addr;
use std::{sync::Arc, time::SystemTime};

use anyhow::Context;
use bitvec::order::Msb0;
use bitvec::vec::BitVec;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::{self};
use tokio::sync::mpsc::{self};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
};

use crate::state::event::{PeerEvent, SystemEvent};
use crate::state::torrent::TorrentSharedState;
use crate::Result;

use super::stream::FramedStream;
use bittorrent_peer_protocol::{BittorrentP2pMessage, Decode, Encode, Handshake};

const DEFAULT_BLOCK_SIZE: usize = 16_384;

/// Represents a single peer
#[derive(Debug)]
pub struct Peer<S: AsyncReadExt + AsyncWriteExt + Unpin> {
    stream: FramedStream<S>,
    state: Arc<RwLock<TorrentSharedState>>,
    /// Queue for receiving system-wide events from the main loop
    broadcast_rx: broadcast::Receiver<SystemEvent>,
    /// Channel for sending peer-level events to the main loop
    tx: mpsc::UnboundedSender<(Ipv4Addr, PeerEvent)>,
    /// Channel for receiving peer-level events from the main loop
    rx: mpsc::Receiver<PeerEvent>,
    /// Peer's IP address
    peer_ip: Ipv4Addr,
    /// Torrent's metainfo hash
    info_hash: Option<[u8; 20]>,
    /// Bitvec with all pieces that a peer has
    present_pieces: Option<BitVec<u8, Msb0>>,
    /// Indicated whether a client has requested a block from the peer
    // TODO: work stealing?
    // TODO: request cancellation if unanswered for too long?
    has_requested_piece: Option<SystemTime>,
    /// Is used to calculate correct blocks
    piece_size: usize,
    /// Last piece often has size different from other pieces
    last_piece_size: usize,
    /// An id of a single in-flight piece (if any)
    in_flight_piece: Option<(u32, usize)>,
    /// This `Vec` is of fixed size. We reuse it for all pieces that we download from this peer
    piece_buf: Vec<u8>,
    /// Number of bytes downloaded for the current in-flight piece
    downloaded_piece_bytes: usize,
    /// Whether the client is interested in the remote peer
    client_interested: bool,
    /// Whether the client chokes the remote peer
    client_choked: bool,
    /// Whether the remote peer is interested in the client
    peer_interested: bool,
    /// Whether the remote peer choked the client
    peer_choked: bool,
    try_get_piece: bool,
}

impl<S: AsyncReadExt + AsyncWriteExt + Unpin + std::fmt::Debug> Peer<S> {
    pub fn new(
        stream: S,
        state: Arc<RwLock<TorrentSharedState>>,
        broadcast_rx: broadcast::Receiver<SystemEvent>,
        peer_tx: mpsc::UnboundedSender<(Ipv4Addr, PeerEvent)>,
        peer_ip: Ipv4Addr,
        info_hash: Option<[u8; 20]>,
        piece_sizes: (usize, usize),
    ) -> (Self, mpsc::Sender<PeerEvent>) {
        let (tx, rx) = mpsc::channel::<PeerEvent>(10);

        let stream = FramedStream::new(stream);

        let (piece_size, last_piece_size) = piece_sizes;

        let peer = Peer {
            state,
            stream,
            peer_ip,
            info_hash,
            piece_size,
            last_piece_size,
            tx: peer_tx,
            broadcast_rx,
            rx,
            has_requested_piece: None,
            present_pieces: None,
            in_flight_piece: None,
            piece_buf: vec![0; piece_size],
            client_interested: false,
            client_choked: true,
            peer_interested: false,
            peer_choked: true,
            try_get_piece: false,
            downloaded_piece_bytes: 0,
        };

        (peer, tx)
    }

    #[tracing::instrument(err, skip(self), fields(peer_ip = %self.peer_ip, client_state = ?[self.client_interested, self.client_choked, self.peer_interested, self.peer_choked]))]
    pub async fn handle(&mut self) -> anyhow::Result<()> {
        // handle handshake
        let data_length = self.stream.find_handshake_length().await?;
        let Some(data) = self.stream.read_bytes(data_length) else {
            anyhow::bail!("bug: missing handshake bytes?");
        };
        // TODO: should I use some field from it?
        let _handshake = Handshake::decode(data)?;

        let mut output_frame = Vec::new();

        self.set_chocked(false, &mut output_frame).await?;
        self.set_interested(true, &mut output_frame).await?;

        tracing::trace!("starting a peer loop");
        loop {
            tokio::select! {
                // We can't use a pattern matching here to unpack Result because we want to check
                // for a possible error
                // TODO: won't this always trigger and burn CPU cycles?
                length = self.stream.find_message_length() => {
                    if let Some(length) = length? {
                        let Some(data) = self.stream.read_bytes(length) else {
                            anyhow::bail!("bug: missing message bytes?");
                        };

                        let message = BittorrentP2pMessage::decode(data)?;
                        self.handle_message(message).await?;
                    }
                }
                system_event = self.broadcast_rx.recv() => {
                        match system_event {
                            Ok(event) => {
                                if !self.handle_system_event(event, &mut output_frame).await? {
                                    // We can finish
                                    break;
                                };
                            }
                            Err(recv_error) => {
                                match recv_error {
                                    RecvError::Closed =>
                                        anyhow::bail!("bug: system event channel was closed before all peers finished?"),
                                    RecvError::Lagged(_) => todo!("Log"),
                                }
                            }
                        }
                }
                result = self.rx.recv() => {
                    if let Some(peer_event) = result {
                        self.handle_peer_event(peer_event, &mut output_frame).await?;
                    } else {
                        // TODO: shutdown?
                    }
                }
            }

            if self.present_pieces.is_some()
                && (self.in_flight_piece.is_none() && (self.client_interested || self.try_get_piece))
            {
                // Already done
                self.try_get_piece = false;

                let finished_downloading = self.state.read().await.finished_downloading();
                if !finished_downloading {
                    let next_piece = self
                        .state
                        .write()
                        .await
                        // SAFETY: checked above
                        .get_next_missing_piece_idx(self.present_pieces.as_ref().unwrap());
                    if let Some((piece_idx, is_last)) = next_piece {
                        // We found a new piece that can be downloaded from this peer
                        if !self.client_interested {
                            self.set_interested(true, &mut output_frame).await?;
                        }

                        let piece_size = if is_last { self.last_piece_size } else { self.piece_size };
                        self.in_flight_piece = Some((piece_idx, piece_size));
                    } else {
                        // The peer has nothing to offer, so stop trying to get something until
                        // we receive a `Have` message from him
                        if self.client_interested {
                            self.set_interested(false, &mut output_frame).await?;
                        }
                    }
                } else {
                    // We can safely exit, as we downloaded all pieces
                    if self.client_interested {
                        self.set_interested(false, &mut output_frame).await?;
                    }

                    tracing::trace!("peer exiting");

                    self.tx
                        .send((self.peer_ip, PeerEvent::Disconnected))
                        .context("Error while sending a Disconnected peer event")?;

                    break;
                }
            }

            // Send a piece request if not chocked, interested, and not downloading a piece
            if !self.peer_choked && self.in_flight_piece.is_some() && self.has_requested_piece.is_none() {
                // SAFETY: checked above
                let (piece_idx, piece_size) = self.in_flight_piece.as_ref().unwrap();

                self.send_piece_request(*piece_idx, *piece_size, &mut output_frame)
                    .await?;
            }

            // Ask for a new block if already downloading a piece but didn't request a block
            // if !self.peer_choked && self.in_flight_piece.is_some() && self.has_requested_block.is_none() {
            //     self.send_piece_request(None, &mut output_frame).await?;
            // }

            self.stream.flush_to_stream(&output_frame).await?;
            output_frame.clear();
        }

        // Send all queued messages (if any)
        self.stream.flush_to_stream(&output_frame).await?;

        Ok(())
    }

    #[tracing::instrument(level = "trace", ret, err, skip(self, output_frame))]
    async fn handle_system_event(&mut self, event: SystemEvent, output_frame: &mut Vec<u8>) -> anyhow::Result<bool> {
        match event {
            // Inform the peer that we have a new piece available
            // TODO: cancellation?
            SystemEvent::NewPieceAdded(piece_idx) => self.send_have_message(piece_idx, output_frame).await?,
            SystemEvent::PieceFailed(piece_idx) => {
                if let Some(present_pieces) = self.present_pieces.as_ref() {
                    if present_pieces
                        .get(try_into!(piece_idx, usize)?)
                        .as_deref()
                        .is_some_and(|&val| val)
                    {
                        // We should try to download the failing piece if it's available on the
                        // current peer
                        self.try_get_piece = true;
                    }
                }
            }
            SystemEvent::DownloadFinished => {
                tracing::trace!("peer exiting");

                self.tx
                    .send((self.peer_ip, PeerEvent::Disconnected))
                    .context("Error while sending a Disconnected peer event")?;

                return Ok(false);
            }
        };

        Ok(true)
    }

    async fn handle_peer_event(&mut self, _event: PeerEvent, _output_frame: &mut Vec<u8>) -> Result<()> {
        // No events to handle yet
        Ok(())
    }

    /// Changes [Peer::client_interested] to the provided value
    #[tracing::instrument(level = "trace", err, skip(self, output_frame))]
    async fn send_piece_request(&mut self, idx: u32, piece_size: usize, output_frame: &mut Vec<u8>) -> Result<()> {
        // // TODO: fix conversions
        // let (piece_idx, begin, block_size) = if let Some(piece_idx) = self.in_flight_piece {
        //     let block_size = DEFAULT_BLOCK_SIZE.min(self.piece_size - self.piece_buf.len()) as u32;
        //     let begin = self.piece_buf.len() as u32;
        //     (piece_idx, begin, block_size)
        // } else if let Some(new_piece_idx) = idx {
        //     self.in_flight_piece = Some(new_piece_idx);
        //     (new_piece_idx, 0, DEFAULT_BLOCK_SIZE as u32)
        // } else {
        //     return Err(Error::InternalError(
        //         "None was passed to send piece request while requesting a new piece",
        //     ));
        // };

        let mut leftover_piece_size = piece_size;
        let mut begin = 0u32;
        while leftover_piece_size > 0 {
            let block_size = DEFAULT_BLOCK_SIZE.min(leftover_piece_size) as u32;
            self.send_block_request(idx, begin, block_size, output_frame).await?;
            leftover_piece_size -= block_size as usize;
            begin += block_size;
        }

        self.has_requested_piece = Some(SystemTime::now());

        Ok(())
    }

    #[tracing::instrument(level = "trace", err, skip(self, output_frame))]
    async fn send_block_request(&self, index: u32, begin: u32, length: u32, output_frame: &mut Vec<u8>) -> Result<()> {
        Ok(BittorrentP2pMessage::Request { index, begin, length }
            .encode(output_frame)
            .await?)
    }

    /// Sets [Peer::client_interested] to the provided value
    #[tracing::instrument(level = "trace", err, skip(self, output_frame))]
    async fn set_interested(&mut self, client_interested: bool, output_frame: &mut Vec<u8>) -> Result<()> {
        if client_interested {
            BittorrentP2pMessage::Interested.encode(output_frame).await?;
        } else {
            BittorrentP2pMessage::NotInterested.encode(output_frame).await?;
        }

        self.client_interested = client_interested;

        Ok(())
    }

    /// Sets [Peer::client_choked] to the provided value
    #[tracing::instrument(level = "trace", err, skip(self, output_frame))]
    async fn set_chocked(&mut self, client_choked: bool, output_frame: &mut Vec<u8>) -> Result<()> {
        if client_choked {
            BittorrentP2pMessage::Choke.encode(output_frame).await?;
        } else {
            BittorrentP2pMessage::Unchoke.encode(output_frame).await?;
        }

        self.client_interested = client_choked;

        Ok(())
    }

    #[tracing::instrument(level = "trace", err, skip(self, output_frame))]
    async fn send_have_message(&self, piece_idx: u32, output_frame: &mut Vec<u8>) -> Result<()> {
        Ok(BittorrentP2pMessage::Have(piece_idx).encode(output_frame).await?)
    }

    #[tracing::instrument(level = "trace", err, skip_all)]
    async fn handle_message(&mut self, message: BittorrentP2pMessage) -> anyhow::Result<()> {
        use BittorrentP2pMessage::*;

        match message {
            Choke => self.peer_choked = true,
            Unchoke => self.peer_choked = false,
            Interested => self.peer_interested = true,
            NotInterested => self.peer_interested = false,
            Have(piece_idx) => {
                if let Some(ref mut bitvec) = self.present_pieces {
                    bitvec.set(piece_idx as usize, true);
                    // HACK: set interested to indicate that this peer (maybe) has someting to offer
                    if !self.client_interested {
                        self.try_get_piece = true;
                    }
                } else {
                    // TODO: should this be an error or should we create an empty bitvec at the
                    // beggining?
                    anyhow::bail!("bug: have message received while having an empty bitvec");
                }
            }
            Bitfield(mut bitvec) => {
                let state = self.state.read().await;

                // remove spare bits
                let piece_amount = state.get_number_of_pieces();
                bitvec.truncate(piece_amount);

                self.present_pieces = Some(bitvec);
            }
            Request { index, begin, length } => {
                // TODO: allow peers to download from us
                tracing::trace!(index, begin, length, "received a Request message from peer");
            }
            Piece { index, begin, block } => {
                if self.in_flight_piece.is_none() {
                    anyhow::bail!("bug: received a block while not downloading a piece");
                }

                if self
                    .in_flight_piece
                    .is_some_and(|(stored_index, _)| stored_index != index)
                {
                    anyhow::bail!("bug: received piece and the in-flight one have different indexes");
                }

                let begin = try_into!(begin, usize)?;

                if begin + block.len() > self.piece_size {
                    anyhow::bail!(
                        "bug: received piece has larger size than expected: {} vs {}",
                        begin + block.len(),
                        self.piece_size
                    );
                }

                (&mut self.piece_buf[begin..begin + block.len()]).write_all(&block)?;

                self.downloaded_piece_bytes += block.len();

                // Did we download the piece already?
                // TODO: this is error prone. Find a better way to check if the piece in the last
                // one with the custom size
                if self.downloaded_piece_bytes == self.piece_size || self.downloaded_piece_bytes == self.last_piece_size
                {
                    self.has_requested_piece = None;
                    self.downloaded_piece_bytes = 0;
                    // SAFETY: is Some at this point
                    let (index, piece_size) = self.in_flight_piece.take().unwrap();
                    // TODO: can we avoid allocation?
                    let mut piece = std::mem::replace(&mut self.piece_buf, vec![0; self.piece_size]);
                    // Remove additional zeroes if the downloaded piece was the last one
                    piece.truncate(piece_size);

                    self.tx
                        .send((self.peer_ip, PeerEvent::PieceDownloaded(index, piece)))
                        .context("Error while sending a PieceDownloaded peer event")?;
                }
            }
            Cancel { index, begin, length } => {
                tracing::trace!(index, begin, length, "received a Cancel message from peer");
            }
            Port(port) => {
                tracing::trace!(?port, "received a Port message");
            }
            KeepAlive => {
                // TODO: what to do with it?
                tracing::trace!("received a KeepAlice message");
            }
        };

        Ok(())
    }
}
