mod file_metadata;

use std::net::SocketAddrV4;

pub use file_metadata::TorrentFileMetadata;

use anyhow::Context;
use bittorrent_peer_protocol::Block;
use tokio::sync::mpsc;

use super::{
    piece_hash_verifier::PieceHashVerifier,
    util::{find_file_offsets_for_data, write_data_to_files},
    Storage, StorageOp,
};

pub struct StorageManager<'a> {
    storage: &'a mut dyn Storage,
    piece_hash_verifier: PieceHashVerifier,
    file_metadata: TorrentFileMetadata,
    piece_length: u64,
    number_of_pieces: usize,
    total_torrent_length: usize,
}

impl<'a> StorageManager<'a> {
    pub fn new(
        storage: &'a mut dyn Storage,
        file_metadata: TorrentFileMetadata,
        piece_length: u64,
        piece_hash_verifier: PieceHashVerifier,
        number_of_pieces: usize,
        total_torrent_length: u64,
    ) -> anyhow::Result<Self> {
        Ok(StorageManager {
            storage,
            piece_hash_verifier,
            file_metadata,
            piece_length,
            number_of_pieces,
            total_torrent_length: try_into!(total_torrent_length, usize)?,
        })
    }

    pub async fn checksum_verification(&mut self, piece_hashes: &[[u8; 20]]) -> anyhow::Result<Option<u32>> {
        tracing::info!("Starting checksum verification");
        let starting_piece = self
            .piece_hash_verifier
            .check_all_pieces(
                self.storage,
                self.file_metadata.file_infos.as_slice(),
                piece_hashes,
                self.total_torrent_length,
            )
            .context("checksum verification failed")?;
        if let Some(starting_piece_idx) = starting_piece {
            tracing::info!(%starting_piece_idx, "Finished checksum verification, found a starting piece");
        } else {
            tracing::info!("Finished checksum verification, all files were already downloaded");
        };

        Ok(starting_piece)
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
                        Some(begin),
                    )
                    .context("error while finding offsets for a block")?
                    .context("bug: failed to find a matching file for a block?")?;

                    write_data_to_files(self.storage, &block, file_offsets, &self.file_metadata.file_infos)
                        .context("error while writing block to files")?;
                }
                StorageOp::CheckPieceHash((peer_addr, piece_idx, expected_hash)) => {
                    let verification_result = self
                        .piece_hash_verifier
                        .verify_piece_hash(
                            self.storage,
                            self.file_metadata.file_infos.as_slice(),
                            piece_idx,
                            self.number_of_pieces,
                            self.total_torrent_length,
                            &expected_hash,
                        )
                        .context("error while verifying piece hash")?;

                    tx.send((peer_addr, piece_idx, verification_result))
                        .await
                        .context("error sending piece hash verification result")?;
                }
            };
        }

        Ok(())
    }
}
