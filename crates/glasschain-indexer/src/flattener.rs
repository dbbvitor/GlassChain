//! Analytical Flattener — transforms nested JSON asset data into flat
//! SQL / ClickHouse-compatible records.
//!
//! ## Overview
//!
//! The [`AnalyticalFlattener`] ingests [`IndexedBlock`] + [`IndexedTransaction`]
//! pairs from the indexer and, for every `AssetRegistration` transaction,
//! produces a [`FlatAssetRecord`]: a fully denormalised, analytics-ready row
//! with no nested JSON, suitable for direct insertion into columnar databases.
//!
//! [`VerifiableLineage`] combines a custody chain (from [`ProvenanceIndex`])
//! with the corresponding flat records, backing the `GetVerifiableLineage`
//! gRPC endpoint introduced in Phase 5.
//!
//! ## Example
//!
//! ```rust,ignore
//! let mut flattener = AnalyticalFlattener::new();
//! flattener.ingest_indexed_block(&block, &transactions);
//!
//! let csv_rows: Vec<String> = flattener
//!     .records()
//!     .iter()
//!     .map(AnalyticalFlattener::to_csv_row)
//!     .collect();
//! ```

use crate::indexer::{IndexedBlock, IndexedTransaction};
use crate::provenance::{CustodyEvent, ProvenanceIndex};
use glasschain_core::{MetadataTrustScore, Transaction, TransactionKind};
use serde::{Deserialize, Serialize};

// ── FlatAssetRecord ───────────────────────────────────────────────────────────

/// SQL / ClickHouse-compatible flat representation of a
/// [`TraceableAssetRegistration`] transaction.
///
/// Every field is a primitive, [`String`], or `Option<String>` — no nested
/// JSON.  This struct maps 1:1 to a database row in a `flat_asset_records`
/// table and is the primary output of the [`AnalyticalFlattener`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlatAssetRecord {
    // ── Chain provenance ───────────────────────────────────────────────────
    /// Chain index of the block that committed this transaction.
    pub block_index: u64,

    /// Hash of the committing block.
    pub block_hash: String,

    /// Unix timestamp (seconds) of the committing block.
    pub block_timestamp: u64,

    /// Transaction identifier (UUID v4).
    pub transaction_id: String,

    /// Unix timestamp (seconds) of the transaction.
    pub transaction_timestamp: u64,

    // ── Asset identity (GS1 / SNCM fields) ────────────────────────────────
    /// Global Trade Item Number (GTIN-14 or EAN-13).
    ///
    /// Required for Anvisa SNCM compliance (RDC 157/2017).
    pub gtin: Option<String>,

    /// Production batch / lot number.
    ///
    /// Required for Anvisa SNCM compliance.
    pub batch_number: Option<String>,

    /// Product expiry date in ISO-8601 format (`YYYY-MM-DD`).
    ///
    /// Required for Anvisa SNCM compliance.
    pub expiry_date: Option<String>,

    /// Unique serialisation number (GS1 SSCC or manufacturer-assigned serial).
    ///
    /// Required for Anvisa SNCM individual-unit traceability.
    pub serial_number: Option<String>,

    /// Anvisa product registration number (`MS xxxxxx.xxxxxx`).
    pub anvisa_registration: Option<String>,

    /// CNPJ or legal entity identifier of the manufacturer.
    pub manufacturer_id: Option<String>,

    // ── Asset context ──────────────────────────────────────────────────────
    /// Human-readable product name.
    pub product_name: String,

    /// Current custodian's node / participant identifier.
    pub custodian_id: String,

    /// ISO-3166-1 alpha-2 country code of origin.
    pub country_of_origin: Option<String>,

    /// Storage temperature range in degrees Celsius (e.g. `"2-8"`).
    pub storage_temp_celsius: Option<String>,

    /// Quantity of units in this asset record.
    pub quantity: u64,

    // ── Event context ──────────────────────────────────────────────────────
    /// Type of supply-chain event (e.g. `"manufacture"`, `"dispatch"`, `"receive"`).
    pub event_type: String,

    /// Node / participant originating this registration event.
    pub originator_id: String,

    /// Optional reference to a linked purchase order transaction.
    pub purchase_order_ref: Option<String>,

    // ── Computed trust score ───────────────────────────────────────────────
    /// Metadata trust score in \[0, 100\], computed from GS1 / SNCM field completeness.
    pub trust_score: u8,

    /// `true` when `trust_score >= 80` (Anvisa SNCM standard compliance threshold).
    pub is_standard_compliant: bool,

    /// Comma-separated list of missing core fields that reduced the trust score.
    /// Empty string when all core fields are present.
    pub missing_core_fields: String,
}

// ── FlattenerError ────────────────────────────────────────────────────────────

/// Errors that can occur when flattening a transaction into a [`FlatAssetRecord`].
#[derive(Debug, thiserror::Error)]
pub enum FlattenerError {
    /// The transaction's `payload_json` could not be parsed as a
    /// [`Transaction`] (the format the indexer stores).
    #[error("deserialization error: {0}")]
    Deserialization(#[from] serde_json::Error),

    /// The transaction's `kind` field is not `"AssetRegistration"`.
    #[error("not an asset registration transaction")]
    NotAssetRegistration,
}

// ── AnalyticalFlattener ───────────────────────────────────────────────────────

/// Transforms indexed transactions into flat, analytics-ready records suitable
/// for SQL / `ClickHouse` ingestion.
///
/// ## Ingest path
///
/// Call [`ingest_indexed_block`][`AnalyticalFlattener::ingest_indexed_block`]
/// for every new block produced by the indexer.  Non-`AssetRegistration`
/// transactions are silently skipped; malformed payloads are logged and skipped.
///
/// ## Query methods
///
/// The flattener exposes a set of zero-copy query methods
/// ([`records_by_gtin`][`AnalyticalFlattener::records_by_gtin`],
/// [`standard_compliant_records`][`AnalyticalFlattener::standard_compliant_records`],
/// etc.) for use by analytics pipelines and gRPC handlers.
#[derive(Debug)]
pub struct AnalyticalFlattener {
    records: Vec<FlatAssetRecord>,
}

impl AnalyticalFlattener {
    /// Create an empty [`AnalyticalFlattener`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Process all transactions in `transactions` that belong to `block`,
    /// building and storing a [`FlatAssetRecord`] for every
    /// `AssetRegistration` transaction.
    ///
    /// Non-`AssetRegistration` transactions are silently skipped.
    /// Malformed `payload_json` fields are logged at `warn` level and skipped.
    pub fn ingest_indexed_block(
        &mut self,
        block: &IndexedBlock,
        transactions: &[IndexedTransaction],
    ) {
        for tx in transactions {
            if tx.kind != "AssetRegistration" {
                continue;
            }
            match Self::flatten_transaction(tx, block.index, &block.hash, block.timestamp) {
                Ok(record) => self.records.push(record),
                Err(e) => {
                    log::warn!(
                        "flattener: skipping tx {} in block {}: {}",
                        tx.id,
                        block.index,
                        e
                    );
                }
            }
        }
    }

    /// Flatten a single [`IndexedTransaction`] into a [`FlatAssetRecord`].
    ///
    /// The `block_index`, `block_hash`, and `block_timestamp` arguments supply
    /// provenance metadata that is not carried in the transaction itself.
    ///
    /// # Errors
    ///
    /// - [`FlattenerError::NotAssetRegistration`] — `tx.kind` is not
    ///   `"AssetRegistration"`, or the parsed transaction's kind is not an
    ///   [`AssetRegistration`](glasschain_core::TransactionKind::AssetRegistration).
    /// - [`FlattenerError::Deserialization`] — `tx.payload_json` cannot be
    ///   parsed as a [`Transaction`] (the format the indexer stores).
    pub fn flatten_transaction(
        tx: &IndexedTransaction,
        block_index: u64,
        block_hash: &str,
        block_timestamp: u64,
    ) -> Result<FlatAssetRecord, FlattenerError> {
        if tx.kind != "AssetRegistration" {
            return Err(FlattenerError::NotAssetRegistration);
        }

        // The indexer stores the full Transaction JSON in `payload_json`
        // (`index_block` / `indexed_transactions_of`), so unwrap the kind.
        let full_tx: Transaction = serde_json::from_str(&tx.payload_json)?;
        let TransactionKind::AssetRegistration(reg) = full_tx.kind else {
            return Err(FlattenerError::NotAssetRegistration);
        };
        let score = MetadataTrustScore::compute(&reg.asset);

        Ok(FlatAssetRecord {
            block_index,
            block_hash: block_hash.to_owned(),
            block_timestamp,
            transaction_id: tx.id.clone(),
            transaction_timestamp: tx.timestamp,
            gtin: reg.asset.gtin.clone(),
            batch_number: reg.asset.batch_number.clone(),
            expiry_date: reg.asset.expiry_date.clone(),
            serial_number: reg.asset.serial_number.clone(),
            anvisa_registration: reg.asset.anvisa_registration.clone(),
            manufacturer_id: reg.asset.manufacturer_id.clone(),
            product_name: reg.asset.product_name.clone(),
            custodian_id: reg.asset.custodian_id.clone(),
            country_of_origin: reg.asset.country_of_origin.clone(),
            storage_temp_celsius: reg.asset.storage_temp_celsius.clone(),
            quantity: reg.asset.quantity,
            event_type: reg.event_type.clone(),
            originator_id: reg.originator_id.clone(),
            purchase_order_ref: reg.purchase_order_ref,
            trust_score: score.score,
            is_standard_compliant: score.is_standard,
            missing_core_fields: score.missing_core_fields.join(","),
        })
    }

    /// Return a slice of all stored flat records in insertion order.
    #[must_use]
    pub fn records(&self) -> &[FlatAssetRecord] {
        &self.records
    }

    /// Return all records whose GTIN matches `gtin`.
    #[must_use]
    pub fn records_by_gtin(&self, gtin: &str) -> Vec<&FlatAssetRecord> {
        self.records
            .iter()
            .filter(|r| r.gtin.as_deref() == Some(gtin))
            .collect()
    }

    /// Return all records whose `custodian_id` matches `custodian_id`.
    #[must_use]
    pub fn records_by_custodian(&self, custodian_id: &str) -> Vec<&FlatAssetRecord> {
        self.records
            .iter()
            .filter(|r| r.custodian_id == custodian_id)
            .collect()
    }

    /// Return all records whose `batch_number` matches `batch_number`.
    #[must_use]
    pub fn records_by_batch(&self, batch_number: &str) -> Vec<&FlatAssetRecord> {
        self.records
            .iter()
            .filter(|r| r.batch_number.as_deref() == Some(batch_number))
            .collect()
    }

    /// Return all records that meet the Anvisa SNCM standard compliance
    /// threshold (`trust_score >= 80`).
    #[must_use]
    pub fn standard_compliant_records(&self) -> Vec<&FlatAssetRecord> {
        self.records
            .iter()
            .filter(|r| r.is_standard_compliant)
            .collect()
    }

    /// Return all records whose `trust_score < 80`.
    ///
    /// Low-trust records surface suppliers with incomplete regulatory metadata
    /// in the "Low Trust" analytics bucket.
    #[must_use]
    pub fn low_trust_records(&self) -> Vec<&FlatAssetRecord> {
        self.records.iter().filter(|r| r.trust_score < 80).collect()
    }

    /// Return the sum of `quantity` across all records that have the given GTIN.
    #[must_use]
    pub fn total_quantity_for_gtin(&self, gtin: &str) -> u64 {
        self.records_by_gtin(gtin).iter().map(|r| r.quantity).sum()
    }

    /// Return the CSV header row matching all [`FlatAssetRecord`] fields in
    /// column order.
    ///
    /// The header is a single line of 22 comma-separated column names with no
    /// trailing newline.
    #[must_use]
    pub const fn to_csv_header() -> &'static str {
        "block_index,block_hash,block_timestamp,transaction_id,transaction_timestamp,\
gtin,batch_number,expiry_date,serial_number,anvisa_registration,manufacturer_id,\
product_name,custodian_id,country_of_origin,storage_temp_celsius,quantity,\
event_type,originator_id,purchase_order_ref,trust_score,is_standard_compliant,\
missing_core_fields"
    }

    /// Serialize a [`FlatAssetRecord`] as a CSV data row.
    ///
    /// - `Option<String>` fields are serialized as an empty string when `None`.
    /// - Fields that may contain commas (e.g. `missing_core_fields`) are
    ///   enclosed in double-quotes per RFC 4180, with internal double-quotes
    ///   doubled.
    #[must_use]
    pub fn to_csv_row(record: &FlatAssetRecord) -> String {
        /// Render `Option<&str>` as empty string when absent.
        fn opt(v: Option<&str>) -> String {
            v.unwrap_or("").to_owned()
        }

        /// RFC 4180 quoting: enclose in double-quotes when the value contains
        /// a comma, double-quote, or newline.
        fn quote(s: &str) -> String {
            if s.contains(',') || s.contains('"') || s.contains('\n') {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        }

        [
            record.block_index.to_string(),
            record.block_hash.clone(),
            record.block_timestamp.to_string(),
            record.transaction_id.clone(),
            record.transaction_timestamp.to_string(),
            opt(record.gtin.as_deref()),
            opt(record.batch_number.as_deref()),
            opt(record.expiry_date.as_deref()),
            opt(record.serial_number.as_deref()),
            opt(record.anvisa_registration.as_deref()),
            opt(record.manufacturer_id.as_deref()),
            record.product_name.clone(),
            record.custodian_id.clone(),
            opt(record.country_of_origin.as_deref()),
            opt(record.storage_temp_celsius.as_deref()),
            record.quantity.to_string(),
            record.event_type.clone(),
            record.originator_id.clone(),
            opt(record.purchase_order_ref.as_deref()),
            record.trust_score.to_string(),
            record.is_standard_compliant.to_string(),
            quote(&record.missing_core_fields),
        ]
        .join(",")
    }
}

impl Default for AnalyticalFlattener {
    /// Delegates to [`AnalyticalFlattener::new`].
    fn default() -> Self {
        Self::new()
    }
}

// ── VerifiableLineage ─────────────────────────────────────────────────────────

/// The full verifiable lineage for an asset, combining custody events from the
/// [`ProvenanceIndex`] with their corresponding flat analytical records.
///
/// This is the data payload returned by the `GetVerifiableLineage` gRPC
/// endpoint introduced in Phase 5.  It gives auditors and AI agents a single,
/// self-contained view of an asset's journey through the supply chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiableLineage {
    /// Composite asset identifier (e.g. `"GTIN:07891234100016:SN:SN-001"`).
    pub asset_id: String,

    /// Ordered list of custody events for this asset.
    pub custody_chain: Vec<CustodyEvent>,

    /// Flat analytical records corresponding to this asset's registrations.
    pub flat_records: Vec<FlatAssetRecord>,

    /// `true` if the number of custody events equals the number of flat records,
    /// indicating every chain step has a corresponding analytical record.
    pub is_complete: bool,

    /// Average trust score across all flat records.
    /// Returns `0.0` when there are no records.
    pub trust_score_avg: f64,
}

impl VerifiableLineage {
    /// Build a [`VerifiableLineage`] for the given `asset_id`.
    ///
    /// The GTIN is extracted from `asset_id` using the canonical formats:
    /// - `"GTIN:<gtin>"`
    /// - `"GTIN:<gtin>:SN:<serial>"`
    /// - `"GTIN:<gtin>:BATCH:<batch>"`
    ///
    /// Flat records are matched by GTIN; custody events are fetched by the full
    /// `asset_id` from the [`ProvenanceIndex`].
    #[must_use]
    pub fn build(
        asset_id: &str,
        provenance: &ProvenanceIndex,
        flattener: &AnalyticalFlattener,
    ) -> Self {
        let custody_chain: Vec<CustodyEvent> = provenance.get_custody_chain(asset_id).to_vec();

        let flat_records: Vec<FlatAssetRecord> =
            extract_gtin(asset_id).map_or_else(Vec::new, |gtin| {
                flattener
                    .records_by_gtin(gtin)
                    .into_iter()
                    .cloned()
                    .collect()
            });

        let is_complete = custody_chain.len() == flat_records.len();

        #[allow(clippy::cast_precision_loss)]
        let trust_score_avg = if flat_records.is_empty() {
            0.0
        } else {
            flat_records
                .iter()
                .map(|r| f64::from(r.trust_score))
                .sum::<f64>()
                / flat_records.len() as f64
        };

        Self {
            asset_id: asset_id.to_owned(),
            custody_chain,
            flat_records,
            is_complete,
            trust_score_avg,
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Extract the GTIN component from a canonical asset identifier.
///
/// Handles:
/// - `"GTIN:<g>"` → `<g>`
/// - `"GTIN:<g>:SN:<s>"` → `<g>`
/// - `"GTIN:<g>:BATCH:<b>"` → `<g>`
///
/// Returns `None` for unknown / non-GTIN formats.
fn extract_gtin(asset_id: &str) -> Option<&str> {
    let rest = asset_id.strip_prefix("GTIN:")?;
    Some(rest.find(':').map_or(rest, |pos| &rest[..pos]))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{
        MetadataTrustScore, TraceableAsset, TraceableAssetRegistration, Transaction,
        TransactionKind,
    };

    // ── Fixtures ──────────────────────────────────────────────────────────────

    /// A fully-populated asset — trust score 100, all SNCM fields present.
    fn full_asset() -> TraceableAsset {
        TraceableAsset {
            gtin: Some("07891234100016".into()),
            batch_number: Some("BATCH-001".into()),
            expiry_date: Some("2027-12-31".into()),
            serial_number: Some("SN-001".into()),
            anvisa_registration: Some("MS 1.0000.0001.001-1".into()),
            manufacturer_id: Some("12.345.678/0001-99".into()),
            product_name: "Dipirona 500mg".into(),
            custodian_id: "fabricante-abc".into(),
            country_of_origin: Some("BR".into()),
            storage_temp_celsius: Some("15-30".into()),
            quantity: 100,
        }
    }

    /// A minimal asset with no optional fields — trust score 0.
    fn minimal_asset() -> TraceableAsset {
        TraceableAsset {
            gtin: None,
            batch_number: None,
            expiry_date: None,
            serial_number: None,
            anvisa_registration: None,
            manufacturer_id: None,
            product_name: "Unknown Drug".into(),
            custodian_id: "unknown-node".into(),
            country_of_origin: None,
            storage_temp_celsius: None,
            quantity: 1,
        }
    }

    /// Build an [`IndexedTransaction`] carrying an `AssetRegistration`
    /// transaction serialised the way [`InMemoryIndexer::index_block`] stores
    /// it: the full [`Transaction`] JSON in `payload_json`.
    fn make_asset_tx(asset: TraceableAsset, event_type: &str, tx_id: &str) -> IndexedTransaction {
        let reg = TraceableAssetRegistration {
            asset,
            event_type: event_type.into(),
            originator_id: "originator-1".into(),
            purchase_order_ref: None,
        };
        let tx = Transaction::with_id(tx_id, TransactionKind::AssetRegistration(reg));
        IndexedTransaction {
            id: tx.id.clone(),
            block_index: 1,
            timestamp: 1_700_000_000,
            kind: "AssetRegistration".to_owned(),
            payload_json: serde_json::to_string(&tx).unwrap(),
        }
    }

    /// A minimal [`IndexedBlock`] used as provenance metadata in tests.
    fn make_block() -> IndexedBlock {
        IndexedBlock {
            index: 1,
            hash: "abc123def456".to_owned(),
            previous_hash: "000000000000".to_owned(),
            timestamp: 1_700_000_000,
            transaction_count: 0,
            transaction_ids: vec![],
        }
    }

    // ── Test 1: flatten a complete AssetRegistration ──────────────────────────

    #[test]
    fn test_flatten_single_asset_registration() {
        let tx = make_asset_tx(full_asset(), "manufacture", "tx-001");
        let record =
            AnalyticalFlattener::flatten_transaction(&tx, 1, "abc123def456", 1_700_000_000)
                .expect("should flatten successfully");

        // Provenance metadata
        assert_eq!(record.block_index, 1);
        assert_eq!(record.block_hash, "abc123def456");
        assert_eq!(record.block_timestamp, 1_700_000_000);
        assert_eq!(record.transaction_id, "tx-001");
        assert_eq!(record.transaction_timestamp, 1_700_000_000);

        // Asset identity
        assert_eq!(record.gtin.as_deref(), Some("07891234100016"));
        assert_eq!(record.batch_number.as_deref(), Some("BATCH-001"));
        assert_eq!(record.expiry_date.as_deref(), Some("2027-12-31"));
        assert_eq!(record.serial_number.as_deref(), Some("SN-001"));
        assert_eq!(
            record.anvisa_registration.as_deref(),
            Some("MS 1.0000.0001.001-1")
        );
        assert_eq!(
            record.manufacturer_id.as_deref(),
            Some("12.345.678/0001-99")
        );

        // Asset context
        assert_eq!(record.product_name, "Dipirona 500mg");
        assert_eq!(record.custodian_id, "fabricante-abc");
        assert_eq!(record.quantity, 100);

        // Event context
        assert_eq!(record.event_type, "manufacture");
        assert_eq!(record.originator_id, "originator-1");
        assert!(record.purchase_order_ref.is_none());

        // Trust score — verify against MetadataTrustScore directly
        let expected = MetadataTrustScore::compute(&full_asset());
        assert_eq!(record.trust_score, expected.score);
        assert_eq!(record.is_standard_compliant, expected.is_standard);
        assert_eq!(
            record.missing_core_fields,
            expected.missing_core_fields.join(",")
        );
        assert!(record.missing_core_fields.is_empty());
    }

    // ── Test 2: non-AssetRegistration kind returns NotAssetRegistration ────────

    #[test]
    fn test_flatten_non_asset_tx_returns_error() {
        let tx = IndexedTransaction {
            id: "tx-supply".to_owned(),
            block_index: 1,
            timestamp: 1_700_000_000,
            kind: "SupplyOffer".to_owned(),
            payload_json: "{}".to_owned(),
        };
        let err = AnalyticalFlattener::flatten_transaction(&tx, 1, "abc123", 1_700_000_000)
            .expect_err("should return an error for non-AssetRegistration kind");

        assert!(
            matches!(err, FlattenerError::NotAssetRegistration),
            "expected NotAssetRegistration, got {err:?}"
        );
    }

    // ── Test 3: ingest_indexed_block stores only AssetRegistration txs ────────

    #[test]
    fn test_flattener_ingest_block() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();

        let asset_tx = make_asset_tx(full_asset(), "manufacture", "tx-asset");
        let supply_tx = IndexedTransaction {
            id: "tx-supply".to_owned(),
            block_index: 1,
            timestamp: 1_700_000_000,
            kind: "SupplyOffer".to_owned(),
            payload_json: "{}".to_owned(),
        };
        let inventory_tx = IndexedTransaction {
            id: "tx-inventory".to_owned(),
            block_index: 1,
            timestamp: 1_700_000_000,
            kind: "InventoryUpdate".to_owned(),
            payload_json: "{}".to_owned(),
        };

        flattener.ingest_indexed_block(&block, &[asset_tx, supply_tx, inventory_tx]);

        assert_eq!(
            flattener.records().len(),
            1,
            "only AssetRegistration transactions should be stored"
        );
        assert_eq!(flattener.records()[0].transaction_id, "tx-asset");
    }

    // ── Test 4: records_by_gtin returns correct subset ────────────────────────

    #[test]
    fn test_records_by_gtin() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();

        // Two transactions sharing the same GTIN
        let tx1 = make_asset_tx(full_asset(), "manufacture", "tx-001");
        let tx2 = make_asset_tx(full_asset(), "dispatch", "tx-002");

        // One transaction with a different GTIN
        let mut other = full_asset();
        other.gtin = Some("99999999999999".into());
        let tx3 = make_asset_tx(other, "manufacture", "tx-003");

        flattener.ingest_indexed_block(&block, &[tx1, tx2, tx3]);

        let by_target_gtin = flattener.records_by_gtin("07891234100016");
        assert_eq!(
            by_target_gtin.len(),
            2,
            "expected 2 records for the target GTIN"
        );

        let by_other_gtin = flattener.records_by_gtin("99999999999999");
        assert_eq!(
            by_other_gtin.len(),
            1,
            "expected 1 record for the other GTIN"
        );

        let by_unknown = flattener.records_by_gtin("00000000000000");
        assert!(
            by_unknown.is_empty(),
            "expected no records for an unknown GTIN"
        );
    }

    // ── Test 5: standard_compliant_records filters by trust_score >= 80 ───────

    #[test]
    fn test_standard_compliant_records() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();

        // score 100 — all fields present
        let tx_full = make_asset_tx(full_asset(), "manufacture", "tx-full");
        // score 0 — all optional fields absent
        let tx_minimal = make_asset_tx(minimal_asset(), "manufacture", "tx-minimal");

        flattener.ingest_indexed_block(&block, &[tx_full, tx_minimal]);

        let compliant = flattener.standard_compliant_records();
        assert_eq!(compliant.len(), 1, "only 1 compliant record expected");
        assert_eq!(compliant[0].transaction_id, "tx-full");
        assert!(compliant[0].is_standard_compliant);

        // Verify the non-compliant record is not included
        assert!(flattener
            .low_trust_records()
            .iter()
            .any(|record| record.transaction_id == "tx-minimal"));
    }

    // ── Test 6: low_trust_records returns records with trust_score < 80 ───────

    #[test]
    fn test_low_trust_records() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();

        // score 100 — standard compliant
        let tx_full = make_asset_tx(full_asset(), "manufacture", "tx-full");
        // score 0 — all core fields missing
        let tx_minimal = make_asset_tx(minimal_asset(), "receive", "tx-minimal");

        flattener.ingest_indexed_block(&block, &[tx_full, tx_minimal]);

        let low = flattener.low_trust_records();
        assert_eq!(low.len(), 1, "only 1 low-trust record expected");
        assert_eq!(low[0].transaction_id, "tx-minimal");
        assert_eq!(low[0].trust_score, 0);
        assert!(!low[0].is_standard_compliant);
        assert!(!low[0].missing_core_fields.is_empty());
    }

    // ── Test 7: total_quantity_for_gtin sums across matching records ──────────

    #[test]
    fn test_total_quantity_for_gtin() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();

        let mut a1 = full_asset();
        a1.quantity = 50;
        a1.serial_number = Some("SN-001".into());

        let mut a2 = full_asset();
        a2.quantity = 75;
        a2.serial_number = Some("SN-002".into());

        // Different GTIN — must NOT be included in the sum
        let mut other = full_asset();
        other.gtin = Some("99999999999999".into());
        other.quantity = 1000;

        let tx1 = make_asset_tx(a1, "manufacture", "tx-001");
        let tx2 = make_asset_tx(a2, "dispatch", "tx-002");
        let tx3 = make_asset_tx(other, "manufacture", "tx-003");

        flattener.ingest_indexed_block(&block, &[tx1, tx2, tx3]);

        assert_eq!(
            flattener.total_quantity_for_gtin("07891234100016"),
            125,
            "expected 50 + 75 = 125 for the target GTIN"
        );
        assert_eq!(
            flattener.total_quantity_for_gtin("99999999999999"),
            1000,
            "expected 1000 for the other GTIN"
        );
        assert_eq!(
            flattener.total_quantity_for_gtin("00000000000000"),
            0,
            "expected 0 for an unknown GTIN"
        );
    }

    // ── Test 8: CSV header column count == CSV row comma-separated field count ─

    #[test]
    fn test_csv_row_roundtrip() {
        let tx = make_asset_tx(full_asset(), "manufacture", "tx-csv");
        let record =
            AnalyticalFlattener::flatten_transaction(&tx, 1, "abc123def456", 1_700_000_000)
                .expect("should flatten for CSV test");

        let header = AnalyticalFlattener::to_csv_header();
        let row = AnalyticalFlattener::to_csv_row(&record);

        let header_col_count = header.split(',').count();
        let row_col_count = row.split(',').count();

        assert_eq!(
            header_col_count, row_col_count,
            "header has {header_col_count} columns but row splits into \
             {row_col_count} comma-separated parts"
        );

        // Exact column count sanity check — FlatAssetRecord has 22 fields
        assert_eq!(
            header_col_count, 22,
            "FlatAssetRecord must produce exactly 22 CSV columns"
        );

        // Spot-check a few values are present in the row
        assert!(
            row.contains("07891234100016"),
            "GTIN must appear in the row"
        );
        assert!(
            row.contains("Dipirona 500mg"),
            "product_name must appear in the row"
        );
        assert!(row.contains("100"), "trust_score must appear in the row");
    }

    // ── Test 9: flatten_transaction returns Deserialization on malformed payload ──

    #[test]
    fn test_flatten_transaction_deserialization_error() {
        let tx = IndexedTransaction {
            id: "tx-bad".to_owned(),
            block_index: 1,
            timestamp: 1_700_000_000,
            kind: "AssetRegistration".to_owned(),
            payload_json: "{not valid json".to_owned(),
        };
        let err = AnalyticalFlattener::flatten_transaction(&tx, 1, "abc123", 1_700_000_000)
            .expect_err("malformed payload should fail to deserialize");
        assert!(
            matches!(err, FlattenerError::Deserialization(_)),
            "expected Deserialization, got {err:?}"
        );
    }

    // ── Test 10: ingest_indexed_block warns and skips a malformed AssetRegistration ──

    #[test]
    fn test_ingest_indexed_block_skips_malformed_asset() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();

        let good = make_asset_tx(full_asset(), "manufacture", "tx-good");
        let malformed = IndexedTransaction {
            id: "tx-malformed".to_owned(),
            block_index: 1,
            timestamp: 1_700_000_000,
            kind: "AssetRegistration".to_owned(),
            payload_json: "{oops".to_owned(),
        };

        flattener.ingest_indexed_block(&block, &[good, malformed]);

        assert_eq!(
            flattener.records().len(),
            1,
            "malformed AssetRegistration must be skipped"
        );
        assert_eq!(flattener.records()[0].transaction_id, "tx-good");
    }

    // ── Test 11: records_by_custodian filters by custodian_id ──

    #[test]
    fn test_records_by_custodian() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();

        let mut a1 = full_asset();
        a1.custodian_id = "fab-a".into();
        let mut a2 = full_asset();
        a2.custodian_id = "fab-b".into();
        let mut a3 = full_asset();
        a3.custodian_id = "fab-a".into();

        flattener.ingest_indexed_block(
            &block,
            &[
                make_asset_tx(a1, "manufacture", "tx-1"),
                make_asset_tx(a2, "manufacture", "tx-2"),
                make_asset_tx(a3, "dispatch", "tx-3"),
            ],
        );

        let fab_a = flattener.records_by_custodian("fab-a");
        assert_eq!(fab_a.len(), 2);
        assert!(fab_a.iter().all(|r| r.custodian_id == "fab-a"));

        let fab_b = flattener.records_by_custodian("fab-b");
        assert_eq!(fab_b.len(), 1);

        assert!(flattener.records_by_custodian("none").is_empty());
    }

    // ── Test 12: records_by_batch filters by batch_number ──

    #[test]
    fn test_records_by_batch() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();

        let mut a1 = full_asset();
        a1.batch_number = Some("BATCH-A".into());
        let mut a2 = full_asset();
        a2.batch_number = Some("BATCH-B".into());
        let mut a3 = full_asset();
        a3.batch_number = Some("BATCH-A".into());

        flattener.ingest_indexed_block(
            &block,
            &[
                make_asset_tx(a1, "manufacture", "tx-1"),
                make_asset_tx(a2, "manufacture", "tx-2"),
                make_asset_tx(a3, "dispatch", "tx-3"),
            ],
        );

        let batch_a = flattener.records_by_batch("BATCH-A");
        assert_eq!(batch_a.len(), 2);
        assert!(batch_a
            .iter()
            .all(|r| r.batch_number.as_deref() == Some("BATCH-A")));

        let batch_b = flattener.records_by_batch("BATCH-B");
        assert_eq!(batch_b.len(), 1);

        assert!(flattener.records_by_batch("NOPE").is_empty());
    }

    // ── Test 13: to_csv_row RFC 4180 quotes commas, quotes, and newlines ──

    #[test]
    fn test_to_csv_row_quotes_special_characters() {
        let tx = make_asset_tx(full_asset(), "manufacture", "tx-csv");
        let mut record =
            AnalyticalFlattener::flatten_transaction(&tx, 1, "abc123def456", 1_700_000_000)
                .unwrap();
        record.missing_core_fields = "a,b\"c\nd".into();

        let row = AnalyticalFlattener::to_csv_row(&record);

        // RFC 4180: a value containing comma, quote and newline is wrapped in
        // double-quotes and internal double-quotes are doubled.
        assert!(row.contains("\"a,b\"\"c\nd\""));
    }

    // ── Test 14: VerifiableLineage::build — SN full path with average trust ──

    #[test]
    fn test_verifiable_lineage_sn_full_path() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();
        let mut low = minimal_asset();
        low.gtin = Some("07891234100016".into());
        let full_score = MetadataTrustScore::compute(&full_asset()).score;
        let low_score = MetadataTrustScore::compute(&low).score;
        flattener.ingest_indexed_block(
            &block,
            &[
                make_asset_tx(full_asset(), "manufacture", "tx-001"),
                make_asset_tx(low, "dispatch", "tx-002"),
            ],
        );

        let mut provenance = ProvenanceIndex::new();
        for (i, ev) in ["manufacture", "dispatch"].into_iter().enumerate() {
            provenance.record_event(CustodyEvent {
                asset_id: "GTIN:07891234100016:SN:SN-001".into(),
                event_type: ev.to_string(),
                custodian_id: "node-1".into(),
                transaction_id: format!("ct-{i}"),
                block_index: 1,
                timestamp: 1_700_000_000 + i as u64,
            });
        }

        let lineage =
            VerifiableLineage::build("GTIN:07891234100016:SN:SN-001", &provenance, &flattener);

        assert_eq!(lineage.flat_records.len(), 2, "records matched by GTIN");
        assert!(lineage.is_complete, "custody events == flat records");
        let expected_avg = f64::from(full_score).midpoint(f64::from(low_score));
        assert!(
            (lineage.trust_score_avg - expected_avg).abs() < f64::EPSILON,
            "expected avg {expected_avg}, got {}",
            lineage.trust_score_avg
        );
        assert!(lineage.trust_score_avg > 0.0 && lineage.trust_score_avg < 100.0);
    }

    // ── Test 15: VerifiableLineage::build — :BATCH: format extracts GTIN ──

    #[test]
    fn test_verifiable_lineage_batch_format() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();
        flattener.ingest_indexed_block(
            &block,
            &[make_asset_tx(full_asset(), "manufacture", "tx-001")],
        );

        let provenance = ProvenanceIndex::new();
        let lineage = VerifiableLineage::build(
            "GTIN:07891234100016:BATCH:BATCH-001",
            &provenance,
            &flattener,
        );
        assert_eq!(
            lineage.flat_records.len(),
            1,
            ":BATCH: format extracts GTIN"
        );
        assert!(!lineage.is_complete, "no custody events recorded");
    }

    // ── Test 16: VerifiableLineage::build — bare GTIN fallback ──

    #[test]
    fn test_verifiable_lineage_gtin_only() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();
        flattener.ingest_indexed_block(
            &block,
            &[make_asset_tx(full_asset(), "manufacture", "tx-001")],
        );

        let mut provenance = ProvenanceIndex::new();
        provenance.record_event(CustodyEvent {
            asset_id: "GTIN:07891234100016".into(),
            event_type: "manufacture".into(),
            custodian_id: "node-1".into(),
            transaction_id: "ct-1".into(),
            block_index: 1,
            timestamp: 1_700_000_000,
        });

        let lineage = VerifiableLineage::build("GTIN:07891234100016", &provenance, &flattener);
        assert_eq!(lineage.flat_records.len(), 1, "bare GTIN matches records");
        assert!(lineage.is_complete);
        assert!(
            (lineage.trust_score_avg - 100.0).abs() < f64::EPSILON,
            "expected avg 100.0, got {}",
            lineage.trust_score_avg
        );
    }

    // ── Test 17: VerifiableLineage::build — non-GTIN id, incomplete, zero avg ──

    #[test]
    fn test_verifiable_lineage_unknown_format_empty_avg() {
        let mut flattener = AnalyticalFlattener::new();
        let block = make_block();
        flattener.ingest_indexed_block(
            &block,
            &[make_asset_tx(full_asset(), "manufacture", "tx-001")],
        );

        let mut provenance = ProvenanceIndex::new();
        provenance.record_event(CustodyEvent {
            asset_id: "PRODUCT:Drug A".into(),
            event_type: "manufacture".into(),
            custodian_id: "node-1".into(),
            transaction_id: "ct-1".into(),
            block_index: 1,
            timestamp: 1_700_000_000,
        });

        let lineage = VerifiableLineage::build("PRODUCT:Drug A", &provenance, &flattener);
        assert!(
            lineage.flat_records.is_empty(),
            "non-GTIN id matches no records"
        );
        assert!(!lineage.is_complete, "custody event without a flat record");
        assert!(
            lineage.trust_score_avg.abs() < f64::EPSILON,
            "expected avg 0.0, got {}",
            lineage.trust_score_avg
        );
    }
}
