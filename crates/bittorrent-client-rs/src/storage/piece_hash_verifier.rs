use std::io::Write;

use anyhow::Context;

use crate::{state::torrent::PieceState, util::piece_size_from_idx};

use super::{
    util::{find_file_offsets_for_data, read_data_from_files},
    FileInfo, Storage,
};

pub struct PieceHashVerifier {
    piece_length: usize,
    buf: Vec<u8>,
}

impl PieceHashVerifier {
    pub fn new(piece_length: usize) -> Self {
        PieceHashVerifier {
            piece_length,
            buf: vec![0; piece_length],
        }
    }

    pub fn check_all_pieces(
        &mut self,
        storage: &mut dyn Storage,
        file_infos: &[FileInfo],
        piece_hashes: &[[u8; 20]],
        torrent_length: usize,
    ) -> anyhow::Result<(usize, Vec<PieceState>)> {
        let number_of_pieces = piece_hashes.len();
        let mut pieces = (0..number_of_pieces)
            .map(|_| PieceState::Queued)
            .collect::<Vec<PieceState>>();
        let mut verified_pieces = 0;
        for piece_idx in 0..number_of_pieces {
            let expected_piece_hash = piece_hashes.get(piece_idx).context("bug: piece with no hash")?;
            match self
                .verify_piece_hash(
                    storage,
                    file_infos,
                    try_into!(piece_idx, u32)?,
                    number_of_pieces,
                    torrent_length,
                    expected_piece_hash,
                )
                .context("piece hash verification failed")?
            {
                Some(has_matching_hash) => {
                    if has_matching_hash {
                        verified_pieces += 1;
                        pieces[piece_idx] = PieceState::Verified;
                    }
                }
                // Data is missing because the file is incomplete, skip further checks
                None => break,
            };
        }

        Ok((verified_pieces, pieces))
    }

    pub(super) fn verify_piece_hash(
        &mut self,
        storage: &mut dyn Storage,
        file_infos: &[FileInfo],
        piece_idx: u32,
        number_of_pieces: usize,
        torrent_length: usize,
        expected_hash: &[u8; 20],
    ) -> anyhow::Result<Option<bool>> {
        let expected_piece_length =
            piece_size_from_idx(number_of_pieces, torrent_length, self.piece_length, piece_idx)?;
        let file_offsets = find_file_offsets_for_data(file_infos, piece_idx, try_into!(self.piece_length, u64)?, None)
            .context("error while finding offsets for a piece")?
            .context("bug: failed to find a matching file for a piece?")?;

        let had_enough_bytes =
            read_data_from_files(storage, &mut self.buf, file_offsets, file_infos, expected_piece_length)
                .context("error while reading piece from files")?;

        if !had_enough_bytes {
            // Skip hash verification, as it would fail inevitably
            Ok(None)
        } else {
            let mut hasher = crypto_hash::Hasher::new(crypto_hash::Algorithm::SHA1);
            hasher.write_all(&self.buf).context("error while updating hasher")?;
            let mut calculated_hash = [0u8; 20];
            calculated_hash.copy_from_slice(&hasher.finish());
            Ok(Some(&calculated_hash == expected_hash))
        }
    }
}
