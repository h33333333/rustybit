use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub(crate) trait KrpcQueryMessage {
    type ResponseType: KrpcResponseMessage;
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct GetPeersQueryMessage {
    pub(crate) id: String,
    pub(crate) info_hash: String,
}

impl KrpcQueryMessage for GetPeersQueryMessage {
    type ResponseType = GetPeersResponse;
}

pub(crate) trait KrpcResponseMessage {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct GetPeersResponse {
    pub(crate) values: Option<Vec<u8>>,
    pub(crate) nodes: Option<Vec<u8>>,
}

impl KrpcResponseMessage for GetPeersResponse {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct KrpcMessage<M>
where
    M: KrpcQueryMessage + Debug + PartialEq + Eq,
    M::ResponseType: Serialize + Debug + PartialEq + Eq,
{
    #[serde(rename = "t")]
    pub(crate) transaction_id: String,
    #[serde(flatten)]
    #[serde(bound(deserialize = "M: Deserialize<'de>, M::ResponseType: Deserialize<'de>"))]
    pub(crate) message_type: KrpcMessageType<M>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "y")]
pub(crate) enum KrpcMessageType<M: KrpcQueryMessage> {
    #[serde(rename = "q")]
    Query(M),
    #[serde(rename = "r")]
    Response(M::ResponseType),
    #[serde(rename = "e")]
    Error(ErrorMsg),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ErrorMsg {
    #[serde(rename = "e")]
    errors: Vec<(u16, String)>,
}

#[cfg(test)]
mod tests {

    use crate::requests::GetPeersResponse;

    use super::{GetPeersQueryMessage, KrpcMessage, KrpcMessageType};

    #[test]
    fn get_peers_roundtrip_test() {
        let query_message = KrpcMessage {
            transaction_id: "absolutely_valid_id".to_string(),
            message_type: KrpcMessageType::Query(GetPeersQueryMessage {
                id: "another_valid_id".to_string(),
                info_hash: "valid_info_hash".to_string(),
            }),
        };

        let ser_result = serde_bencode::to_string(&query_message).expect("serialization failed");

        let deserialized_query_message =
            serde_bencode::from_str::<KrpcMessage<GetPeersQueryMessage>>(&ser_result).expect("deserialization failed");

        assert_eq!(query_message, deserialized_query_message);
    }

    #[test]
    fn get_peers_resp_roundtrip_test() {
        let query_resp_message = KrpcMessage {
            transaction_id: "absolutely_valid_id".to_string(),
            message_type: KrpcMessageType::<GetPeersQueryMessage>::Response(GetPeersResponse {
                values: Some(vec![10, 20, 13, 12, 1]),
                nodes: Some(vec![255, 255, 255, 255, 255]),
            }),
        };

        let ser_result = serde_bencode::to_string(&query_resp_message).expect("serialization failed");

        let deserialized_query_resp_message =
            serde_bencode::from_str::<KrpcMessage<GetPeersQueryMessage>>(&ser_result).expect("deserialization failed");

        assert_eq!(query_resp_message, deserialized_query_resp_message);
    }
}
