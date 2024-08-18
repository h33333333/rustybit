use tokio::{sync::oneshot, time::Instant};

use self::buffer::CircularBuffer;
use std::{
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    time::Duration,
};

mod buffer;

pub static DOWNLOADED_BYTES: AtomicUsize = AtomicUsize::new(0);
pub static DOWNLOADED_PIECES: AtomicUsize = AtomicUsize::new(0);
pub static NUMBER_OF_PEERS: AtomicU8 = AtomicU8::new(0);

struct Snapshot {
    time: Instant,
    downloaded_bytes: usize,
    bytes_left: usize,
    kibps: f64,
    number_of_peers: u8,
}

impl Snapshot {
    fn new(time: Instant, downloaded: usize, left: usize, download_speed: f64, number_of_peers: u8) -> Self {
        Snapshot {
            time,
            downloaded_bytes: downloaded,
            bytes_left: left,
            kibps: download_speed,
            number_of_peers,
        }
    }
}

pub struct Stats {
    snapshots: CircularBuffer<Snapshot>,
    total_length: usize,
}

impl Stats {
    pub fn new(downloaded: usize, left: usize, total_length: usize) -> Self {
        let mut snapshots = CircularBuffer::new(15);
        snapshots.push_back(Snapshot::new(Instant::now(), downloaded, left, 0., 0));

        Stats {
            snapshots,
            total_length,
        }
    }

    pub async fn collect_stats(&mut self, mut cancellation: oneshot::Receiver<()>) {
        tracing::debug!("starting the stats collector");

        let mut stats_collecting_interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                snapshot_time = stats_collecting_interval.tick() => {
                    self.snapshot(snapshot_time);
                    self.print_stats();
                }
                _ = &mut cancellation => {
                    // We don't care about the result, we exit either way
                    break;
                }
            }
        }

        tracing::debug!("shutting down the stats collector");
    }

    fn print_stats(&self) {
        let last_snapshot = self.snapshots.get_last().expect("not a single snapshot?");

        let download_completion_percent = {
            let downloaded_kb = (last_snapshot.downloaded_bytes / 1000) as f64;
            let total_kb = (self.total_length / 1000) as f64;
            if total_kb == 0. {
                100.
            } else {
                (downloaded_kb / total_kb) * 100.
            }
        };

        let (download_speed, measurement_unit, eta_seconds) = {
            let average_kibps =
                self.snapshots.iter().map(|snapshot| snapshot.kibps).sum::<f64>() / self.snapshots.len() as f64;

            let eta_seconds = if average_kibps == 0. {
                f64::INFINITY
            } else {
                last_snapshot.bytes_left as f64 / (average_kibps * 1024.)
            };

            if average_kibps >= 512. {
                // Convert to mibps
                (average_kibps / 1024., "MiB", eta_seconds)
            } else {
                (average_kibps, "KiB", eta_seconds)
            }
        };

        tracing::info!(
            "ETA: {:.2} s - {:.2}% - ↓{:.1} {}/s - peers: {}",
            eta_seconds,
            download_completion_percent,
            download_speed,
            measurement_unit,
            last_snapshot.number_of_peers
        );
    }

    fn snapshot(&mut self, snapshot_time: Instant) {
        let downloaded_bytes = DOWNLOADED_BYTES.load(Ordering::Relaxed);
        let number_of_peers = NUMBER_OF_PEERS.load(Ordering::Relaxed);

        // SAFETY: we always have at least one snapshot in the buffer, as we add the first one
        // ourselves
        let prev_snapshot = self.snapshots.get_last().expect("bug: not a single snapshot?");

        let downloaded_since_last = downloaded_bytes - prev_snapshot.downloaded_bytes;
        let bytes_left = prev_snapshot.bytes_left - downloaded_since_last;
        let time_since_last_snapshot = prev_snapshot.time.elapsed().as_secs_f64();
        let kibps = {
            let downloaded_kibs = (downloaded_since_last / 1024) as f64;
            downloaded_kibs / time_since_last_snapshot
        };

        self.snapshots.push_back(Snapshot::new(
            snapshot_time,
            downloaded_bytes,
            bytes_left,
            kibps,
            number_of_peers,
        ));
    }
}
