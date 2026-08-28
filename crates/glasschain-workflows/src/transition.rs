//! The `TransitionResult` half of the Corda Action/Event/TransitionResult
//! algebra, and the [`Transition`] contract: one type per transition.

use crate::action::Action;
use crate::event::Event;
use crate::state::FlowState;

/// The outcome of applying a [`Transition`] to `(state, event)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionResult<S> {
    /// The flow's state after the transition.
    pub state: S,

    /// The actions the runtime must perform, in order.
    pub actions: Vec<Action>,

    /// `true` when the flow reached a terminal state; the runner clears the
    /// flow's checkpoint after a completing transition.
    pub completed: bool,
}

impl<S> TransitionResult<S> {
    /// Build a transition result.
    #[must_use]
    pub const fn new(state: S, actions: Vec<Action>, completed: bool) -> Self {
        Self {
            state,
            actions,
            completed,
        }
    }
}

/// One transition in a flow's state machine — the Corda `transitions/`
/// decomposition: **one named type per transition**.
///
/// A transition is a pure, deterministic function
/// `(state, event) -> TransitionResult`: no I/O, no wall-clock reads, no
/// randomness.  Re-applying the same `(state, event)` yields the same result,
/// which is what makes checkpoint replay sound.
///
/// [`Transition::apply`] is only called when [`Transition::matches`] returned
/// `true` for the same pair; implementations should still return a graceful
/// unchanged state rather than panic if the pair no longer matches (e.g.
/// because the transition table changed after a checkpoint was written).
pub trait Transition<S: FlowState> {
    /// Stable name of this transition (diagnostics).
    fn name(&self) -> &'static str;

    /// Whether this transition handles `(state, event)`.
    fn matches(&self, state: &S, event: &Event) -> bool;

    /// Apply the transition. Deterministic: equal inputs → equal outputs.
    fn apply(&self, state: &S, event: &Event) -> TransitionResult<S>;
}
