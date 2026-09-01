//! Certification and audit flows (ticket #43): a committed, anchored lot is
//! attested by a certifier or auditor, emitting a `quality_certification` or
//! `audit_attestation` record that references the immutable lot commitment.
//!
//! Both families share one required-field shape (`lot_ref`, `issuer`, `scope`,
//! `valid_from`, `valid_to`, `status`, `evidence_manifest`), so one
//! parameterized implementation serves both processes:
//!
//! ```text
//! AwaitingLot ──(RecordCommitted: anchored lot)──▶ LotAnchored { lot_ref, lot_commitment }
//! LotAnchored  ──(Woken: "attest")───────────────▶ Completed
//!               └── emits: <family> { lot_ref, issuer, scope, valid_from, valid_to,
//!                        status: "valid", evidence_manifest } with its anchor set
//! ```
//!
//! The source lot record is never mutated: the attestation is a new,
//! append-only record whose `lot_ref` points at the anchored lot.

use crate::action::Action;
use crate::event::Event;
use crate::runner::FlowRunner;
use crate::state::FlowState;
use crate::transition::{Transition, TransitionResult};
use glasschain_core::crypto::sha256;
use glasschain_core::CanonicalRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Configuration for one attestation (certification or audit) flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationConfig {
    /// The record family to emit: `"quality_certification"` (certification
    /// flow) or `"audit_attestation"` (audit flow). The constructor fixes this
    /// field, overriding whatever a caller sets.
    pub family: &'static str,
    /// The attesting organization (both the envelope issuer and the payload
    /// `issuer`).
    pub issuer: String,
    /// What the attestation covers (e.g. `"cold-chain"`, `"gmp"`).
    pub scope: String,
    /// ISO-8601 validity start (`YYYY-MM-DD`).
    pub valid_from: String,
    /// ISO-8601 validity end (`YYYY-MM-DD`).
    pub valid_to: String,
    /// Unix-seconds issue stamp for the emitted record — flow config, not the
    /// wall clock, so a replayed emission is byte-identical.
    pub issued_at: u64,
}

/// The certification/audit flow's protocol state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttestationFlowState {
    /// Waiting for the anchored lot record.
    AwaitingLot,
    /// The lot is anchored; the flow holds its immutable commitment.
    LotAnchored {
        /// `record_id` of the anchored lot.
        lot_ref: String,
        /// The lot record's canonical commitment (immutable anchor).
        lot_commitment: String,
    },
    /// The attestation was emitted; terminal state.
    Completed { record_ref: String },
}

impl FlowState for AttestationFlowState {
    fn step(&self) -> &'static str {
        match self {
            Self::AwaitingLot => "awaiting_lot",
            Self::LotAnchored { .. } => "lot_anchored",
            Self::Completed { .. } => "completed",
        }
    }
}

/// Transition 1: anchor the immutable lot commitment from a committed,
/// anchored lot record.
#[derive(Debug)]
pub struct AnchorLotTransition;

impl Transition<AttestationFlowState> for AnchorLotTransition {
    fn name(&self) -> &'static str {
        "AnchorLot"
    }

    fn matches(&self, state: &AttestationFlowState, event: &Event) -> bool {
        matches!(state, AttestationFlowState::AwaitingLot)
            && matches!(
                event,
                Event::RecordCommitted(record)
                    if record.schema_id == "lot" && record.commitment.is_some()
            )
    }

    fn apply(
        &self,
        state: &AttestationFlowState,
        event: &Event,
    ) -> TransitionResult<AttestationFlowState> {
        let Event::RecordCommitted(lot) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Some(commitment) = &lot.commitment else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        TransitionResult::new(
            AttestationFlowState::LotAnchored {
                lot_ref: lot.record_id.clone(),
                lot_commitment: commitment.clone(),
            },
            Vec::new(),
            false,
        )
    }
}

/// Transition 2: emit the attestation record for the anchored lot. The wake
/// reason must be `"attest"` — the attestation is an operator decision, not an
/// automatic consequence of anchoring.
#[derive(Debug, Clone)]
pub struct EmitAttestationTransition {
    pub config: AttestationConfig,
}

impl Transition<AttestationFlowState> for EmitAttestationTransition {
    fn name(&self) -> &'static str {
        "EmitAttestation"
    }

    fn matches(&self, state: &AttestationFlowState, event: &Event) -> bool {
        matches!(state, AttestationFlowState::LotAnchored { .. })
            && matches!(event, Event::Woken(reason) if reason == "attest")
    }

    fn apply(
        &self,
        state: &AttestationFlowState,
        event: &Event,
    ) -> TransitionResult<AttestationFlowState> {
        let AttestationFlowState::LotAnchored {
            lot_ref,
            lot_commitment,
        } = state
        else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let Event::Woken(_) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        let attestation = build_attestation(lot_ref, lot_commitment, &self.config);
        TransitionResult::new(
            AttestationFlowState::Completed {
                record_ref: attestation.record_id.clone(),
            },
            vec![Action::EmitRecord(attestation)],
            true,
        )
    }
}

/// The certification flow: emits `quality_certification` records.
#[must_use]
pub fn certification_flow(config: AttestationConfig) -> FlowRunner<AttestationFlowState> {
    attestation_flow("quality_certification", config)
}

/// The audit flow: emits `audit_attestation` records.
#[must_use]
pub fn audit_flow(config: AttestationConfig) -> FlowRunner<AttestationFlowState> {
    attestation_flow("audit_attestation", config)
}

/// Build an attestation flow over `family` with `config.family` fixed.
fn attestation_flow(
    family: &'static str,
    mut config: AttestationConfig,
) -> FlowRunner<AttestationFlowState> {
    config.family = family;
    FlowRunner::new(
        "attestation",
        vec![
            Box::new(AnchorLotTransition),
            Box::new(EmitAttestationTransition { config }),
        ],
    )
}

/// Build the emitted attestation record.
///
/// `record_id` and every content field derive from the inputs; the anchor
/// commitment is computed over the canonical form (anchored family, ADR-006).
fn build_attestation(
    lot_ref: &str,
    lot_commitment: &str,
    config: &AttestationConfig,
) -> CanonicalRecord {
    // The embedded EvidenceManifest (ADR-005) carries a 64-hex commitment,
    // derived deterministically from the immutable lot anchor and the
    // attestation's own scope and validity.
    let manifest_commitment = sha256(
        format!(
            "manifest|{lot_commitment}|{}|{}|{}",
            config.scope, config.valid_from, config.valid_to
        )
        .as_bytes(),
    );
    let mut evidence_manifest = serde_json::Map::new();
    evidence_manifest.insert(
        "manifest_commitment".to_owned(),
        Value::String(manifest_commitment),
    );
    let mut payload = BTreeMap::new();
    payload.insert("lot_ref".to_owned(), Value::String(lot_ref.to_owned()));
    payload.insert("issuer".to_owned(), Value::String(config.issuer.clone()));
    payload.insert("scope".to_owned(), Value::String(config.scope.clone()));
    payload.insert(
        "valid_from".to_owned(),
        Value::String(config.valid_from.clone()),
    );
    payload.insert(
        "valid_to".to_owned(),
        Value::String(config.valid_to.clone()),
    );
    payload.insert("status".to_owned(), Value::String("valid".to_owned()));
    payload.insert(
        "evidence_manifest".to_owned(),
        Value::Object(evidence_manifest),
    );
    let mut record = CanonicalRecord::new(
        config.issued_at,
        config.family,
        payload,
        config.issuer.clone(),
    );
    record.record_id = format!("{}:{lot_ref}", config.family);
    // Anchored family: the record must carry its own canonical commitment.
    if let Ok(commitment) = record.commitment() {
        record.commitment = Some(commitment);
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::FlowTriage;
    use glasschain_core::providers::in_memory::InMemoryStorageProvider;
    use std::sync::Arc;

    fn config(family: &'static str) -> AttestationConfig {
        AttestationConfig {
            family,
            issuer: "org-certifier".to_owned(),
            scope: "cold-chain".to_owned(),
            valid_from: "2026-09-01".to_owned(),
            valid_to: "2027-09-01".to_owned(),
            issued_at: 1_700_000_100,
        }
    }

    fn storage() -> Arc<dyn glasschain_core::StorageProvider> {
        Arc::new(InMemoryStorageProvider::new())
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

    #[test]
    fn test_certification_flow_emits_anchored_record() {
        let storage = storage();
        let triage = FlowTriage::new();
        let flow = certification_flow(config("quality_certification"));
        let initial = AttestationFlowState::AwaitingLot;
        let (lot_event, lot_commitment) = anchored_lot();

        let outcome = flow
            .handle(&storage, &triage, "cert:lot-1", &initial, &lot_event)
            .expect("anchor")
            .expect("applies");
        assert_eq!(
            outcome.state,
            AttestationFlowState::LotAnchored {
                lot_ref: "lot-1".into(),
                lot_commitment,
            }
        );

        let outcome = flow
            .handle(
                &storage,
                &triage,
                "cert:lot-1",
                &initial,
                &Event::Woken("attest".into()),
            )
            .expect("attest")
            .expect("applies");
        // Action-carrying transitions complete once their actions are acked.
        assert!(!outcome.completed);
        let [Action::EmitRecord(record)] = &outcome.actions[..] else {
            panic!("expected one EmitRecord, got {:?}", outcome.actions);
        };
        assert_eq!(record.schema_id, "quality_certification");
        assert_eq!(record.record_id, "quality_certification:lot-1");
        for field in [
            "lot_ref",
            "issuer",
            "scope",
            "valid_from",
            "valid_to",
            "status",
            "evidence_manifest",
        ] {
            assert!(
                record.payload.contains_key(field),
                "missing required field {field}"
            );
        }
        assert_eq!(
            record.payload.get("status").and_then(Value::as_str),
            Some("valid")
        );
        assert_eq!(
            record.payload.get("lot_ref").and_then(Value::as_str),
            Some("lot-1")
        );
        // Anchored family: the record carries its own canonical commitment.
        let expected = record.commitment().expect("commitment");
        assert_eq!(record.commitment, Some(expected));

        // Ack completes the flow and clears its checkpoint.
        flow.ack(&storage, &triage, "cert:lot-1", 1)
            .expect("ack completes");
        assert!(
            flow.current_state(&storage, "cert:lot-1")
                .expect("state")
                .is_none(),
            "a completed flow's checkpoint is cleared"
        );
        assert!(triage.entry("cert:lot-1").is_none());
    }

    #[test]
    fn test_audit_flow_emits_distinct_family() {
        let storage = storage();
        let triage = FlowTriage::new();
        let flow = audit_flow(config("audit_attestation"));
        let initial = AttestationFlowState::AwaitingLot;
        let (lot_event, _commitment) = anchored_lot();
        flow.handle(&storage, &triage, "audit:lot-1", &initial, &lot_event)
            .expect("anchor")
            .expect("applies");
        let outcome = flow
            .handle(
                &storage,
                &triage,
                "audit:lot-1",
                &initial,
                &Event::Woken("attest".into()),
            )
            .expect("attest")
            .expect("applies");
        let [Action::EmitRecord(record)] = &outcome.actions[..] else {
            panic!("expected one EmitRecord, got {:?}", outcome.actions);
        };
        assert_eq!(record.schema_id, "audit_attestation");
        assert_eq!(record.record_id, "audit_attestation:lot-1");
    }

    #[test]
    fn test_unanchored_lot_does_not_start_the_flow() {
        let storage = storage();
        let triage = FlowTriage::new();
        let flow = certification_flow(config("quality_certification"));
        let initial = AttestationFlowState::AwaitingLot;
        let (lot_event, _commitment) = anchored_lot();
        let Event::RecordCommitted(mut lot) = lot_event else {
            panic!("expected record");
        };
        lot.commitment = None; // unanchored: must be ignored
        let outcome = flow
            .handle(
                &storage,
                &triage,
                "cert:lot-1",
                &initial,
                &Event::RecordCommitted(lot),
            )
            .expect("handle");
        assert!(outcome.is_none(), "unanchored lot must not anchor the flow");
    }
}
