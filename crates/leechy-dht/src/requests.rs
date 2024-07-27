use serde::{de::Visitor, Deserialize, Deserializer, Serialize};
use std::{fmt::Debug, marker::PhantomData};

pub trait KrpcQueryMessage {
    type ResponseType: KrpcResponseMessage;
}

pub trait KrpcResponseMessage {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetPeersQueryMessage {
    #[serde(with = "serde_bytes")]
    pub id: [u8; 20],
    #[serde(with = "serde_bytes")]
    pub info_hash: [u8; 20],
}

impl KrpcQueryMessage for GetPeersQueryMessage {
    type ResponseType = GetPeersResponse;
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetPeersResponse {
    #[serde(default)]
    // #[serde(with = "serde_bytes", default)]
    pub values: Option<Vec<CompactPeerInfo>>,
    #[serde(with = "serde_bytes", default)]
    pub nodes: Option<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct CompactPeerInfo(pub [u8; 6]);

impl<'de> Deserialize<'de> for CompactPeerInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PeerInfoVisitor {}
        impl<'de> Visitor<'de> for PeerInfoVisitor {
            type Value = CompactPeerInfo;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a byte array")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v.len() != 6 {
                    return Err(serde::de::Error::invalid_length(v.len(), &"slice of length 6"));
                }
                let peer_info = TryInto::<[u8; 6]>::try_into(v).map_err(serde::de::Error::custom)?;

                Ok(CompactPeerInfo(peer_info))
            }
        }

        deserializer.deserialize_bytes(PeerInfoVisitor {})
    }
}

impl KrpcResponseMessage for GetPeersResponse {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PingQueryMessage {
    #[serde(with = "serde_bytes")]
    pub id: [u8; 20],
}

impl<'a> KrpcQueryMessage for PingQueryMessage {
    type ResponseType = PingResponse;
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PingResponse {
    #[serde(with = "serde_bytes")]
    pub id: [u8; 20],
}

impl KrpcResponseMessage for PingResponse {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KrpcMessage<M = GetPeersQueryMessage>
where
    M: KrpcQueryMessage + Debug + PartialEq + Eq,
    M::ResponseType: Serialize + Debug + PartialEq + Eq,
{
    #[serde(rename = "t")]
    #[serde(with = "serde_bytes")]
    pub(crate) transaction_id: [u8; 2],
    #[serde(flatten)]
    #[serde(bound(deserialize = "M: Deserialize<'de>, M::ResponseType: Deserialize<'de>"))]
    pub(crate) message_type: KrpcMessageType<M>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "y")]
pub enum KrpcMessageType<M: KrpcQueryMessage> {
    #[serde(rename = "q")]
    Query {
        #[serde(rename = "q")]
        name: String,
        #[serde(rename = "a")]
        query: M,
    },
    #[serde(rename = "r")]
    Response {
        #[serde(rename = "r")]
        response: M::ResponseType,
    },
    #[serde(rename = "e")]
    Error {
        #[serde(rename = "e")]
        error: ErrorMsg,
    },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorMsg {
    #[serde(rename = "e")]
    errors: Vec<(u16, String)>,
}

#[cfg(test)]
mod tests {
    use super::{GetPeersQueryMessage, KrpcMessage, KrpcMessageType};
    use crate::requests::GetPeersResponse;

    #[test]
    fn get_peers_roundtrip_test() {
        let query_message = KrpcMessage {
            transaction_id: [1, 2],
            message_type: KrpcMessageType::Query {
                name: "get_peers".to_string(),
                query: GetPeersQueryMessage {
                    id: [0; 20],
                    info_hash: [1; 20],
                },
            },
        };

        let ser_result = serde_bencode::to_string(&query_message).expect("serialization failed");

        let deserialized_query_message =
            serde_bencode::from_str::<KrpcMessage>(&ser_result).expect("deserialization failed");

        assert_eq!(query_message, deserialized_query_message);
    }

    #[test]
    fn get_peers_resp_roundtrip_test() {
        let query_resp_message = KrpcMessage {
            transaction_id: [0, 1],
            message_type: KrpcMessageType::Response {
                response: GetPeersResponse {
                    values: Some(vec![0, 1, 2, 3]),
                    nodes: Some(vec![4, 5, 6, 7]),
                },
            },
        };

        let ser_result = serde_bencode::to_string(&query_resp_message).expect("serialization failed");

        let deserialized_query_resp_message =
            serde_bencode::from_str::<KrpcMessage>(&ser_result).expect("deserialization failed");

        assert_eq!(query_resp_message, deserialized_query_resp_message);
    }
}
