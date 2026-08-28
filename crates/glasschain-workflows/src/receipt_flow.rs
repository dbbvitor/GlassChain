//! Example flow: shipment → receipt over canonical v1 records.
//!
//! Demonstrates the framework's canonical-record contract (ticket #40 AC3):
//! a flow **consumes** committed canonical records as inputs and **emits** new
//! canonical records as outputs, referencing the immutable lot commitment
//! without ever mutating the source records.
//!
//! Shape:
//! ```text
//! AwaitingLot ──(RecordCommitted: lot)──▶ LotAnchored { lot_ref, lot_commitment }
//! LotAnchored  ──(RecordCommitted: shipment for lot_ref)──▶ Completed
//!               └── emits: delivery_receipt { shipment_ref, receiver_id, received_at }
//! ```
//!
//! The emitted record carries an empty signature set: attaching verified
//! signatures is the endorsement layer's job (#45), performed by the runtime
//! before submission.

use crate::action::Action;
use crate::event::Event;
use crate::runner::FlowRunner;
use crate::state::FlowState;
use crate::transition::{Transition, TransitionResult};
use glasschain_core::CanonicalRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The shipment→receipt flow's protocol state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptFlowState {
    /// Waiting for the lot record to anchor.
    AwaitingLot,
    /// The lot is anchored; the flow holds its immutable commitment.
    LotAnchored {
        /// `record_id` of the anchored lot.
        lot_ref: String,
        /// The lot record's canonical commitment (immutable anchor).
        lot_commitment: String,
    },
    /// The receipt was emitted; terminal state.
    Completed { receipt_ref: String },
}

impl FlowState for ReceiptFlowState {
    fn step(&self) -> &'static str {
        match self {
            Self::AwaitingLot => "awaiting_lot",
            Self::LotAnchored { .. } => "lot_anchored",
            Self::Completed { .. } => "completed",
        }
    }
}

/// Transition 1: anchor an immutable lot commitment from a committed lot
/// record.
#[derive(Debug)]
pub struct AnchorLotTransition;

impl Transition<ReceiptFlowState> for AnchorLotTransition {
    fn name(&self) -> &'static str {
        "AnchorLot"
    }

    fn matches(&self, state: &ReceiptFlowState, event: &Event) -> bool {
        matches!(state, ReceiptFlowState::AwaitingLot)
            && matches!(
                event,
                Event::RecordCommitted(record)
                    if record.schema_id == "lot" && record.commitment.is_some()
            )
    }

    fn apply(&self, state: &ReceiptFlowState, event: &Event) -> TransitionResult<ReceiptFlowState> {
        let Event::RecordCommitted(lot) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Some(commitment) = &lot.commitment else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        TransitionResult::new(
            ReceiptFlowState::LotAnchored {
                lot_ref: lot.record_id.clone(),
                lot_commitment: commitment.clone(),
            },
            Vec::new(),
            false,
        )
    }
}

/// Transition 2: consume a shipment for the anchored lot and emit the
/// delivery receipt.
#[derive(Debug, Clone)]
pub struct ShipmentToReceiptTransition {
    /// The receiving organization (receipt `receiver_id`).
    pub receiver_id: String,
    /// The issuing identity stamped on the emitted receipt.
    pub issuer: String,
    /// ISO-8601 date (`YYYY-MM-DD`) of receipt — flow config, not wall clock.
    pub received_on: String,
}

impl Transition<ReceiptFlowState> for ShipmentToReceiptTransition {
    fn name(&self) -> &'static str {
        "ShipmentToReceipt"
    }

    fn matches(&self, state: &ReceiptFlowState, event: &Event) -> bool {
        let ReceiptFlowState::LotAnchored { lot_ref, .. } = state else {
            return false;
        };
        let Event::RecordCommitted(shipment) = event else {
            return false;
        };
        shipment.schema_id == "shipment"
            && shipment.payload.get("lot_ref").and_then(Value::as_str) == Some(lot_ref.as_str())
    }

    fn apply(&self, state: &ReceiptFlowState, event: &Event) -> TransitionResult<ReceiptFlowState> {
        let Event::RecordCommitted(shipment) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let receipt = build_receipt(shipment, self);
        TransitionResult::new(
            ReceiptFlowState::Completed {
                receipt_ref: receipt.record_id.clone(),
            },
            vec![Action::EmitRecord(receipt)],
            true,
        )
    }
}

/// The standard shipment→receipt flow definition.
#[must_use]
pub fn shipment_receipt_flow(
    receiver_id: &str,
    issuer: &str,
    received_on: &str,
) -> FlowRunner<ReceiptFlowState> {
    FlowRunner::new(
        "shipment_receipt",
        vec![
            Box::new(AnchorLotTransition),
            Box::new(ShipmentToReceiptTransition {
                receiver_id: receiver_id.to_owned(),
                issuer: issuer.to_owned(),
                received_on: received_on.to_owned(),
            }),
        ],
    )
}

/// Build the `delivery_receipt` emitted for a consumed shipment.
///
/// Every field derives deterministically from the inputs: `record_id` and
/// `occurred_at` come from the shipment and `received_on` from the transition
/// config — never the wall clock — so replaying the transition emits the
/// identical record and the ledger dedupes it.
fn build_receipt(
    shipment: &CanonicalRecord,
    transition: &ShipmentToReceiptTransition,
) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert(
        "shipment_ref".to_owned(),
        Value::String(shipment.record_id.clone()),
    );
    payload.insert(
        "receiver_id".to_owned(),
        Value::String(transition.receiver_id.clone()),
    );
    payload.insert(
        "received_at".to_owned(),
        Value::String(transition.received_on.clone()),
    );

    let mut receipt = CanonicalRecord::new(
        shipment.occurred_at + 1,
        "delivery_receipt",
        payload,
        transition.issuer.clone(),
    );
    receipt.record_id = format!("receipt:{}", shipment.record_id);
    receipt
}
