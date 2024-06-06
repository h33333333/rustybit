use sha1::{Digest, Sha1};

pub fn calculate_piece_hash(piece: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();

    hasher.update(piece);

    hasher.finalize().into()
}
