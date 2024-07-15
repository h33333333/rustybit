mod file_storage;

use anyhow::Context;
use bittorrent_peer_protocol::Block;
use sha1::{Digest, Sha1};
use std::cmp::Ordering;
use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

use crate::parser::Info;
use crate::{Error, Result};

use self::file_storage::FileStorage;

pub enum StorageOp {
    AddBlock(Block),
    CheckPieceHash((SocketAddrV4, u32, [u8; 20])),
}

pub struct StorageManager {
    storage: Box<dyn Storage + Send>,
    torrent_mode: TorrentMode,
    piece_length: u64,
}

pub trait Storage {
    fn write_all(&mut self, file_idx: usize, offset: u64, data: &[u8]) -> anyhow::Result<()>;
    fn read_exact(&mut self, file_idx: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()>;
    fn read_to_end(&mut self, file_idx: usize, buf: &mut Vec<u8>) -> anyhow::Result<()>;
}

impl StorageManager {
    pub fn new(torrent_info: &mut Info, base_path: &Path) -> anyhow::Result<StorageManager> {
        let torrent_mode = TorrentMode::new(torrent_info, base_path)?;

        let paths = {
            let mut paths = Vec::with_capacity(torrent_info.files.as_ref().map(|files| files.len()).unwrap_or(1));
            match &torrent_mode {
                TorrentMode::SingleFile(file_info) => paths.push((file_info.path.as_path(), file_info.length)),
                TorrentMode::MultiFile(file_infos) => file_infos
                    .iter()
                    .for_each(|file_info| paths.push((file_info.path.as_path(), file_info.length))),
            };
            paths
        };

        // TODO: use on-stack dispatch?
        let storage: Box<dyn Storage + Send> =
            Box::new(FileStorage::new(&paths).context("error while creating an FS backend")?);

        Ok(StorageManager {
            storage,
            torrent_mode,
            piece_length: torrent_info.piece_length,
        })
    }

    // TODO: this is too slow. How can I speed it up?
    pub fn verify_downloaded_files(&mut self, piece_hashes: &[[u8; 20]]) -> anyhow::Result<Option<usize>> {
        let expected_piece_length = try_into!(self.piece_length, usize)?;
        match &self.torrent_mode {
            TorrentMode::SingleFile(file_info) => {
                let max_expected_length = try_into!(file_info.length, usize)?;
                let mut file_contents_buf = Vec::with_capacity(max_expected_length);
                self.storage
                    .read_to_end(0, &mut file_contents_buf)
                    .context("error while reading file contents for verification")?;

                match file_contents_buf.len().cmp(&max_expected_length) {
                    Ordering::Greater => {
                        // File is too big, we will have to start downloading it from the beginning
                        return Ok(Some(0));
                    }
                    Ordering::Equal => {
                        // TODO: verify MD5 hash here
                        if let Some(expected_md5_hash) = &file_info.md5_sum {
                            let calculated_hash = md5::compute(&file_contents_buf);
                            if expected_md5_hash == &String::from_utf8(calculated_hash.to_ascii_lowercase())? {
                                return Ok(None);
                            }
                        }
                    }
                    Ordering::Less => {}
                };

                let mut hasher = Sha1::new();
                for (piece_idx, piece) in file_contents_buf.chunks(expected_piece_length).enumerate() {
                    hasher.update(piece);

                    let calculated_hash: [u8; 20] = hasher.finalize_reset().into();
                    let expected_hash = piece_hashes.get(piece_idx).with_context(|| {
                        format!(
                            "bug: no hash for a piece: idx {}, total hashes {}",
                            piece_idx,
                            piece_hashes.len()
                        )
                    })?;

                    if &calculated_hash != expected_hash {
                        // Verification of the current piece has failed, so we should start downloading from it
                        return Ok(Some(piece_idx));
                    }
                }
            }
            TorrentMode::MultiFile(file_infos) => {
                // Reuse the same buffer, clearing it for each new file
                let mut file_contents_buf = Vec::with_capacity(try_into!(file_infos[0].length, usize)?);
                let mut hasher = Sha1::new();
                let mut cross_file_piece_bytes_left = 0;
                let mut piece_idx = 0;

                let mut file_infos_iter = file_infos.iter().peekable();
                while let Some(file_info) = file_infos_iter.next() {
                    let max_expected_length = try_into!(file_info.length, usize)?;
                    self.storage
                        .read_to_end(0, &mut file_contents_buf)
                        .context("error while reading file contents for verification")?;

                    match file_contents_buf.len().cmp(&max_expected_length) {
                        Ordering::Greater => {
                            // File is too big, we will have to start downloading it from the beginning
                            return Ok(Some(piece_idx));
                        }
                        Ordering::Equal => {
                            // TODO: verify MD5 hash here
                            // FIXME: I shouldn't return here, as the previous file check could
                            // have failed and we can still have cross_file_piece_bytes_left set

                            continue;
                        }
                        Ordering::Less => {}
                    };

                    if cross_file_piece_bytes_left > 0 {
                        let Some(missing_piece_bytes) = file_contents_buf.get(0..cross_file_piece_bytes_left) else {
                            // If we don't have enough bytes to calculate the piece hash, then this
                            // piece is the problematic one and we should download it again.
                            return Ok(Some(piece_idx));
                        };

                        hasher.update(missing_piece_bytes);
                        let calculated_hash: [u8; 20] = hasher.finalize_reset().into();
                        let expected_hash = piece_hashes.get(piece_idx).with_context(|| {
                            format!(
                                "bug: no hash for a piece: idx {}, total hashes {}",
                                piece_idx,
                                piece_hashes.len()
                            )
                        })?;

                        if &calculated_hash != expected_hash {
                            // Verification of the cross-file piece has failed, so we should start downloading from it
                            return Ok(Some(piece_idx));
                        }

                        piece_idx += 1;
                        file_contents_buf.drain(0..cross_file_piece_bytes_left);
                        cross_file_piece_bytes_left = 0;
                    }

                    for piece in file_contents_buf.chunks(expected_piece_length) {
                        hasher.update(piece);

                        if piece.len() < expected_piece_length
                            && file_contents_buf.len() == max_expected_length
                            && file_infos_iter.peek().is_some()
                        {
                            // NOTE: we can only get in here if we have a full file, its MD5
                            // checksum verification has failed, and there is at least one more file.
                            // If this is the case, then the last piece of the file is the failing one
                            // (because otherwise we would have returned earlier) and it also crosses the file boundary.

                            // This piece crosses file boundary, so we can verify its hash only
                            // when we read the next file
                            cross_file_piece_bytes_left = expected_piece_length - piece.len();
                            continue;
                        }

                        let calculated_hash: [u8; 20] = hasher.finalize_reset().into();
                        let expected_hash = piece_hashes.get(piece_idx).with_context(|| {
                            format!(
                                "bug: no hash for a piece: idx {}, total hashes {}",
                                piece_idx,
                                piece_hashes.len()
                            )
                        })?;

                        if &calculated_hash != expected_hash {
                            // Verification of the current piece has failed, so we should start downloading from it
                            return Ok(Some(piece_idx));
                        }

                        piece_idx += 1;
                    }
                    file_contents_buf.clear();
                }
            }
        }

        Ok(None)
    }

    pub async fn listen_for_blocks(
        &mut self,
        mut rx: mpsc::Receiver<StorageOp>,
        tx: mpsc::Sender<(SocketAddrV4, u32, bool)>,
    ) -> anyhow::Result<()> {
        let mut piece = vec![0; try_into!(self.piece_length, usize)?];

        while let Some(requested_op) = rx.recv().await {
            match requested_op {
                StorageOp::AddBlock(Block {
                    index,
                    begin,
                    mut block,
                }) => {
                    match &mut self.torrent_mode {
                        TorrentMode::SingleFile(_) => {
                            let offset = (index as u64 * self.piece_length) + begin as u64;

                            self.storage.write_all(0, offset, block.as_ref()).with_context(|| {
                                format!(
                                    "error while writing a block to the file: index {}, offset {}",
                                    index, begin
                                )
                            })?;
                        }
                        TorrentMode::MultiFile(file_infos) => {
                            let global_block_offset = (index as u64 * self.piece_length) + begin as u64;

                            let block_len = block.len() as u64;
                            let Some((file_idx, offset, bytes_to_write)) = file_infos
                                .iter()
                                .enumerate()
                                .scan(0, |state, (file_idx, file_info)| {
                                    // We need this to calculate an in-file offset
                                    let prev_length = *state;

                                    // Update the length
                                    *state += file_info.length;

                                    // Look for a matching file
                                    if global_block_offset < *state {
                                        // Check if we cross a file boundary
                                        let lfile_bytes_to_write = if global_block_offset + block_len < *state {
                                            // We don't cross the file boundary
                                            block_len
                                        } else {
                                            // We cross the file boundary and have to do two writes
                                            let rfile_bytes_to_write = global_block_offset + block_len - *state;
                                            block_len - rfile_bytes_to_write
                                        };

                                        let offset_into_file = global_block_offset - prev_length;

                                        Some(Some((file_idx, offset_into_file, lfile_bytes_to_write)))
                                    } else {
                                        Some(None)
                                    }
                                })
                                .find(|el| el.is_some())
                                // We use nested Options to make scan traverse the collection as long as we need
                                // instead of stopping at the first None. Thus, the first Option is always going be
                                // Some.
                                .context("bug: scan returned None?")?
                            else {
                                anyhow::bail!("bug: failed to find a matching file for a block?");
                            };

                            let next_file_bytes = block.split_off(try_into!(bytes_to_write, usize)?);

                            self.storage
                                .write_all(file_idx, offset, block.as_ref())
                                .with_context(|| {
                                    format!(
                                "error while writing a block to the file: file index {}, piece index {}, offset {}",
                                file_idx, index, begin
                            )
                                })?;

                            if !next_file_bytes.is_empty() {
                                self.storage
                                .write_all(file_idx + 1, 0, next_file_bytes.as_ref())
                                .with_context(|| {
                                    format!(
                                    "error while writing a block that crosses file boundary to the rightmost file: file index {}, piece index {}, offset {}",
                                    file_idx + 1, index, begin
                                )
                                })?;
                            }
                        }
                    }
                }
                StorageOp::CheckPieceHash((peer_addr, piece_idx, expected_hash)) => {
                    let mut hasher = Sha1::new();

                    match &mut self.torrent_mode {
                        TorrentMode::SingleFile(_) => {
                            let offset = piece_idx as u64 * self.piece_length;

                            self.storage
                                .read_exact(0, offset, &mut piece)
                                .context("error reading piece")?;
                        }
                        TorrentMode::MultiFile(file_infos) => {
                            let global_piece_offset = piece_idx as u64 * self.piece_length;

                            let Some((file_idx, offset, bytes_to_read)) = file_infos
                                .iter()
                                .enumerate()
                                .scan(0, |state, (file_idx, file_info)| {
                                    // We need this to calculate an in-file offset
                                    let prev_length = *state;

                                    // Update the length
                                    *state += file_info.length;

                                    // Look for a matching file
                                    if global_piece_offset < *state {
                                        // Check if we cross a file boundary
                                        let lfile_bytes_to_write = if global_piece_offset + self.piece_length < *state {
                                            // We don't cross the file boundary
                                            self.piece_length
                                        } else {
                                            // We cross the file boundary and have to do two writes
                                            let rfile_bytes_to_write = global_piece_offset + self.piece_length - *state;
                                            self.piece_length - rfile_bytes_to_write
                                        };

                                        let offset_into_file = global_piece_offset - prev_length;

                                        Some(Some((file_idx, offset_into_file, lfile_bytes_to_write)))
                                    } else {
                                        Some(None)
                                    }
                                })
                                .find(|el| el.is_some())
                                // We use nested Options to make scan traverse the collection as long as we need
                                // instead of stopping at the first None. Thus, the first Option is always going be
                                // Some.
                                .context("bug: scan returned None?")?
                            else {
                                anyhow::bail!("bug: failed to find a matching file for a block?");
                            };

                            self.storage
                                .read_exact(file_idx, offset, &mut piece[..try_into!(bytes_to_read, usize)?])
                                .context("error reading piece")?;

                            if bytes_to_read != self.piece_length {
                                self.storage
                                    .read_exact(file_idx + 1, 0, &mut piece[try_into!(bytes_to_read, usize)?..])
                                    .context("error reading piece")?;
                            }
                        }
                    };

                    hasher.update(&piece);

                    let calculated_hash: [u8; 20] = hasher.finalize().into();

                    tx.send((peer_addr, piece_idx, calculated_hash == expected_hash))
                        .await
                        .context("error sending piece hash verification result")?;
                }
            };
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct TorrentFileInfo {
    path: PathBuf,
    length: u64,
    md5_sum: Option<String>,
}

impl TorrentFileInfo {
    pub fn new(path: PathBuf, length: u64, md5_sum: Option<String>) -> Self {
        TorrentFileInfo { path, length, md5_sum }
    }
}

#[derive(Debug)]
pub enum TorrentMode {
    SingleFile(TorrentFileInfo),
    MultiFile(Vec<TorrentFileInfo>),
}

impl TorrentMode {
    pub fn new(info: &mut Info, base_path: &Path) -> Result<Self> {
        if let Some(files) = info.files.as_deref_mut() {
            let mut torrent_file_infos = Vec::with_capacity(files.len());
            for file in files.iter_mut() {
                let mut path = base_path.to_path_buf();
                file.path.iter().for_each(|path_part| {
                    path.push(path_part);
                });

                torrent_file_infos.push(TorrentFileInfo::new(
                    path,
                    file.length,
                    std::mem::take(&mut file.md5sum),
                ));
            }

            Ok(TorrentMode::MultiFile(torrent_file_infos))
        } else {
            let mut path = base_path.to_path_buf();
            path.push(&info.name);

            let length = info.length.ok_or_else(|| {
                Error::InternalError(
                    "Error while starting up a torrent: the `length` field is missing a single-file download mode",
                )
            })?;

            let torrent_info = TorrentFileInfo::new(path, length, std::mem::take(&mut info.md5sum));

            Ok(TorrentMode::SingleFile(torrent_info))
        }
    }
}
