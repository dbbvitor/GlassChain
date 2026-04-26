//! gRPC service layer for `GlassChain` (Phase 1 — Tonic + Prost).
//!
//! This crate exposes a fully type-safe gRPC API for:
//! - Reading the chain (`LedgerService`)
//! - Submitting transactions
//! - Node administration (`NodeService`)
//!
//! ## Proto definition
//! The service contract is defined in
//! `proto/glasschain/v1/glasschain.proto` (package `glasschain.v1`) and
//! compiled at build time by `tonic-build`.  The generated code lives in
//! [`proto::glasschain_v1`].
//!
//! ## Running the server
//! ```rust,no_run
//! use glasschain_network::Node;
//! use glasschain_rpc::server::GlasschainServer;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let node = Arc::new(Node::new("node-1", "0.0.0.0:8000", 2));
//!     GlasschainServer::new(node)
//!         .serve("[::1]:50051".parse().unwrap())
//!         .await
//!         .unwrap();
//! }
//! ```

pub mod server;

/// Auto-generated protobuf/gRPC code for `package glasschain.v1`.
pub mod proto {
    pub mod glasschain_v1 {
        tonic::include_proto!("glasschain.v1");
    }
}

pub use server::GlasschainServer;
