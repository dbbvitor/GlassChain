//! # `GlassChain` workflows
//!
//! A Corda-style state-machine workflow framework (ticket #40), in the
//! `node/services/statemachine/` decomposition: an explicit
//! **Action / Event / `TransitionResult`** algebra, one type per transition,
//! checkpoint persistence over the existing [`StorageProvider`] seam, and a
//! triage view for stuck flows.
//!
//! ## The algebra
//!
//! - [`Event`] — an input (a committed canonical record, a committed legacy
//!   transaction, or a resume wake-up).
//! - [`Transition`] — one named type per transition: a pure, deterministic
//!   function `(state, event) -> TransitionResult`. No I/O, no wall clock, no
//!   randomness.
//! - [`Action`] — an effect a transition requests (emit a transaction, emit a
//!   canonical record). The runner executes actions; the caller performs the
//!   actual submission.
//! - [`TransitionResult`] — the next state, the actions, and completion.
//!
//! [`FlowRunner`] drives `(state, event)` pairs through an ordered transition
//! table, persisting a [`Checkpoint`] via [`CheckpointStore`] (storage key
//! `workflow:checkpoint:<flow_id>`).
//!
//! ## Durability contract
//!
//! The runner never performs an action's I/O itself: [`FlowRunner::handle`]
//! returns the actions to execute, the caller executes them durably, and
//! [`FlowRunner::ack`] — the only place the checkpoint advances — records the
//! progress.
//!
//! - **No loss**: an interrupted flow resumes from its checkpoint and
//!   re-delivers the not-yet-acknowledged actions deterministically.
//! - **At-least-once execution, exactly-once effects**: the action in flight
//!   when a crash hit may run twice; because emissions carry deterministic ids
//!   (see [`Action`]), the ledger dedupes them.
//! - [`FlowTriage::stuck_flows`] surfaces flows whose last durable point is
//!   older than a staleness threshold; flows re-surface with their stored
//!   timestamp when driven after a triage restart.
//!
//! ## Canonical records
//!
//! Flows consume committed [`CanonicalRecord`]s as [`Event::RecordCommitted`]
//! and emit new ones via [`Action::EmitRecord`], referencing immutable lot
//! commitments without mutating source records — see [`shipment_receipt_flow`].
//! Signature attachment is the endorsement layer's job (#45), performed by the
//! runtime before submission.
//!
//! [`CanonicalRecord`]: glasschain_core::CanonicalRecord

mod action;
mod checkpoint;
mod error;
mod event;
mod receipt_flow;
mod runner;
mod state;
mod transition;
mod triage;

pub use action::Action;
pub use checkpoint::{Checkpoint, CheckpointStore, CHECKPOINT_PREFIX};
pub use error::WorkflowError;
pub use event::Event;
pub use receipt_flow::{
    shipment_receipt_flow, AnchorLotTransition, ReceiptFlowState, ShipmentToReceiptTransition,
};
pub use runner::{FlowOutcome, FlowRunner};
pub use state::FlowState;
pub use transition::{Transition, TransitionResult};
pub use triage::{FlowTriage, TriageEntry};
