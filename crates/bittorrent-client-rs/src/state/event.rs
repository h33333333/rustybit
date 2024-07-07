use bittorrent_peer_protocol::Block;

#[derive(Clone, Debug)]
pub enum SystemEvent {
    /// Contains corresponding piece index
    NewPieceAdded(u32),
    /// Event indicates that peers that didn't have anything to offer previously now can be used to
    /// retry downloading a failed piece if they have it
    PieceFailed(u32),
}

pub enum PeerEvent {
    BlockDownloaded(Block),
    /// Is sent when the download is fully finished or a peer encountered an error
    Disconnected,
}
