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
pub struct FlowRunner<S> {
    flow_kind: &'static str,
    transitions: Vec<Box<dyn Transition<S>>>,
}

impl<S: FlowState> FlowRunner<S> {
    /// Build a flow definition.
    #[must_use]
    pub fn new(flow_kind: &'static str, transitions: Vec<Box<dyn Transition<S>>>) -> Self {
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
