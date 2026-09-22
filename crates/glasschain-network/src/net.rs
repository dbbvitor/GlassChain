//! TCP transport seam (wayfinder #154): the node's sockets are tokio's real
//! sockets by default and turmoil's simulated sockets while a `turmoil::Sim`
//! is running, so deterministic chaos runs can partition hosts over the
//! simulated network without a compiler-cfg runtime swap.
//!
//! With the `turmoil-sim` feature on, both transports are compiled and the
//! running context picks one, which keeps the feature additive: outside a
//! simulation (including `--all-features` builds) every node still uses real
//! sockets.

#[cfg(not(feature = "turmoil-sim"))]
pub use tokio::net::{TcpListener, TcpStream};

/// Connect to `addr` on the active transport.
#[cfg(not(feature = "turmoil-sim"))]
pub async fn connect(addr: &str) -> std::io::Result<TcpStream> {
    TcpStream::connect(addr).await
}

/// Adopt an already-bound std listener.
#[cfg(not(feature = "turmoil-sim"))]
pub fn from_std(listener: std::net::TcpListener) -> std::io::Result<TcpListener> {
    TcpListener::from_std(listener)
}

#[cfg(feature = "turmoil-sim")]
pub use simulated::{connect, from_std, TcpListener};

#[cfg(feature = "turmoil-sim")]
mod simulated {
    use std::net::SocketAddr;
    use tokio::io::{AsyncRead, AsyncWrite};

    /// A peer connection on whichever transport is active.
    pub type TcpStream = Box<dyn AsyncStream>;

    /// The bounds `tokio_rustls` needs, kept nameable so the boxed stream can
    /// carry them through.
    pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}

    impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

    /// Accepting listener on whichever transport is active.
    pub struct TcpListener(Inner);

    enum Inner {
        Real(tokio::net::TcpListener),
        Sim(turmoil::net::TcpListener),
    }

    impl TcpListener {
        /// # Errors
        ///
        /// Returns the transport's bind error (address in use, permission).
        pub async fn bind(addr: &str) -> std::io::Result<Self> {
            if turmoil::in_simulation() {
                Ok(Self(Inner::Sim(
                    turmoil::net::TcpListener::bind(addr).await?,
                )))
            } else {
                Ok(Self(Inner::Real(
                    tokio::net::TcpListener::bind(addr).await?,
                )))
            }
        }

        /// # Errors
        ///
        /// Returns the transport's accept error.
        pub async fn accept(&self) -> std::io::Result<(TcpStream, SocketAddr)> {
            match &self.0 {
                Inner::Real(listener) => {
                    let (stream, addr) = listener.accept().await?;
                    Ok((Box::new(stream), addr))
                }
                Inner::Sim(listener) => {
                    let (stream, addr) = listener.accept().await?;
                    Ok((Box::new(stream), addr))
                }
            }
        }
    }

    /// # Errors
    ///
    /// Returns the transport's connect error (refused, unreachable).
    pub async fn connect(addr: &str) -> std::io::Result<TcpStream> {
        if turmoil::in_simulation() {
            Ok(Box::new(turmoil::net::TcpStream::connect(addr).await?))
        } else {
            Ok(Box::new(tokio::net::TcpStream::connect(addr).await?))
        }
    }

    /// Simulated ports are claimed by [`TcpListener::bind`] inside the
    /// simulation, so turmoil has nothing to adopt.
    ///
    /// # Errors
    ///
    /// Always returns an error while a simulation is active; the real
    /// transport error otherwise.
    pub fn from_std(listener: std::net::TcpListener) -> std::io::Result<TcpListener> {
        if turmoil::in_simulation() {
            return Err(std::io::Error::other(
                "pre-bound listeners are not supported under turmoil",
            ));
        }
        Ok(TcpListener(Inner::Real(tokio::net::TcpListener::from_std(
            listener,
        )?)))
    }
}
