use std::net::{Ipv4Addr, SocketAddrV4};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, Bytes};

pub const PORT: u16 = 6881;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Started,
    Stopped,
    Completed,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackerRequest<'a> {
    /// SHA1 hash of the info field in the MetaInfo struct
    #[serde(skip_serializing)]
    info_hash: [u8; 20],
    /// Unique client ID. Randomly generated
    peer_id: &'a str,
    /// The port number the client is listening on
    port: u16,
    /// The total amount uploaded
    uploaded: u64,
    /// The total amount downloaded
    downloaded: u64,
    /// The number of bytes the client STILL has to download
    left: u64,
    /// States that the client accepts a compact response. Is always set to 1
    compact: u8,
    /// If omitted - this message is a message sent on a regular basis without any specific event
    event: Option<EventType>,
}

impl<'a> TrackerRequest<'a> {
    pub fn new(
        peer_id: &'a str,
        info_hash: [u8; 20],
        download_state: &[Option<u64>; 3],
        event: Option<EventType>,
    ) -> Self {
        TrackerRequest {
            info_hash,
            peer_id,
            port: PORT,
            uploaded: download_state[0].unwrap_or_default(),
            downloaded: download_state[1].unwrap_or_default(),
            left: download_state[2].unwrap_or_default(),
            compact: 1,
            event,
        }
    }

    pub fn into_query_params(self) -> anyhow::Result<String> {
        let info_hash = form_urlencoded::byte_serialize(&self.info_hash).collect::<String>();

        // HACK: Serialize everything but `info_hash`. I didn't find a way to properly serialize it,
        // as serde can't serialize arrays and I also can't use URL encoding on the array to make it
        // a String before serializing with Serde, as that results in double URL encoding
        let mut serialized = serde_urlencoded::to_string(self).context("failed to serialize the tracker request")?;

        // Add `peer_id`
        serialized.push_str("&info_hash=");
        serialized.push_str(&info_hash);

        Ok(serialized)
    }
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct TrackerResponse {
    /// A human-readable error
    #[serde(rename = "failure reason")]
    failure_reason: Option<String>,
    /// Number of seconds to wait between regular requests
    interval: Option<u64>,
    /// List of peers
    #[serde_as(as = "Option<Bytes>")]
    peers: Option<Vec<u8>>,
}

impl TrackerResponse {
    /// Uses compact format as described in [BEP-23](https://www.bittorrent.org/beps/bep_0023.https)
    pub fn get_peers(&self) -> Option<anyhow::Result<Vec<SocketAddrV4>>> {
        self.peers.as_ref().map(|peers| {
            // BitTorrent represents each peer as a 6-byte value. The first 4 bytes are the IP address, and the last 2 are the port
            let mut peer_ips: Vec<SocketAddrV4> = Vec::with_capacity(peers.len() / 6);
            for chunk in peers.chunks(6) {
                let ip = try_into!(&chunk[..4], [u8; 4])?;
                let port = ((chunk[4] as u16) << 8) | chunk[5] as u16;

                let ip_addr = Ipv4Addr::from(ip);

                peer_ips.push(SocketAddrV4::new(ip_addr, port));
            }
            Ok(peer_ips)
        })
    }
}
