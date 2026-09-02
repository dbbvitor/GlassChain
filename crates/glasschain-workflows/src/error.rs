//! Workflow errors.

use thiserror::Error;

/// Errors produced by the workflow framework.
#[derive(Debug, Error)]
pub enum WorkflowError {
    /// The storage backend failed an operation.
    #[error("storage error: {0}")]
    Storage(String),

    /// A checkpoint or flow state failed JSON (de)serialization.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A checkpoint's stored state does not deserialize into the flow's state
    /// type — the flow definition changed since the checkpoint was written.
    #[error("checkpoint state does not match the flow definition: {0}")]
    CheckpointDeserialization(String),

    /// A checkpoint holds a pending event that no transition handles — the
    /// flow's transition table changed since the checkpoint was written.
    #[error(
        "checkpoint pending event no longer matches any transition for flow {flow_id}: {event}"
    )]
    CheckpointMismatch { flow_id: String, event: String },

    /// The system clock is set before the Unix epoch.
    #[error("system clock is before the Unix epoch")]
    Clock,
}
