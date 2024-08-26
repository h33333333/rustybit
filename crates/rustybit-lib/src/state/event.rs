use bittorrent_peer_protocol::Block;

pub enum PeerEvent {
    BlockDownloaded(Block),
    /// Is sent when the download is fully finished or a peer encountered an error
    Disconnected,
}

pub enum TorrentManagerReq {
    CancelPiece(u32),
    Disconnect(&'static str),
}
