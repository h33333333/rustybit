use std::{sync::Arc, time::Duration};

use tokio::{
    sync::{oneshot, Mutex},
    time::interval,
};

use super::StatsSharedState;

pub struct StatsPrinter {
    printing_interval_millis: u64,
    state: Arc<Mutex<StatsSharedState>>,
    total_bytes: usize,
}

impl StatsPrinter {
    const DEFAULT_PRINTING_INTERVAL_MILLIS: u64 = 2000;

    pub fn new(state: Arc<Mutex<StatsSharedState>>, printing_interval: Option<u64>, total_bytes: usize) -> Self {
        StatsPrinter {
            printing_interval_millis: printing_interval.unwrap_or(Self::DEFAULT_PRINTING_INTERVAL_MILLIS),
            state,
            total_bytes,
        }
    }

    pub async fn handle(&self, mut cancellation: oneshot::Receiver<()>) {
        let mut interval = interval(Duration::from_millis(self.printing_interval_millis));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let state = self.state.lock().await;

                    let downloaded_percents = (state.download_progress.0 as f64 / self.total_bytes as f64) * 100.;
                    let avg_bytes_per_sec = state.speed_stats.iter().sum::<f64>() / state.speed_stats.len() as f64;

                    let download_speed_mibs = avg_bytes_per_sec / 1024. / 1024.;

                    let eta_seconds = state.download_progress.1 as f64 / avg_bytes_per_sec;

                    // TODO: handle cases when ETA is NaN
                    tracing::info!(
                        "ETA: {:.3} s - {:.2}% - ↓{:.1} MiB/s - peers: {}",
                        eta_seconds,
                        downloaded_percents,
                        download_speed_mibs,
                        state.number_of_peers
                    );
                }
                _ = &mut cancellation => {
                    break
                }

            }
        }
    }
}
