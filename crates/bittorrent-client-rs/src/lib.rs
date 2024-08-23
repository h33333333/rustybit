#[macro_use]
pub mod macros;

pub mod args;
pub mod logging;
pub mod parser;
pub mod stats;
pub mod torrent;
pub mod torrent_meta;
pub mod tracker;
pub mod util;

mod buffer;
mod peer;
mod peer_connection_manager;
mod state;
mod storage;

use std::{future::Future, time::Duration};

pub use peer::handle_peer;
pub use state::torrent::{Torrent, TorrentSharedState};
pub use storage::{FileStorage, PieceHashVerifier, Storage, StorageManager, TorrentFileMetadata};

pub(crate) const DEFAULT_BLOCK_SIZE: u32 = 16_384;

pub trait WithTimeout<T, E> {
    fn with_timeout(
        self,
        name: &'static str,
        timeout: Duration,
    ) -> impl std::future::Future<Output = anyhow::Result<T>> + Send
    where
        Self: Future<Output = std::result::Result<T, E>>;
}

impl<F, T, E> WithTimeout<T, E> for F
where
    F: Future<Output = std::result::Result<T, E>> + Send,
    anyhow::Error: From<E>,
{
    async fn with_timeout(self, name: &'static str, timeout: Duration) -> anyhow::Result<T> {
        match tokio::time::timeout(timeout, self).await {
            Ok(result) => Ok(result?),
            Err(elapsed) => {
                anyhow::bail!("'{}' task timed out: {}", name, elapsed);
            }
        }
    }
}

pub trait Elapsed<T> {
    fn with_elapsed(
        self,
        name: &'static str,
        threshold: Option<Duration>,
    ) -> impl std::future::Future<Output = T> + Send
    where
        Self: Future<Output = T>;
}

impl<F, T> Elapsed<T> for F
where
    F: Future<Output = T> + Send,
{
    async fn with_elapsed(self, name: &'static str, expected: Option<Duration>) -> T
    where
        Self: Future<Output = T>,
    {
        let start = std::time::Instant::now();
        let result = self.await;

        let elapsed = start.elapsed();
        if expected.is_some_and(|expected| elapsed > expected) {
            // SAFETY: checked above
            tracing::trace!( expected = ?expected.unwrap(), ?elapsed, "'{}' task took more time than expected to execute", name);
        } else {
            tracing::trace!("'{}' task took {:?} to execute", name, elapsed);
        }

        result
    }
}
