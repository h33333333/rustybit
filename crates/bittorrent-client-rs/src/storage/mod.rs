mod file_storage;
mod piece_hash_verifier;
mod storage_manager;
mod util;

use bittorrent_peer_protocol::Block;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncSeek};

pub use storage_manager::StorageManager;

#[derive(Debug)]
pub struct FileInfo {
    path: PathBuf,
    length: u64,
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

trait AsyncReadSeek: AsyncRead + AsyncSeek {}

impl<T> AsyncReadSeek for T where T: AsyncRead + AsyncSeek {}

trait Storage {
    fn write_all(&mut self, file_idx: usize, offset: u64, data: &[u8]) -> anyhow::Result<()>;
    fn read_exact(&mut self, file_idx: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()>;
    fn get_ro_file(&self, file_idx: usize) -> anyhow::Result<Pin<Box<dyn AsyncReadSeek + Send>>>;
}
