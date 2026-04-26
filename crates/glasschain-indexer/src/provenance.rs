//! Verifiable Data Lineage — Provenance API.
//!
//! The [`ProvenanceIndex`] tracks every custody transfer event for every
//! traceable asset, building a complete "Chain of Custody" that can be queried
//! by AI models, auditors, or regulatory agencies.
//!
//! ## Usage
//! ```rust
//! use glasschain_indexer::provenance::{ProvenanceIndex, CustodyEvent};
//!
//! let mut index = ProvenanceIndex::new();
//! index.record_event(CustodyEvent {
//!     asset_id: "GTIN:07891234100016:SN-001".into(),
//!     event_type: "manufacture".into(),
//!     custodian_id: "fabricante-abc".into(),
//!     transaction_id: "tx-001".into(),
//!     block_index: 1,
//!     timestamp: 1_700_000_000,
//! });
//!
//! let history = index.get_custody_chain("GTIN:07891234100016:SN-001");
//! assert_eq!(history.len(), 1);
//! ```

use glasschain_core::{Block, TransactionKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single custody transfer event for a traceable asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustodyEvent {
    /// Composite asset identifier (e.g. `"GTIN:<gtin>:SN:<serial>"`).
    pub asset_id: String,
    /// Type of custody event (e.g. `"manufacture"`, `"dispatch"`, `"receive"`).
    pub event_type: String,
    /// Node/participant taking custody.
    pub custodian_id: String,
    /// ID of the transaction that recorded this event.
    pub transaction_id: String,
    /// Block index that committed this transaction.
    pub block_index: u64,
    /// Unix timestamp of the transaction.
    pub timestamp: u64,
}

/// Builds and queries the verifiable data lineage (provenance) index.
///
/// The index maps asset identifiers to their ordered list of custody events,
/// enabling O(1) lookups for any asset's complete history.
#[derive(Debug, Default)]
pub struct ProvenanceIndex {
    /// `asset_id → [CustodyEvent]` in chronological order.
    custody_chains: HashMap<String, Vec<CustodyEvent>>,
}

impl ProvenanceIndex {
    /// Create an empty provenance index.
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a custody event for an asset.
    pub fn record_event(&mut self, event: CustodyEvent) {
        self.custody_chains
            .entry(event.asset_id.clone())
            .or_default()
            .push(event);
    }

    /// Process all [`AssetRegistration`] transactions in a block and record
    /// their custody events in the index.
    ///
    /// This is the primary ingest path: call `ingest_block` for every new
    /// block to keep the provenance index up to date.
    pub fn ingest_block(&mut self, block: &Block) {
        for tx in &block.transactions {
            if let TransactionKind::AssetRegistration(ref reg) = tx.kind {
                let asset_id = asset_id_for(&reg.asset);
                self.record_event(CustodyEvent {
                    asset_id,
                    event_type: reg.event_type.clone(),
                    custodian_id: reg.asset.custodian_id.clone(),
                    transaction_id: tx.id.clone(),
                    block_index: block.index,
                    timestamp: tx.timestamp,
                });
            }
        }
    }

    /// Return the full ordered custody chain for an asset.
    ///
    /// Returns an empty slice if no events have been recorded for the asset.
    #[must_use] 
    pub fn get_custody_chain(&self, asset_id: &str) -> &[CustodyEvent] {
        self.custody_chains
            .get(asset_id)
            .map(std::vec::Vec::as_slice)
            .unwrap_or_default()
    }

    /// Return all asset IDs currently tracked by the provenance index.
    #[must_use] 
    pub fn tracked_assets(&self) -> Vec<&str> {
        self.custody_chains.keys().map(std::string::String::as_str).collect()
    }

    /// Verify that an asset's custody chain is complete: every mandatory
    /// custody event type is present in the correct order.
    ///
    /// `expected_events` is the ordered list of event types (e.g.
    /// `["manufacture", "dispatch", "receive"]`).
    ///
    /// Returns `true` when all expected events appear in order in the chain.
    #[must_use] 
    pub fn verify_lineage(&self, asset_id: &str, expected_events: &[&str]) -> bool {
        let chain = self.get_custody_chain(asset_id);
        if chain.len() < expected_events.len() {
            return false;
        }
        let mut chain_iter = chain.iter();
        for expected in expected_events {
            let found = chain_iter.any(|e| e.event_type == *expected);
            if !found {
                return false;
            }
        }
        true
    }
}

/// Construct a canonical asset identifier from a [`TraceableAsset`].
///
/// If both GTIN and serial number are present, the asset ID is
/// `"GTIN:<gtin>:SN:<serial>"`, uniquely identifying the serialised unit.
/// Otherwise, the batch-level ID `"GTIN:<gtin>:BATCH:<batch>"` is used.
fn asset_id_for(asset: &glasschain_core::TraceableAsset) -> String {
    match (&asset.gtin, &asset.serial_number, &asset.batch_number) {
        (Some(gtin), Some(sn), _) => format!("GTIN:{gtin}:SN:{sn}"),
        (Some(gtin), _, Some(batch)) => format!("GTIN:{gtin}:BATCH:{batch}"),
        (Some(gtin), _, _) => format!("GTIN:{gtin}"),
        _ => format!("PRODUCT:{}", asset.product_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{
        TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
    };

    fn asset(gtin: &str, serial: &str, custodian: &str) -> TraceableAsset {
        TraceableAsset {
            gtin: Some(gtin.into()),
            batch_number: Some("BATCH-001".into()),
            expiry_date: Some("2027-12-31".into()),
            serial_number: Some(serial.into()),
            anvisa_registration: None,
            manufacturer_id: None,
            product_name: "Drug A".into(),
            custodian_id: custodian.into(),
            country_of_origin: None,
            storage_temp_celsius: None,
            quantity: 1,
        }
    }

    fn asset_tx(asset: TraceableAsset, event_type: &str) -> Transaction {
        Transaction::new(TransactionKind::AssetRegistration(
            TraceableAssetRegistration {
                asset,
                event_type: event_type.into(),
                originator_id: "test".into(),
                purchase_order_ref: None,
            },
        ))
    }

    fn make_block(transactions: Vec<Transaction>) -> Block {
        let mut b = Block::new(1, transactions, "0".into());
        b.mine(1);
        b
    }

    #[test]
    fn test_record_and_retrieve_event() {
        let mut idx = ProvenanceIndex::new();
        idx.record_event(CustodyEvent {
            asset_id: "GTIN:123:SN:001".into(),
            event_type: "manufacture".into(),
            custodian_id: "fab".into(),
            transaction_id: "tx-1".into(),
            block_index: 1,
            timestamp: 1000,
        });
        let chain = idx.get_custody_chain("GTIN:123:SN:001");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].event_type, "manufacture");
    }

    #[test]
    fn test_ingest_block_populates_index() {
        let mut idx = ProvenanceIndex::new();
        let txs = vec![
            asset_tx(asset("07891234100016", "SN-001", "fab"), "manufacture"),
            asset_tx(asset("07891234100016", "SN-001", "distributor"), "dispatch"),
        ];
        let block = make_block(txs);
        idx.ingest_block(&block);

        let chain = idx.get_custody_chain("GTIN:07891234100016:SN:SN-001");
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn test_verify_lineage_complete() {
        let mut idx = ProvenanceIndex::new();
        for event in ["manufacture", "dispatch", "receive"] {
            idx.record_event(CustodyEvent {
                asset_id: "A:001".into(),
                event_type: event.into(),
                custodian_id: "node".into(),
                transaction_id: format!("tx-{event}"),
                block_index: 1,
                timestamp: 1000,
            });
        }
        assert!(idx.verify_lineage("A:001", &["manufacture", "dispatch", "receive"]));
    }

    #[test]
    fn test_verify_lineage_incomplete() {
        let mut idx = ProvenanceIndex::new();
        idx.record_event(CustodyEvent {
            asset_id: "A:001".into(),
            event_type: "manufacture".into(),
            custodian_id: "fab".into(),
            transaction_id: "tx-1".into(),
            block_index: 1,
            timestamp: 1000,
        });
        // "receive" is missing
        assert!(!idx.verify_lineage("A:001", &["manufacture", "dispatch", "receive"]));
    }

    #[test]
    fn test_tracked_assets() {
        let mut idx = ProvenanceIndex::new();
        idx.record_event(CustodyEvent {
            asset_id: "A:001".into(),
            event_type: "manufacture".into(),
            custodian_id: "fab".into(),
            transaction_id: "tx-1".into(),
            block_index: 1,
            timestamp: 1000,
        });
        idx.record_event(CustodyEvent {
            asset_id: "B:002".into(),
            event_type: "manufacture".into(),
            custodian_id: "fab".into(),
            transaction_id: "tx-2".into(),
            block_index: 1,
            timestamp: 1001,
        });
        assert_eq!(idx.tracked_assets().len(), 2);
    }
}
