// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
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
//! | [`redb_backend`]   | Pure-Rust embedded ACID KV store (recommended) |
//! | [`transient`]      | TTL-scoped private-payload side store          |
//!
//! The `redb` backend is suitable for single-node and moderate-load
//! deployments. For high-throughput production clusters, another adapter
//! following the same [`StorageProvider`][glasschain_core::StorageProvider]
//! trait can be dropped in without changing any node code.

pub mod redb_backend;
pub mod transient;

pub use redb_backend::RedbStorageProvider;
pub use transient::{TransientStore, TRANSIENT_PREFIX};
