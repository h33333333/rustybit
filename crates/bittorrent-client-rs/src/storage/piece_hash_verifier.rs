use std::{future::Future, io::SeekFrom, sync::Arc};

use anyhow::Context;
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::RwLock,
    task::JoinSet,
};

use crate::util::piece_size_from_idx;

use super::{util::find_file_offsets_for_data, FileInfo, Storage};

const MAX_VERIFICATION_MEMORY_USAGE_B: usize = 256_000_000;

pub(super) struct PieceHashVerifier {
    storage: Arc<RwLock<dyn Storage + Send + Sync>>,
    piece_length: usize,
    max_parallel_hashing_tasks: usize,
}

impl PieceHashVerifier {
    pub(super) fn new(piece_length: usize, storage: Arc<RwLock<dyn Storage + Send + Sync>>) -> Self {
        let max_parallel_hashing_tasks = 2.max(MAX_VERIFICATION_MEMORY_USAGE_B / piece_length);

        PieceHashVerifier {
            storage,
            piece_length,
            max_parallel_hashing_tasks,
        }
    }

    pub(super) async fn check_all_pieces(
        &self,
        file_infos: &[FileInfo],
        piece_hashes: &[[u8; 20]],
        torrent_length: usize,
    ) -> anyhow::Result<Option<u32>> {
        let mut current_piece_idx = 0;
        let number_of_pieces = piece_hashes.len();

        let mut piece_hash_verification_tasks = JoinSet::new();
        while piece_hash_verification_tasks.len() < self.max_parallel_hashing_tasks
            && current_piece_idx < try_into!(number_of_pieces, u32)?
        {
            let expected_hash = *piece_hashes
                .get(try_into!(current_piece_idx, usize)?)
                .with_context(|| {
                    format!(
                        "bug: no hash for a piece: idx {}, total hashes {}",
                        current_piece_idx,
                        piece_hashes.len()
                    )
                })?;

            piece_hash_verification_tasks.spawn(self.verify_piece_hash(
                file_infos,
                current_piece_idx,
                number_of_pieces,
                torrent_length,
                expected_hash,
            )?);

            current_piece_idx += 1;
        }

        let mut smallest_failining_piece = None;
        while let Some(result) = piece_hash_verification_tasks.join_next().await {
            match result.context("bug: piece hash verification task panicked?")? {
                Ok((piece_idx, is_valid)) => {
                    if !is_valid {
                        match smallest_failining_piece {
                            Some(current_failing_piece_idx) => {
                                let new_smallest_failing_piece = if piece_idx < current_failing_piece_idx {
                                    piece_idx
                                } else {
                                    current_failing_piece_idx
                                };
                                smallest_failining_piece = Some(new_smallest_failing_piece);
                            }
                            None => {
                                smallest_failining_piece = Some(piece_idx);
                            }
                        }
                    }

                    if smallest_failining_piece.is_none() && current_piece_idx < try_into!(number_of_pieces, u32)? {
                        let expected_hash =
                            *piece_hashes
                                .get(try_into!(current_piece_idx, usize)?)
                                .with_context(|| {
                                    format!(
                                        "bug: no hash for a piece: idx {}, total hashes {}",
                                        current_piece_idx,
                                        piece_hashes.len()
                                    )
                                })?;

                        piece_hash_verification_tasks.spawn(self.verify_piece_hash(
                            file_infos,
                            current_piece_idx,
                            number_of_pieces,
                            torrent_length,
                            expected_hash,
                        )?);

                        current_piece_idx += 1;
                    }
                }
                Err(e) => {
                    piece_hash_verification_tasks.abort_all();
                    return Err(e.context("piece hashing task"));
                }
            }
        }

        Ok(smallest_failining_piece)
    }

    pub(super) fn verify_piece_hash(
        &self,
        file_infos: &[FileInfo],
        piece_idx: u32,
        number_of_pieces: usize,
        torrent_length: usize,
        expected_hash: [u8; 20],
    ) -> anyhow::Result<impl Future<Output = anyhow::Result<(u32, bool)>>> {
        let expected_piece_length =
            piece_size_from_idx(number_of_pieces, torrent_length, self.piece_length, piece_idx)?;
        let file_offsets = find_file_offsets_for_data(
            file_infos,
            piece_idx,
            try_into!(self.piece_length, u64)?,
            expected_piece_length,
            None,
        )
        .context("error while finding offsets for a piece")?
        .context("bug: failed to find a matching file for a piece?")?;

        let storage = self.storage.clone();
        Ok(async move {
            let lfile_bytes_to_read = try_into!(file_offsets.lfile_bytes, usize)?;

            let mut lfile = storage
                .read()
                .await
                .get_ro_file(file_offsets.file_idx)
                .context("error while getting a R/O file")?;

            lfile
                .seek(SeekFrom::Start(file_offsets.offset_into_file))
                .await
                .context("error while seeking a piece's position in file")?;

            let mut piece = vec![0; expected_piece_length];
            if let Err(e) = lfile.read_exact(&mut piece[..lfile_bytes_to_read]).await {
                match e.kind() {
                    std::io::ErrorKind::UnexpectedEof => {}
                    _ => anyhow::bail!("error while reading pice from a file: {}", e),
                }
            }

            if file_offsets.rfile_bytes > 0 {
                let mut rfile = storage
                    .read()
                    .await
                    .get_ro_file(file_offsets.file_idx + 1)
                    .context("error while getting a R/O file")?;

                if let Err(e) = rfile
                    .read_exact(&mut piece[lfile_bytes_to_read..expected_piece_length])
                    .await
                {
                    match e.kind() {
                        std::io::ErrorKind::UnexpectedEof => {}
                        _ => anyhow::bail!("error while reading pice from a file: {}", e),
                    }
                }
            }

            let mut hasher = Sha1::new();
            hasher.update(&piece);
            let calculated_hash: [u8; 20] = hasher.finalize().into();

            Ok((piece_idx, calculated_hash == expected_hash))
        })
    }
}
