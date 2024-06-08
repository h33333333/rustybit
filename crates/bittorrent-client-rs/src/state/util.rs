use crate::Result;
use std::path::Path;

use sha1::{Digest, Sha1};
use tokio::{
    fs,
    io::{AsyncSeekExt, AsyncWriteExt},
};

pub fn calculate_piece_hash(piece: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();

    hasher.update(piece);

    hasher.finalize().into()
}

pub async fn write_to_file(path: &Path, position: u64, data: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new().create(true).write(true).open(path).await?;

    file.seek(std::io::SeekFrom::Start(position)).await?;

    file.write_all(data).await?;

    Ok(())
}
