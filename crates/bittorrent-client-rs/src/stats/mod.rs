use anyhow::Context;
use tokio::sync::{mpsc::UnboundedReceiver, oneshot, Mutex};

use self::{buffer::CircularBuffer, collector::StatsCollector, printer::StatsPrinter};
use std::{sync::Arc, time::SystemTime};

mod buffer;
mod collector;
mod printer;

pub struct StatsEntry {
    downloaded_at: SystemTime,
    number_of_peers: usize,
    downloaded_bytes: usize,
}

impl StatsEntry {
    pub fn new(downloaded_at: SystemTime, peers: usize, downloaded_bytes: usize) -> Self {
        StatsEntry {
            number_of_peers: peers,
            downloaded_at,
            downloaded_bytes,
        }
    }
}

#[tracing::instrument(err, skip_all)]
pub async fn stats(
    downloaded: usize,
    left: usize,
    total: usize,
    rx: UnboundedReceiver<StatsEntry>,
) -> anyhow::Result<()> {
    tracing::debug!("starting the stats collector/printer");

    let state = Arc::new(Mutex::new(StatsSharedState::new(downloaded, left)));

    let mut collector = StatsCollector::new(state.clone(), rx);
    let collector_handle = tokio::spawn(async move { collector.handle().await });

    let printer = StatsPrinter::new(state.clone(), Some(300), total);
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let printer_handle = tokio::spawn(async move { printer.handle(cancel_rx).await });

    collector_handle.await.context("stats collector task")??;

    if cancel_tx.send(()).is_err() {
        tracing::error!("bug: printer exited too early?");
    };

    printer_handle.await.context("stats printer task")?;

    tracing::debug!("successfully stopped stats collector and printer");

    Ok(())
}

struct StatsSharedState {
    prev_piece_download_time: SystemTime,
    speed_stats: CircularBuffer<f64>,
    number_of_peers: usize,
    download_progress: (usize, usize),
}

impl StatsSharedState {
    fn new(downloaded: usize, left: usize) -> Self {
        StatsSharedState {
            prev_piece_download_time: SystemTime::now(),
            speed_stats: CircularBuffer::new(30),
            number_of_peers: 0,
            download_progress: (downloaded, left),
        }
    }
}
