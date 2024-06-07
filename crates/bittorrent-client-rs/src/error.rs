use std::fmt::Debug;
use std::result;

use thiserror::Error;

pub type Result<T> = result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("error while parsing the torrent file: {0}")]
    ParsingError(#[from] serde_bencode::error::Error),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error("an error happened: {0}")]
    InternalError(&'static str),
    #[error("downloaded piece has incorrect hash: expected {0:?}, got {1:?}")]
    PieceHashMismatch([u8; 20], [u8; 20]),
    #[error("wrong info hash: {0:?}")]
    BadInfoHash([u8; 20]),
    #[error("wrong piece index: {0}. Total number of pieces: {1}")]
    WrongPieceIndex(u32, u32),
    #[error("error while encoding or decoding a bittorrent message: {0}")]
    BittorentProtocolError(#[from] bittorrent_peer_protocol::Error),
}
