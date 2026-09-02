//! Recall, quarantine, and dispute flows (ticket #44): a public recall
//! lifecycle over the `recall` record family and custodian responses over the
//! `inventory_transformation` family, all referencing the immutable lot
//! commitment.
//!
//! Three first-class flows:
//!
//! - [`recall_flow`] — the issuer's lifecycle:
//!   `recall{status:"issued"}` → `active` → `completed`, one append-only
//!   record per status change (source records are never mutated).
//! - [`quarantine_flow`] — a lot custodian observes the **public** recall
//!   record (custodians are never the recall's counterparty — this is what
//!   makes the trail traversable) and quarantines the lot.
//! - [`dispute_flow`] — a custodian disputes the recall. The dispute reason
//!   travels only in the wake reason and the transient checkpoint — never into
//!   a committed payload, because the `inventory_transformation` whitelist
//!   admits only `lot_ref` and `transformation_type` (ADR-010 §1 by
//!   construction).
//!
//! Every emitted `record_id` and `occurred_at` derives from the config or the
//! consumed record; hosts submit with `Transaction::with_id(record.record_id, …)`
//! and keep `lot_ref` globally unique (same contract as [`crate::purchase_flow`]).

use crate::action::Action;
use crate::event::Event;
use crate::runner::FlowRunner;
use crate::state::FlowState;
use crate::transition::{Transition, TransitionResult};
use glasschain_core::CanonicalRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Configuration for the recall lifecycle flow (the issuer's side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallConfig {
    /// The issuing organization (envelope issuer and payload `issued_by`).
    pub issuer: String,
    /// The anchored lot being recalled (`lot_ref`).
    pub lot_ref: String,
    /// Unix-seconds seed for the emitted records' `occurred_at`.
    pub issued_at: u64,
}

/// The recall lifecycle's protocol state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecallFlowState {
    /// Waiting for the anchored lot record.
    AwaitingLot,
    /// The lot is anchored; the flow holds its immutable commitment.
    LotAnchored {
        /// `record_id` of the anchored lot.
        lot_ref: String,
        /// The lot record's canonical commitment (immutable anchor).
        lot_commitment: String,
    },
    /// The recall was issued (`status: "issued"` record emitted).
    Recalled {
        lot_ref: String,
        /// `record_id` of the issued recall record.
        recall_ref: String,
        /// The public recall reason (carried into every status record).
        reason: String,
    },
    /// The recall was activated (`status: "active"` record emitted).
    RecallActive {
        lot_ref: String,
        /// `record_id` of the most recent status record.
        recall_ref: String,
        reason: String,
    },
    /// Terminal: the recall completed.
    Completed {
        /// `record_id` of the final status record.
        recall_ref: String,
    },
}

impl FlowState for RecallFlowState {
    fn step(&self) -> &'static str {
        match self {
            Self::AwaitingLot => "awaiting_lot",
            Self::LotAnchored { .. } => "lot_anchored",
            Self::Recalled { .. } => "recalled",
            Self::RecallActive { .. } => "recall_active",
            Self::Completed { .. } => "completed",
        }
    }
}

/// Anchor the immutable lot commitment of the **configured** lot.
///
/// Any other lot record is ignored — a recall must never anchor a different
/// batch than the one it was configured for.
#[derive(Debug, Clone)]
pub struct RecallAnchorLotTransition {
    pub config: RecallConfig,
}

impl Transition<RecallFlowState> for RecallAnchorLotTransition {
    fn name(&self) -> &'static str {
        "AnchorLot"
    }

    fn matches(&self, state: &RecallFlowState, event: &Event) -> bool {
        matches!(state, RecallFlowState::AwaitingLot)
            && matches!(
                event,
                Event::RecordCommitted(record)
                    if record.schema_id == "lot"
                        && record.commitment.is_some()
                        // A recall must anchor exactly the configured lot —
                        // anchoring whichever lot committed first would let a
                        // recall trail point at the wrong batch.
                        && record.record_id == self.config.lot_ref
            )
    }

    fn apply(&self, state: &RecallFlowState, event: &Event) -> TransitionResult<RecallFlowState> {
        let Event::RecordCommitted(lot) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Some(commitment) = &lot.commitment else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        TransitionResult::new(
            RecallFlowState::LotAnchored {
                lot_ref: lot.record_id.clone(),
                lot_commitment: commitment.clone(),
            },
            Vec::new(),
            false,
        )
    }
}

/// Issue the recall: wake reason `"recall:<reason>"` emits the
/// `status: "issued"` record — the public anchor of the whole trail.
#[derive(Debug, Clone)]
pub struct IssueRecallTransition {
    pub config: RecallConfig,
}

impl Transition<RecallFlowState> for IssueRecallTransition {
    fn name(&self) -> &'static str {
        "IssueRecall"
    }

    fn matches(&self, state: &RecallFlowState, event: &Event) -> bool {
        matches!(state, RecallFlowState::LotAnchored { .. })
            && matches!(event, Event::Woken(reason) if reason.starts_with("recall:"))
    }

    fn apply(&self, state: &RecallFlowState, event: &Event) -> TransitionResult<RecallFlowState> {
        let RecallFlowState::LotAnchored { lot_ref, .. } = state else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Event::Woken(reason) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let recall_reason = reason.strip_prefix("recall:").unwrap_or_default();
        let record = build_recall(lot_ref, recall_reason, "issued", &self.config);
        let recall_ref = record.record_id.clone();
        TransitionResult::new(
            RecallFlowState::Recalled {
                lot_ref: lot_ref.clone(),
                recall_ref,
                reason: recall_reason.to_owned(),
            },
            vec![Action::EmitRecord(record)],
            false,
        )
    }
}

/// Activate the recall: emits a NEW record with `status: "active"`.
#[derive(Debug, Clone)]
pub struct ActivateRecallTransition {
    pub config: RecallConfig,
}

impl Transition<RecallFlowState> for ActivateRecallTransition {
    fn name(&self) -> &'static str {
        "ActivateRecall"
    }

    fn matches(&self, state: &RecallFlowState, event: &Event) -> bool {
        matches!(state, RecallFlowState::Recalled { .. })
            && matches!(event, Event::Woken(reason) if reason == "activate")
    }

    fn apply(&self, state: &RecallFlowState, event: &Event) -> TransitionResult<RecallFlowState> {
        let RecallFlowState::Recalled {
            lot_ref,
            recall_ref: _,
            reason,
        } = state
        else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Event::Woken(_) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let record = build_recall(lot_ref, reason, "active", &self.config);
        TransitionResult::new(
            RecallFlowState::RecallActive {
                lot_ref: lot_ref.clone(),
                recall_ref: record.record_id.clone(),
                reason: reason.clone(),
            },
            vec![Action::EmitRecord(record)],
            false,
        )
    }
}

/// Complete the recall: emits a NEW record with `status: "completed"` and
/// reaches the terminal state.
#[derive(Debug, Clone)]
pub struct CompleteRecallTransition {
    pub config: RecallConfig,
}

impl Transition<RecallFlowState> for CompleteRecallTransition {
    fn name(&self) -> &'static str {
        "CompleteRecall"
    }

    fn matches(&self, state: &RecallFlowState, event: &Event) -> bool {
        matches!(state, RecallFlowState::RecallActive { .. })
            && matches!(event, Event::Woken(reason) if reason == "complete")
    }

    fn apply(&self, state: &RecallFlowState, event: &Event) -> TransitionResult<RecallFlowState> {
        let RecallFlowState::RecallActive {
            lot_ref, reason, ..
        } = state
        else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Event::Woken(_) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let record = build_recall(lot_ref, reason, "completed", &self.config);
        TransitionResult::new(
            RecallFlowState::Completed {
                recall_ref: record.record_id.clone(),
            },
            vec![Action::EmitRecord(record)],
            true,
        )
    }
}

/// The issuer's recall lifecycle flow: issue → activate → complete, one
/// append-only `recall` record per status.
#[must_use]
pub fn recall_flow(config: RecallConfig) -> FlowRunner<RecallFlowState> {
    FlowRunner::new(
        "recall",
        vec![
            Box::new(RecallAnchorLotTransition {
                config: config.clone(),
            }),
            Box::new(IssueRecallTransition {
                config: config.clone(),
            }),
            Box::new(ActivateRecallTransition {
                config: config.clone(),
            }),
            Box::new(CompleteRecallTransition { config }),
        ],
    )
}

/// Build a `recall` record for `lot_ref` with `status`.
fn build_recall(
    lot_ref: &str,
    reason: &str,
    status: &'static str,
    config: &RecallConfig,
) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert("lot_ref".to_owned(), Value::String(lot_ref.to_owned()));
    payload.insert("reason".to_owned(), Value::String(reason.to_owned()));
    payload.insert("status".to_owned(), Value::String(status.to_owned()));
    payload.insert("issued_by".to_owned(), Value::String(config.issuer.clone()));
    let mut record =
        CanonicalRecord::new(config.issued_at, "recall", payload, config.issuer.clone());
    record.record_id = match status {
        "issued" => format!("recall:{lot_ref}"),
        other => format!("recall:{lot_ref}:{other}"),
    };
    record
}

/// Configuration for a custodian response flow (quarantine or dispute).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallResponseConfig {
    /// The responding (custodian) organization.
    pub org: String,
    /// The lot the custodian holds and watches for recalls.
    pub lot_ref: String,
    /// Unix-seconds seed for the emitted record's `occurred_at`.
    pub responded_at: u64,
}

/// The custodian response flow's protocol state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecallResponseState {
    /// Watching the public chain for a recall on the held lot.
    WatchingLot { lot_ref: String },
    /// A recall on the held lot was observed.
    RecallObserved {
        lot_ref: String,
        /// `record_id` of the observed recall record.
        recall_ref: String,
    },
    /// The response record was emitted; terminal.
    Responded {
        lot_ref: String,
        /// `record_id` of the emitted custody/status record.
        response_ref: String,
    },
}

impl FlowState for RecallResponseState {
    fn step(&self) -> &'static str {
        match self {
            Self::WatchingLot { .. } => "watching_lot",
            Self::RecallObserved { .. } => "recall_observed",
            Self::Responded { .. } => "responded",
        }
    }
}

/// Observe the public recall record for the lot this custodian holds.
#[derive(Debug, Clone)]
pub struct ObserveRecallTransition {
    pub config: RecallResponseConfig,
}

impl Transition<RecallResponseState> for ObserveRecallTransition {
    fn name(&self) -> &'static str {
        "ObserveRecall"
    }

    fn matches(&self, state: &RecallResponseState, event: &Event) -> bool {
        let RecallResponseState::WatchingLot { lot_ref } = state else {
            return false;
        };
        matches!(
            event,
            Event::RecordCommitted(record)
                if record.schema_id == "recall"
                    && record.payload.get("lot_ref").and_then(Value::as_str)
                        == Some(lot_ref.as_str())
        )
    }

    fn apply(
        &self,
        state: &RecallResponseState,
        event: &Event,
    ) -> TransitionResult<RecallResponseState> {
        let Event::RecordCommitted(recall) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        TransitionResult::new(
            RecallResponseState::RecallObserved {
                lot_ref: self.config.lot_ref.clone(),
                recall_ref: recall.record_id.clone(),
            },
            Vec::new(),
            false,
        )
    }
}

/// Emit the custodian's `inventory_transformation` custody/status record for
/// `transformation_type` (e.g. `"quarantine"`, `"disputed"`).
#[derive(Debug, Clone)]
pub struct RespondTransition {
    pub config: RecallResponseConfig,
    /// The transformation type this flow responds with.
    pub transformation_type: &'static str,
}

impl Transition<RecallResponseState> for RespondTransition {
    fn name(&self) -> &'static str {
        "Respond"
    }

    fn matches(&self, state: &RecallResponseState, event: &Event) -> bool {
        matches!(state, RecallResponseState::RecallObserved { .. })
            && matches!(event, Event::Woken(reason) if self.wake_matches(reason))
    }

    fn apply(
        &self,
        state: &RecallResponseState,
        event: &Event,
    ) -> TransitionResult<RecallResponseState> {
        let RecallResponseState::RecallObserved { lot_ref, .. } = state else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Event::Woken(_) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let record = build_response(lot_ref, self.transformation_type, &self.config);
        let response_ref = record.record_id.clone();
        TransitionResult::new(
            RecallResponseState::Responded {
                lot_ref: lot_ref.clone(),
                response_ref,
            },
            vec![Action::EmitRecord(record)],
            true,
        )
    }
}

impl RespondTransition {
    /// Whether `reason` is the business wake-up for this response. The dispute
    /// wake may carry the (off-chain) reason as a `"dispute:<reason>"` suffix.
    fn wake_matches(&self, reason: &str) -> bool {
        match self.transformation_type {
            "quarantine" => reason == "quarantine",
            "disputed" => reason == "dispute" || reason.starts_with("dispute:"),
            other => reason == other,
        }
    }
}

/// Build the custodian's `inventory_transformation` record.
fn build_response(
    lot_ref: &str,
    transformation_type: &str,
    config: &RecallResponseConfig,
) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert("lot_ref".to_owned(), Value::String(lot_ref.to_owned()));
    payload.insert(
        "transformation_type".to_owned(),
        Value::String(transformation_type.to_owned()),
    );
    let mut record = CanonicalRecord::new(
        config.responded_at,
        "inventory_transformation",
        payload,
        config.org.clone(),
    );
    record.record_id = format!("transformation:{lot_ref}:{transformation_type}");
    record
}

/// The quarantine flow: a custodian observes a public recall on its lot and
/// quarantines the held stock.
#[must_use]
pub fn quarantine_flow(config: RecallResponseConfig) -> FlowRunner<RecallResponseState> {
    response_flow("quarantine", config, "quarantine")
}

/// The dispute flow: a custodian observes a public recall on its lot and
/// disputes it. The dispute reason travels in the wake reason and stays in
/// flow state — never on the global chain.
#[must_use]
pub fn dispute_flow(config: RecallResponseConfig) -> FlowRunner<RecallResponseState> {
    response_flow("dispute", config, "disputed")
}

/// Shared constructor for the two custodian response flows.
fn response_flow(
    kind: &'static str,
    config: RecallResponseConfig,
    transformation_type: &'static str,
) -> FlowRunner<RecallResponseState> {
    FlowRunner::new(
        kind,
        vec![
            Box::new(ObserveRecallTransition {
                config: config.clone(),
            }),
            Box::new(RespondTransition {
                config,
                transformation_type,
            }),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::FlowTriage;
    use glasschain_core::providers::in_memory::InMemoryStorageProvider;
    use std::sync::Arc;

    const FLOW_ID: &str = "recall:lot-1";

    fn storage() -> Arc<dyn glasschain_core::StorageProvider> {
        Arc::new(InMemoryStorageProvider::new())
    }

    fn recall_config() -> RecallConfig {
        RecallConfig {
            issuer: "org-maker".to_owned(),
            lot_ref: "lot-1".to_owned(),
            issued_at: 1_700_000_200,
        }
    }

    fn response_config(org: &str) -> RecallResponseConfig {
        RecallResponseConfig {
            org: org.to_owned(),
            lot_ref: "lot-1".to_owned(),
            responded_at: 1_700_000_300,
        }
    }

    fn anchored_lot() -> (Event, String) {
        let mut payload = BTreeMap::new();
        payload.insert("lot_id".to_owned(), Value::String("LOT-1".into()));
        payload.insert("product_id".to_owned(), Value::String("SKU-001".into()));
        payload.insert("batch_number".to_owned(), Value::String("B-2026".into()));
        let mut lot = CanonicalRecord::new(1_700_000_000, "lot", payload, "org-maker");
        lot.record_id = "lot-1".to_owned();
        let commitment = lot.commitment().expect("lot commitment");
        lot.commitment = Some(commitment.clone());
        (Event::RecordCommitted(lot), commitment)
    }

    fn recall_event(lot_ref: &str) -> Event {
        let mut payload = BTreeMap::new();
        payload.insert("lot_ref".to_owned(), Value::String(lot_ref.to_owned()));
        payload.insert("reason".to_owned(), Value::String("contamination".into()));
        payload.insert("status".to_owned(), Value::String("issued".into()));
        payload.insert("issued_by".to_owned(), Value::String("org-maker".into()));
        let mut recall = CanonicalRecord::new(1_700_000_200, "recall", payload, "org-maker");
        recall.record_id = format!("recall:{lot_ref}");
        Event::RecordCommitted(recall)
    }

    #[test]
    fn test_recall_lifecycle_emits_append_only_status_trail() {
        let storage = storage();
        let triage = FlowTriage::new();
        let flow = recall_flow(recall_config());
        let initial = RecallFlowState::AwaitingLot;
        let (lot_event, lot_commitment) = anchored_lot();

        let outcome = flow
            .handle(&storage, &triage, FLOW_ID, &initial, &lot_event)
            .expect("anchor")
            .expect("applies");
        assert_eq!(
            outcome.state,
            RecallFlowState::LotAnchored {
                lot_ref: "lot-1".into(),
                lot_commitment,
            }
        );

        // Issue: emits the "issued" recall record.
        let outcome = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::Woken("recall:contamination-suspected".into()),
            )
            .expect("issue")
            .expect("applies");
        let [Action::EmitRecord(recall)] = &outcome.actions[..] else {
            panic!("expected the recall emission, got {:?}", outcome.actions);
        };
        assert_eq!(recall.record_id, "recall:lot-1");
        assert_eq!(
            recall.payload.get("status").and_then(Value::as_str),
            Some("issued")
        );
        assert_eq!(
            recall.payload.get("reason").and_then(Value::as_str),
            Some("contamination-suspected")
        );
        assert_eq!(
            recall.payload.get("issued_by").and_then(Value::as_str),
            Some("org-maker")
        );
        flow.ack(&storage, &triage, FLOW_ID, 1).expect("ack issue");

        // Activate: a NEW record with status "active" (append-only trail).
        let outcome = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::Woken("activate".into()),
            )
            .expect("activate")
            .expect("applies");
        let [Action::EmitRecord(active)] = &outcome.actions[..] else {
            panic!(
                "expected the activation emission, got {:?}",
                outcome.actions
            );
        };
        assert_eq!(active.record_id, "recall:lot-1:active");
        assert_eq!(
            active.payload.get("status").and_then(Value::as_str),
            Some("active")
        );
        flow.ack(&storage, &triage, FLOW_ID, 1)
            .expect("ack activate");

        // Complete: terminal, one more append-only record.
        let outcome = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::Woken("complete".into()),
            )
            .expect("complete")
            .expect("applies");
        assert_eq!(
            outcome.state,
            RecallFlowState::Completed {
                recall_ref: "recall:lot-1:completed".into(),
            }
        );
        let [Action::EmitRecord(completed)] = &outcome.actions[..] else {
            panic!(
                "expected the completion emission, got {:?}",
                outcome.actions
            );
        };
        assert_eq!(completed.record_id, "recall:lot-1:completed");
        assert_eq!(
            completed.payload.get("status").and_then(Value::as_str),
            Some("completed")
        );
        flow.ack(&storage, &triage, FLOW_ID, 1)
            .expect("ack complete");
        assert!(flow
            .current_state(&storage, FLOW_ID)
            .expect("state")
            .is_none());
    }

    #[test]
    fn test_unanchored_lot_does_not_start_recall() {
        let storage = storage();
        let triage = FlowTriage::new();
        let flow = recall_flow(recall_config());
        let initial = RecallFlowState::AwaitingLot;
        let (lot_event, _commitment) = anchored_lot();
        let Event::RecordCommitted(mut lot) = lot_event else {
            panic!("expected record");
        };
        lot.commitment = None;
        let outcome = flow
            .handle(
                &storage,
                &triage,
                FLOW_ID,
                &initial,
                &Event::RecordCommitted(lot),
            )
            .expect("handle");
        assert!(outcome.is_none(), "unanchored lot must not anchor the flow");
    }

    #[test]
    fn test_quarantine_flow_observes_public_recall() {
        let storage = storage();
        let triage = FlowTriage::new();
        let flow = quarantine_flow(response_config("org-distributor"));
        let initial = RecallResponseState::WatchingLot {
            lot_ref: "lot-1".to_owned(),
        };

        // A recall on another lot must be ignored.
        assert!(
            flow.handle(
                &storage,
                &triage,
                "response:lot-2",
                &initial,
                &recall_event("lot-2")
            )
            .expect("handle")
            .is_none(),
            "another lot's recall must not be observed"
        );

        let outcome = flow
            .handle(
                &storage,
                &triage,
                "response:lot-1",
                &initial,
                &recall_event("lot-1"),
            )
            .expect("observe")
            .expect("applies");
        assert_eq!(
            outcome.state,
            RecallResponseState::RecallObserved {
                lot_ref: "lot-1".into(),
                recall_ref: "recall:lot-1".into(),
            }
        );

        let outcome = flow
            .handle(
                &storage,
                &triage,
                "response:lot-1",
                &initial,
                &Event::Woken("quarantine".into()),
            )
            .expect("quarantine")
            .expect("applies");
        // Action-carrying transitions complete once their actions are acked.
        assert!(!outcome.completed);
        let [Action::EmitRecord(response)] = &outcome.actions[..] else {
            panic!(
                "expected the quarantine emission, got {:?}",
                outcome.actions
            );
        };
        assert_eq!(response.record_id, "transformation:lot-1:quarantine");
        assert_eq!(response.schema_id, "inventory_transformation");
        assert_eq!(
            response
                .payload
                .get("transformation_type")
                .and_then(Value::as_str),
            Some("quarantine")
        );
        flow.ack(&storage, &triage, "response:lot-1", 1)
            .expect("ack completes");
        assert!(flow
            .current_state(&storage, "response:lot-1")
            .expect("state")
            .is_none());
    }

    #[test]
    fn test_dispute_flow_keeps_reason_off_chain() {
        let storage = storage();
        let triage = FlowTriage::new();
        let flow = dispute_flow(response_config("org-pharmacy"));
        let initial = RecallResponseState::WatchingLot {
            lot_ref: "lot-1".to_owned(),
        };
        flow.handle(
            &storage,
            &triage,
            "response:lot-1",
            &initial,
            &recall_event("lot-1"),
        )
        .expect("observe")
        .expect("applies");

        let outcome = flow
            .handle(
                &storage,
                &triage,
                "response:lot-1",
                &initial,
                &Event::Woken("dispute:batch-not-in-our-stock".into()),
            )
            .expect("dispute")
            .expect("applies");
        let [Action::EmitRecord(response)] = &outcome.actions[..] else {
            panic!("expected the dispute emission, got {:?}", outcome.actions);
        };
        assert_eq!(response.record_id, "transformation:lot-1:disputed");
        assert_eq!(
            response
                .payload
                .get("transformation_type")
                .and_then(Value::as_str),
            Some("disputed")
        );
        // The dispute reason must not enter the record payload.
        let payload_json = serde_json::to_string(&response.payload).unwrap();
        assert!(
            !payload_json.contains("batch-not-in-our-stock"),
            "dispute reason leaked on-chain: {payload_json}"
        );
    }
}
