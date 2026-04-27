//! Event Bus — stream validated transactions to downstream services.
//!
//! The [`EventBusProvider`] trait abstracts over message brokers.  The
//! [`InMemoryEventBus`] is the default (zero-dependency) implementation,
//! suitable for testing and single-process deployments.
//!
//! ## Plugging in Kafka / Redpanda (`rdkafka`)
//!
//! ```rust,ignore
//! use rdkafka::producer::{FutureProducer, FutureRecord};
//! use rdkafka::ClientConfig;
//! use glasschain_indexer::event_bus::{EventBusProvider, IndexerEvent};
//!
//! struct KafkaEventBus {
//!     producer: FutureProducer,
//!     topic: String,
//! }
//!
//! impl KafkaEventBus {
//!     fn new(brokers: &str, topic: &str) -> Self {
//!         let producer: FutureProducer = ClientConfig::new()
//!             .set("bootstrap.servers", brokers)
//!             .create()
//!             .expect("kafka producer");
//!         Self { producer, topic: topic.into() }
//!     }
//! }
//!
//! impl EventBusProvider for KafkaEventBus {
//!     fn publish(&self, event: IndexerEvent) -> Result<(), EventBusError> {
//!         let payload = serde_json::to_string(&event)?;
//!         // rdkafka is async; this is a simplified synchronous example.
//!         let record = FutureRecord::to(&self.topic)
//!             .payload(&payload)
//!             .key(&event.transaction_id);
//!         // self.producer.send(record, Duration::from_secs(5)).await?;
//!         Ok(())
//!     }
//!     fn name(&self) -> &str { "kafka" }
//! }
//! ```

use glasschain_core::Block;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tokio::sync::broadcast;

/// A validated ledger event streamed to downstream consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerEvent {
    /// Type of event: `"block_committed"` or `"transaction_committed"`.
    pub event_type: String,
    /// Block index in which the event was committed.
    pub block_index: u64,
    /// Transaction ID (empty for `"block_committed"` events).
    pub transaction_id: String,
    /// Transaction kind discriminant.
    pub transaction_kind: String,
    /// Unix timestamp of the transaction or block.
    pub timestamp: u64,
    /// Full JSON payload.
    pub payload_json: String,
}

/// Errors from the event bus layer.
#[derive(Debug, thiserror::Error)]
pub enum EventBusError {
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("publish error: {0}")]
    Publish(String),
}

/// Abstraction over a message broker / event streaming system.
///
/// Implement this trait to route `GlassChain` events to Kafka, Redpanda,
/// `RabbitMQ`, or any other message bus.
pub trait EventBusProvider: Send + Sync {
    /// Publish a single event to the bus.
    ///
    /// # Errors
    ///
    /// Returns an [`EventBusError`] if the event could not be serialized or
    /// delivered to the underlying broker.
    fn publish(&self, event: IndexerEvent) -> Result<(), EventBusError>;

    /// Publish all transactions in a block to the bus.
    ///
    /// # Errors
    ///
    /// Returns an [`EventBusError`] if any per-transaction or block-level event
    /// could not be serialized or delivered to the underlying broker.
    fn publish_block(&self, block: &Block) -> Result<(), EventBusError> {
        for tx in &block.transactions {
            let kind = match &tx.kind {
                glasschain_core::TransactionKind::SupplyOffer(_) => "SupplyOffer",
                glasschain_core::TransactionKind::PurchaseOrder(_) => "PurchaseOrder",
                glasschain_core::TransactionKind::ContractCreation(_) => "ContractCreation",
                glasschain_core::TransactionKind::ContractExecution(_) => "ContractExecution",
                glasschain_core::TransactionKind::InventoryUpdate(_) => "InventoryUpdate",
                glasschain_core::TransactionKind::AssetRegistration(_) => "AssetRegistration",
            };
            self.publish(IndexerEvent {
                event_type: "transaction_committed".into(),
                block_index: block.index,
                transaction_id: tx.id.clone(),
                transaction_kind: kind.into(),
                timestamp: tx.timestamp,
                payload_json: serde_json::to_string(&tx)?,
            })?;
        }
        // Also publish a block-level event.
        self.publish(IndexerEvent {
            event_type: "block_committed".into(),
            block_index: block.index,
            transaction_id: String::new(),
            transaction_kind: String::new(),
            timestamp: block.timestamp,
            payload_json: serde_json::json!({
                "index": block.index,
                "hash": block.hash,
                "tx_count": block.transactions.len()
            })
            .to_string(),
        })?;
        Ok(())
    }

    /// Human-readable name for this event bus implementation.
    fn name(&self) -> &str;
}

/// In-memory event bus backed by a `tokio::sync::broadcast` channel.
///
/// All published events are buffered and can be received by multiple async
/// subscribers via [`InMemoryEventBus::subscribe`].
pub struct InMemoryEventBus {
    sender: broadcast::Sender<IndexerEvent>,
    /// Local log of all published events (for test assertions).
    log: Mutex<Vec<IndexerEvent>>,
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl InMemoryEventBus {
    /// Create a new in-memory bus with the given channel buffer capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            log: Mutex::new(Vec::new()),
        }
    }

    /// Subscribe to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<IndexerEvent> {
        self.sender.subscribe()
    }

    /// Return a snapshot of all events published so far.
    ///
    /// # Panics
    ///
    /// Panics if the internal event-log [`Mutex`] is poisoned.
    pub fn event_log(&self) -> Vec<IndexerEvent> {
        self.log.lock().unwrap().clone()
    }
}

impl EventBusProvider for InMemoryEventBus {
    fn publish(&self, event: IndexerEvent) -> Result<(), EventBusError> {
        self.log.lock().unwrap().push(event.clone());
        // Ignore send errors when there are no active receivers.
        let _ = self.sender.send(event);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "in-memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{Block, InventoryUpdate, Transaction, TransactionKind};

    fn sample_block() -> Block {
        let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-001".into(),
            owner_id: "node-1".into(),
            quantity_delta: 50,
            reason: "test".into(),
        }));
        let mut b = Block::new(1, vec![tx], "0".into());
        b.mine(1);
        b
    }

    #[test]
    fn test_publish_and_log() {
        let bus = InMemoryEventBus::default();
        bus.publish(IndexerEvent {
            event_type: "test".into(),
            block_index: 0,
            transaction_id: "tx-1".into(),
            transaction_kind: "InventoryUpdate".into(),
            timestamp: 1000,
            payload_json: "{}".into(),
        })
        .unwrap();
        assert_eq!(bus.event_log().len(), 1);
    }

    #[test]
    fn test_publish_block_generates_tx_plus_block_event() {
        let bus = InMemoryEventBus::default();
        let block = sample_block();
        bus.publish_block(&block).unwrap();
        let log = bus.event_log();
        // 1 tx + 1 block event
        assert_eq!(log.len(), 2);
        assert!(log.iter().any(|e| e.event_type == "transaction_committed"));
        assert!(log.iter().any(|e| e.event_type == "block_committed"));
    }

    #[tokio::test]
    async fn test_subscribe_receives_event() {
        use tokio::time::{timeout, Duration};
        let bus = InMemoryEventBus::default();
        let mut rx = bus.subscribe();
        let event = IndexerEvent {
            event_type: "block_committed".into(),
            block_index: 1,
            transaction_id: String::new(),
            transaction_kind: String::new(),
            timestamp: 2000,
            payload_json: "{}".into(),
        };
        bus.publish(event.clone()).unwrap();
        let received = timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.block_index, 1);
    }

    #[test]
    fn test_event_bus_name() {
        let bus = InMemoryEventBus::default();
        assert_eq!(bus.name(), "in-memory");
    }
}
