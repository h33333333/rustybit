use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt::Debug};

pub trait KrpcQueryMessage<'a> {
    type ResponseType: KrpcResponseMessage<'a>;
}

pub trait KrpcResponseMessage<'a> {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetPeersQueryMessage {
    #[serde(with = "serde_bytes")]
    pub id: [u8; 20],
    #[serde(with = "serde_bytes")]
    pub info_hash: [u8; 20],
}

impl<'a> KrpcQueryMessage<'a> for GetPeersQueryMessage {
    type ResponseType = GetPeersResponse<'a>;
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetPeersResponse<'a> {
    #[serde(default, borrow)]
    pub values: Option<Cow<'a, [CompactPeerInfo<'a>]>>,
    #[serde(with = "serde_bytes", default, borrow)]
    pub nodes: Option<Cow<'a, [u8]>>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
pub struct CompactPeerInfo<'a>(#[serde(with = "serde_bytes", borrow)] pub Cow<'a, [u8]>);

impl<'a> KrpcResponseMessage<'a> for GetPeersResponse<'a> {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PingQueryMessage {
    #[serde(with = "serde_bytes")]
    pub id: [u8; 20],
}

impl<'a> KrpcQueryMessage<'a> for PingQueryMessage {
    type ResponseType = PingResponse;
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PingResponse {
    #[serde(with = "serde_bytes")]
    pub id: [u8; 20],
}

impl<'a> KrpcResponseMessage<'a> for PingResponse {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KrpcMessage<'a, M = GetPeersQueryMessage>
where
    M: KrpcQueryMessage<'a> + Debug + PartialEq + Eq,
    M::ResponseType: Serialize + Debug + PartialEq + Eq,
{
    #[serde(rename = "t")]
    pub(crate) transaction_id: Cow<'a, str>,
    #[serde(
        flatten,
        borrow,
        bound(deserialize = "M: Deserialize<'de>, M::ResponseType: Deserialize<'de>")
    )]
    pub(crate) message_type: KrpcMessageType<'a, M>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "y")]
pub enum KrpcMessageType<'a, M: KrpcQueryMessage<'a>> {
    #[serde(rename = "q")]
    Query {
        #[serde(rename = "q")]
        name: Cow<'a, str>,
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
        #[serde(rename = "e", borrow)]
        error: (u16, Cow<'a, str>),
    },
}

#[cfg(test)]
mod tests {
    use super::{GetPeersQueryMessage, KrpcMessage, KrpcMessageType};
    use crate::requests::{CompactPeerInfo, GetPeersResponse};

    #[test]
    fn get_peers_roundtrip_test() {
        let query_message = KrpcMessage {
            transaction_id: "123".into(),
            message_type: KrpcMessageType::Query {
                name: "get_peers".into(),
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
        let peer_info = vec![CompactPeerInfo(b"test".into())];
        let nodes = b"big node";

        let query_resp_message = KrpcMessage {
            transaction_id: "123".into(),
            message_type: KrpcMessageType::Response {
                response: GetPeersResponse {
                    values: Some((&peer_info).into()),
                    nodes: Some(nodes.as_ref().into()),
                },
            },
        };

        let ser_result = serde_bencode::to_string(&query_resp_message).expect("serialization failed");

        dbg!(&ser_result);

        let deserialized_query_resp_message =
            serde_bencode::from_str::<KrpcMessage>(&ser_result).expect("deserialization failed");

        assert_eq!(query_resp_message, deserialized_query_resp_message);
    }
}
