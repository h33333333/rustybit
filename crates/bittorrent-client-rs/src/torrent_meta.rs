use crate::Result;

#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub info_hash: [u8; 20],
    pub piece_size: usize,
    pub number_of_pieces: usize,
    pub total_length: usize,
}

impl TorrentMeta {
    pub fn new(info_hash: [u8; 20], piece_size: usize, total_length: usize, number_of_pieces: usize) -> Result<Self> {
        Ok(TorrentMeta {
            info_hash,
            piece_size,
            number_of_pieces,
            total_length,
        })
    }
}
