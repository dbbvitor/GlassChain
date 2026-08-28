//! Flow checkpoints: durable points that let an interrupted flow resume.
//!
//! Checkpoints persist through the existing [`StorageProvider`] state seam
//! (key prefix `workflow:checkpoint:`) — no new storage backend.

use crate::error::WorkflowError;
use crate::event::Event;
use glasschain_core::StorageProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Storage-key prefix for flow checkpoints.
pub const CHECKPOINT_PREFIX: &str = "workflow:checkpoint:";

/// A durable point in a flow's execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Checkpoint {
    /// The flow instance id — the checkpoint key.
    pub flow_id: String,

    /// The flow definition (transition table) this checkpoint belongs to.
    pub flow_kind: String,

    /// The flow's serialized state.
    ///
    /// While [`Self::pending_event`] is `Some`, this is the state **before**
    /// the pending transition, so a resume can re-apply the same event and
    /// re-derive the same actions deterministically.
    pub state: serde_json::Value,

    /// The event whose transition produced pending actions; `None` when the
    /// flow is waiting for its next event.
    pub pending_event: Option<Event>,

    /// How many of the pending actions were already executed.  A resume skips
    /// the first `next_action` entries of the re-derived action list.
    pub next_action: usize,

    /// Unix timestamp (seconds) of the last durable point — feeds the triage
    /// staleness view.
    pub updated_at: u64,
}

/// Checkpoint persistence over the [`StorageProvider`] seam.
pub struct CheckpointStore {
    storage: Arc<dyn StorageProvider>,
}

impl CheckpointStore {
    /// Build a store writing under the default `workflow:checkpoint:` prefix.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>) -> Self {
        Self { storage }
    }

    /// The storage key for a flow's checkpoint.
    #[must_use]
    pub fn key(flow_id: &str) -> String {
        format!("{CHECKPOINT_PREFIX}{flow_id}")
    }

    /// Persist (or overwrite) a flow's checkpoint.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::Storage`] if the backend fails, or
    /// [`WorkflowError::Serialization`] if the checkpoint cannot be encoded.
    pub fn save(&self, checkpoint: &Checkpoint) -> Result<(), WorkflowError> {
        let bytes = serde_json::to_vec(checkpoint)?;
        self.storage
            .put_state(&Self::key(&checkpoint.flow_id), &bytes)
            .map_err(|e| WorkflowError::Storage(e.to_string()))
    }

    /// Load a flow's checkpoint, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::Storage`] if the backend fails, or
    /// [`WorkflowError::Serialization`] if the stored bytes are not a
    /// checkpoint.
    pub fn load(&self, flow_id: &str) -> Result<Option<Checkpoint>, WorkflowError> {
        let Some(bytes) = self
            .storage
            .get_state(&Self::key(flow_id))
            .map_err(|e| WorkflowError::Storage(e.to_string()))?
        else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    /// Remove a flow's checkpoint (after completion).
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::Storage`] if the backend fails.
    pub fn delete(&self, flow_id: &str) -> Result<(), WorkflowError> {
        self.storage
            .delete_state(&Self::key(flow_id))
            .map_err(|e| WorkflowError::Storage(e.to_string()))
    }
}
