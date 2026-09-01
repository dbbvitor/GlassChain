use glasschain_core::{Block, CapabilityAdvertisement, Transaction};
use serde::{Deserialize, Serialize};

/// Maximum wire-frame size accepted (16 MiB).
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Every message exchanged between `GlassChain` peers is one of these variants.
///
/// Messages are serialised as JSON and framed with a 4-byte big-endian length
/// prefix, so each variant must be small enough to fit within
/// [`MAX_MESSAGE_SIZE`].
// Inline payloads preserve the stable JSON protocol shape and public API.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "msg", content = "data")]
pub enum Message {
    /// Initial handshake sent when a TCP connection is established.
    Hello {
        /// The sender's node identifier.
        node_id: String,
        /// Hex-encoded fingerprint of the sender's TLS certificate.
        ///
        /// Peers should verify that this matches the certificate fingerprint
        /// observed during the TLS session before trusting the advertised
        /// identity and listen address.
        tls_cert_fingerprint: String,
        /// The sender's chain length (used for chain-sync decisions).
        chain_length: u64,
        /// Protocol version string (e.g. `"glasschain/3"`).
        version: String,
        /// Capabilities this peer supports (ADR-010 decision 6). Peers lacking
        /// an active capability are treated as read-only observers.
        #[serde(default)]
        capabilities: Vec<CapabilityAdvertisement>,
        /// The sender's organization (the collection-membership principal,
        /// ADR-003). Defaults keep pre-`/3` peers decodable.
        #[serde(default)]
        org: String,
        /// The sender's stable TCP listening address (e.g. `"192.168.1.5:8000"`).
        ///
        /// Peers must use this address (rather than the TCP source address, which
        /// is ephemeral for inbound connections) when adding the sender to their
        /// known-peers set and when reconnecting.
        listen_addr: String,
    },

    /// Broadcast a new transaction to all connected peers.
    Transaction(Transaction),

    /// Announce a newly mined block.
    Block(Block),

    /// A private data collection payload, sent **point-to-point** between
    /// collection members only (ADR-003, ticket #46) — never broadcast. The
    /// receiver's node verifies its own membership, the sender's membership,
    /// and `commitment == sha256(payload)` before holding the payload in its
    /// transient store; the globally replicated chain carries only the
    /// commitment (via [`Block`]'s redacted write set).
    PrivatePayload {
        /// The collection this payload belongs to.
        collection: String,
        /// SHA-256 of `payload` — the exact commitment the chain records.
        commitment: String,
        /// The private payload bytes (never replicated globally).
        payload: Vec<u8>,
    },

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
///
/// `/3` adds the private-payload wire message (ADR-003, ticket #46): private
/// data collections exist on the transport, so a `/2` peer can neither send
/// nor receive member-gated payloads. The version gate keeps such peers out of
/// the mesh instead of letting them silently miss private writes. (The `/2`
/// bump marked the BFT consensus seam.)
pub const PROTOCOL_VERSION: &str = "glasschain/3";
