use crate::{Decode, Encode, Error, Result};
use bitvec::order::Msb0;
use bitvec::vec::BitVec;
use bytes::Buf;
use bytes::Bytes;
use std::io::Cursor;
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub enum MessageId {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have,
    Bitfield,
    Request,
    Piece,
    Cancel,
    Port,
}

impl TryFrom<u8> for MessageId {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            0 => Self::Choke,
            1 => Self::Unchoke,
            2 => Self::Interested,
            3 => Self::NotInterested,
            4 => Self::Have,
            5 => Self::Bitfield,
            6 => Self::Request,
            7 => Self::Piece,
            8 => Self::Cancel,
            9 => Self::Port,
            _ => return Err(Error::ConversionError("MessageId is out of range")),
        })
    }
}

/// Represents all possible `Peer Wire Protocol` messages
/// [Source](https://wiki.theory.org/BitTorrentSpecification#request:_.3Clen.3D0013.3E.3Cid.3D6.3E.3Cindex.3E.3Cbegin.3E.3Clength.3E)
#[derive(Debug, PartialEq)]
pub enum BittorrentP2pMessage {
    /// The keep-alive message is a message with zero bytes, specified with the length prefix set to zero.
    /// There is no message ID and no payload.
    ///
    /// Peers may close a connection if they receive no messages (keep-alive or any other message) for a
    /// certain period of time, so a keep-alive message must be sent to maintain the connection alive if
    /// no command have been sent for a given amount of time. This amount of time is generally **two minutes**.
    KeepAlive,
    /// When a peer chokes the client, it is a notification that no requests will be answered until the client is unchoked.
    /// The client should not attempt to send requests for blocks, and it should consider all pending (unanswered) requests
    /// to be discarded by the remote peer.
    Choke,
    Unchoke,
    /// This message is a notification that the remote peer will begin requesting blocks when the client unchokes them.
    Interested,
    NotInterested,
    /// The have message is fixed length.
    /// The payload is the zero-based index of a piece that has just been successfully downloaded and verified via the hash.
    Have(u32),
    /// The bitfield message may only be sent immediately after the handshaking sequence is completed, and before any other messages are sent.
    /// It is optional, and need not be sent if a client has no pieces.
    ///
    /// The payload is a bitfield representing the pieces that have been successfully downloaded.
    /// The high bit in the first byte corresponds to piece index 0.
    /// Bits that are cleared indicated a missing piece, and set bits indicate a valid and available piece.
    /// Spare bits at the end are set to zero.
    Bitfield(BitVec<u8, Msb0>),
    /// The request message is fixed length, and is used to request a block. The payload contains the following information:
    ///
    /// index: integer specifying the zero-based piece index
    /// begin: integer specifying the zero-based byte offset within the piece
    /// length: integer specifying the requested length.
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    /// A single block of data
    ///
    /// index: integer specifying the zero-based piece index
    /// begin: integer specifying the zero-based byte offset within the piece
    /// block: block of data, which is a subset of the piece specified by index.
    Piece {
        index: u32,
        begin: u32,
        block: Bytes,
    },
    /// The cancel message is used to cancel block requests. The payload is identical to that of the [BittorrentP2pMessage::Request] message.
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    /// The port message is sent by versions that implement a DHT tracker.
    /// The listen port is the port this peer's DHT node is listening on.
    /// This peer should be inserted in the local routing table (if DHT tracker is supported).
    Port(u16),
}

impl BittorrentP2pMessage {
    pub const FIXED_PART_LENGTH: usize = 4 /* length */;

    pub fn message_id(&self) -> Option<MessageId> {
        use BittorrentP2pMessage::{
            Bitfield, Cancel, Choke, Have, Interested, KeepAlive, NotInterested, Piece, Port, Request, Unchoke,
        };

        match self {
            KeepAlive => None,
            message => Some(match message {
                Choke => MessageId::Choke,
                Unchoke => MessageId::Unchoke,
                Interested => MessageId::Interested,
                NotInterested => MessageId::NotInterested,
                Have(_) => MessageId::Have,
                Bitfield(_) => MessageId::Bitfield,
                Request { .. } => MessageId::Request,
                Piece { .. } => MessageId::Piece,
                Cancel { .. } => MessageId::Cancel,
                Port(_) => MessageId::Port,
                KeepAlive => unreachable!("KeepAlive was covered before"),
            }),
        }
    }

    pub fn length_and_message_id(&self) -> (u32, Option<MessageId>) {
        use BittorrentP2pMessage::{
            Bitfield, Cancel, Choke, Have, Interested, KeepAlive, NotInterested, Piece, Port, Request, Unchoke,
        };

        let length = match self {
            KeepAlive => 0,
            Choke | Unchoke | Interested | NotInterested => 1,
            Have(_) => 5,
            Bitfield(bitvec) => 1 + bitvec.len(),
            Request { .. } | Cancel { .. } => 13,
            Piece { block, .. } => 9 + block.len(),
            Port(_) => 3,
        };

        // SAFETY: this should be safe as both bitvec and piece shouldn't be ever larger that u32::MAX
        (
            u32::try_from(length).expect("Bittorrent message length should fit in a u32"),
            self.message_id(),
        )
    }
}

impl Encode for BittorrentP2pMessage {
    async fn encode<T>(&self, dst: &mut T) -> Result<()>
    where
        T: AsyncWriteExt + Unpin,
    {
        use BittorrentP2pMessage::{
            Bitfield, Cancel, Choke, Have, Interested, KeepAlive, NotInterested, Piece, Port, Request, Unchoke,
        };

        let (length, message_id) = self.length_and_message_id();

        dst.write_all(&length.to_be_bytes()).await?;

        if let Some(message_id) = message_id {
            dst.write_all(&[message_id as u8]).await?;
        }

        match self {
            // These message types have no additional info
            KeepAlive | Choke | Unchoke | Interested | NotInterested => {}
            Have(piece_idx) => {
                dst.write_all(&piece_idx.to_be_bytes()).await?;
            }
            Bitfield(bitfield) => {
                dst.write_all(bitfield.as_raw_slice()).await?;
            }
            Request { index, begin, length } => {
                dst.write_all(&index.to_be_bytes()).await?;
                dst.write_all(&begin.to_be_bytes()).await?;
                dst.write_all(&length.to_be_bytes()).await?;
            }
            Piece { index, begin, block } => {
                dst.write_all(&index.to_be_bytes()).await?;
                dst.write_all(&begin.to_be_bytes()).await?;
                dst.write_all(block).await?;
            }
            Cancel { index, begin, length } => {
                dst.write_all(&index.to_be_bytes()).await?;
                dst.write_all(&begin.to_be_bytes()).await?;
                dst.write_all(&length.to_be_bytes()).await?;
            }
            Port(port) => {
                dst.write_all(&port.to_be_bytes()).await?;
            }
        };

        Ok(())
    }
}

impl<'a> Decode<'a> for BittorrentP2pMessage {
    fn decode(src: &'a [u8]) -> Result<Self> {
        check_length!(src.remaining(), Self::FIXED_PART_LENGTH);

        let mut src = Cursor::new(src);

        let length = src
            .get_u32()
            .try_into()
            .map_err(|_| Error::ConversionError("error when converting BitTorrent message length to usize"))?;

        if length == 0 {
            return Ok(BittorrentP2pMessage::KeepAlive);
        }

        check_length!(src.remaining(), length);

        let message_id: MessageId = src.get_u8().try_into()?;

        Ok(match message_id {
            MessageId::Choke => Self::Choke,
            MessageId::Unchoke => Self::Unchoke,
            MessageId::Interested => Self::Interested,
            MessageId::NotInterested => Self::NotInterested,
            MessageId::Have => Self::Have(src.get_u32()),
            MessageId::Bitfield => {
                // account for message id
                let mut raw_bitfield = vec![0; length - 1];
                src.copy_to_slice(&mut raw_bitfield);
                dbg!(&raw_bitfield);
                Self::Bitfield(BitVec::from_vec(raw_bitfield))
            }
            MessageId::Request => Self::Request {
                index: src.get_u32(),
                begin: src.get_u32(),
                length: src.get_u32(),
            },
            MessageId::Piece => {
                let index = src.get_u32();
                let begin = src.get_u32();
                let block_length = length - 1 /* message id */ - std::mem::size_of::<u32>() * 2 /* index + begin */;
                let block = src.copy_to_bytes(block_length);
                Self::Piece { index, begin, block }
            }
            MessageId::Cancel => Self::Cancel {
                index: src.get_u32(),
                begin: src.get_u32(),
                length: src.get_u32(),
            },
            MessageId::Port => Self::Port(src.get_u16()),
        })
    }
}
