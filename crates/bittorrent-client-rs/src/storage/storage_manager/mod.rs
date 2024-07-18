mod file_metadata;

use std::{net::SocketAddrV4, path::Path, sync::Arc};

use file_metadata::TorrentFileMetadata;

use anyhow::Context;
use bittorrent_peer_protocol::Block;
use tokio::sync::{mpsc, RwLock};

use crate::parser::Info;

use super::{
    file_storage::FileStorage, piece_hash_verifier::PieceHashVerifier, util::find_file_offsets_for_data, Storage,
    StorageOp,
};

pub struct StorageManager {
    storage: Arc<RwLock<dyn Storage + Send + Sync>>,
    piece_hash_verifier: PieceHashVerifier,
    file_metadata: TorrentFileMetadata,
    piece_length: u64,
    number_of_pieces: usize,
    total_torrent_length: usize,
}

impl StorageManager {
    pub fn new(torrent_info: &mut Info, base_path: &Path, total_torrent_length: u64) -> anyhow::Result<StorageManager> {
        let torrent_mode = TorrentFileMetadata::new(torrent_info, base_path)?;

        let paths = torrent_mode
            .file_infos
            .iter()
            .map(|file_info| (file_info.path.as_path(), file_info.length))
            .collect::<Vec<(&Path, u64)>>();

        let storage: Arc<RwLock<dyn Storage + Send + Sync>> = Arc::new(RwLock::new(
            FileStorage::new(&paths).context("error while creating an FS backend")?,
        ));

        let piece_hash_verifier = PieceHashVerifier::new(try_into!(torrent_info.piece_length, usize)?, storage.clone());

        Ok(StorageManager {
            storage,
            piece_hash_verifier,
            file_metadata: torrent_mode,
            piece_length: torrent_info.piece_length,
            number_of_pieces: torrent_info.pieces.len() / 20,
            total_torrent_length: try_into!(total_torrent_length, usize)?,
        })
    }

    pub async fn checksum_verification(&mut self, piece_hashes: &[[u8; 20]]) -> anyhow::Result<Option<u32>> {
        self.piece_hash_verifier
            .check_all_pieces(
                self.file_metadata.file_infos.as_slice(),
                piece_hashes,
                self.total_torrent_length,
            )
            .await
            .context("verifying pieces failed")
    }

    pub async fn listen_for_blocks(
        &mut self,
        mut rx: mpsc::Receiver<StorageOp>,
        tx: mpsc::Sender<(SocketAddrV4, u32, bool)>,
    ) -> anyhow::Result<()> {
        while let Some(requested_op) = rx.recv().await {
            match requested_op {
                StorageOp::AddBlock(Block { index, begin, block }) => {
                    let file_offsets = find_file_offsets_for_data(
                        &self.file_metadata.file_infos,
                        index,
                        self.piece_length,
                        block.len(),
                        Some(begin),
                    )
                    .context("error while finding offsets for a block")?
                    .context("bug: failed to find a matching file for a block?")?;

                    let lfile_bytes = try_into!(file_offsets.lfile_bytes, usize)?;
                    self.storage
                        .write()
                        .await
                        .write_all(
                            file_offsets.file_idx,
                            file_offsets.offset_into_file,
                            &block[0..lfile_bytes],
                        )
                        .with_context(|| {
                            format!(
                                "error while writing a block to the file: file index {}, piece index {}, offset {}",
                                file_offsets.file_idx, index, begin
                            )
                        })?;

                    if file_offsets.rfile_bytes > 0 {
                        self.storage.write().await
                                .write_all(file_offsets.file_idx + 1, 0, &block[lfile_bytes..])
                                .with_context(|| {
                                    format!(
                                    "error while writing a block that crosses file boundary to the rightmost file: file index {}, piece index {}, offset {}",
                                    file_offsets.file_idx + 1, index, begin
                                )
                                })?;
                    }
                }
                StorageOp::CheckPieceHash((peer_addr, piece_idx, expected_hash)) => {
                    let (_, is_correct) = self
                        .piece_hash_verifier
                        .verify_piece_hash(
                            self.file_metadata.file_infos.as_slice(),
                            piece_idx,
                            self.number_of_pieces,
                            self.total_torrent_length,
                            expected_hash,
                        )
                        .context("error while creating a piece hash verifier task")?
                        .await
                        .context("error while verifying piece hash")?;

                    tx.send((peer_addr, piece_idx, is_correct))
                        .await
                        .context("error sending piece hash verification result")?;
                }
            };
        }

        Ok(())
    }
}
