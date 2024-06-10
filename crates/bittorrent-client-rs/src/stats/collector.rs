use std::sync::Arc;

use anyhow::Context;
use tokio::sync::{mpsc::UnboundedReceiver, Mutex};

use super::{StatsEntry, StatsSharedState};

pub(super) struct StatsCollector {
    state: Arc<Mutex<StatsSharedState>>,
    stats_rx: UnboundedReceiver<StatsEntry>,
}

impl StatsCollector {
    pub fn new(state: Arc<Mutex<StatsSharedState>>, rx: UnboundedReceiver<StatsEntry>) -> Self {
        StatsCollector { state, stats_rx: rx }
    }

    #[tracing::instrument(err, skip_all)]
    pub async fn handle(&mut self) -> anyhow::Result<()> {
        loop {
            match self.stats_rx.recv().await {
                Some(stats) => {
                    let mut state = self.state.lock().await;

                    // Update download progress
                    state.download_progress.0 += stats.downloaded_bytes;
                    state.download_progress.1 -= stats.downloaded_bytes;

                    state.number_of_peers = stats.number_of_peers;

                    let download_duration_secs = stats
                        .downloaded_at
                        .duration_since(state.prev_piece_download_time)
                        .context("bug: received piece SystemTime is in the future?")?
                        .as_secs_f64();
                    let downloaded_piece_size = stats.downloaded_bytes as f64;

                    // Add the most recent download speed

                    let bytes_per_sec = downloaded_piece_size / download_duration_secs;
                    state.speed_stats.push_back(bytes_per_sec);

                    state.prev_piece_download_time = stats.downloaded_at;
                }
                None => {
                    tracing::debug!("all torrent handlers exited, shutting down the stats collector");

                    break Ok(());
                }
            }
        }
    }
}
