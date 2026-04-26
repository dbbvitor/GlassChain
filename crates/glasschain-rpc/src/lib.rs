//! gRPC service layer for GlassChain (Phase 1 — Tonic + Prost).
//!
//! This crate exposes a fully type-safe gRPC API for:
//! - Reading the chain (`LedgerService`)
//! - Submitting transactions
//! - Node administration (`NodeService`)
//!
//! ## Proto definition
//! The service contract is defined in `proto/glasschain.proto` and compiled
//! at build time by `tonic-build`.  The generated code lives in
//! [`proto::glasschain`].
//!
//! ## Running the server
//! ```rust,no_run
//! use glasschain_rpc::server::GlasschainServer;
//!
//! #[tokio::main]
//! async fn main() {
//!     GlasschainServer::new(difficulty: 2)
//!         .serve("[::1]:50051".parse().unwrap())
//!         .await
//!         .unwrap();
//! }
//! ```

pub mod server;

/// Auto-generated protobuf/gRPC code.
pub mod proto {
    pub mod glasschain {
        tonic::include_proto!("glasschain");
    }
}

pub use server::GlasschainServer;
