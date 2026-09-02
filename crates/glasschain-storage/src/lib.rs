//! Pluggable storage backends for `GlassChain`.
//!
//! This crate provides concrete implementations of the
//! [`StorageProvider`][glasschain_core::StorageProvider] trait introduced in
//! Phase 1 of the architecture plan.
//!
//! ## Available backends
//!
//! | Feature / Module   | Description                                    |
//! |:-------------------|:-----------------------------------------------|
//! | [`sled_backend`]   | Pure-Rust embedded KV store (recommended)      |
//!
//! The `sled` backend is suitable for single-node and moderate-load
//! deployments.  For high-throughput production clusters, a `RocksDB` adapter
//! following the same [`StorageProvider`][glasschain_core::StorageProvider]
//! trait can be dropped in without changing any node code.

pub mod sled_backend;
pub mod transient;

pub use sled_backend::SledStorageProvider;
pub use transient::{TransientStore, TRANSIENT_PREFIX};
