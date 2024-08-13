use crate::requests::{GetPeersQueryMessage, KrpcMessage, KrpcMessageType};
use crate::util::generate_node_id;
use anyhow::Context;
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use std::num::NonZeroU32;
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
const INFLIGHT_REQUEST_TIMEOUT_SECS: f64 = 1.;

#[derive(Debug)]
pub struct DhtRequester {
    read_buf: Vec<u8>,
    serialized_get_peers_message: Vec<u8>,
    node_queue: VecDeque<SocketAddrV4>,
    processed_nodes: Vec<SocketAddrV4>,
    seen_peers: Vec<Ipv4Addr>,
    inflight_requests: VecDeque<(Instant, SocketAddrV4)>,
    rate_limiter: DefaultDirectRateLimiter,
}

impl DhtRequester {
    pub fn new(bootstrap_node_addrs: Vec<SocketAddrV4>, info_hash: [u8; 20]) -> anyhow::Result<Self> {
        if bootstrap_node_addrs.is_empty() {
            anyhow::bail!("No bootstrap nodes specified");
        }

        let node_queue = VecDeque::from(bootstrap_node_addrs);

        let message = KrpcMessage {
            transaction_id: "10".into(),
            message_type: KrpcMessageType::Query {
                name: "get_peers".into(),
                query: GetPeersQueryMessage {
                    id: generate_node_id(),
                    info_hash,
                },
            },
        };

        let rate_limiter = RateLimiter::direct(Quota::per_second(NonZeroU32::new(200).unwrap()));

        let mut serialized_message = Vec::new();
        serde_bencode::to_writer(&message, &mut serialized_message)
            .context("failed to serialize a get_peers DHT message")?;

        Ok(DhtRequester {
            read_buf: vec![0; DEFAULT_BUF_SIZE],
            serialized_get_peers_message: serialized_message,
            node_queue,
            processed_nodes: Vec::new(),
            inflight_requests: VecDeque::new(),
            seen_peers: Vec::new(),
            rate_limiter,
        })
    }

    #[tracing::instrument(level = "debug", err, skip_all)]
    pub async fn process_dht_nodes(
        &mut self,
        mut cancellation: oneshot::Receiver<()>,
        peer_queue_sender: mpsc::Sender<SocketAddrV4>,
    ) -> anyhow::Result<()> {
        let socket = UdpSocket::bind("0.0.0.0:6881").await.context("binding UDP socket")?;

        let mut request_cleanup_interval =
            tokio::time::interval(Duration::from_secs_f64(INFLIGHT_REQUEST_TIMEOUT_SECS));

        'main: loop {
            tokio::select! {
                result = socket.recv_from(&mut self.read_buf) => {
                    let (read_bytes, from_node) = result.context("receiving a message from a node")?;
                    let from_node = match from_node {
                        SocketAddr::V4(addr) => addr,
                        _ => {
                            tracing::debug!("received a message from node using IPv6: {}", from_node);
                            continue;
                        }
                    };

                    self.inflight_requests.retain(|(_, node_addr)| node_addr != &from_node);

                    tracing::trace!("Received a response from DHT node: {}", from_node);

                    match serde_bencode::from_bytes::<KrpcMessage>(&self.read_buf[..read_bytes]) {
                        Ok(get_peers_response) => {
                            match get_peers_response.message_type {
                                KrpcMessageType::Query { query, .. } => {
                                    tracing::warn!(
                                        addr = %from_node,
                                        "Unexpected response from DHT node. Expected get_peers response, got Query: {:?}",
                                        query
                                    )
                                },
                                KrpcMessageType::Response { response }=> {
                                    if !response.nodes.is_some() && !response.values.is_some() {
                                        tracing::debug!(
                                            addr = %from_node,
                                            "Bad get_peers response from node: no nodes or peers",
                                        )
                                    }

                                    if let Some(nodes) = response.nodes {
                                        for compact_node_info in nodes.chunks(26) {
                                            let compact_node_addr = &compact_node_info[20..];
                                            let ip = TryInto::<[u8; 4]>::try_into(&compact_node_addr[..4]).context("converting IP from slice to array")?;
                                            let port = ((compact_node_addr[4] as u16) << 8) | compact_node_addr[5] as u16;

                                            let ip_addr = Ipv4Addr::from(ip);
                                            let node_addr = SocketAddrV4::new(ip_addr, port);

                                            if !self.processed_nodes.contains(&node_addr) && !self.node_queue.contains(&node_addr) {
                                                self.node_queue.push_back(node_addr);
                                            }
                                        }
                                    }

                                    if let Some(peers) = response.values {
                                        for compact_peer_addr in peers.iter() {
                                            let ip = TryInto::<[u8; 4]>::try_into(&compact_peer_addr.0[..4]).context("converting IP from slice to array")?;
                                            let port = ((compact_peer_addr.0[4] as u16) << 8) | compact_peer_addr.0[5] as u16;

                                            let ip_addr = Ipv4Addr::from(ip);
                                            if !self.seen_peers.contains(&ip_addr) {
                                                self.seen_peers.push(ip_addr);
                                                let peer_addr = SocketAddrV4::new(ip_addr, port);
                                                match peer_queue_sender.send(peer_addr).await {
                                                    Ok(_) => {},
                                                    Err(_) => {
                                                        tracing::debug!("Receiving half of the peer queue sender was dropped, shutting down...");
                                                        break 'main;
                                                    }

                                                };
                                            }
                                        }
                                    }
                                },
                                KrpcMessageType::Error { error } => {
                                    tracing::debug!(
                                        addr = %from_node,
                                        "Got Error from DHT node: {:?}",
                                        error
                                    )
                                }
                            }
                        }
                        Err(e) => tracing::debug!(addr = %from_node, "An error happened while decoding a message from DHT node: {}", e)}
                }
                _ = self.rate_limiter.until_ready(), if !self.node_queue.is_empty() => {
                    self
                        .query_next_node(&socket)
                        .await
                        .context("querying next node in the queue")?;
                }
                _ = request_cleanup_interval.tick() => {}
                _ = &mut cancellation => {
                    tracing::debug!("Cancellation requsted. Requester is exiting");
                    break;
                }
            }

            // Drop oldest inflight requests to allow making new ones
            if self
                .inflight_requests
                .front()
                .is_some_and(|(req_time, _)| req_time.elapsed().as_secs_f64() > INFLIGHT_REQUEST_TIMEOUT_SECS)
            {
                self.inflight_requests
                    .retain(|(req_time, _)| req_time.elapsed().as_secs_f64() > INFLIGHT_REQUEST_TIMEOUT_SECS);
            }

            if self.node_queue.is_empty() && self.inflight_requests.is_empty() {
                // There is nothing left to do, as there are no more nodes to query
                tracing::debug!("No DHT nodes left to query. Requester is exiting");
                break;
            }
        }

        Ok(())
    }

    #[tracing::instrument(level = "debug", err, skip_all)]
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
