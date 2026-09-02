//! The `Action` half of the Corda Action/Event/TransitionResult algebra.
//!
//! A [`Transition`](crate::Transition) produces [`Action`]s; the
//! [`FlowRunner`](crate::FlowRunner) executes them in order and returns them
//! to the caller as [`FlowOutcome::outputs`](crate::FlowOutcome).  Actions are
//! the only place side effects enter the framework — transitions themselves are
//! pure functions of `(state, event)`.

use glasschain_core::{CanonicalRecord, Transaction};

/// An effect a transition asks the runtime to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Emit a legacy transaction (e.g. an autonomously generated purchase
    /// order from the offer→PO automation).
    ///
    /// `Transaction::id` **must be deterministic** (use
    /// [`Transaction::with_id`]) so a replayed emission is identical and the
    /// ledger dedupes it: action execution is at-least-once across
    /// interruptions, and determinism makes the *effect* exactly-once.
    EmitTransaction(Transaction),

    /// Emit a canonical v1 record.
    ///
    /// `record_id` (and every content field) must be derived from the inputs —
    /// never from wall-clock time or randomness — for the same replay
    /// idempotency guarantee. Cryptographic signatures are attached by the
    /// endorsement layer before submission, not by the flow.
    EmitRecord(CanonicalRecord),
}

impl Action {
    /// Stable discriminant for logging and triage.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::EmitTransaction(_) => "EmitTransaction",
            Self::EmitRecord(_) => "EmitRecord",
        }
    }
}
