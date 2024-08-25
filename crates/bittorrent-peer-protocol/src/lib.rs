#[macro_use]
mod macros;
mod error;
mod messages;

use std::future::Future;

pub use error::Error;
pub use messages::{BittorrentP2pMessage, Block, BlockRequest, Handshake, MessageId};
use tokio::io::AsyncWriteExt;

pub type Result<T> = std::result::Result<T, Error>;

pub trait Encode {
    fn encode<T>(&self, dst: &mut T) -> impl Future<Output = Result<()>>
    where
        T: AsyncWriteExt + Unpin;
}

pub trait Decode<'a> {
    fn decode(src: &'a [u8]) -> Result<Self>
    where
        Self: Sized;
}
