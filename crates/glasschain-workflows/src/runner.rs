// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! The flow runner: drives `(state, event)` pairs through a transition table,
//! persisting checkpoints and delivering the produced actions to the caller.
//!
//! The runner never performs the actions' I/O itself: [`FlowRunner::handle`]
//! returns the actions to execute, the caller executes them durably, and then
//! acknowledges with [`FlowRunner::ack`] — which is the only place the
//! checkpoint advances.  That ordering closes the loss window: a crash before
//! submission re-delivers the action, and a crash after submission but before
//! acknowledgement re-executes it (at-least-once), which deterministic ids
//! make exactly-once at the ledger.

use crate::action::Action;
use crate::checkpoint::{Checkpoint, CheckpointStore};
use crate::error::WorkflowError;
use crate::event::Event;
use crate::state::FlowState;
use crate::transition::{Transition, TransitionResult};
use crate::triage::FlowTriage;
use glasschain_core::StorageProvider;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// The result of driving a flow with one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowOutcome<S> {
    /// The flow's state after the event's transition.
    pub state: S,

    /// The actions the caller must execute durably, in order, then
    /// acknowledge with [`FlowRunner::ack`].  Empty when the flow has no
    /// external work for this event.
    pub actions: Vec<Action>,

    /// `true` when the flow is terminal.  When `actions` is non-empty, the
    /// flow completes once those actions are executed and acknowledged; the
    /// runner clears the checkpoint on the final [`FlowRunner::ack`].
    pub completed: bool,
}

/// A flow definition: a stable kind name plus the ordered transition table.
///
/// The first transition whose [`Transition::matches`] accepts the pair wins —
/// dispatch order is part of the flow definition and therefore deterministic.
/// Transitions are `Send + Sync` so a runner can be shared across tasks.
pub struct FlowRunner<S> {
    flow_kind: &'static str,
    transitions: Vec<Box<dyn Transition<S> + Send + Sync>>,
}

impl<S: FlowState> FlowRunner<S> {
    /// Build a flow definition.
    #[must_use]
    pub fn new(
        flow_kind: &'static str,
        transitions: Vec<Box<dyn Transition<S> + Send + Sync>>,
    ) -> Self {
        Self {
            flow_kind,
            transitions,
        }
    }

    /// The flow definition's stable kind name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        self.flow_kind
    }

    /// Drive `flow_id` with `event`, resuming from its checkpoint if one
    /// exists or starting from `initial_state` otherwise.
    ///
    /// # Durability contract
    ///
    /// - `handle` returns the actions to execute **without executing them**;
    ///   the checkpoint still says nothing ran.  The caller executes each
    ///   action durably and then calls [`Self::ack`] with the count executed —
    ///   only `ack` advances the checkpoint.  A crash before submission
    ///   re-delivers the action on resume (**no loss**).
    /// - A crash after submission but before `ack` re-executes the action
    ///   (**at-least-once**); because emissions carry deterministic ids (see
    ///   [`Action`]), the ledger dedupes them — the *effect* is exactly-once.
    /// - A flow with pending work is busy: `handle` returns the remaining
    ///   actions and ignores the incoming event — re-deliver the event after
    ///   acknowledging.
    ///
    /// Returns `Ok(None)` when the flow ignores the event (no transition
    /// matched) or a `Resumed` event finds nothing pending.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::Storage`] when the backend fails,
    /// [`WorkflowError::CheckpointMismatch`] when a pending event no longer
    /// matches any transition, and [`WorkflowError::CheckpointDeserialization`]
    /// when a stored state no longer fits the flow definition.
    pub fn handle(
        &self,
        storage: &Arc<dyn StorageProvider>,
        triage: &FlowTriage,
        flow_id: &str,
        initial_state: &S,
        event: &Event,
    ) -> Result<Option<FlowOutcome<S>>, WorkflowError> {
        let store = CheckpointStore::new(Arc::clone(storage));

        let checkpoint = store.load(flow_id)?;
        let mut state = match &checkpoint {
            Some(saved) => serde_json::from_value(saved.state.clone())
                .map_err(|error| WorkflowError::CheckpointDeserialization(error.to_string()))?,
            None => initial_state.clone(),
        };

        // ── 1. Finish work a previous interruption left pending ─────────────
        if let Some(saved) = &checkpoint {
            if let Some(pending) = &saved.pending_event {
                let Some(result) = self.try_apply(&state, pending) else {
                    return Err(WorkflowError::CheckpointMismatch {
                        flow_id: flow_id.to_owned(),
                        event: format!("{pending:?}"),
                    });
                };
                if saved.next_action >= result.actions.len() {
                    // Every action was executed and acknowledged; only the
                    // final bookkeeping of the interrupted transition is left.
                    self.finalize(&store, triage, flow_id, &result.state, result.completed)?;
                    if result.completed {
                        return Ok(Some(FlowOutcome {
                            state: result.state,
                            actions: Vec::new(),
                            completed: true,
                        }));
                    }
                    state = result.state;
                    if matches!(event, Event::Resumed(_)) {
                        return Ok(Some(FlowOutcome {
                            state,
                            actions: Vec::new(),
                            completed: false,
                        }));
                    }
                } else {
                    // Re-deliver the not-yet-acknowledged actions.
                    return Ok(Some(FlowOutcome {
                        state: result.state,
                        actions: result.actions[saved.next_action..].to_vec(),
                        completed: false,
                    }));
                }
            } else {
                // Waiting checkpoint: re-surface the flow in triage with its
                // stored timestamp (re-discovery after a triage restart).
                triage.record(flow_id, &saved.flow_kind, state.step(), saved.updated_at);
                if matches!(event, Event::Resumed(_)) {
                    return Ok(None);
                }
            }
        } else if matches!(event, Event::Resumed(_)) {
            return Ok(None);
        }

        // ── 2. Handle the incoming event ────────────────────────────────────
        let Some(result) = self.try_apply(&state, event) else {
            return Ok(None);
        };
        if result.actions.is_empty() {
            // Nothing external to lose: finalize immediately.
            self.finalize(&store, triage, flow_id, &result.state, result.completed)?;
            return Ok(Some(FlowOutcome {
                state: result.state,
                actions: Vec::new(),
                completed: result.completed,
            }));
        }
        self.save_checkpoint(&store, triage, flow_id, &state, Some(event), 0)?;
        Ok(Some(FlowOutcome {
            state: result.state,
            actions: result.actions,
            completed: false,
        }))
    }

    /// Acknowledge that the caller durably executed the first `executed`
    /// actions of `flow_id`'s pending transition.
    ///
    /// Advances the checkpoint past the executed actions; when every action is
    /// acknowledged, finalizes the transition (clearing the checkpoint on
    /// completion, or persisting the waiting state otherwise).  This is the
    /// only place a pending transition's progress moves forward.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::Storage`] when the backend fails,
    /// [`WorkflowError::CheckpointMismatch`] when the pending event no longer
    /// matches any transition, and
    /// [`WorkflowError::CheckpointDeserialization`] when the stored state no
    /// longer fits the flow definition.
    pub fn ack(
        &self,
        storage: &Arc<dyn StorageProvider>,
        triage: &FlowTriage,
        flow_id: &str,
        executed: usize,
    ) -> Result<(), WorkflowError> {
        let store = CheckpointStore::new(Arc::clone(storage));
        let Some(saved) = store.load(flow_id)? else {
            return Ok(());
        };
        let Some(pending) = &saved.pending_event else {
            return Ok(());
        };
        let state: S = serde_json::from_value(saved.state).map_err(|error| {
            WorkflowError::CheckpointDeserialization(format!(
                "flow kind {}: {error}",
                self.flow_kind
            ))
        })?;
        let Some(result) = self.try_apply(&state, pending) else {
            return Err(WorkflowError::CheckpointMismatch {
                flow_id: flow_id.to_owned(),
                event: format!("{pending:?}"),
            });
        };

        let next = executed.min(result.actions.len());
        if next < result.actions.len() {
            self.save_checkpoint(&store, triage, flow_id, &state, Some(pending), next)?;
            return Ok(());
        }
        self.finalize(&store, triage, flow_id, &result.state, result.completed)
    }

    /// The flow's current state from its checkpoint, if it has one.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::Storage`] if the backend fails, or
    /// [`WorkflowError::CheckpointDeserialization`] if the stored state no
    /// longer fits the flow definition.
    pub fn current_state(
        &self,
        storage: &Arc<dyn StorageProvider>,
        flow_id: &str,
    ) -> Result<Option<S>, WorkflowError> {
        let store = CheckpointStore::new(Arc::clone(storage));
        let Some(saved) = store.load(flow_id)? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_value(saved.state).map_err(
            |error| {
                WorkflowError::CheckpointDeserialization(format!(
                    "flow kind {}: {error}",
                    self.flow_kind
                ))
            },
        )?))
    }

    /// Dispatch `(state, event)` to the first matching transition.
    #[must_use]
    fn try_apply(&self, state: &S, event: &Event) -> Option<TransitionResult<S>> {
        let transition = self.transitions.iter().find(|t| t.matches(state, event))?;
        log::debug!(
            "workflow {}: applying transition {} for step {}",
            self.flow_kind,
            transition.name(),
            state.step()
        );
        Some(transition.apply(state, event))
    }

    /// Close out a transition whose actions are all acknowledged: clear the
    /// checkpoint on completion, or persist the waiting state otherwise.
    fn finalize(
        &self,
        store: &CheckpointStore,
        triage: &FlowTriage,
        flow_id: &str,
        state: &S,
        completed: bool,
    ) -> Result<(), WorkflowError> {
        if completed {
            store.delete(flow_id)?;
            triage.clear(flow_id);
        } else {
            self.save_checkpoint(store, triage, flow_id, state, None, 0)?;
        }
        Ok(())
    }

    /// Persist a checkpoint and mirror it into the triage view.
    fn save_checkpoint(
        &self,
        store: &CheckpointStore,
        triage: &FlowTriage,
        flow_id: &str,
        state: &S,
        pending_event: Option<&Event>,
        next_action: usize,
    ) -> Result<(), WorkflowError> {
        let checkpoint = Checkpoint {
            flow_id: flow_id.to_owned(),
            flow_kind: self.flow_kind.to_owned(),
            state: serde_json::to_value(state)?,
            pending_event: pending_event.cloned(),
            next_action,
            step: state.step().to_owned(),
            updated_at: unix_now()?,
        };
        triage.record(flow_id, self.flow_kind, state.step(), checkpoint.updated_at);
        store.save(&checkpoint)
    }
}

/// Current Unix time in seconds.
fn unix_now() -> Result<u64, WorkflowError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| WorkflowError::Clock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{Checkpoint, CheckpointStore};
    use crate::receipt_flow::{shipment_receipt_flow, ReceiptFlowState};
    use crate::receipt_flow::{AnchorLotTransition, ShipmentToReceiptTransition};
    use crate::transition::Transition;
    use glasschain_core::providers::in_memory::InMemoryStorageProvider;
    use glasschain_core::{CanonicalRecord, RecordSignature, StorageProvider};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn storage() -> Arc<dyn StorageProvider> {
        Arc::new(InMemoryStorageProvider::new())
    }

    fn anchored() -> ReceiptFlowState {
        ReceiptFlowState::LotAnchored {
            lot_ref: "lot-1".to_owned(),
            lot_commitment: "c".to_owned(),
        }
    }

    fn completed(receipt_ref: &str) -> ReceiptFlowState {
        ReceiptFlowState::Completed {
            receipt_ref: receipt_ref.to_owned(),
        }
    }

    fn completed_receipt() -> ReceiptFlowState {
        completed("receipt:x")
    }

    fn signed(record: &mut CanonicalRecord, signer: &str) {
        record.signatures.push(RecordSignature {
            algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
            signer: signer.to_owned(),
            signature_bytes: b"sig".to_vec(),
        });
    }

    fn lot_record(occurred_at: u64) -> CanonicalRecord {
        let payload = BTreeMap::from([
            ("lot_id".to_owned(), json!("LOT-1")),
            ("product_id".to_owned(), json!("SKU-1")),
            ("batch_number".to_owned(), json!("BATCH-1")),
        ]);
        let mut lot = CanonicalRecord::new(occurred_at, "lot", payload, "plant-1");
        let commitment = lot.commitment().unwrap();
        lot.commitment = Some(commitment);
        signed(&mut lot, "plant-1");
        lot
    }

    fn shipment_record(lot_ref: &str, occurred_at: u64) -> CanonicalRecord {
        let payload = BTreeMap::from([
            ("lot_ref".to_owned(), json!(lot_ref)),
            ("from_org".to_owned(), json!("plant-1")),
            ("to_org".to_owned(), json!("receiver-1")),
        ]);
        let mut shipment = CanonicalRecord::new(occurred_at, "shipment", payload, "shipper-1");
        signed(&mut shipment, "shipper-1");
        shipment
    }

    fn runner() -> FlowRunner<ReceiptFlowState> {
        shipment_receipt_flow("receiver-1", "issuer-1", "2026-09-16")
    }

    /// `kind()` is the stable flow name used by triage and logging.
    #[test]
    fn runner_kind_matches_flow_definition() {
        assert_eq!(runner().kind(), "shipment_receipt");
        assert_eq!(
            FlowRunner::<ReceiptFlowState>::new("other", vec![]).kind(),
            "other"
        );
    }

    #[test]
    fn action_kind_names_both_variants() {
        use crate::action::Action;
        let tx = glasschain_core::Transaction::new(
            glasschain_core::TransactionKind::InventoryUpdate(glasschain_core::InventoryUpdate {
                product_id: "p".to_owned(),
                owner_id: "o".to_owned(),
                quantity_delta: 1,
                reason: "r".to_owned(),
            }),
        );
        assert_eq!(Action::EmitTransaction(tx).kind(), "EmitTransaction");
        assert_eq!(Action::EmitRecord(lot_record(1)).kind(), "EmitRecord");
    }

    #[test]
    fn receipt_flow_step_names_every_state() {
        assert_eq!(ReceiptFlowState::AwaitingLot.step(), "awaiting_lot");
        assert_eq!(
            ReceiptFlowState::LotAnchored {
                lot_ref: "lot-1".to_owned(),
                lot_commitment: "c".to_owned(),
            }
            .step(),
            "lot_anchored"
        );
        assert_eq!(
            ReceiptFlowState::Completed {
                receipt_ref: "receipt:lot-1".to_owned(),
            }
            .step(),
            "completed"
        );
    }

    #[test]
    fn transition_names_are_stable() {
        assert_eq!(AnchorLotTransition.name(), "AnchorLot");
        assert_eq!(
            ShipmentToReceiptTransition {
                receiver_id: "r".to_owned(),
                issuer: "i".to_owned(),
                received_on: "d".to_owned(),
            }
            .name(),
            "ShipmentToReceipt"
        );
    }

    /// Defensive arms: `apply` called with an event that `matches` would have
    /// rejected returns the state unchanged with no actions.
    #[test]
    fn receipt_transitions_defensive_apply_arms_are_noops() {
        let anchored = ReceiptFlowState::LotAnchored {
            lot_ref: "lot-1".to_owned(),
            lot_commitment: "c".to_owned(),
        };
        let resumed = Event::Resumed("test".to_owned());

        let anchor = AnchorLotTransition.apply(&ReceiptFlowState::AwaitingLot, &resumed);
        assert_eq!(anchor.state, ReceiptFlowState::AwaitingLot);
        assert!(anchor.actions.is_empty());
        assert!(!anchor.completed);

        let no_commitment = CanonicalRecord::new(1, "lot", BTreeMap::new(), "plant-1");
        let anchor = AnchorLotTransition.apply(
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(no_commitment),
        );
        assert_eq!(anchor.state, ReceiptFlowState::AwaitingLot);

        let ship = ShipmentToReceiptTransition {
            receiver_id: "r".to_owned(),
            issuer: "i".to_owned(),
            received_on: "d".to_owned(),
        };
        let out = ship.apply(&anchored, &resumed);
        assert_eq!(out.state, anchored);
    }

    /// Resume on a flow with no checkpoint is a no-op (`Ok(None)`), and `ack`
    /// on an unknown flow or a waiting checkpoint is a no-op too.
    #[test]
    fn resume_and_ack_without_pending_work_are_noops() {
        let store = storage();
        let triage = FlowTriage::new();
        let flow = runner();

        let fresh = flow
            .handle(
                &store,
                &triage,
                "f1",
                &ReceiptFlowState::AwaitingLot,
                &Event::Resumed("restart".into()),
            )
            .unwrap();
        assert!(fresh.is_none());

        flow.ack(&store, &triage, "unknown", 1).unwrap();

        // Waiting checkpoint (no pending event): ack does nothing.
        flow.handle(
            &store,
            &triage,
            "f2",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(lot_record(1)),
        )
        .unwrap()
        .unwrap();
        flow.ack(&store, &triage, "f2", 1).unwrap();
    }

    /// An interruption where every action was executed and acknowledged:
    /// resume finalizes without re-delivering — completed or not — and a
    /// follow-up real event still drives the flow.
    #[test]
    fn interrupted_transition_fully_acked_resumes_without_redelivery() {
        let store = storage();
        let triage = FlowTriage::new();
        let flow = runner();
        let lot = Event::RecordCommitted(lot_record(1));
        let shipment = Event::RecordCommitted(shipment_record("lot-1", 2));

        // Interrupted transition that completes on resume: a pending shipment
        // with every action acknowledged (next_action == actions.len()).
        let store_c = CheckpointStore::new(Arc::clone(&store));
        store_c
            .save(&Checkpoint {
                flow_id: "f".to_owned(),
                flow_kind: "shipment_receipt".to_owned(),
                state: serde_json::to_value(anchored()).unwrap(),
                pending_event: Some(shipment),
                next_action: 1,
                step: "lot_anchored".to_owned(),
                updated_at: 1,
            })
            .unwrap();
        let out = flow
            .handle(
                &store,
                &triage,
                "f",
                &ReceiptFlowState::AwaitingLot,
                &Event::Resumed("r".into()),
            )
            .unwrap()
            .unwrap();
        assert!(out.completed);
        assert!(out.actions.is_empty());
        // The receipt id derives from the shipment record's (random) id.
        assert!(
            matches!(out.state, ReceiptFlowState::Completed { receipt_ref } if receipt_ref.starts_with("receipt:"))
        );

        // Interrupted, fully acked, but NOT terminal: the resume finalizes
        // the pending transition and returns the state; a `Resumed` event
        // stops there, a real event continues driving the flow.
        let store_c = CheckpointStore::new(Arc::clone(&store));
        store_c
            .save(&Checkpoint {
                flow_id: "g".to_owned(),
                flow_kind: "shipment_receipt".to_owned(),
                state: serde_json::to_value(ReceiptFlowState::AwaitingLot).unwrap(),
                pending_event: Some(lot),
                next_action: 0, // AnchorLot emits no actions: 0 >= 0
                step: "awaiting_lot".to_owned(),
                updated_at: 1,
            })
            .unwrap();
        let resumed_out = flow
            .handle(
                &store,
                &triage,
                "g",
                &ReceiptFlowState::AwaitingLot,
                &Event::Resumed("r".into()),
            )
            .unwrap()
            .unwrap();
        assert!(!resumed_out.completed);
        assert!(resumed_out.actions.is_empty());
        // The resume re-applies the pending event deterministically, so the
        // state is re-derived from the lot record (fresh UUID per construct).
        assert!(matches!(
            resumed_out.state,
            ReceiptFlowState::LotAnchored { .. }
        ));

        // A real event on top of the finalized-but-uncompleted flow proceeds.
        let next = flow
            .handle(
                &store,
                &triage,
                "g",
                &ReceiptFlowState::AwaitingLot,
                &Event::Woken("x".into()),
            )
            .unwrap();
        assert!(next.is_none()); // Woken matches nothing in the receipt flow
    }

    #[test]
    fn interrupted_transition_redelivers_unacked_actions() {
        let store = storage();
        let triage = FlowTriage::new();
        let flow = runner();
        let store_c = CheckpointStore::new(Arc::clone(&store));

        // Interrupted with zero actions acknowledged: the resume re-delivers
        // the full remaining action list (at-least-once).
        store_c
            .save(&Checkpoint {
                flow_id: "f".to_owned(),
                flow_kind: "shipment_receipt".to_owned(),
                state: serde_json::to_value(anchored()).unwrap(),
                pending_event: Some(Event::RecordCommitted(shipment_record("lot-1", 2))),
                next_action: 0,
                step: "lot_anchored".to_owned(),
                updated_at: 1,
            })
            .unwrap();
        let out = flow
            .handle(
                &store,
                &triage,
                "f",
                &ReceiptFlowState::AwaitingLot,
                &Event::Resumed("r".into()),
            )
            .unwrap()
            .unwrap();
        assert!(!out.completed);
        assert_eq!(out.actions.len(), 1);
        assert!(matches!(out.actions[0], Action::EmitRecord(_)));

        // Partial acknowledgement skips the executed prefix.
        let store_c = CheckpointStore::new(Arc::clone(&store));
        store_c
            .save(&Checkpoint {
                flow_id: "f".to_owned(),
                flow_kind: "shipment_receipt".to_owned(),
                state: serde_json::to_value(anchored()).unwrap(),
                pending_event: Some(Event::RecordCommitted(shipment_record("lot-1", 2))),
                next_action: 1,
                step: "lot_anchored".to_owned(),
                updated_at: 1,
            })
            .unwrap();
        let out = flow
            .handle(
                &store,
                &triage,
                "f",
                &ReceiptFlowState::AwaitingLot,
                &Event::Resumed("r".into()),
            )
            .unwrap()
            .unwrap();
        assert!(out.completed);
        assert!(out.actions.is_empty());
    }

    /// A pending event the current transition table no longer accepts fails
    /// the checkpoint closed — in `handle` and in `ack`.
    #[test]
    fn stale_pending_event_fails_closed_everywhere() {
        let store = storage();
        let triage = FlowTriage::new();
        let flow = runner();
        let store_c = CheckpointStore::new(Arc::clone(&store));

        // Completed state cannot match a pending lot event (AnchorLot only
        // fires on AwaitingLot).
        let mismatch = Checkpoint {
            flow_id: "f".to_owned(),
            flow_kind: "shipment_receipt".to_owned(),
            state: serde_json::to_value(completed_receipt()).unwrap(),
            pending_event: Some(Event::RecordCommitted(lot_record(1))),
            next_action: 0,
            step: "completed".to_owned(),
            updated_at: 1,
        };
        store_c.save(&mismatch).unwrap();
        assert!(matches!(
            flow.handle(
                &store,
                &triage,
                "f",
                &ReceiptFlowState::AwaitingLot,
                &Event::Resumed("r".into())
            ),
            Err(WorkflowError::CheckpointMismatch { .. })
        ));
        assert!(matches!(
            flow.ack(&store, &triage, "f", 1),
            Err(WorkflowError::CheckpointMismatch { .. })
        ));

        // AwaitingLot state cannot match a pending shipment event either.
        store_c
            .save(&Checkpoint {
                flow_id: "g".to_owned(),
                flow_kind: "shipment_receipt".to_owned(),
                state: serde_json::to_value(ReceiptFlowState::AwaitingLot).unwrap(),
                pending_event: Some(Event::RecordCommitted(shipment_record("lot-1", 2))),
                next_action: 0,
                step: "awaiting_lot".to_owned(),
                updated_at: 1,
            })
            .unwrap();
        assert!(matches!(
            flow.ack(&store, &triage, "g", 1),
            Err(WorkflowError::CheckpointMismatch { .. })
        ));
    }

    /// A stored state the flow definition no longer fits fails the checkpoint
    /// closed with a deserialization error — in `handle`, `ack` and
    /// `current_state`.
    #[test]
    fn corrupt_checkpoint_state_fails_closed_everywhere() {
        let store = storage();
        let triage = FlowTriage::new();
        let flow = runner();
        let store_c = CheckpointStore::new(Arc::clone(&store));

        let corrupt = Checkpoint {
            flow_id: "f".to_owned(),
            flow_kind: "shipment_receipt".to_owned(),
            state: json!("not a receipt state"),
            // `ack` checks for pending work before deserializing the state,
            // so a pending event is required to reach the state parse.
            pending_event: Some(Event::Resumed("r".to_owned())),
            next_action: 0,
            step: String::new(),
            updated_at: 1,
        };
        store_c.save(&corrupt).unwrap();
        assert!(matches!(
            flow.handle(
                &store,
                &triage,
                "f",
                &ReceiptFlowState::AwaitingLot,
                &Event::Resumed("r".into())
            ),
            Err(WorkflowError::CheckpointDeserialization(_))
        ));
        assert!(matches!(
            flow.ack(&store, &triage, "f", 0),
            Err(WorkflowError::CheckpointDeserialization(_))
        ));
        assert!(matches!(
            flow.current_state(&store, "f"),
            Err(WorkflowError::CheckpointDeserialization(_))
        ));
        assert!(flow.current_state(&store, "unknown").unwrap().is_none());
    }

    /// `ack` with a partially-executed pending transition advances
    /// `next_action` without finalizing.
    #[test]
    fn ack_advances_partial_progress() {
        let store = storage();
        let triage = FlowTriage::new();
        let flow = runner();

        // Produce a pending shipment transition (1 action, unacked).
        let out = flow
            .handle(
                &store,
                &triage,
                "f",
                &anchored(),
                &Event::RecordCommitted(shipment_record("lot-1", 2)),
            )
            .unwrap()
            .unwrap();
        assert_eq!(out.actions.len(), 1);

        // Executed 0 of 1 (clamped below the pending length): the checkpoint
        // stays with the same pending event.
        flow.ack(&store, &triage, "f", 0).unwrap();
        let state = flow.current_state(&store, "f").unwrap().unwrap();
        assert_eq!(state, anchored());

        // Executing everything finalizes the completed flow (checkpoint gone).
        flow.ack(&store, &triage, "f", 5).unwrap();
        let state = flow.current_state(&store, "f").unwrap();
        assert!(state.is_none());
    }
}
