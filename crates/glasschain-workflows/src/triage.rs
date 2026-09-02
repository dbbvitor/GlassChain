//! The flow triage view: surfaces stuck flows.
//!
//! The runner records every durable point here; an operator polls
//! [`FlowTriage::stuck_flows`] to find flows that have not advanced past a
//! staleness threshold (e.g. a counterparty that stopped responding).

use std::collections::HashMap;
use std::sync::Mutex;

/// One known flow, as seen by the triage view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageEntry {
    /// Flow instance id.
    pub flow_id: String,
    /// Flow definition (transition table) the instance belongs to.
    pub flow_kind: String,
    /// The flow's current step.
    pub step: String,
    /// Unix timestamp (seconds) of the last durable point.
    pub updated_at: u64,
}

/// In-process registry of flow progress, updated by the runner on every
/// checkpoint write and cleared on completion.
///
/// # ponytail: in-memory registry, lost on restart — flows are re-discovered
/// lazily when driven again. Add a checkpoint scan (storage `list` capability)
/// when triage must survive restarts (#43/#44 need it first).
#[derive(Debug, Default)]
pub struct FlowTriage {
    entries: Mutex<HashMap<String, TriageEntry>>,
}

impl FlowTriage {
    /// Create an empty triage registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or refresh) a flow's latest durable point.
    pub fn record(&self, flow_id: &str, flow_kind: &str, step: &str, updated_at: u64) {
        let mut entries = self.lock();
        entries.insert(
            flow_id.to_owned(),
            TriageEntry {
                flow_id: flow_id.to_owned(),
                flow_kind: flow_kind.to_owned(),
                step: step.to_owned(),
                updated_at,
            },
        );
    }

    /// Forget a completed flow.
    pub fn clear(&self, flow_id: &str) {
        self.lock().remove(flow_id);
    }

    /// The triage entry for one flow, if known.
    #[must_use]
    pub fn entry(&self, flow_id: &str) -> Option<TriageEntry> {
        self.lock().get(flow_id).cloned()
    }

    /// All known flows, ordered by flow id (deterministic for operators and
    /// tests).
    #[must_use]
    pub fn known_flows(&self) -> Vec<TriageEntry> {
        let mut flows: Vec<TriageEntry> = self.lock().values().cloned().collect();
        flows.sort_by(|a, b| a.flow_id.cmp(&b.flow_id));
        flows
    }

    /// Flows whose last durable point is older than `stale_after_secs`,
    /// measured from `now` (both Unix seconds). Ordered by flow id.
    #[must_use]
    pub fn stuck_flows(&self, now: u64, stale_after_secs: u64) -> Vec<TriageEntry> {
        self.known_flows()
            .into_iter()
            .filter(|entry| now.saturating_sub(entry.updated_at) > stale_after_secs)
            .collect()
    }

    /// Lock the registry, recovering from a poisoned mutex (a panic in another
    /// thread must not take triage down with it).  `unwrap_or_else` is the
    /// poison-recovery form — it cannot itself panic, so the no-`unwrap` rule
    /// is honored.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, TriageEntry>> {
        self.entries.lock().unwrap_or_else(|poisoned| {
            log::warn!("flow triage: recovering from a poisoned lock");
            poisoned.into_inner()
        })
    }
}
