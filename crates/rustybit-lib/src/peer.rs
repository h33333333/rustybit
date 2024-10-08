use std::collections::VecDeque;
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use bittorrent_peer_protocol::{BittorrentP2pMessage, Block, BlockRequest, Encode, Handshake};
use bitvec::order::Msb0;
use bitvec::vec::BitVec;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self};
use tokio::sync::{self, RwLock};
use tokio::time;

use crate::buffer::ReadBuf;
use crate::state::event::{PeerEvent, TorrentManagerReq};
use crate::state::torrent::PieceState;
use crate::torrent_meta::TorrentMeta;
use crate::util::piece_size_from_idx;
use crate::{Elapsed, TorrentSharedState, WithTimeout, DEFAULT_BLOCK_SIZE};

#[tracing::instrument(level = "error", err(level = tracing::Level::DEBUG), skip_all, fields(%peer_addr))]
pub async fn handle_peer(
    peer_addr: SocketAddrV4,
    metadata: TorrentMeta,
    client_peer_id: [u8; 20],
    state: Arc<RwLock<TorrentSharedState>>,
    tx: mpsc::UnboundedSender<(SocketAddrV4, PeerEvent)>,
    new_peer_req_tx: mpsc::UnboundedSender<(SocketAddrV4, mpsc::Sender<TorrentManagerReq>)>,
) -> anyhow::Result<()> {
    let mut stream = TcpStream::connect(peer_addr)
        .with_timeout("peer connect", Duration::from_secs(5))
        .await
        .context("establishing connection with a peer")?;

    tracing::debug!("connected to a peer");

    // Send the handshake message
    let handshake_message = Handshake::new(metadata.info_hash, client_peer_id);
    handshake_message.encode(&mut stream).await?;

    let mut read_buf = ReadBuf::new();
    let handshake = read_buf
        .read_handshake(&mut stream)
        .with_timeout("read_handshake", Duration::from_secs(5))
        .await
        .context("reading peer handshake")?;

    if handshake.pstr != Handshake::DEFAULT_PSTR {
        tracing::debug!("Peer sent a bad PSTR, disconnecting: {}", handshake.pstr);
        anyhow::bail!("bad handshake pstr")
    }

    tracing::trace!("read a handshake");

    let mut output = Vec::with_capacity(metadata.piece_size * 2);
    let mut handler = PeerHandler::new(state, metadata, peer_addr);

    let (manager_req_tx, mut manager_req_rx) = sync::mpsc::channel(10);
    new_peer_req_tx
        .send((peer_addr, manager_req_tx))
        .context("error while registering a new peer with the manager")?;

    tracing::trace!("handshakes done, starting a peer handling task");

    let clonex_tx = tx.clone();
    let task_handle = tokio::spawn(async move {
        let mut piece_request_interval = time::interval(Duration::from_secs(1));
        let mut keep_alive_interval = time::interval(Duration::from_secs(120));
        // Skip the first tick, as it completes immediately and we just opened a new connection
        keep_alive_interval.tick().await;

        loop {
            tokio::select! {
                message = read_buf.read_message(&mut stream).with_elapsed("read_message", Some(Duration::from_millis(100))) => {
                    let message = message.context("reading message")?;
                    if let Some(event) = handler
                        .handle_message(message, &mut output)
                        .with_elapsed("handle_message", Some(Duration::from_millis(50)))
                        .await?
                    {
                        clonex_tx.send((peer_addr, event)).context("sending a peer event")?;
                    };
                }
                req = manager_req_rx.recv() => {
                    match req.context("bug: manager dropped the event-sending half?")? {
                        TorrentManagerReq::CancelPiece(piece_idx) => {
                            handler.cancel_block_requests_for_piece(piece_idx, &mut output).await.context("cancelling block requests for stolen piece")?;
                        }
                        TorrentManagerReq::Disconnect(reason) => {
                            tracing::trace!(%reason, "cancellation requested, peer exiting");
                            break;
                        }
                    }
                }
                // Try to steal a piece
                _ = piece_request_interval.tick() => {}
                _ = keep_alive_interval.tick() => {
                    // It's time to send a Keep Alive message
                    handler.send_keep_alive(&mut output).await?;
                }
            }

            if !handler.peer_choked
                && handler.present_pieces.is_some()
                && handler.block_requests_queue.len() < PeerHandler::MAX_PENDING_BLOCK_REQUESTS
            {
                let need_blocks = PeerHandler::MAX_PENDING_BLOCK_REQUESTS - handler.block_requests_queue.len();
                let number_of_pieces = need_blocks / handler.get_blocks_per_piece() + 1;
                let next_pieces = handler
                    .state
                    .write()
                    .await
                    // SAFETY: checked above
                    .get_next_missing_piece_indexes(
                        handler.peer_addr,
                        handler.present_pieces.as_ref().unwrap(),
                        number_of_pieces,
                    )
                    .context("bug: getting next pieces failed?")?;
                if let Some(piece_indexes) = next_pieces {
                    // We found new pieces that can be downloaded from this peer
                    for (index, ..) in piece_indexes.into_iter() {
                        handler
                            .add_block_requests_for_piece(index)
                            .with_context(|| format!("adding block requests for piece: {}", index))?;
                    }
                }
            }

            // Send block requests if we have them and are not choked
            handler.send_block_requests(&mut output).await?;

            stream.write_all(&output).await.context("writing to the stream")?;
            output.clear();
        }

        Ok::<(), anyhow::Error>(())
    });

    match task_handle.await.context("peer handling task panicked")? {
        Ok(()) => Ok(()),
        Err(e) => {
            tx.send((peer_addr, PeerEvent::Disconnected))
                .with_context(|| format!("peer graceful shutdown failed. Error that caused shutdown: {:#}", e))?;
            Err(e)
        }
    }
}

struct PeerHandler {
    peer_addr: SocketAddrV4,
    state: Arc<RwLock<TorrentSharedState>>,
    /// Contains all torrent-related information that a peer handler may need
    torrent_metadata: TorrentMeta,
    /// Bitvec with all pieces that a peer has
    present_pieces: Option<BitVec<u8, Msb0>>,
    /// Block requests that will be sent when we receive a response to previous ones
    block_requests_queue: VecDeque<BlockRequest>,
    /// Block requests that we sent and expect a response
    sent_block_requests: Vec<BlockRequest>,
    /// Whether the client is interested in the remote peer
    client_interested: bool,
    /// Whether the client chokes the remote peer
    client_choked: bool,
    /// Whether the remote peer is interested in the client
    peer_interested: bool,
    /// Whether the remote peer choked the client
    peer_choked: bool,
}

impl PeerHandler {
    const MAX_PENDING_BLOCK_REQUESTS: usize = 70;

    pub fn new(state: Arc<RwLock<TorrentSharedState>>, torrent_metadata: TorrentMeta, peer_addr: SocketAddrV4) -> Self {
        PeerHandler {
            peer_addr,
            state,
            block_requests_queue: VecDeque::with_capacity(Self::MAX_PENDING_BLOCK_REQUESTS),
            sent_block_requests: Vec::with_capacity(Self::MAX_PENDING_BLOCK_REQUESTS),
            torrent_metadata,
            client_choked: true,
            peer_choked: true,
            present_pieces: None,
            client_interested: false,
            peer_interested: false,
        }
    }

    #[tracing::instrument(level = "debug", err, skip_all)]
    async fn handle_message(
        &mut self,
        message: BittorrentP2pMessage,
        output: &mut Vec<u8>,
    ) -> anyhow::Result<Option<PeerEvent>> {
        use BittorrentP2pMessage::*;
        tracing::trace!(message_id = ?message.message_id(), "handling a message");

        match message {
            Choke => self.peer_choked = true,
            Unchoke => self.peer_choked = false,
            Interested => self.peer_interested = true,
            NotInterested => self.peer_interested = false,
            Have(piece_idx) => {
                let bitvec = self
                    .present_pieces
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("bug: have message received before bitvec"))?;

                bitvec.set(piece_idx as usize, true);
            }
            Bitfield(mut bitvec) => {
                // Remove spare bits
                let state = self.state.read().await;
                let n_of_pieces = state.get_number_of_pieces();
                bitvec.truncate(n_of_pieces);
                self.present_pieces = Some(bitvec);

                drop(state);

                // We can start asking for pieces now
                self.send_chocked(false, output).await?;
                self.send_interested(true, output).await?;
            }
            Request(BlockRequest { index, begin, length }) => {
                tracing::trace!(index, begin, length, "received a Request message from peer");
            }
            Piece(Block { index, begin, block }) => {
                if self.sent_block_requests.is_empty() {
                    // A block was most likely cancelled before
                    return Ok(None);
                }

                let Some(block_request_idx) = self
                    .sent_block_requests
                    .iter()
                    .position(|req| req.index == index && req.begin == begin)
                else {
                    // This piece was stolen earlier
                    self.cancel_block_requests_for_piece(index, output).await?;
                    return Ok(None);
                };
                self.sent_block_requests.remove(block_request_idx);

                let state = self.state.read().await;
                let piece = state
                    .get_piece_status(try_into!(index, usize)?)
                    .context("bug: unexsisting piece index?")?;
                match piece {
                    PieceState::Downloading { peer, .. } if *peer == self.peer_addr => {
                        return Ok(Some(PeerEvent::BlockDownloaded(Block { index, begin, block })));
                    }
                    PieceState::Downloading { .. } | PieceState::Downloaded { .. } | PieceState::Verified => {
                        // Someone stole the piece, ignoring the received block
                        drop(state);
                        self.cancel_block_requests_for_piece(index, output).await?;
                        return Ok(None);
                    }
                    _ => anyhow::bail!("bug: someone put a piece that we were downloading back in the queue?"),
                }
            }
            Cancel { index, begin, length } => {
                tracing::trace!(index, begin, length, "received a Cancel message from peer");
            }
            Port(port) => {
                tracing::trace!(?port, "received a Port message");
            }
            KeepAlive => {
                tracing::debug!("received a KeepAlive message");
            }
        };

        Ok(None)
    }

    async fn send_block_requests(&mut self, output: &mut Vec<u8>) -> anyhow::Result<()> {
        for _ in 0..(Self::MAX_PENDING_BLOCK_REQUESTS - self.sent_block_requests.len()) {
            if let Some(request) = self.block_requests_queue.pop_front() {
                BittorrentP2pMessage::Request(request.clone()).encode(output).await?;
                self.sent_block_requests.push(request);
            } else {
                break;
            }
        }

        Ok(())
    }

    async fn cancel_block_requests_for_piece(&mut self, piece_idx: u32, output: &mut Vec<u8>) -> anyhow::Result<()> {
        // Remove queued requests for this piece if any
        self.block_requests_queue.retain(|block| block.index != piece_idx);

        let requests_to_cancel = self
            .sent_block_requests
            .iter()
            .enumerate()
            .filter(|(_, block)| block.index == piece_idx)
            .map(|(idx, _)| idx)
            .collect::<Vec<usize>>();

        for (offset, index) in requests_to_cancel.iter().enumerate() {
            let BlockRequest { index, begin, length } = self.sent_block_requests.swap_remove(index - offset);
            BittorrentP2pMessage::Cancel { index, begin, length }
                .encode(output)
                .await?;
        }

        Ok(())
    }

    async fn send_interested(&mut self, client_interested: bool, output: &mut Vec<u8>) -> anyhow::Result<()> {
        if client_interested {
            BittorrentP2pMessage::Interested.encode(output).await?;
        } else {
            BittorrentP2pMessage::NotInterested.encode(output).await?;
        }

        self.client_interested = client_interested;

        Ok(())
    }

    async fn send_chocked(&mut self, client_choked: bool, output: &mut Vec<u8>) -> anyhow::Result<()> {
        if client_choked {
            BittorrentP2pMessage::Choke.encode(output).await?;
        } else {
            BittorrentP2pMessage::Unchoke.encode(output).await?;
        }

        self.client_choked = client_choked;

        Ok(())
    }

    async fn send_keep_alive(&self, output: &mut Vec<u8>) -> anyhow::Result<()> {
        BittorrentP2pMessage::KeepAlive.encode(output).await?;

        Ok(())
    }

    fn add_block_requests_for_piece(&mut self, index: u32) -> anyhow::Result<()> {
        let mut leftover_piece_size = piece_size_from_idx(
            self.torrent_metadata.number_of_pieces,
            self.torrent_metadata.total_length,
            self.torrent_metadata.piece_size,
            try_into!(index, usize)?,
        ) as u32;
        let mut begin = 0u32;
        while leftover_piece_size > 0 {
            let length = DEFAULT_BLOCK_SIZE.min(leftover_piece_size);
            let request = BlockRequest { index, begin, length };
            self.block_requests_queue.push_back(request);
            leftover_piece_size -= length;
            begin += length;
        }

        Ok(())
    }

    fn get_blocks_per_piece(&self) -> usize {
        let piece_block_size = DEFAULT_BLOCK_SIZE as usize;
        // Round upwards
        self.torrent_metadata.piece_size / piece_block_size
            + (self.torrent_metadata.piece_size % piece_block_size != 0) as usize
    }
}
