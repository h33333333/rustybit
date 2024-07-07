use crate::Result;
use rand::{distributions::Alphanumeric, Rng};

pub fn generate_peer_id() -> String {
    let mut rng = rand::thread_rng();
    let mut peer_id = String::with_capacity(20);
    peer_id.push_str("RustyBit-");
    peer_id.extend((0..11).map(|_| rng.sample(Alphanumeric) as char));
    peer_id
}

pub fn piece_size_from_idx(number_of_pieces: usize, total_length: usize, piece_size: usize, idx: u32) -> Result<usize> {
    let size = if number_of_pieces - 1 == try_into!(idx, usize)? {
        let remainder = total_length % piece_size;
        if remainder == 0 {
            piece_size
        } else {
            remainder
        }
    } else {
        piece_size
    };

    Ok(size)
}
