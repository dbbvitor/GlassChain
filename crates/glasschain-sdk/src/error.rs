//! Error types for the `glasschain-sdk` crate.

#![allow(clippy::module_name_repetitions)]

use thiserror::Error;

/// All errors that can be produced by the `glasschain-sdk` crate.
///
/// Most variants are self-explanatory.  The two `#[from]` conversions
/// (`Serialization` and `Core`) let callers propagate those underlying errors
/// with `?` without manually wrapping them.
#[derive(Debug, Error)]
pub enum SdkError {
    /// A low-level gRPC transport failure (connection refused, TLS error, …).
    #[error("gRPC transport error: {0}")]
    Transport(String),

    /// The remote node returned a non-OK gRPC status code.
    #[error("gRPC status: {code} — {message}")]
    GrpcStatus {
        /// gRPC status code as a string (e.g. `"NOT_FOUND"`, `"UNAVAILABLE"`).
        code: String,
        /// Human-readable message from the server.
        message: String,
    },

    /// JSON (de)serialisation failed.  Automatically constructed from
    /// [`serde_json::Error`] via the `?` operator.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// An identity or key-material error (wraps a string description so that
    /// the SDK does not expose `glasschain-identity` types in its public API).
    #[error("identity error: {0}")]
    Identity(String),

    /// The remote node explicitly rejected a submitted transaction.
    #[error("transaction rejected: {reason}")]
    TransactionRejected {
        /// Server-supplied rejection reason.
        reason: String,
    },

    /// A requested resource (block, asset, transaction) was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// A `glasschain-core` error bubbled up through the SDK.  Automatically
    /// constructed from [`glasschain_core::CoreError`] via the `?` operator.
    #[error("core error: {0}")]
    Core(#[from] glasschain_core::CoreError),
}
