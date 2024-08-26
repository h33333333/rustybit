mod file_storage;
mod piece_hash_verifier;
mod storage_manager;
mod util;

use std::net::SocketAddrV4;
use std::path::PathBuf;

use bittorrent_peer_protocol::Block;
pub use file_storage::FileStorage;
pub use piece_hash_verifier::PieceHashVerifier;
pub use storage_manager::{StorageManager, TorrentFileMetadata};

#[derive(Debug)]
pub struct FileInfo {
    pub path: PathBuf,
    pub length: u64,
}

impl FileInfo {
    pub fn new(path: PathBuf, length: u64) -> Self {
        FileInfo { path, length }
    }
}

pub enum StorageOp {
    AddBlock(Block),
    CheckPieceHash((SocketAddrV4, u32, [u8; 20])),
}

pub trait Storage: Send + Sync {
    fn write_all(&mut self, file_idx: usize, offset: u64, data: &[u8]) -> anyhow::Result<()>;
    fn read_exact(&mut self, file_idx: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<bool>;
}
