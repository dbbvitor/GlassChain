//! The purchase-to-settlement flow (ticket #43): RFQ → Quote → PO →
//! Acceptance → Shipment → Receipt → Dispute → Settlement as stateful
//! multi-party flows over canonical v1 records.
//!
//! Both organizations run the same state machine with role-specific starting
//! states; committed canonical records are the coordination bus between the
//! parties' runners.
//!
//! ## Record mapping (the v1 registry is frozen at 13 families — ADR-006)
//!
//! | Step | Record interaction |
//! |---|---|
//! | RFQ | flow-initial state; commercial terms stay off the global chain (ADR-010 §1 — no RFQ family exists by design) |
//! | Quote | flow state; the off-chain quote acceptance wakes the flow |
//! | PO | **emits** `purchase_order` — the negotiated outcome's first public commitment |
//! | Acceptance | **consumes** the committed `purchase_order` |
//! | Shipment | **emits** `shipment` |
//! | Receipt | **emits** `delivery_receipt` |
//! | Dispute | **consumes** the `delivery_receipt` reference |
//! | Settlement | terminal state referencing the committed PO |
//!
//! Every emitted `record_id` and `occurred_at` derives from the config or the
//! consumed record — never the wall clock — so a replayed emission is
//! identical and the ledger dedupes it (at-least-once execution,
//! exactly-once effects). Two host conventions make that guarantee hold:
//! emissions are submitted with `Transaction::with_id(record.record_id, …)`,
//! and the host-supplied `rfq_id`/`lot_ref` must be globally unique — record
//! ids are derived from them (`po:<rfq_id>`, `shipment:<po_ref>:<lot_ref>`)
//! and the ledger silently drops a duplicate id.
//!
//! Signature attachment is the endorsement layer's job, performed by the
//! runtime before submission.

use crate::action::Action;
use crate::event::Event;
use crate::receipt_flow::build_receipt;
use crate::runner::FlowRunner;
use crate::state::FlowState;
use crate::transition::{Transition, TransitionResult};
use glasschain_core::CanonicalRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Configuration for one party's purchase-to-settlement flow.
///
/// Everything is fixed up front (no wall clock, no randomness): checkpoints
/// replay deterministically from these values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseFlowConfig {
    /// This party's organization identifier.
    pub org: String,
    /// The counterparty's organization identifier.
    pub counterparty: String,
    /// Product under negotiation.
    pub product_id: String,
    /// Agreed quantity (integer minor units of the product).
    pub quantity: u64,
    /// Agreed currency.
    pub currency: String,
    /// The anchored lot being traded (`lot_ref` on the shipment).
    pub lot_ref: String,
    /// The RFQ's stable identifier (seeds the PO's `record_id`).
    pub rfq_id: String,
    /// Unix-seconds seed for the PO's `occurred_at` (the negotiation close).
    pub negotiated_at: u64,
    /// ISO-8601 date (`YYYY-MM-DD`) of the expected delivery (receipt field).
    pub delivery_on: String,
}

/// The purchase-to-settlement flow's protocol state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PurchaseFlowState {
    /// Buyer: the RFQ is issued; the flow holds the (off-chain) terms.
    RfqIssued { rfq_id: String },
    /// Buyer: the seller's off-chain quote was accepted.
    QuoteAccepted { rfq_id: String, quote_id: String },
    /// Buyer: the PO was emitted; waiting for the seller's shipment.
    AwaitingShipment { po_ref: String },
    /// Seller: waiting for a committed purchase order (seller's initial state).
    AwaitingPurchaseOrder,
    /// Seller: the PO was consumed (acceptance); waiting for the ship decision.
    PoAccepted { po_ref: String },
    /// Seller: the shipment was emitted; waiting for the delivery receipt.
    AwaitingReceipt {
        po_ref: String,
        shipment_ref: String,
    },
    /// Buyer: the shipment was consumed and the receipt emitted (either party
    /// reaches this state on consuming the receipt record).
    Delivered { po_ref: String, receipt_ref: String },
    /// Either party: a dispute was raised on the delivered receipt.
    Disputed {
        po_ref: String,
        receipt_ref: String,
        reason: String,
    },
    /// Terminal: the deal settled.
    Settled { po_ref: String },
}

impl FlowState for PurchaseFlowState {
    fn step(&self) -> &'static str {
        match self {
            Self::RfqIssued { .. } => "rfq_issued",
            Self::QuoteAccepted { .. } => "quote_accepted",
            Self::AwaitingShipment { .. } => "awaiting_shipment",
            Self::AwaitingPurchaseOrder => "awaiting_purchase_order",
            Self::PoAccepted { .. } => "po_accepted",
            Self::AwaitingReceipt { .. } => "awaiting_receipt",
            Self::Delivered { .. } => "delivered",
            Self::Disputed { .. } => "disputed",
            Self::Settled { .. } => "settled",
        }
    }
}

/// Wake-reason prefix that closes the off-chain quote negotiation; the rest of
/// the reason is the agreed quote id.
const QUOTE_ACCEPTED_PREFIX: &str = "quote-accepted:";

/// Buyer transition 1 (Quote step): the off-chain quote acceptance wakes the
/// flow. Consumes no record — pricing never enters the global chain.
#[derive(Debug)]
pub struct AcceptQuoteTransition;

impl Transition<PurchaseFlowState> for AcceptQuoteTransition {
    fn name(&self) -> &'static str {
        "AcceptQuote"
    }

    fn matches(&self, state: &PurchaseFlowState, event: &Event) -> bool {
        let PurchaseFlowState::RfqIssued { .. } = state else {
            return false;
        };
        matches!(event, Event::Woken(reason) if reason.starts_with(QUOTE_ACCEPTED_PREFIX))
    }

    fn apply(
        &self,
        state: &PurchaseFlowState,
        event: &Event,
    ) -> TransitionResult<PurchaseFlowState> {
        let (PurchaseFlowState::RfqIssued { rfq_id }, Event::Woken(reason)) = (state, event) else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let quote_id = reason[QUOTE_ACCEPTED_PREFIX.len()..].to_owned();
        TransitionResult::new(
            PurchaseFlowState::QuoteAccepted {
                rfq_id: rfq_id.clone(),
                quote_id,
            },
            Vec::new(),
            false,
        )
    }
}

/// Buyer transition 2 (PO step): commit the negotiated terms as a public
/// `purchase_order` record — the first on-chain artifact of the deal.
#[derive(Debug, Clone)]
pub struct CommitPurchaseOrderTransition {
    pub config: PurchaseFlowConfig,
}

impl Transition<PurchaseFlowState> for CommitPurchaseOrderTransition {
    fn name(&self) -> &'static str {
        "CommitPurchaseOrder"
    }

    fn matches(&self, state: &PurchaseFlowState, event: &Event) -> bool {
        matches!(state, PurchaseFlowState::QuoteAccepted { .. })
            && matches!(event, Event::Woken(reason) if reason == "commit-po")
    }

    fn apply(
        &self,
        state: &PurchaseFlowState,
        event: &Event,
    ) -> TransitionResult<PurchaseFlowState> {
        let PurchaseFlowState::QuoteAccepted { rfq_id, .. } = state else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Event::Woken(_) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let purchase_order = build_purchase_order(rfq_id, &self.config);
        let po_ref = purchase_order.record_id.clone();
        TransitionResult::new(
            PurchaseFlowState::AwaitingShipment { po_ref },
            vec![Action::EmitRecord(purchase_order)],
            false,
        )
    }
}

/// Seller transition 1 (Acceptance step): consume the committed
/// `purchase_order` addressed to this seller.
#[derive(Debug, Clone)]
pub struct AcceptPurchaseOrderTransition {
    pub config: PurchaseFlowConfig,
}

impl Transition<PurchaseFlowState> for AcceptPurchaseOrderTransition {
    fn name(&self) -> &'static str {
        "AcceptPurchaseOrder"
    }

    fn matches(&self, state: &PurchaseFlowState, event: &Event) -> bool {
        state == &PurchaseFlowState::AwaitingPurchaseOrder
            && matches!(
                event,
                Event::RecordCommitted(record)
                    if record.schema_id == "purchase_order"
                        && record.payload.get("buyer_id").and_then(Value::as_str)
                            == Some(self.config.counterparty.as_str())
                        && record.payload.get("seller_id").and_then(Value::as_str)
                            == Some(self.config.org.as_str())
            )
    }

    fn apply(
        &self,
        state: &PurchaseFlowState,
        event: &Event,
    ) -> TransitionResult<PurchaseFlowState> {
        let Event::RecordCommitted(purchase_order) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        TransitionResult::new(
            PurchaseFlowState::PoAccepted {
                po_ref: purchase_order.record_id.clone(),
            },
            Vec::new(),
            false,
        )
    }
}

/// Seller transition 2 (Shipment step): ship the lot against the accepted PO,
/// emitting the public `shipment` record (the custody edge's first half).
#[derive(Debug, Clone)]
pub struct ShipOrderTransition {
    pub config: PurchaseFlowConfig,
}

impl Transition<PurchaseFlowState> for ShipOrderTransition {
    fn name(&self) -> &'static str {
        "ShipOrder"
    }

    fn matches(&self, state: &PurchaseFlowState, event: &Event) -> bool {
        matches!(state, PurchaseFlowState::PoAccepted { .. })
            && matches!(event, Event::Woken(reason) if reason == "ship")
    }

    fn apply(
        &self,
        state: &PurchaseFlowState,
        event: &Event,
    ) -> TransitionResult<PurchaseFlowState> {
        let PurchaseFlowState::PoAccepted { po_ref } = state else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Event::Woken(_) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Some(occurred_at) = self.config.shipment_timestamp() else {
            // `po:<rfq_id>` did not parse — a config invariant broke; the flow
            // stays put rather than emitting a mis-stamped record.
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let shipment = build_shipment(po_ref, occurred_at, &self.config);
        let shipment_ref = shipment.record_id.clone();
        TransitionResult::new(
            PurchaseFlowState::AwaitingReceipt {
                po_ref: po_ref.clone(),
                shipment_ref,
            },
            vec![Action::EmitRecord(shipment)],
            false,
        )
    }
}

/// Buyer transition 3 (Receipt step): consume the incoming shipment and emit
/// the `delivery_receipt` — the custody edge's second half.
#[derive(Debug, Clone)]
pub struct RecordDeliveryTransition {
    pub config: PurchaseFlowConfig,
}

impl Transition<PurchaseFlowState> for RecordDeliveryTransition {
    fn name(&self) -> &'static str {
        "RecordDelivery"
    }

    fn matches(&self, state: &PurchaseFlowState, event: &Event) -> bool {
        let PurchaseFlowState::AwaitingShipment { po_ref } = state else {
            return false;
        };
        let Event::RecordCommitted(record) = event else {
            return false;
        };
        record.schema_id == "shipment"
            && record.payload.get("lot_ref").and_then(Value::as_str)
                == Some(self.config.lot_ref.as_str())
            && record.payload.get("from_org").and_then(Value::as_str)
                == Some(self.config.counterparty.as_str())
            && record.payload.get("to_org").and_then(Value::as_str)
                == Some(self.config.org.as_str())
            && record.record_id.starts_with(&format!("shipment:{po_ref}:"))
    }

    fn apply(
        &self,
        state: &PurchaseFlowState,
        event: &Event,
    ) -> TransitionResult<PurchaseFlowState> {
        let (PurchaseFlowState::AwaitingShipment { po_ref }, Event::RecordCommitted(shipment)) =
            (state, event)
        else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let receipt = build_receipt(
            shipment,
            &crate::receipt_flow::ShipmentToReceiptTransition {
                receiver_id: self.config.org.clone(),
                issuer: self.config.org.clone(),
                received_on: self.config.delivery_on.clone(),
            },
        );
        TransitionResult::new(
            PurchaseFlowState::Delivered {
                po_ref: po_ref.clone(),
                receipt_ref: receipt.record_id.clone(),
            },
            vec![Action::EmitRecord(receipt)],
            false,
        )
    }
}

/// Seller transition 3: consume the delivery receipt for the shipped lot.
#[derive(Debug)]
pub struct AwaitSettlementTransition;

impl Transition<PurchaseFlowState> for AwaitSettlementTransition {
    fn name(&self) -> &'static str {
        "AwaitSettlement"
    }

    fn matches(&self, state: &PurchaseFlowState, event: &Event) -> bool {
        let PurchaseFlowState::AwaitingReceipt { shipment_ref, .. } = state else {
            return false;
        };
        matches!(
            event,
            Event::RecordCommitted(record)
                if record.schema_id == "delivery_receipt"
                    && record.payload.get("shipment_ref").and_then(Value::as_str)
                        == Some(shipment_ref.as_str())
        )
    }

    fn apply(
        &self,
        state: &PurchaseFlowState,
        event: &Event,
    ) -> TransitionResult<PurchaseFlowState> {
        let (
            PurchaseFlowState::AwaitingReceipt {
                po_ref,
                shipment_ref: _,
            },
            Event::RecordCommitted(receipt),
        ) = (state, event)
        else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        TransitionResult::new(
            PurchaseFlowState::Delivered {
                po_ref: po_ref.clone(),
                receipt_ref: receipt.record_id.clone(),
            },
            Vec::new(),
            false,
        )
    }
}

/// Either party (Dispute step): raise a dispute over the delivered receipt.
/// The wake reason after `"dispute:"` carries the dispute reason.
#[derive(Debug)]
pub struct RaiseDisputeTransition;

impl Transition<PurchaseFlowState> for RaiseDisputeTransition {
    fn name(&self) -> &'static str {
        "RaiseDispute"
    }

    fn matches(&self, state: &PurchaseFlowState, event: &Event) -> bool {
        matches!(state, PurchaseFlowState::Delivered { .. })
            && matches!(event, Event::Woken(reason) if reason.starts_with("dispute:"))
    }

    fn apply(
        &self,
        state: &PurchaseFlowState,
        event: &Event,
    ) -> TransitionResult<PurchaseFlowState> {
        let (
            PurchaseFlowState::Delivered {
                po_ref,
                receipt_ref,
            },
            Event::Woken(reason),
        ) = (state, event)
        else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        TransitionResult::new(
            PurchaseFlowState::Disputed {
                po_ref: po_ref.clone(),
                receipt_ref: receipt_ref.clone(),
                reason: reason["dispute:".len()..].to_owned(),
            },
            Vec::new(),
            false,
        )
    }
}

/// Either party (Settlement step): settle the deal — terminal. Reachable from
/// `Delivered` (clean settlement) or `Disputed` (resolved dispute).
#[derive(Debug)]
pub struct SettleTransition;

impl Transition<PurchaseFlowState> for SettleTransition {
    fn name(&self) -> &'static str {
        "Settle"
    }

    fn matches(&self, state: &PurchaseFlowState, event: &Event) -> bool {
        matches!(
            state,
            PurchaseFlowState::Delivered { .. } | PurchaseFlowState::Disputed { .. }
        ) && matches!(event, Event::Woken(reason) if reason == "settle")
    }

    fn apply(
        &self,
        state: &PurchaseFlowState,
        _event: &Event,
    ) -> TransitionResult<PurchaseFlowState> {
        let po_ref = match state {
            PurchaseFlowState::Delivered { po_ref, .. }
            | PurchaseFlowState::Disputed { po_ref, .. } => po_ref.clone(),
            _ => return TransitionResult::new(state.clone(), Vec::new(), false),
        };
        TransitionResult::new(PurchaseFlowState::Settled { po_ref }, Vec::new(), true)
    }
}

/// The buyer's purchase-to-settlement flow (RFQ → Quote → PO → Receipt →
/// Dispute → Settlement).
#[must_use]
pub fn buyer_flow(config: &PurchaseFlowConfig) -> FlowRunner<PurchaseFlowState> {
    FlowRunner::new(
        "purchase_to_settlement",
        vec![
            Box::new(AcceptQuoteTransition),
            Box::new(CommitPurchaseOrderTransition {
                config: config.clone(),
            }),
            Box::new(RecordDeliveryTransition {
                config: config.clone(),
            }),
            Box::new(RaiseDisputeTransition),
            Box::new(SettleTransition),
        ],
    )
}

/// The seller's purchase-to-settlement flow (Acceptance → Shipment →
/// Dispute → Settlement).
#[must_use]
pub fn seller_flow(config: &PurchaseFlowConfig) -> FlowRunner<PurchaseFlowState> {
    FlowRunner::new(
        "purchase_to_settlement",
        vec![
            Box::new(AcceptPurchaseOrderTransition {
                config: config.clone(),
            }),
            Box::new(ShipOrderTransition {
                config: config.clone(),
            }),
            Box::new(AwaitSettlementTransition),
            Box::new(RaiseDisputeTransition),
            Box::new(SettleTransition),
        ],
    )
}

/// Build the `purchase_order` record for the negotiated terms.
fn build_purchase_order(rfq_id: &str, config: &PurchaseFlowConfig) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert(
        "product_id".to_owned(),
        Value::String(config.product_id.clone()),
    );
    payload.insert("buyer_id".to_owned(), Value::String(config.org.clone()));
    payload.insert(
        "seller_id".to_owned(),
        Value::String(config.counterparty.clone()),
    );
    payload.insert("quantity".to_owned(), Value::from(config.quantity));
    payload.insert(
        "currency".to_owned(),
        Value::String(config.currency.clone()),
    );
    let mut record = CanonicalRecord::new(
        config.negotiated_at,
        "purchase_order",
        payload,
        config.org.clone(),
    );
    record.record_id = format!("po:{rfq_id}");
    record
}

/// Build the `shipment` record for the accepted PO.
fn build_shipment(po_ref: &str, occurred_at: u64, config: &PurchaseFlowConfig) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert("lot_ref".to_owned(), Value::String(config.lot_ref.clone()));
    payload.insert("from_org".to_owned(), Value::String(config.org.clone()));
    payload.insert(
        "to_org".to_owned(),
        Value::String(config.counterparty.clone()),
    );
    let mut record = CanonicalRecord::new(occurred_at, "shipment", payload, config.org.clone());
    record.record_id = format!("shipment:{po_ref}:{}", config.lot_ref);
    record
}

impl PurchaseFlowConfig {
    /// The shipment's `occurred_at`: one second after the PO's, derived from
    /// the config seed the same way `build_purchase_order` stamps the PO.
    const fn shipment_timestamp(&self) -> Option<u64> {
        self.negotiated_at.checked_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::FlowTriage;
    use glasschain_core::providers::in_memory::InMemoryStorageProvider;
    use std::sync::Arc;

    fn config(org: &str, counterparty: &str) -> PurchaseFlowConfig {
        PurchaseFlowConfig {
            org: org.to_owned(),
            counterparty: counterparty.to_owned(),
            product_id: "SKU-001".to_owned(),
            quantity: 100,
            currency: "BRL".to_owned(),
            lot_ref: "lot-1".to_owned(),
            rfq_id: "rfq-1".to_owned(),
            negotiated_at: 1_700_000_000,
            delivery_on: "2026-09-01".to_owned(),
        }
    }

    const FLOW_ID: &str = "purchase:rfq-1";

    fn storage() -> Arc<dyn glasschain_core::StorageProvider> {
        Arc::new(InMemoryStorageProvider::new())
    }

    fn committed(schema_id: &str, record_id: &str, payload: &[(&str, Value)]) -> Event {
        let mut record = CanonicalRecord::new(
            1_700_000_000,
            schema_id,
            payload
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
            "org-maker",
        );
        record.record_id = record_id.to_owned();
        Event::RecordCommitted(record)
    }

    /// The buyer flow's initial state, derived from the config.
    fn buyer_initial(config: &PurchaseFlowConfig) -> PurchaseFlowState {
        PurchaseFlowState::RfqIssued {
            rfq_id: config.rfq_id.clone(),
        }
    }

    /// Drive the buyer flow through the quote and PO steps: quote acceptance,
    /// PO emission, ack. Returns the emitted `purchase_order` record.
    fn drive_to_awaiting_shipment(
        flow: &FlowRunner<PurchaseFlowState>,
        storage: &Arc<dyn glasschain_core::StorageProvider>,
        triage: &FlowTriage,
        initial: &PurchaseFlowState,
    ) -> CanonicalRecord {
        let outcome = flow
            .handle(
                storage,
                triage,
                FLOW_ID,
                initial,
                &Event::Woken("quote-accepted:q-1".into()),
            )
            .expect("handle quote")
            .expect("quote transition applies");
        assert_eq!(
            outcome.state,
            PurchaseFlowState::QuoteAccepted {
                rfq_id: "rfq-1".into(),
                quote_id: "q-1".into(),
            }
        );
        assert!(outcome.actions.is_empty());

        let outcome = flow
            .handle(
                storage,
                triage,
                FLOW_ID,
                initial,
                &Event::Woken("commit-po".into()),
            )
            .expect("handle commit-po")
            .expect("po transition applies");
        let PurchaseFlowState::AwaitingShipment { po_ref } = &outcome.state else {
            panic!("expected AwaitingShipment, got {:?}", outcome.state);
        };
        assert_eq!(po_ref, "po:rfq-1");
        let [Action::EmitRecord(purchase_order)] = &outcome.actions[..] else {
            panic!("expected one EmitRecord, got {:?}", outcome.actions);
        };
        purchase_order.clone()
    }

    #[test]
    fn test_buyer_po_emission_is_deterministic() {
        let replay_storage = storage();
        let storage = storage();
        let triage = FlowTriage::new();
        let config = config("org-buyer", "org-maker");
        let flow = buyer_flow(&config);
        let initial = buyer_initial(&config);

        let purchase_order = drive_to_awaiting_shipment(&flow, &storage, &triage, &initial);
        assert_eq!(purchase_order.record_id, "po:rfq-1");
        assert_eq!(purchase_order.schema_id, "purchase_order");
        assert_eq!(
            purchase_order
                .payload
                .get("buyer_id")
                .and_then(Value::as_str),
            Some("org-buyer")
        );
        assert_eq!(
            purchase_order
                .payload
                .get("quantity")
                .and_then(Value::as_u64),
            Some(100)
        );

        // Ack advances the checkpoint; an independent flow with the same
        // config emits the byte-identical record (deterministic ids).
        flow.ack(&storage, &triage, FLOW_ID, 1).expect("ack po");
        let replay_flow = buyer_flow(&config);
        let replay_initial = buyer_initial(&config);
        let replay =
            drive_to_awaiting_shipment(&replay_flow, &replay_storage, &triage, &replay_initial);
        assert_eq!(replay, purchase_order, "emissions are deterministic");
    }

    #[test]
    fn test_buyer_receipt_dispute_and_settlement() {
        let storage = storage();
        let triage = FlowTriage::new();
        let config = config("org-buyer", "org-maker");
        let flow = buyer_flow(&config);
        let initial = buyer_initial(&config);

        let purchase_order = drive_to_awaiting_shipment(&flow, &storage, &triage, &initial);
        flow.ack(&storage, &triage, FLOW_ID, 1).expect("ack po");

        // Receipt step: the incoming shipment is consumed, receipt emitted.
        let shipment = committed(
            "shipment",
            "shipment:po:rfq-1:lot-1",
            &[
                ("lot_ref", Value::String("lot-1".into())),
                ("from_org", Value::String("org-maker".into())),
                ("to_org", Value::String("org-buyer".into())),
            ],
        );
        let outcome = flow
            .handle(&storage, &triage, FLOW_ID, &initial, &shipment)
            .expect("handle shipment")
            .expect("transition applies");
        let PurchaseFlowState::Delivered {
            po_ref,
            receipt_ref,
        } = &outcome.state
        else {
            panic!("expected Delivered, got {:?}", outcome.state);
        };
        assert_eq!(po_ref, "po:rfq-1");
        assert_eq!(receipt_ref, "receipt:shipment:po:rfq-1:lot-1");
        let [Action::EmitRecord(receipt)] = &outcome.actions[..] else {
            panic!("expected one EmitRecord, got {:?}", outcome.actions);
        };
        assert_eq!(receipt.schema_id, "delivery_receipt");
        assert_eq!(
            receipt.payload.get("receiver_id").and_then(Value::as_str),
            Some("org-buyer")
        );
        flow.ack(&storage, &triage, FLOW_ID, 1)
            .expect("ack receipt");

        // Dispute and settlement closes the chain.
        let outcome = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::Woken("dispute:packaging-damaged".into()),
            )
            .expect("handle dispute")
            .expect("transition applies");
        assert!(matches!(outcome.state, PurchaseFlowState::Disputed { .. }));
        let outcome = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::Woken("settle".into()),
            )
            .expect("handle settle")
            .expect("transition applies");
        assert_eq!(
            outcome.state,
            PurchaseFlowState::Settled {
                po_ref: "po:rfq-1".into(),
            }
        );
        assert!(outcome.completed);
        assert_eq!(purchase_order.record_id, "po:rfq-1");
    }

    #[test]
    fn test_seller_chain_consumes_po_emits_shipment() {
        let storage = storage();
        let triage = FlowTriage::new();
        let config = config("org-maker", "org-buyer");
        let flow = seller_flow(&config);
        let initial = PurchaseFlowState::AwaitingPurchaseOrder;

        // A PO for a different seller must not match.
        let other_po = committed(
            "purchase_order",
            "po:other",
            &[
                ("product_id", Value::String("SKU-001".into())),
                ("buyer_id", Value::String("org-buyer".into())),
                ("seller_id", Value::String("org-other".into())),
                ("quantity", Value::from(100)),
                ("currency", Value::String("BRL".into())),
            ],
        );
        assert!(
            flow.handle(&storage, &triage, FLOW_ID, &initial, &other_po)
                .expect("handle other po")
                .is_none(),
            "another seller's PO must be ignored"
        );

        // Acceptance consumes this seller's PO.
        let po = committed(
            "purchase_order",
            "po:rfq-1",
            &[
                ("product_id", Value::String("SKU-001".into())),
                ("buyer_id", Value::String("org-buyer".into())),
                ("seller_id", Value::String("org-maker".into())),
                ("quantity", Value::from(100)),
                ("currency", Value::String("BRL".into())),
            ],
        );
        let outcome = flow
            .handle(&storage, &triage, FLOW_ID, &initial, &po)
            .expect("handle po")
            .expect("transition applies");
        assert_eq!(
            outcome.state,
            PurchaseFlowState::PoAccepted {
                po_ref: "po:rfq-1".into(),
            }
        );

        // Shipment step emits the custody edge's first half.
        let outcome = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::Woken("ship".into()),
            )
            .expect("handle ship")
            .expect("transition applies");
        let PurchaseFlowState::AwaitingReceipt {
            po_ref,
            shipment_ref,
        } = &outcome.state
        else {
            panic!("expected AwaitingReceipt, got {:?}", outcome.state);
        };
        assert_eq!(po_ref, "po:rfq-1");
        assert_eq!(shipment_ref, "shipment:po:rfq-1:lot-1");
        let [Action::EmitRecord(shipment)] = &outcome.actions[..] else {
            panic!("expected one EmitRecord, got {:?}", outcome.actions);
        };
        assert_eq!(shipment.record_id, "shipment:po:rfq-1:lot-1");
        assert_eq!(shipment.occurred_at, config.negotiated_at + 1);
        assert_eq!(
            shipment.payload.get("from_org").and_then(Value::as_str),
            Some("org-maker")
        );
        assert_eq!(
            shipment.payload.get("to_org").and_then(Value::as_str),
            Some("org-buyer")
        );
        flow.ack(&storage, &triage, FLOW_ID, 1)
            .expect("ack shipment");

        // The buyer's delivery receipt completes the custody edge.
        let receipt = committed(
            "delivery_receipt",
            "receipt:shipment:po:rfq-1:lot-1",
            &[
                (
                    "shipment_ref",
                    Value::String("shipment:po:rfq-1:lot-1".into()),
                ),
                ("receiver_id", Value::String("org-buyer".into())),
                ("received_at", Value::String("2026-09-01".into())),
            ],
        );
        let outcome = flow
            .handle(&storage, &triage, FLOW_ID, &initial, &receipt)
            .expect("handle receipt")
            .expect("transition applies");
        assert!(matches!(outcome.state, PurchaseFlowState::Delivered { .. }));
    }

    #[test]
    fn test_interrupted_emission_redelivers_without_state_loss() {
        let storage = storage();
        let triage = FlowTriage::new();
        let config = config("org-buyer", "org-maker");
        let flow = buyer_flow(&config);
        let initial = buyer_initial(&config);
        flow.handle(
            &storage,
            &triage,
            FLOW_ID,
            &initial,
            &Event::Woken("quote-accepted:q-1".into()),
        )
        .expect("quote")
        .expect("applies");

        // The PO emission starts but the runtime crashes before ack.
        let first = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::Woken("commit-po".into()),
            )
            .expect("handle")
            .expect("applies");
        assert_eq!(first.actions.len(), 1);

        // Resume without ack: the same action is re-delivered from the
        // checkpoint — no loss, no state divergence.
        let resumed = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::Resumed("resume after crash".into()),
            )
            .expect("resume")
            .expect("pending work re-delivered");
        assert_eq!(resumed.actions, first.actions);
        assert_eq!(resumed.state, first.state);

        // Ack once: the checkpoint advances to AwaitingShipment.
        flow.ack(&storage, &triage, FLOW_ID, 1).expect("ack");
        let state = flow.current_state(&storage, FLOW_ID).expect("state");
        assert_eq!(
            state,
            Some(PurchaseFlowState::AwaitingShipment {
                po_ref: "po:rfq-1".into(),
            })
        );
    }
}
