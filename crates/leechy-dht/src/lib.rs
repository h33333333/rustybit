mod requests;

use anyhow::Context;
use requests::{GetPeersQueryMessage, KrpcMessage, KrpcMessageType};
use std::{
    collections::VecDeque,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot},
    time::Instant,
};

const DEFAULT_BUF_SIZE: usize = 65_536;
const MAX_INFLIGHT_REQUEST: usize = 5;
const INFLIGHT_REQUEST_TIMEOUT_SECS: f64 = 5.;

#[derive(Debug)]
struct DhtRequester {
    read_buf: Vec<u8>,
    serialized_get_peers_message: Vec<u8>,
    node_queue: VecDeque<SocketAddrV4>,
    processed_nodes: Vec<SocketAddrV4>,
    inflight_requests: VecDeque<(Instant, SocketAddrV4)>,
}

impl DhtRequester {
    pub fn new(bootstrap_node_addrs: Vec<SocketAddrV4>, info_hash: String) -> anyhow::Result<Self> {
        if bootstrap_node_addrs.is_empty() {
            anyhow::bail!("No bootstrap nodes specified");
        }

        let node_queue = VecDeque::from(bootstrap_node_addrs);

        let message = KrpcMessage {
            // TODO: generate randomly
            transaction_id: "1".to_string(),
            message_type: KrpcMessageType::Query(GetPeersQueryMessage {
                // TODO: generate randomly
                id: "test".to_string(),
                info_hash,
            }),
        };
        let serialized_message =
            serde_bencode::to_bytes(&message).context("failed to serialize a get_peers DHT message")?;

        Ok(DhtRequester {
            read_buf: vec![0; DEFAULT_BUF_SIZE],
            serialized_get_peers_message: serialized_message,
            node_queue,
            processed_nodes: Vec::new(),
            inflight_requests: VecDeque::new(),
        })
    }

    #[tracing::instrument(level = "trace", err)]
    pub async fn process_dht_nodes(
        &mut self,
        cancellation: oneshot::Receiver<()>,
        peer_queue_sender: mpsc::Sender<SocketAddrV4>,
    ) -> anyhow::Result<()> {
        let socket = UdpSocket::bind("0.0.0.0:6881").await.context("binding UDP socket")?;

        let mut request_send_interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                result = socket.recv_from(&mut self.read_buf) => {
                    let (read_bytes, from_node) = result.context("receiving a message from a node")?;
                    let from_node = match from_node {
                        SocketAddr::V4(addr) => addr,
                        _ => {
                            tracing::error!("received a message from node using IPv6: {}", from_node);
                            continue;
                        }
                    };

                    self.inflight_requests.retain(|(_, node_addr)| node_addr != &from_node);

                    match serde_bencode::from_bytes::<KrpcMessage<GetPeersQueryMessage>>(&self.read_buf[..read_bytes]) {
                        Ok(get_peers_response) => {
                            match get_peers_response.message_type {
                                KrpcMessageType::Query(q) => {
                                    tracing::warn!(
                                        addr = %from_node,
                                        "Unexpected response from DHT node. Expected get_peers response, got Query: {:?}",
                                        q
                                    )
                                },
                                KrpcMessageType::Response(resp) => {
                                    if !resp.nodes.is_some() && !resp.values.is_some() {
                                        tracing::error!(
                                            addr = %from_node,
                                            "Bad get_peers response from node: no nodes or peers",
                                        )
                                    }

                                    if let Some(nodes) = resp.nodes {
                                        for compact_node_info in nodes.chunks(26) {
                                            let compact_node_addr = &compact_node_info[20..];
                                            let ip = TryInto::<[u8; 4]>::try_into(compact_node_addr).context("converting IP from slice to array")?;
                                            let port = ((compact_node_addr[4] as u16) << 8) | compact_node_addr[5] as u16;

                                            let ip_addr = Ipv4Addr::from(ip);
                                            let node_addr = SocketAddrV4::new(ip_addr, port);
                                            self.node_queue.push_back(node_addr);
                                        }
                                    }

                                    if let Some(peers) = resp.values {
                                        for compact_peer_addr in peers.chunks(6) {
                                            let ip = TryInto::<[u8; 4]>::try_into(&compact_peer_addr[..4]).context("converting IP from slice to array")?;
                                            let port = ((compact_peer_addr[4] as u16) << 8) | compact_peer_addr[5] as u16;

                                            let ip_addr = Ipv4Addr::from(ip);
                                            let peer_addr = SocketAddrV4::new(ip_addr, port);

                                            peer_queue_sender.send(peer_addr).await.context("sending peer addr to the peer connect queue")?;
                                        }
                                    }
                                },
                                KrpcMessageType::Error(e) => {
                                    tracing::error!(
                                        addr = %from_node,
                                        "Got Error from DHT node: {:?}",
                                        e
                                    )
                                }
                            }
                        }
                        Err(e) => tracing::error!(addr = %from_node, "An error happened while decoding a message from DHT node: {}", e)
                    }
                }
                _ = request_send_interval.tick() => {}
            }

            // Drop the oldest inflight request to allow making new ones
            if self
                .inflight_requests
                .front()
                .is_some_and(|(req_time, _)| req_time.elapsed().as_secs_f64() > INFLIGHT_REQUEST_TIMEOUT_SECS)
            {
                self.inflight_requests.pop_front();
            }

            if self.inflight_requests.len() < MAX_INFLIGHT_REQUEST
                && !self
                    .query_next_node(&socket)
                    .await
                    .context("querying next node in the queue")?
                && self.inflight_requests.is_empty()
            {
                // There is nothing left to do, as there are no more nodes to query
                break;
            }
        }

        Ok(())
    }

    #[tracing::instrument(level = "trace", err)]
    pub async fn query_next_node(&mut self, socket: &UdpSocket) -> anyhow::Result<bool> {
        let Some(next_node) = self.node_queue.pop_front() else {
            // We can't continue from this point, as there are no other nodes to request data from.
            return Ok(false);
        };

        // Mark a node as processed, so that we won't try it again if some peer sends it back to us
        // again
        self.processed_nodes.push(next_node);

        // Recored request time
        let request_time = Instant::now();
        self.inflight_requests.push_back((request_time, next_node));

        socket
            .send_to(&self.serialized_get_peers_message, next_node)
            .await
            .with_context(|| format!("sending a get_peers message to '{}'", next_node))?;

        Ok(true)
    }
}
