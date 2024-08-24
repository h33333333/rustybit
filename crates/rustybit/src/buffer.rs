use anyhow::Context;
use bittorrent_peer_protocol::{BittorrentP2pMessage, Decode, Handshake};
use tokio::{io::AsyncReadExt, net::TcpStream};

pub struct ReadBuf {
    inner: Vec<u8>,
    offset: usize,
    processed: usize,
}

impl ReadBuf {
    const DEFAULT_BUFFER_CAPACITY: usize = 16_384 * 2;

    pub fn new() -> Self {
        ReadBuf {
            inner: vec![0; ReadBuf::DEFAULT_BUFFER_CAPACITY],
            // Filled bytes
            offset: 0,
            // Already processed bytes (i.e. decoded)
            processed: 0,
        }
    }

    #[tracing::instrument(level = "error", err(level = "debug"), skip_all)]
    pub async fn read_handshake(&mut self, stream: &mut TcpStream) -> anyhow::Result<Handshake> {
        // Read from stream until we get a handshake
        loop {
            if self.offset > 0 {
                // We have at least 1 byte, checked above
                let pstr_length = self.inner[0] as usize;

                if self.offset >= Handshake::FIXED_PART_LENGTH + pstr_length {
                    let handshake = Handshake::decode(&self.inner).context("decoding handshake")?;
                    self.processed += Handshake::FIXED_PART_LENGTH + pstr_length;
                    return Ok(handshake);
                }
            }

            let read = stream
                .read(&mut self.inner)
                .await
                .context("reading handshake message")?;
            self.offset += read;
        }
    }

    #[tracing::instrument(level = "error", err(level = "debug"), skip_all)]
    pub async fn read_message(&mut self, stream: &mut TcpStream) -> anyhow::Result<BittorrentP2pMessage> {
        self.reset_processed();
        loop {
            let bytes_left = if self.offset > BittorrentP2pMessage::FIXED_PART_LENGTH {
                let message_length = try_into!(
                    u32::from_be_bytes(try_into!(
                        &self.inner[0..BittorrentP2pMessage::FIXED_PART_LENGTH],
                        [u8; 4]
                    )?),
                    usize
                )?;

                // Check how many bytes we still have to read to have a full message
                let missing_bytes =
                    (BittorrentP2pMessage::FIXED_PART_LENGTH + message_length).saturating_sub(self.offset);

                if missing_bytes == 0 {
                    let message_bytes = &self.inner[0..BittorrentP2pMessage::FIXED_PART_LENGTH + message_length];
                    let message = BittorrentP2pMessage::decode(message_bytes).context("decoding received message")?;
                    self.processed += BittorrentP2pMessage::FIXED_PART_LENGTH + message_length;
                    return Ok(message);
                }

                missing_bytes
            } else {
                BittorrentP2pMessage::FIXED_PART_LENGTH
            };

            // Account for some large messages (e.g. a huge bitfield)
            if self.offset + bytes_left > self.inner.capacity() {
                let to_add = self.offset + bytes_left - self.inner.capacity();
                tracing::trace!(
                    "resizing the buffer: {} -> {}",
                    self.inner.capacity(),
                    self.inner.capacity() + to_add
                );
                self.resize_inner(to_add);
            }

            let read = stream
                .read(&mut self.inner[self.offset..])
                .await
                .context("reading message")?;
            self.offset += read;
        }
    }

    fn reset_processed(&mut self) {
        if self.processed > 0 {
            if self.offset > self.processed {
                self.inner.copy_within(self.processed..self.offset, 0);
            }
            self.offset -= self.processed;
            self.processed = 0;
        }
    }

    fn resize_inner(&mut self, to_add: usize) {
        self.inner.resize(self.inner.len() + to_add, 0);
    }
}
