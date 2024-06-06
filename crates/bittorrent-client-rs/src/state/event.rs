#[derive(Clone, Debug)]
pub enum SystemEvent {
    /// Contains corresponding piece index
    NewPieceAdded(u32),
    /// Event indicates that peers that didn't have anything to offer previously now can be used to
    /// retry downloading a failed piece if they have it
    PieceFailed(u32),
    /// All pieces were downloaded
    DownloadFinished,
}

pub enum PeerEvent {
    /// Contains piece index and data
    PieceDownloaded(u32, Vec<u8>),
    /// Is sent when the download is fully finished or a peer encountered an error
    Disconnected,
}
