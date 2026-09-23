// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
use crate::error::NetworkError;
use crate::protocol::{Message, MAX_MESSAGE_SIZE};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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

// ── Framed connection ─────────────────────────────────────────────────────────

/// Wire format:
/// ```text
/// ┌─────────────────────────────────────────────────────────┐
/// │  4 bytes (big-endian u32) – JSON payload length in bytes│
/// │  N bytes – UTF-8 JSON payload                           │
/// └─────────────────────────────────────────────────────────┘
/// ```
///
/// For concurrent reading and writing, split the stream with
/// [`tokio::net::TcpStream::into_split`] and wrap the halves in a
/// [`PeerReader`] / [`PeerWriter`] pair that can each run in its own task.
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn test_framed_roundtrip() {
        let (client, server) = duplex(1024);
        let mut writer = PeerWriter::new(client, "peer:8000".into());
        let mut reader = PeerReader::new(server, "peer:8000".into());

        writer
            .send(&Message::Goodbye {
                reason: "bye".into(),
            })
            .await
            .unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), reader.receive())
            .await
            .expect("receive must not block")
            .unwrap();
        assert!(matches!(
            received,
            Message::Goodbye { reason } if reason == "bye"
        ));
    }

    #[tokio::test]
    async fn test_oversized_frame_rejected() {
        let (mut client, server) = duplex(1024);
        let mut reader = PeerReader::new(server, "peer:8000".into());

        client
            .write_all(
                &(u32::try_from(MAX_MESSAGE_SIZE).expect("16 MiB fits u32") + 1).to_be_bytes(),
            )
            .await
            .unwrap();
        assert!(matches!(
            reader.receive().await,
            Err(NetworkError::MessageTooLarge { .. })
        ));
    }

    #[tokio::test]
    async fn test_clean_eof_maps_to_disconnect() {
        let (client, server) = duplex(64);
        let mut reader = PeerReader::new(server, "peer:8000".into());
        drop(client);
        assert!(matches!(
            reader.receive().await,
            Err(NetworkError::PeerDisconnected(_))
        ));
    }
    #[tokio::test]
    async fn oversized_payload_send_is_refused() {
        // A sink never blocks, so a mutant that skips the size guard fails the
        // assertion instead of hanging on a full duplex buffer.
        let mut writer = PeerWriter::new(tokio::io::sink(), "peer:8000".into());
        let message = Message::Goodbye {
            reason: "x".repeat(MAX_MESSAGE_SIZE + 1),
        };
        assert!(matches!(
            writer.send(&message).await,
            Err(NetworkError::MessageTooLarge { .. })
        ));
    }

    /// A reader that yields `data`, then fails every further read with `kind`.
    struct ErrorAfter {
        data: Vec<u8>,
        pos: usize,
        kind: std::io::ErrorKind,
    }

    impl ErrorAfter {
        fn new(data: Vec<u8>, kind: std::io::ErrorKind) -> Self {
            Self { data, pos: 0, kind }
        }
    }

    impl tokio::io::AsyncRead for ErrorAfter {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let this = self.get_mut();
            if this.pos < this.data.len() {
                let end = (this.pos + buf.remaining()).min(this.data.len());
                let chunk = this.data[this.pos..end].to_vec();
                buf.put_slice(&chunk);
                this.pos = end;
                std::task::Poll::Ready(Ok(()))
            } else {
                std::task::Poll::Ready(Err(std::io::Error::new(this.kind, "injected")))
            }
        }
    }

    /// A non-EOF error on the length header is an I/O error, not a clean
    /// disconnect.
    #[tokio::test]
    async fn non_eof_error_on_length_header_is_io() {
        let reader = ErrorAfter::new(Vec::new(), std::io::ErrorKind::ConnectionReset);
        let mut peer = PeerReader::new(reader, "peer:8000".into());
        assert!(matches!(peer.receive().await, Err(NetworkError::Io(_))));
    }

    /// A non-EOF error on the body is an I/O error, not a clean disconnect.
    #[tokio::test]
    async fn non_eof_error_on_body_is_io() {
        let header = 5u32.to_be_bytes().to_vec();
        let reader = ErrorAfter::new(header, std::io::ErrorKind::ConnectionReset);
        let mut peer = PeerReader::new(reader, "peer:8000".into());
        assert!(matches!(peer.receive().await, Err(NetworkError::Io(_))));
    }

    /// EOF after a valid header is a clean disconnect, not an I/O error.
    #[tokio::test]
    async fn truncated_body_maps_to_disconnect() {
        let header = 5u32.to_be_bytes().to_vec();
        let reader = ErrorAfter::new(header, std::io::ErrorKind::UnexpectedEof);
        let mut peer = PeerReader::new(reader, "peer:8000".into());
        assert!(matches!(
            peer.receive().await,
            Err(NetworkError::PeerDisconnected(_))
        ));
    }

    /// A frame of exactly `MAX_MESSAGE_SIZE` bytes is in bounds: the guard is
    /// `>`, so it must reach the body read (here EOF) rather than be rejected.
    #[tokio::test]
    async fn length_exactly_at_max_is_not_rejected() {
        let header = u32::try_from(MAX_MESSAGE_SIZE)
            .expect("16 MiB fits u32")
            .to_be_bytes()
            .to_vec();
        let reader = ErrorAfter::new(header, std::io::ErrorKind::UnexpectedEof);
        let mut peer = PeerReader::new(reader, "peer:8000".into());
        assert!(matches!(
            peer.receive().await,
            Err(NetworkError::PeerDisconnected(_))
        ));
    }

    /// A payload of exactly `MAX_MESSAGE_SIZE` bytes is accepted by the send
    /// guard (strict `>`), not refused.
    #[tokio::test]
    async fn payload_exactly_at_max_is_sent() {
        let overhead = serde_json::to_vec(&Message::Goodbye {
            reason: String::new(),
        })
        .expect("serializes")
        .len();
        let message = Message::Goodbye {
            reason: "x".repeat(MAX_MESSAGE_SIZE - overhead),
        };
        assert_eq!(
            serde_json::to_vec(&message).expect("serializes").len(),
            MAX_MESSAGE_SIZE
        );
        let mut writer = PeerWriter::new(tokio::io::sink(), "peer:8000".into());
        writer
            .send(&message)
            .await
            .expect("an exactly-max payload must be sent, not refused");
    }
}
