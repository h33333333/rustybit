use std::{net::SocketAddrV4, sync::Arc};

use anyhow::Context as _;
use tokio::{
    sync::{mpsc, oneshot, RwLock},
    task::JoinSet,
};
use tracing::instrument;

use crate::{
    handle_peer,
    state::event::{PeerEvent, TorrentManagerReq},
    torrent_meta::TorrentMeta,
    TorrentSharedState,
};

#[derive(Debug)]
pub struct PeerConnectionManager {
    number_of_peers: usize,
    peer_handler_tasks: JoinSet<anyhow::Result<()>>,
    cancel_rx: oneshot::Receiver<()>,
    peer_queue_rx: mpsc::Receiver<SocketAddrV4>,
    peer_event_tx: mpsc::UnboundedSender<(SocketAddrV4, PeerEvent)>,
    shared_state: Arc<RwLock<TorrentSharedState>>,
    torrent_meta: TorrentMeta,
    peer_id: [u8; 20],
    new_peer_req_tx: mpsc::UnboundedSender<(SocketAddrV4, mpsc::Sender<TorrentManagerReq>)>,
}

impl PeerConnectionManager {
    pub fn new(
        peer_id: [u8; 20],
        shared_state: Arc<RwLock<TorrentSharedState>>,
        torrent_meta: TorrentMeta,
        peer_queue_rx: mpsc::Receiver<SocketAddrV4>,
        peer_event_tx: mpsc::UnboundedSender<(SocketAddrV4, PeerEvent)>,
        cancel_rx: oneshot::Receiver<()>,
        new_peer_req_tx: mpsc::UnboundedSender<(SocketAddrV4, mpsc::Sender<TorrentManagerReq>)>,
    ) -> Self {
        PeerConnectionManager {
            number_of_peers: 0,
            peer_handler_tasks: JoinSet::new(),
            cancel_rx,
            peer_queue_rx,
            peer_event_tx,
            shared_state,
            torrent_meta,
            peer_id,
            new_peer_req_tx,
        }
    }

    #[instrument(level = "error", err(level = "debug"))]
    pub async fn handle(&mut self, initial_peers: Vec<SocketAddrV4>) -> anyhow::Result<()> {
        for peer_address in initial_peers.into_iter() {
            self.spawn_peer(peer_address);
            self.number_of_peers += 1;
        }

        loop {
            tokio::select! {
                Some(next_peer) = self.peer_queue_rx.recv(), if self.number_of_peers < 90 => {
                    self.spawn_peer( next_peer);
                    self.number_of_peers +=1;
                },
                Some(peer_handler_result) = self.peer_handler_tasks.join_next() => {
                        self.number_of_peers -= 1;
                        if let Err(e) = peer_handler_result.context("peer handler task")? {
                            tracing::debug!("an error happened in a peer: {:#}", e);
                        }
                }
                _ = &mut self.cancel_rx => {
                    tracing::debug!("cancellation requested, aborting peer tasks");
                    self.peer_handler_tasks.abort_all();
                    // Wait for all tasks to exit
                    while let Some(_) = self.peer_handler_tasks.join_next().await {}
                    tracing::debug!("all peers were aborted successfully, shutting down the manager");
                    break;
                }
                else => {
                    tracing::debug!("peer queue sender and all peers exited, shutting down the peer manager");
                    break;
                }
            }
        }

        Ok(())
    }

    fn spawn_peer(&mut self, peer_addr: SocketAddrV4) {
        self.peer_handler_tasks.spawn(handle_peer(
            peer_addr,
            self.torrent_meta.clone(),
            self.peer_id,
            self.shared_state.clone(),
            self.peer_event_tx.clone(),
            self.new_peer_req_tx.clone(),
        ));
    }
}
