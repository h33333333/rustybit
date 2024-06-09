use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::Result;

use bittorrent_peer_protocol::{BittorrentP2pMessage, Handshake};

#[derive(Debug)]
struct Frame {
    inner: Vec<u8>,
    offset: usize,
}

impl Frame {
    const DEFAULT_BUFFER_CAPACITY: usize = 16_384 * 2;

    pub fn new() -> Self {
        Frame {
            inner: vec![0; Frame::DEFAULT_BUFFER_CAPACITY],
            offset: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.offset
    }

    pub fn is_empty(&self) -> bool {
        self.offset == 0
    }

    #[tracing::instrument(level = "trace", skip(self))]
    pub fn read_bytes(&mut self, length: usize) -> Option<&[u8]> {
        if length > self.len() {
            tracing::trace!(available = self.len(), requested = length, "didn't have enougth bytes");
            return None;
        }

        self.offset -= length;

        Some(&self.inner[0..length])
    }

    pub fn peek_bytes(&self, length: usize) -> Option<&[u8]> {
        self.inner.get(..length)
    }

    pub fn advance_offset(&mut self, size: usize) {
        self.offset += size;
    }

    pub fn get_buf_of_size(&mut self, size: usize) -> &mut [u8] {
        &mut self.inner[self.offset..self.offset + size]
    }
}

#[derive(Debug)]
pub struct FramedStream<S: AsyncReadExt + AsyncWriteExt + Unpin> {
    stream: S,
    frame: Frame,
}

impl<S: AsyncReadExt + AsyncWriteExt + Unpin> FramedStream<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            frame: Frame::new(),
        }
    }

    #[tracing::instrument(level = "trace", err, skip(self))]
    pub async fn read_from_stream(&mut self, size: usize) -> Result<usize> {
        let buf = self.frame.get_buf_of_size(size);
        let read = self.stream.read(buf).await?;
        self.frame.advance_offset(read);

        Ok(read)
    }

    #[tracing::instrument(level = "trace", err, skip_all)]
    pub async fn flush_to_stream(&mut self, data: &[u8]) -> Result<()> {
        self.stream.write_all(data).await?;
        Ok(self.stream.flush().await?)
    }

    #[tracing::instrument(level = "trace", err, skip(self))]
    pub async fn find_message_length(&mut self) -> Result<Option<usize>> {
        if self.frame.len() < BittorrentP2pMessage::FIXED_PART_LENGTH {
            // TODO: this takes too long (100-300ms)
            self.read_from_stream(BittorrentP2pMessage::FIXED_PART_LENGTH).await?;
        }

        if let Some(length) = self.frame.peek_bytes(4) {
            let length = try_into!(u32::from_be_bytes(try_into!(length, [u8; 4])?), usize)?;

            if self.frame.len() < BittorrentP2pMessage::FIXED_PART_LENGTH + length {
                // TODO: this takes too long (100-300ms)
                self.read_from_stream(BittorrentP2pMessage::FIXED_PART_LENGTH + length - self.frame.len())
                    .await?;
            }

            // We still need to check as we may not have read everything
            return Ok((self.frame.len() >= BittorrentP2pMessage::FIXED_PART_LENGTH + length)
                .then_some(BittorrentP2pMessage::FIXED_PART_LENGTH + length));
        }

        Ok(None)
    }

    pub async fn find_handshake_length(&mut self) -> Result<usize> {
        // Read from stream until we get a handshake
        loop {
            if !self.frame.is_empty() {
                // We have at least 1 byte, checked above
                let pstr_length = self.frame.peek_bytes(1).unwrap()[0] as usize;

                if self.frame.len() >= Handshake::FIXED_PART_LENGTH + pstr_length {
                    return Ok(Handshake::FIXED_PART_LENGTH + pstr_length);
                }

                self.read_from_stream(Handshake::FIXED_PART_LENGTH + pstr_length - self.frame.len())
                    .await?;
            } else {
                // Read pstr length
                self.read_from_stream(1).await?;
            }
        }
    }

    pub fn read_bytes(&mut self, length: usize) -> Option<&[u8]> {
        self.frame.read_bytes(length)
    }
}
