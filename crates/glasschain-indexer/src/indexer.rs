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

/// Build the indexed transactions for `block` — the exact records
/// [`InMemoryIndexer::index_block`] stores.
///
/// # Errors
///
/// Returns [`IndexerError`] if a transaction fails to serialize.
pub fn indexed_transactions_of(block: &Block) -> Result<Vec<IndexedTransaction>, IndexerError> {
    block
        .transactions
        .iter()
        .map(|tx| {
            Ok(IndexedTransaction {
                id: tx.id.clone(),
                block_index: block.index,
                timestamp: tx.timestamp,
                kind: kind_name(tx).to_owned(),
                payload_json: serde_json::to_string(tx)?,
            })
        })
        .collect()
}

/// Abstraction over the analytical storage backend.
///
/// ## Implementing a `PostgreSQL` adapter (`SQLx`)
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
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError`] if serialization fails or the storage backend errors.
    fn index_block(&self, block: &Block) -> Result<(), IndexerError>;

    /// Retrieve an indexed block summary by its chain index.
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError`] if the storage backend errors.
    fn get_block(&self, index: u64) -> Result<Option<IndexedBlock>, IndexerError>;

    /// Retrieve an indexed transaction by its ID.
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError`] if the storage backend errors.
    fn get_transaction(&self, id: &str) -> Result<Option<IndexedTransaction>, IndexerError>;

    /// Return all transaction records for a given block.
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError`] if the storage backend errors.
    fn transactions_in_block(
        &self,
        block_index: u64,
    ) -> Result<Vec<IndexedTransaction>, IndexerError>;

    /// Return the total number of indexed blocks.
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError`] if the storage backend errors.
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
#[derive(Debug)]
pub struct InMemoryIndexer {
    blocks: RwLock<HashMap<u64, IndexedBlock>>,
    transactions: RwLock<HashMap<String, IndexedTransaction>>,
    /// Secondary index: block index → list of transaction IDs.
    block_tx_ids: RwLock<HashMap<u64, Vec<String>>>,
}

impl InMemoryIndexer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            blocks: RwLock::new(HashMap::new()),
            transactions: RwLock::new(HashMap::new()),
            block_tx_ids: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryIndexer {
    fn default() -> Self {
        Self::new()
    }
}

const fn kind_name(tx: &Transaction) -> &'static str {
    match &tx.kind {
        glasschain_core::TransactionKind::SupplyOffer(_) => "SupplyOffer",
        glasschain_core::TransactionKind::PurchaseOrder(_) => "PurchaseOrder",
        glasschain_core::TransactionKind::ContractCreation(_) => "ContractCreation",
        glasschain_core::TransactionKind::ContractExecution(_) => "ContractExecution",
        glasschain_core::TransactionKind::InventoryUpdate(_) => "InventoryUpdate",
        glasschain_core::TransactionKind::AssetRegistration(_) => "AssetRegistration",
        glasschain_core::TransactionKind::CanonicalRecord(_) => "CanonicalRecord",
        glasschain_core::TransactionKind::CapabilityActivation(_) => "CapabilityActivation",
        glasschain_core::TransactionKind::PolicyUpdate(_) => "PolicyUpdate",
    }
}

impl IndexerProvider for InMemoryIndexer {
    fn index_block(&self, block: &Block) -> Result<(), IndexerError> {
        let ib = IndexedBlock::from(block);
        let block_index = block.index;
        self.blocks.write().unwrap().insert(block_index, ib);

        let tx_ids: Vec<String> = block.transactions.iter().map(|t| t.id.clone()).collect();
        self.block_tx_ids
            .write()
            .unwrap()
            .insert(block_index, tx_ids);

        {
            let mut txns = self.transactions.write().unwrap();
            for tx in indexed_transactions_of(block)? {
                txns.insert(tx.id.clone(), tx);
            }
        }
        Ok(())
    }

    fn get_block(&self, index: u64) -> Result<Option<IndexedBlock>, IndexerError> {
        Ok(self.blocks.read().unwrap().get(&index).cloned())
    }

    fn get_transaction(&self, id: &str) -> Result<Option<IndexedTransaction>, IndexerError> {
        Ok(self.transactions.read().unwrap().get(id).cloned())
    }

    fn transactions_in_block(
        &self,
        block_index: u64,
    ) -> Result<Vec<IndexedTransaction>, IndexerError> {
        let result = {
            let ids = self.block_tx_ids.read().unwrap();
            let txns = self.transactions.read().unwrap();
            ids.get(&block_index)
                .map(|id_list| {
                    id_list
                        .iter()
                        .filter_map(|id| txns.get(id).cloned())
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(result)
    }

    fn block_count(&self) -> Result<u64, IndexerError> {
        Ok(self.blocks.read().unwrap().len() as u64)
    }

    fn name(&self) -> &'static str {
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

    #[test]
    fn test_transactions_in_block_unknown_returns_empty() {
        let indexer = InMemoryIndexer::new();
        let txns = indexer.transactions_in_block(42).unwrap();
        assert!(txns.is_empty());
    }

    #[test]
    fn test_transactions_in_block_multiple() {
        let indexer = InMemoryIndexer::new();

        let tx1 = sample_tx();
        let tx2 = sample_tx();
        let mut b = Block::new(2, vec![tx1, tx2], "prevhash".into());
        b.mine(1);
        indexer.index_block(&b).unwrap();

        let txns = indexer.transactions_in_block(2).unwrap();
        assert_eq!(txns.len(), 2);
    }

    fn canonical_tx() -> Transaction {
        Transaction::new(TransactionKind::CanonicalRecord(
            glasschain_core::CanonicalRecord::new(
                0,
                "lot",
                std::collections::BTreeMap::from([
                    ("lot_id".into(), serde_json::json!("lot-1")),
                    ("product_id".into(), serde_json::json!("SKU-1")),
                    ("batch_number".into(), serde_json::json!("BATCH-001")),
                ]),
                "node-1",
            ),
        ))
    }

    #[test]
    fn test_kind_name_all_variants() {
        use glasschain_core::{
            ContractExecution, PurchaseConditions, PurchaseOrder, SmartContractDef, SupplyOffer,
            TraceableAsset, TraceableAssetRegistration,
        };

        let conditions = PurchaseConditions {
            max_price_per_unit: 100,
            min_quantity: 1,
            max_quantity: 10,
            max_lead_time_days: 5,
            preferred_seller_id: None,
            currency: "USD".into(),
            auto_execute: true,
        };
        let cases = vec![
            (sample_tx(), "InventoryUpdate"),
            (
                Transaction::new(TransactionKind::SupplyOffer(SupplyOffer {
                    product_id: "SKU-1".into(),
                    product_name: "Drug A".into(),
                    seller_id: "node-1".into(),
                    quantity_available: 100,
                    price_per_unit: 1500,
                    lead_time_days: 3,
                    currency: "USD".into(),
                })),
                "SupplyOffer",
            ),
            (
                Transaction::new(TransactionKind::PurchaseOrder(PurchaseOrder {
                    product_id: "SKU-1".into(),
                    buyer_id: "node-2".into(),
                    seller_id: "node-1".into(),
                    quantity: 5,
                    agreed_price_per_unit: 1500,
                    currency: "USD".into(),
                    contract_id: None,
                })),
                "PurchaseOrder",
            ),
            (
                Transaction::new(TransactionKind::ContractCreation(SmartContractDef {
                    contract_id: "c-1".into(),
                    buyer_id: "node-2".into(),
                    product_id: "SKU-1".into(),
                    conditions,
                    wasm_code_b64: None,
                })),
                "ContractCreation",
            ),
            (
                Transaction::new(TransactionKind::ContractExecution(ContractExecution {
                    contract_id: "c-1".into(),
                    purchase_order_tx_id: "po-1".into(),
                    buyer_id: "node-2".into(),
                    seller_id: "node-1".into(),
                    product_id: "SKU-1".into(),
                    quantity: 5,
                    total_price: 7500,
                    currency: "USD".into(),
                })),
                "ContractExecution",
            ),
            (
                Transaction::new(TransactionKind::AssetRegistration(
                    TraceableAssetRegistration {
                        asset: TraceableAsset {
                            gtin: Some("07891234100016".into()),
                            batch_number: Some("BATCH-001".into()),
                            expiry_date: Some("2027-12-31".into()),
                            serial_number: Some("SN-001".into()),
                            anvisa_registration: None,
                            manufacturer_id: None,
                            product_name: "Drug A".into(),
                            custodian_id: "node-1".into(),
                            country_of_origin: None,
                            storage_temp_celsius: None,
                            quantity: 1,
                        },
                        event_type: "manufacture".into(),
                        originator_id: "node-1".into(),
                        purchase_order_ref: None,
                    },
                )),
                "AssetRegistration",
            ),
            (canonical_tx(), "CanonicalRecord"),
        ];
        for (tx, expected) in cases {
            assert_eq!(kind_name(&tx), expected);
        }
    }
}
