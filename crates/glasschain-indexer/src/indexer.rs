//! Block and transaction indexer.

use glasschain_core::{Block, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

/// A summarised, queryable block record stored in the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedBlock {
    pub index: u64,
    pub hash: String,
    pub previous_hash: String,
    pub timestamp: u64,
    pub transaction_count: usize,
    pub transaction_ids: Vec<String>,
}

impl From<&Block> for IndexedBlock {
    fn from(b: &Block) -> Self {
        Self {
            index: b.index,
            hash: b.hash.clone(),
            previous_hash: b.previous_hash.clone(),
            timestamp: b.timestamp,
            transaction_count: b.transactions.len(),
            transaction_ids: b.transactions.iter().map(|t| t.id.clone()).collect(),
        }
    }
}

/// A summarised, queryable transaction record stored in the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedTransaction {
    pub id: String,
    pub block_index: u64,
    pub timestamp: u64,
    /// Transaction type discriminant (e.g. `"SupplyOffer"`, `"AssetRegistration"`).
    pub kind: String,
    /// Full JSON payload of the transaction.
    pub payload_json: String,
}

/// Abstraction over the analytical storage backend.
///
/// ## Implementing a PostgreSQL adapter (SQLx)
/// ```rust,ignore
/// use sqlx::PgPool;
/// struct PgIndexer { pool: PgPool }
/// impl IndexerProvider for PgIndexer {
///     fn index_block(&self, block: &glasschain_core::Block) -> Result<(), IndexerError> {
///         // INSERT INTO blocks ...
///         // INSERT INTO transactions ...
///         Ok(())
///     }
///     // ...
/// }
/// ```
pub trait IndexerProvider: Send + Sync {
    /// Index all transactions in a block.
    fn index_block(&self, block: &Block) -> Result<(), IndexerError>;

    /// Retrieve an indexed block summary by its chain index.
    fn get_block(&self, index: u64) -> Result<Option<IndexedBlock>, IndexerError>;

    /// Retrieve an indexed transaction by its ID.
    fn get_transaction(&self, id: &str) -> Result<Option<IndexedTransaction>, IndexerError>;

    /// Return all transaction records for a given block.
    fn transactions_in_block(&self, block_index: u64) -> Result<Vec<IndexedTransaction>, IndexerError>;

    /// Return the total number of indexed blocks.
    fn block_count(&self) -> Result<u64, IndexerError>;

    /// Human-readable name for this indexer backend.
    fn name(&self) -> &str;
}

/// Errors from the indexer layer.
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("storage error: {0}")]
    Storage(String),
}

/// In-memory [`IndexerProvider`] — the default (no-dependency) backend.
///
/// This implementation is suitable for testing, development, and single-node
/// deployments where persistence is handled by the Sled storage layer.
#[derive(Debug, Default)]
pub struct InMemoryIndexer {
    blocks: RwLock<HashMap<u64, IndexedBlock>>,
    transactions: RwLock<HashMap<String, IndexedTransaction>>,
}

impl InMemoryIndexer {
    pub fn new() -> Self {
        Self::default()
    }
}

fn kind_name(tx: &Transaction) -> &'static str {
    match &tx.kind {
        glasschain_core::TransactionKind::SupplyOffer(_) => "SupplyOffer",
        glasschain_core::TransactionKind::PurchaseOrder(_) => "PurchaseOrder",
        glasschain_core::TransactionKind::ContractCreation(_) => "ContractCreation",
        glasschain_core::TransactionKind::ContractExecution(_) => "ContractExecution",
        glasschain_core::TransactionKind::InventoryUpdate(_) => "InventoryUpdate",
        glasschain_core::TransactionKind::AssetRegistration(_) => "AssetRegistration",
    }
}

impl IndexerProvider for InMemoryIndexer {
    fn index_block(&self, block: &Block) -> Result<(), IndexerError> {
        let ib = IndexedBlock::from(block);
        let block_index = block.index;
        self.blocks.write().unwrap().insert(block_index, ib);

        let mut txns = self.transactions.write().unwrap();
        for tx in &block.transactions {
            txns.insert(
                tx.id.clone(),
                IndexedTransaction {
                    id: tx.id.clone(),
                    block_index,
                    timestamp: tx.timestamp,
                    kind: kind_name(tx).to_owned(),
                    payload_json: serde_json::to_string(tx)?,
                },
            );
        }
        Ok(())
    }

    fn get_block(&self, index: u64) -> Result<Option<IndexedBlock>, IndexerError> {
        Ok(self.blocks.read().unwrap().get(&index).cloned())
    }

    fn get_transaction(&self, id: &str) -> Result<Option<IndexedTransaction>, IndexerError> {
        Ok(self.transactions.read().unwrap().get(id).cloned())
    }

    fn transactions_in_block(&self, block_index: u64) -> Result<Vec<IndexedTransaction>, IndexerError> {
        Ok(self
            .transactions
            .read()
            .unwrap()
            .values()
            .filter(|t| t.block_index == block_index)
            .cloned()
            .collect())
    }

    fn block_count(&self) -> Result<u64, IndexerError> {
        Ok(self.blocks.read().unwrap().len() as u64)
    }

    fn name(&self) -> &str {
        "in-memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{InventoryUpdate, Transaction, TransactionKind};

    fn sample_tx() -> Transaction {
        Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-001".into(),
            owner_id: "node-1".into(),
            quantity_delta: 100,
            reason: "test".into(),
        }))
    }

    fn sample_block() -> Block {
        let tx = sample_tx();
        let mut b = Block::new(1, vec![tx], "prevhash".into());
        b.mine(1);
        b
    }

    #[test]
    fn test_index_and_retrieve_block() {
        let indexer = InMemoryIndexer::new();
        let b = sample_block();
        indexer.index_block(&b).unwrap();
        let ib = indexer.get_block(1).unwrap().unwrap();
        assert_eq!(ib.index, 1);
        assert_eq!(ib.transaction_count, 1);
    }

    #[test]
    fn test_retrieve_transaction() {
        let indexer = InMemoryIndexer::new();
        let b = sample_block();
        let tx_id = b.transactions[0].id.clone();
        indexer.index_block(&b).unwrap();
        let it = indexer.get_transaction(&tx_id).unwrap().unwrap();
        assert_eq!(it.id, tx_id);
        assert_eq!(it.kind, "InventoryUpdate");
    }

    #[test]
    fn test_transactions_in_block() {
        let indexer = InMemoryIndexer::new();
        let b = sample_block();
        indexer.index_block(&b).unwrap();
        let txns = indexer.transactions_in_block(1).unwrap();
        assert_eq!(txns.len(), 1);
    }

    #[test]
    fn test_block_count() {
        let indexer = InMemoryIndexer::new();
        indexer.index_block(&sample_block()).unwrap();
        assert_eq!(indexer.block_count().unwrap(), 1);
    }

    #[test]
    fn test_missing_block_returns_none() {
        let indexer = InMemoryIndexer::new();
        assert!(indexer.get_block(99).unwrap().is_none());
    }
}
