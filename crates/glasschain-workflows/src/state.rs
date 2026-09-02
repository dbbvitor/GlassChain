//! Flow state: the data a flow carries between events, through checkpoints.

use serde::de::DeserializeOwned;
use serde::Serialize;

/// A flow's protocol state.
///
/// States must be [`Clone`] and JSON-serializable: they travel through
/// checkpoints, so every field must be replay-stable (no wall-clock time, no
/// randomness, no handles to external resources).
pub trait FlowState: Clone + Serialize + DeserializeOwned {
    /// Stable, human-readable name of the current step — surfaced by the
    /// triage view for stuck flows.
    fn step(&self) -> &'static str;
}
