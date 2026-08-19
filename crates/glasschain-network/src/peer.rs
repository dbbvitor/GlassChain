use crate::error::NetworkError;
use crate::protocol::{Message, MAX_MESSAGE_SIZE};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

// ── Framed reader ─────────────────────────────────────────────────────────────

/// The read half of a framed peer connection.
///
/// Wraps any `AsyncRead + Unpin + Send` implementation (plain TCP or TLS)
/// via a boxed trait object so the same type can be used for both transports.
pub struct PeerReader {
    stream: Box<dyn AsyncRead + Unpin + Send>,
    /// Remote peer's `"host:port"` address string.
    pub address: String,
}

impl PeerReader {
    pub fn new(stream: impl AsyncRead + Unpin + Send + 'static, address: String) -> Self {
        Self {
            stream: Box::new(stream),
            address,
        }
    }

    /// # Errors
    ///
    /// Returns [`NetworkError`] when the frame cannot be read or decoded.
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

// ── Framed writer ─────────────────────────────────────────────────────────────

/// The write half of a framed peer connection.
pub struct PeerWriter {
    stream: Box<dyn AsyncWrite + Unpin + Send>,
    pub address: String,
}

impl PeerWriter {
    pub fn new(stream: impl AsyncWrite + Unpin + Send + 'static, address: String) -> Self {
        Self {
            stream: Box::new(stream),
            address,
        }
    }

    /// # Errors
    ///
    /// Returns [`NetworkError`] when the message cannot be encoded or written.
    pub async fn send(&mut self, message: &Message) -> Result<(), NetworkError> {
        let payload = serde_json::to_vec(message)?;
        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(NetworkError::MessageTooLarge {
                size: payload.len(),
                max: MAX_MESSAGE_SIZE,
            });
        }
        let len = u32::try_from(payload.len()).map_err(|_| NetworkError::MessageTooLarge {
            size: payload.len(),
            max: MAX_MESSAGE_SIZE,
        })?;
        self.stream.write_all(&len.to_be_bytes()).await?;
        self.stream.write_all(&payload).await?;
        Ok(())
    }
}

// ── Full-duplex connection (kept for backwards-compatibility) ─────────────────

/// A framed, bidirectional message channel over a [`TcpStream`].
///
/// Wire format:
/// ```text
/// ┌─────────────────────────────────────────────────────────┐
/// │  4 bytes (big-endian u32) – JSON payload length in bytes│
/// │  N bytes – UTF-8 JSON payload                           │
/// └─────────────────────────────────────────────────────────┘
/// ```
///
/// For concurrent reading and writing, prefer [`PeerConnection::into_split`]
/// which returns a [`PeerReader`] / [`PeerWriter`] pair that can each run in
/// their own task.
pub struct PeerConnection {
    stream: TcpStream,
    /// Remote peer's node identifier (set after handshake).
    pub peer_id: Option<String>,
    /// Remote peer's `"host:port"` address string.
    pub address: String,
}

impl PeerConnection {
    /// Wrap an already-connected [`TcpStream`].
    pub const fn new(stream: TcpStream, address: String) -> Self {
        Self {
            stream,
            peer_id: None,
            address,
        }
    }

    /// Split into independent read and write halves for concurrent I/O.
    pub fn into_split(self) -> (PeerReader, PeerWriter) {
        let (read_half, write_half) = self.stream.into_split();
        (
            PeerReader::new(read_half, self.address.clone()),
            PeerWriter::new(write_half, self.address),
        )
    }

    /// Send a [`Message`] to the remote peer.
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] when the message cannot be encoded or written.
    pub async fn send(&mut self, message: &Message) -> Result<(), NetworkError> {
        let payload = serde_json::to_vec(message)?;
        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(NetworkError::MessageTooLarge {
                size: payload.len(),
                max: MAX_MESSAGE_SIZE,
            });
        }
        let len = u32::try_from(payload.len()).map_err(|_| NetworkError::MessageTooLarge {
            size: payload.len(),
            max: MAX_MESSAGE_SIZE,
        })?;
        self.stream.write_all(&len.to_be_bytes()).await?;
        self.stream.write_all(&payload).await?;
        Ok(())
    }

    /// Receive the next [`Message`] from the remote peer.
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] when the frame cannot be read or decoded.
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
