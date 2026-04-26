use crate::asset::TraceableAsset;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// A supply offer posted by a seller into the distributed ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SupplyOffer {
    /// Unique product identifier (e.g. SKU or catalogue ID).
    pub product_id: String,
    /// Human-readable product name.
    pub product_name: String,
    /// Seller node/participant identifier.
    pub seller_id: String,
    /// Units available for purchase.
    pub quantity_available: u64,
    /// Price per unit expressed in `currency`.
    pub price_per_unit: f64,
    /// Estimated fulfilment lead time in calendar days.
    pub lead_time_days: u32,
    /// ISO-4217 currency code (e.g. "USD").
    pub currency: String,
}

/// A purchase order, either raised manually or by a smart contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PurchaseOrder {
    /// Product being purchased.
    pub product_id: String,
    /// Buyer node/participant identifier.
    pub buyer_id: String,
    /// Seller node/participant identifier.
    pub seller_id: String,
    /// Units ordered.
    pub quantity: u64,
    /// Agreed unit price expressed in `currency`.
    pub agreed_price_per_unit: f64,
    /// ISO-4217 currency code.
    pub currency: String,
    /// If raised by a smart contract, the contract's ID.
    pub contract_id: Option<String>,
}

/// Conditions that a [`SupplyOffer`] must satisfy to trigger auto-execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PurchaseConditions {
    /// Maximum acceptable price per unit.
    pub max_price_per_unit: f64,
    /// Minimum quantity the buyer wants to order.
    pub min_quantity: u64,
    /// Total maximum quantity across the entire lifetime of the contract.
    /// The engine accumulates `quantity_purchased` and marks the contract
    /// [`Fulfilled`][crate::ContractStatus] once this cap is reached.
    pub max_quantity: u64,
    /// Maximum acceptable lead time in calendar days.
    pub max_lead_time_days: u32,
    /// If set, only match offers from this specific seller.
    pub preferred_seller_id: Option<String>,
    /// ISO-4217 currency code the buyer operates in.
    pub currency: String,
    /// When `true`, a matching offer triggers an automatic [`PurchaseOrder`].
    pub auto_execute: bool,
}

/// On-ledger smart-contract definition created by a buyer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartContractDef {
    /// Unique contract identifier.
    pub contract_id: String,
    /// Buyer node/participant identifier.
    pub buyer_id: String,
    /// Product the contract targets.
    pub product_id: String,
    /// Conditions that trigger automatic execution.
    pub conditions: PurchaseConditions,
}

/// Registration of a traceable asset on-chain (Phase 3 traceability model).
///
/// Wraps a [`TraceableAsset`] and records the custody-transfer event type,
/// making every step of the supply chain (manufacture → distribution →
/// pharmacy) immutably visible on the ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceableAssetRegistration {
    /// The asset being registered.
    pub asset: TraceableAsset,
    /// Type of supply-chain event (e.g. "manufacture", "dispatch", "receive").
    pub event_type: String,
    /// Node/participant originating this event.
    pub originator_id: String,
    /// Optional reference to a linked purchase order transaction.
    pub purchase_order_ref: Option<String>,
}

/// Recorded when a smart contract successfully executes a purchase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractExecution {
    /// The smart contract that triggered this execution.
    pub contract_id: String,
    /// Resulting purchase-order transaction ID.
    pub purchase_order_tx_id: String,
    /// Buyer node/participant identifier.
    pub buyer_id: String,
    /// Seller node/participant identifier.
    pub seller_id: String,
    /// Product purchased.
    pub product_id: String,
    /// Units ordered.
    pub quantity: u64,
    /// Total order value (quantity × agreed price).
    pub total_price: f64,
    /// ISO-4217 currency code.
    pub currency: String,
}

/// An inventory-level update posted by a participant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InventoryUpdate {
    /// Product whose inventory changed.
    pub product_id: String,
    /// Node/participant that owns this inventory.
    pub owner_id: String,
    /// Positive = stock added, negative = stock consumed.
    pub quantity_delta: i64,
    /// Human-readable reason (e.g. "received shipment", "production consumption").
    pub reason: String,
}

/// Discriminated union of all transaction payloads supported by GlassChain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "payload")]
pub enum TransactionKind {
    SupplyOffer(SupplyOffer),
    PurchaseOrder(PurchaseOrder),
    ContractCreation(SmartContractDef),
    ContractExecution(ContractExecution),
    InventoryUpdate(InventoryUpdate),
    /// Phase 3: on-chain registration of a traceable asset with trust scoring.
    AssetRegistration(TraceableAssetRegistration),
}

/// A single ledger entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transaction {
    /// Globally unique transaction identifier (UUID v4).
    pub id: String,
    /// Unix timestamp (seconds) of when the transaction was created.
    pub timestamp: u64,
    /// The semantic payload of the transaction.
    pub kind: TransactionKind,
}

impl Transaction {
    /// Create a new transaction with a fresh UUID and the current wall-clock time.
    pub fn new(kind: TransactionKind) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before UNIX epoch")
            .as_secs();
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp,
            kind,
        }
    }

    /// Create a transaction with an explicit `id` (e.g. a deterministic ID) and
    /// the current wall-clock time.  Useful when the caller needs the transaction
    /// ID to be reproducible across multiple nodes.
    pub fn with_id(id: impl Into<String>, kind: TransactionKind) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before UNIX epoch")
            .as_secs();
        Self {
            id: id.into(),
            timestamp,
            kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_supply_offer() -> SupplyOffer {
        SupplyOffer {
            product_id: "SKU-001".into(),
            product_name: "Widget A".into(),
            seller_id: "seller-1".into(),
            quantity_available: 500,
            price_per_unit: 12.50,
            lead_time_days: 7,
            currency: "USD".into(),
        }
    }

    #[test]
    fn test_transaction_has_unique_ids() {
        let tx1 = Transaction::new(TransactionKind::SupplyOffer(sample_supply_offer()));
        let tx2 = Transaction::new(TransactionKind::SupplyOffer(sample_supply_offer()));
        assert_ne!(tx1.id, tx2.id);
    }

    #[test]
    fn test_transaction_serialization_roundtrip() {
        let tx = Transaction::new(TransactionKind::SupplyOffer(sample_supply_offer()));
        let json = serde_json::to_string(&tx).expect("serialize");
        let decoded: Transaction = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tx.id, decoded.id);
        assert_eq!(tx.kind, decoded.kind);
    }
}
