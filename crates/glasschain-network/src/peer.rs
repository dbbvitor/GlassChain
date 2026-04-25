use crate::error::NetworkError;
use crate::protocol::{Message, MAX_MESSAGE_SIZE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A framed, bidirectional message channel over a [`TcpStream`].
///
/// Wire format:
/// ```text
/// ┌─────────────────────────────────────────────────────────┐
/// │  4 bytes (big-endian u32) – JSON payload length in bytes│
/// │  N bytes – UTF-8 JSON payload                           │
/// └─────────────────────────────────────────────────────────┘
/// ```
pub struct PeerConnection {
    stream: TcpStream,
    /// Remote peer's node identifier (set after handshake).
    pub peer_id: Option<String>,
    /// Remote peer's `"host:port"` address string.
    pub address: String,
}

impl PeerConnection {
    /// Wrap an already-connected [`TcpStream`].
    pub fn new(stream: TcpStream, address: String) -> Self {
        Self {
            stream,
            peer_id: None,
            address,
        }
    }

    /// Send a [`Message`] to the remote peer.
    ///
    /// The message is serialised to JSON, prefixed with a 4-byte big-endian
    /// length, and written atomically.
    pub async fn send(&mut self, message: &Message) -> Result<(), NetworkError> {
        let payload = serde_json::to_vec(message)?;
        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(NetworkError::MessageTooLarge {
                size: payload.len(),
                max: MAX_MESSAGE_SIZE,
            });
        }
        let len = payload.len() as u32;
        self.stream.write_all(&len.to_be_bytes()).await?;
        self.stream.write_all(&payload).await?;
        Ok(())
    }

    /// Receive the next [`Message`] from the remote peer.
    ///
    /// Returns `Err(NetworkError::PeerDisconnected)` when the remote side
    /// closes the connection cleanly (EOF).
    pub async fn receive(&mut self) -> Result<Message, NetworkError> {
        let mut len_buf = [0u8; 4];
        match self.stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(NetworkError::PeerDisconnected(self.address.clone()));
            }
            Err(e) => return Err(NetworkError::Io(e)),
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > MAX_MESSAGE_SIZE {
            return Err(NetworkError::MessageTooLarge {
                size: len,
                max: MAX_MESSAGE_SIZE,
            });
        }
        let mut buf = vec![0u8; len];
        match self.stream.read_exact(&mut buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(NetworkError::PeerDisconnected(self.address.clone()));
            }
            Err(e) => return Err(NetworkError::Io(e)),
        }
        let message = serde_json::from_slice(&buf)?;
        Ok(message)
    }
}
