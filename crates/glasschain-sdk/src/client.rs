//! High-level `GlassChain` client and related configuration types.
//!
//! This module is the primary entry point of the SDK.  All transaction
//! builders are pure associated functions on [`GlasschainClient`] — they
//! perform no network I/O and can be called without an active connection.

#![allow(clippy::module_name_repetitions)]

use crate::error::SdkError;
use glasschain_core::{
    InventoryUpdate, MetadataTrustScore, PurchaseConditions, PurchaseOrder, SmartContractDef,
    SupplyOffer, TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Configuration used to initialise a [`GlasschainClient`].
///
/// Build one with [`GlasschainClientConfig::new`] and, optionally, chain
/// [`GlasschainClientConfig::with_node_id`] before passing it to
/// [`GlasschainClient::new`].
///
/// # Example
///
/// ```rust
/// use glasschain_sdk::GlasschainClientConfig;
///
/// let config = GlasschainClientConfig::new("http://localhost:9000")
///     .with_node_id("warehouse-node-1");
/// assert_eq!(config.endpoint, "http://localhost:9000");
/// assert_eq!(config.node_id.as_deref(), Some("warehouse-node-1"));
/// ```
#[derive(Debug, Clone)]
pub struct GlasschainClientConfig {
    /// gRPC endpoint, e.g. `"http://localhost:9000"`.
    pub endpoint: String,
    /// Optional node ID used when identity-signing submitted transactions.
    pub node_id: Option<String>,
}

impl GlasschainClientConfig {
    /// Create a new configuration pointing at the given gRPC `endpoint`.
    ///
    /// The `node_id` field is left as `None`; use
    /// [`with_node_id`][Self::with_node_id] to set it.
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            node_id: None,
        }
    }

    /// Set the node ID for identity-signed submissions (builder-pattern).
    ///
    /// Consumes `self` and returns an updated [`GlasschainClientConfig`].
    #[must_use]
    pub fn with_node_id(mut self, id: impl Into<String>) -> Self {
        self.node_id = Some(id.into());
        self
    }
}

// ── Chain status ───────────────────────────────────────────────────────────────

/// A snapshot of the remote chain state returned by
/// `LedgerService.GetChainStatus`.
#[derive(Debug, Clone)]
pub struct ChainStatus {
    /// Current number of committed blocks in the chain (including the genesis block).
    pub chain_length: u64,
    /// Hex-encoded hash of the most recently committed (tip) block.
    pub tip_hash: String,
    /// Number of transactions in the mempool waiting to be mined into a block.
    pub pending_transactions: u64,
}

// ── Client ─────────────────────────────────────────────────────────────────────

/// High-level `GlassChain` client.
///
/// Provides builder methods for all on-chain transaction types and a thin
/// wrapper around the gRPC transport layer.  The `build_*` associated
/// functions are **pure** (no network I/O): they construct the correct
/// [`Transaction`] payload and serialise it to pretty-printed JSON that is
/// ready to submit to the `LedgerService.SubmitTransaction` gRPC endpoint.
///
/// # Quick Start
///
/// Register a traceable pharmaceutical asset in under 10 lines:
///
/// ```rust,no_run
/// use glasschain_sdk::{GlasschainClient, GlasschainClientConfig};
/// use glasschain_core::TraceableAsset;
///
/// # fn main() -> Result<(), glasschain_sdk::SdkError> {
/// let asset = TraceableAsset {
///     gtin: Some("07891234567890".into()),
///     batch_number: Some("LOTE-2025-001".into()),
///     expiry_date: Some("2027-12-31".into()),
///     serial_number: Some("SN-00000001".into()),
///     anvisa_registration: Some("MS 1.0000.0001.001-1".into()),
///     manufacturer_id: Some("12.345.678/0001-99".into()),
///     product_name: "Dipirona 500mg".into(),
///     custodian_id: "my-node".into(),
///     country_of_origin: Some("BR".into()),
///     storage_temp_celsius: Some("15-30".into()),
///     quantity: 1_000,
/// };
/// let tx_json = GlasschainClient::build_asset_registration_tx("my-node", asset, "MANUFACTURE")?;
/// println!("Submit to gRPC SubmitTransaction:\n{tx_json}");
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct GlasschainClient {
    /// Stored configuration (endpoint URL, optional node ID).
    ///
    /// In a full implementation this would also hold an open `tonic::Channel`
    /// handle.  For now the config drives log output and documents the target
    /// endpoint.
    config: GlasschainClientConfig,
}

impl GlasschainClient {
    /// Initialise a client with the given configuration.
    ///
    /// In a full production implementation this would open a `tonic` transport
    /// channel and perform a health-check against the remote node.  Currently
    /// the method is infallible and logs the target endpoint.
    ///
    /// # Errors
    ///
    /// Currently infallible.  Future releases will return
    /// [`SdkError::Transport`] when the gRPC endpoint is unreachable.
    #[allow(clippy::unused_async)] // intentionally async — will drive tonic in a future release
    pub async fn new(config: GlasschainClientConfig) -> Result<Self, SdkError> {
        log::info!(
            "GlasschainClient initialised — endpoint: {}, node_id: {:?}",
            config.endpoint,
            config.node_id,
        );
        Ok(Self { config })
    }

    /// Return the configured gRPC endpoint URL.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
    }

    // ── Transaction builders (pure — no network I/O) ──────────────────────────

    /// Build a [`SupplyOffer`] transaction and serialise it to pretty-printed JSON.
    ///
    /// The returned JSON string is suitable for direct submission to the gRPC
    /// `LedgerService.SubmitTransaction` endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::Serialization`] if JSON serialisation fails
    /// (should be unreachable in practice).
    ///
    /// # Panics
    ///
    /// Panics if the system clock is set before the Unix epoch (propagated
    /// from [`Transaction::new`]).
    #[must_use = "the JSON must be submitted to the ledger to take effect"]
    pub fn build_supply_offer_tx(
        seller_id: &str,
        product_id: &str,
        product_name: &str,
        quantity: u64,
        price_per_unit: u64,
        lead_time_days: u32,
        currency: &str,
    ) -> Result<String, SdkError> {
        let offer = SupplyOffer {
            product_id: product_id.to_owned(),
            product_name: product_name.to_owned(),
            seller_id: seller_id.to_owned(),
            quantity_available: quantity,
            price_per_unit,
            lead_time_days,
            currency: currency.to_owned(),
        };
        log::info!(
            "Building SupplyOffer tx: product={product_id}, seller={seller_id}, \
             qty={quantity}, price={price_per_unit} {currency}"
        );
        let tx = Transaction::new(TransactionKind::SupplyOffer(offer));
        Ok(serde_json::to_string_pretty(&tx)?)
    }

    /// Build a [`PurchaseOrder`] transaction and serialise it to pretty-printed JSON.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::Serialization`] if JSON serialisation fails.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is set before the Unix epoch.
    #[must_use = "the JSON must be submitted to the ledger to take effect"]
    pub fn build_purchase_order_tx(
        buyer_id: &str,
        seller_id: &str,
        product_id: &str,
        quantity: u64,
        agreed_price: u64,
        currency: &str,
    ) -> Result<String, SdkError> {
        let order = PurchaseOrder {
            product_id: product_id.to_owned(),
            buyer_id: buyer_id.to_owned(),
            seller_id: seller_id.to_owned(),
            quantity,
            agreed_price_per_unit: agreed_price,
            currency: currency.to_owned(),
            contract_id: None,
        };
        log::info!(
            "Building PurchaseOrder tx: product={product_id}, \
             buyer={buyer_id} → seller={seller_id}, qty={quantity}"
        );
        let tx = Transaction::new(TransactionKind::PurchaseOrder(order));
        Ok(serde_json::to_string_pretty(&tx)?)
    }

    /// Build a [`TraceableAssetRegistration`] transaction and serialise it to
    /// pretty-printed JSON.
    ///
    /// Before constructing the transaction, the asset's
    /// [`MetadataTrustScore`] is computed and emitted via [`log::info!`].
    /// Missing core regulatory fields are additionally emitted as
    /// [`log::warn!`] messages so participants are nudged to improve data
    /// quality without hard-failing the submission.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::Serialization`] if JSON serialisation fails.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is set before the Unix epoch.
    #[must_use = "the JSON must be submitted to the ledger to take effect"]
    pub fn build_asset_registration_tx(
        originator_id: &str,
        asset: TraceableAsset,
        event_type: &str,
    ) -> Result<String, SdkError> {
        // Compute and log the trust score before consuming the asset.
        let score = MetadataTrustScore::compute(&asset);
        log::info!(
            "Asset trust score for '{}': {}/100 (standard={})",
            asset.product_name,
            score.score,
            score.is_standard,
        );
        if !score.missing_core_fields.is_empty() {
            log::warn!(
                "Asset '{}' is missing core regulatory fields: {:?} — \
                 trust score will be reduced and standard fee discount will not apply",
                asset.product_name,
                score.missing_core_fields,
            );
        }

        let registration = TraceableAssetRegistration {
            asset,
            event_type: event_type.to_owned(),
            originator_id: originator_id.to_owned(),
            purchase_order_ref: None,
        };
        log::info!(
            "Building AssetRegistration tx: event={event_type}, originator={originator_id}"
        );
        let tx = Transaction::new(TransactionKind::AssetRegistration(registration));
        Ok(serde_json::to_string_pretty(&tx)?)
    }

    /// Build an [`InventoryUpdate`] transaction and serialise it to
    /// pretty-printed JSON.
    ///
    /// A positive `quantity_delta` represents stock being added; a negative
    /// value represents stock being consumed or dispatched.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::Serialization`] if JSON serialisation fails.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is set before the Unix epoch.
    #[must_use = "the JSON must be submitted to the ledger to take effect"]
    pub fn build_inventory_update_tx(
        owner_id: &str,
        product_id: &str,
        quantity_delta: i64,
        reason: &str,
    ) -> Result<String, SdkError> {
        let update = InventoryUpdate {
            product_id: product_id.to_owned(),
            owner_id: owner_id.to_owned(),
            quantity_delta,
            reason: reason.to_owned(),
        };
        log::info!(
            "Building InventoryUpdate tx: product={product_id}, \
             owner={owner_id}, delta={quantity_delta:+}, reason=\"{reason}\""
        );
        let tx = Transaction::new(TransactionKind::InventoryUpdate(update));
        Ok(serde_json::to_string_pretty(&tx)?)
    }

    /// Build a smart-contract deployment transaction and serialise it to
    /// pretty-printed JSON.
    ///
    /// The resulting [`Transaction`] contains a [`SmartContractDef`] with no
    /// WASM bytecode (pure-Rust condition matching).  To attach custom WASM
    /// logic, set `wasm_code_b64` on the [`SmartContractDef`] before
    /// wrapping it in a [`Transaction`] manually.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::Serialization`] if JSON serialisation fails.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is set before the Unix epoch.
    #[must_use = "the JSON must be submitted to the ledger to take effect"]
    pub fn build_smart_contract_tx(
        contract_id: &str,
        buyer_id: &str,
        product_id: &str,
        conditions: PurchaseConditions,
    ) -> Result<String, SdkError> {
        let contract = SmartContractDef {
            contract_id: contract_id.to_owned(),
            buyer_id: buyer_id.to_owned(),
            product_id: product_id.to_owned(),
            conditions,
            wasm_code_b64: None,
        };
        log::info!(
            "Building ContractCreation tx: contract={contract_id}, \
             buyer={buyer_id}, product={product_id}"
        );
        let tx = Transaction::new(TransactionKind::ContractCreation(contract));
        Ok(serde_json::to_string_pretty(&tx)?)
    }

    /// Compute the [`MetadataTrustScore`] for a [`TraceableAsset`].
    ///
    /// A thin re-export of [`MetadataTrustScore::compute`] so that callers
    /// need not import the scoring type separately.
    ///
    /// Scores range from **0** (no regulatory metadata) to **100** (all core
    /// and bonus fields present).  Assets with `score >= 80` qualify for the
    /// 50 % transaction-fee discount.
    #[must_use]
    pub fn compute_trust_score(asset: &TraceableAsset) -> MetadataTrustScore {
        MetadataTrustScore::compute(asset)
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::TraceableAsset;

    /// Construct a fully-populated asset (score = 100).
    fn full_asset() -> TraceableAsset {
        TraceableAsset {
            gtin: Some("07891234567890".into()),
            batch_number: Some("LOTE-2025-001".into()),
            expiry_date: Some("2027-12-31".into()),
            serial_number: Some("SN-00000001".into()),
            anvisa_registration: Some("MS 1.0000.0001.001-1".into()),
            manufacturer_id: Some("12.345.678/0001-99".into()),
            product_name: "Dipirona Sódica 500mg".into(),
            custodian_id: "fabricante-xyz".into(),
            country_of_origin: Some("BR".into()),
            storage_temp_celsius: Some("15-30".into()),
            quantity: 1_000,
        }
    }

    /// `build_supply_offer_tx` should produce valid JSON that round-trips
    /// back to a `Transaction` carrying a `SupplyOffer` payload.
    #[test]
    fn test_build_supply_offer_json() {
        let json = GlasschainClient::build_supply_offer_tx(
            "seller-1",
            "SKU-001",
            "Widget A",
            500,
            1_250,
            7,
            "USD",
        )
        .unwrap();
        let tx: Transaction = serde_json::from_str(&json).unwrap();
        assert!(!tx.id.is_empty(), "transaction must have a non-empty id");
        assert!(
            matches!(tx.kind, TransactionKind::SupplyOffer(_)),
            "expected SupplyOffer variant, got {tx:?}",
        );
        if let TransactionKind::SupplyOffer(offer) = &tx.kind {
            assert_eq!(offer.seller_id, "seller-1");
            assert_eq!(offer.product_id, "SKU-001");
            assert_eq!(offer.quantity_available, 500);
            assert_eq!(offer.price_per_unit, 1_250);
        }
    }

    /// `build_asset_registration_tx` should embed the asset inside an
    /// `AssetRegistration` transaction and the JSON should round-trip cleanly.
    #[test]
    fn test_build_asset_registration_json() {
        let asset = full_asset();
        let json =
            GlasschainClient::build_asset_registration_tx("my-node", asset, "MANUFACTURE")
                .unwrap();
        let tx: Transaction = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(tx.kind, TransactionKind::AssetRegistration(_)),
            "expected AssetRegistration variant",
        );
        if let TransactionKind::AssetRegistration(reg) = &tx.kind {
            assert_eq!(reg.originator_id, "my-node");
            assert_eq!(reg.event_type, "MANUFACTURE");
            assert_eq!(
                reg.asset.gtin.as_deref(),
                Some("07891234567890"),
            );
        }
    }

    /// `build_purchase_order_tx` should produce valid JSON that round-trips
    /// back to a `Transaction` carrying a `PurchaseOrder` payload.
    #[test]
    fn test_build_purchase_order_json() {
        let json = GlasschainClient::build_purchase_order_tx(
            "buyer-1", "seller-1", "SKU-001", 100, 1_250, "USD",
        )
        .unwrap();
        let tx: Transaction = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(tx.kind, TransactionKind::PurchaseOrder(_)),
            "expected PurchaseOrder variant",
        );
        if let TransactionKind::PurchaseOrder(order) = &tx.kind {
            assert_eq!(order.buyer_id, "buyer-1");
            assert_eq!(order.seller_id, "seller-1");
            assert_eq!(order.quantity, 100);
            assert_eq!(order.agreed_price_per_unit, 1_250);
            assert!(order.contract_id.is_none());
        }
    }

    /// A fully-populated asset must yield a trust score of exactly 100 and
    /// be flagged as standard-compliant.
    #[test]
    fn test_trust_score_full_asset() {
        let score = GlasschainClient::compute_trust_score(&full_asset());
        assert_eq!(score.score, 100, "full asset must score 100");
        assert!(score.is_standard, "full asset must be standard-compliant");
        assert!(
            score.missing_core_fields.is_empty(),
            "no core fields should be missing",
        );
        assert_eq!(
            score.bonus_fields_present.len(),
            2,
            "both bonus fields (anvisa + manufacturer) must be present",
        );
    }

    /// The builder-pattern on `GlasschainClientConfig` must store the
    /// endpoint and node ID correctly.
    #[test]
    fn test_client_config_builder() {
        let config = GlasschainClientConfig::new("http://localhost:9000")
            .with_node_id("warehouse-node-1");
        assert_eq!(config.endpoint, "http://localhost:9000");
        assert_eq!(
            config.node_id.as_deref(),
            Some("warehouse-node-1"),
        );

        // Config without a node_id should have None.
        let bare = GlasschainClientConfig::new("http://remote:9000");
        assert!(bare.node_id.is_none());
    }
}
