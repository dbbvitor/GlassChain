//! Block indexer, Provenance API, and Event Bus for `GlassChain` (Phase 5).
//!
//! ## Components
//!
//! | Module | Purpose |
//! |:-------|:--------|
//! | [`indexer`] | [`IndexerProvider`] trait + [`InMemoryIndexer`] |
//! | [`provenance`] | Chain-of-custody Provenance API |
//! | [`event_bus`] | [`EventBusProvider`] trait + [`InMemoryEventBus`] |
//!
//! ## `SQLx` / `ClickHouse` integration
//!
//! The [`InMemoryIndexer`] is the default (zero-dependency) implementation.
//! A `PostgreSQL` adapter powered by **`SQLx`** can be enabled by implementing the
//! [`IndexerProvider`] trait on a struct that wraps a `sqlx::PgPool`.  The
//! trait methods map directly to SQL `INSERT` / `SELECT` statements.
//!
//! Example schema (PostgreSQL):
//! ```sql
//! CREATE TABLE blocks (
//!   index      BIGINT PRIMARY KEY,
//!   hash       TEXT NOT NULL,
//!   timestamp  BIGINT NOT NULL,
//!   tx_count   INT NOT NULL
//! );
//! CREATE TABLE transactions (
//!   id         TEXT PRIMARY KEY,
//!   block_idx  BIGINT REFERENCES blocks(index),
//!   kind       TEXT NOT NULL,
//!   timestamp  BIGINT NOT NULL,
//!   payload    JSONB NOT NULL
//! );
//! ```
//!
//! ## Kafka / Redpanda integration
//!
//! Replace [`InMemoryEventBus`] with an [`EventBusProvider`] implementation
//! backed by `rdkafka`:
//! ```rust,ignore
//! use rdkafka::producer::{FutureProducer, FutureRecord};
//! struct KafkaEventBus { producer: FutureProducer, topic: String }
//! impl EventBusProvider for KafkaEventBus { ... }
//! ```

pub mod event_bus;
pub mod flattener;
pub mod indexer;
pub mod provenance;

pub use event_bus::{EventBusProvider, InMemoryEventBus, IndexerEvent};
pub use flattener::{AnalyticalFlattener, FlatAssetRecord, FlattenerError, VerifiableLineage};
pub use indexer::{InMemoryIndexer, IndexedBlock, IndexedTransaction, IndexerProvider};
pub use provenance::{CustodyEvent, ProvenanceIndex};
