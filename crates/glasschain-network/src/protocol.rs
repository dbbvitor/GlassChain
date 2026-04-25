use glasschain_core::{Block, Transaction};
use serde::{Deserialize, Serialize};

/// Maximum wire-frame size accepted (16 MiB).
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Every message exchanged between GlassChain peers is one of these variants.
///
/// Messages are serialised as JSON and framed with a 4-byte big-endian length
/// prefix, so each variant must be small enough to fit within
/// [`MAX_MESSAGE_SIZE`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "msg", content = "data")]
pub enum Message {
    /// Initial handshake sent when a TCP connection is established.
    Hello {
        /// The sender's node identifier.
        node_id: String,
        /// The sender's chain length (used for chain-sync decisions).
        chain_length: u64,
        /// Protocol version string (e.g. `"glasschain/1"`).
        version: String,
    },

    /// Broadcast a new transaction to all connected peers.
    Transaction(Transaction),

    /// Announce a newly mined block.
    Block(Block),

    /// Ask a peer to send its full chain.
    RequestChain,

    /// Response to [`Message::RequestChain`]: the sender's full chain.
    Chain(Vec<Block>),

    /// Ask a peer for its list of known peer addresses.
    RequestPeers,

    /// Response to [`Message::RequestPeers`]: list of `"host:port"` strings.
    Peers(Vec<String>),

    /// Graceful disconnect notification.
    Goodbye { reason: String },
}

/// Current protocol version string.
pub const PROTOCOL_VERSION: &str = "glasschain/1";
