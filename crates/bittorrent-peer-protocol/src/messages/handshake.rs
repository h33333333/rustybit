use bytes::Buf;
use std::{borrow::Cow, io::Cursor};
use tokio::io::AsyncWriteExt;

use crate::{Decode, Encode, Result};

/// The handshake is a required message and must be the first message transmitted by the client.
#[derive(Debug)]
pub struct Handshake<'a> {
    /// String identifier of the protocol. Its length should fit into a *single* byte
    pub pstr: Cow<'a, str>,
    /// Eight reserved bytes that are used to specify extensions of the Bittorrent protocol
    pub extension_bytes: u64,
    /// 20-byte SHA1 hash of the info key in the metainfo file. It's the same hash that was
    /// transmitted in the tracker request.
    pub info_hash: [u8; 20],
    /// 20-byte unique ID for the client. This is usually the same peer ID that was sent in the
    /// tracket request.
    pub peer_id: [u8; 20],
}

impl<'a> Handshake<'a> {
    pub const FIXED_PART_LENGTH: usize = 1 + 20 + 20 + 8;
    pub const DEFAULT_PSTR: &'static str = "BitTorrent protocol";

    pub fn new(info_hash: [u8; 20], peer_id: [u8; 20]) -> Self {
        Handshake {
            pstr: Cow::Borrowed(Handshake::DEFAULT_PSTR),
            extension_bytes: 0,
            info_hash,
            peer_id,
        }
    }
}

impl<'a> Encode for Handshake<'a> {
    async fn encode<T>(&self, dst: &mut T) -> Result<()>
    where
        T: AsyncWriteExt + Unpin,
    {
        // `pstr_len` must be a single byte
        dst.write_all(&(self.pstr.len() as u8).to_be_bytes()).await?;
        // `pstr`
        dst.write_all(self.pstr.as_bytes()).await?;
        // 8 reserved bytes
        dst.write_all(&self.extension_bytes.to_be_bytes()).await?;
        // metainfo hash
        dst.write_all(self.info_hash.as_ref()).await?;
        // client's peer ID
        dst.write_all(&self.peer_id).await?;

        Ok(())
    }
}

impl<'a> Decode<'a> for Handshake<'a> {
    fn decode(src: &'a [u8]) -> Result<Self> {
        let mut src = Cursor::new(src);

        let pstr_len = src.get_u8() as usize;
        let pstr_bytes = {
            let offset = src.position() as usize;
            let bytes = &src.get_ref()[offset..offset + pstr_len];
            src.set_position((offset + pstr_len) as u64);
            bytes
        };
        let pstr = String::from_utf8_lossy(pstr_bytes);

        let extension_bytes = src.get_u64();

        let mut info_hash = [0; 20];
        src.copy_to_slice(info_hash.as_mut());

        let mut peer_id = [0; 20];
        src.copy_to_slice(peer_id.as_mut());

        Ok(Handshake {
            pstr,
            extension_bytes,
            info_hash,
            peer_id,
        })
    }
}
