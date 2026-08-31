//! Capability registry and height-based activation (ADR-010).
//!
//! Capabilities gate every consensus-visible or validation-affecting behavior.
//! The active capability set is network-wide and part of committed chain
//! history: an activation is a signed, append-only control-plane record naming
//! a capability's immutable `(id, version, hash)` identity and a **future**
//! activation height. Validation selects the capability set effective at each
//! block height, so old blocks keep their historical meaning and replay
//! derives the same history from committed blocks (ADR-010 decision 5).

use crate::block::Block;
use crate::canonical::{validate_record, RecordSignature};
use crate::crypto::sha256;
use crate::error::CoreError;
use crate::transaction::TransactionKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One named capability in the network-wide registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    /// Capability identifier (e.g. `"canonical_schema_v1"`).
    pub id: &'static str,
    /// Immutable capability version.
    pub version: u32,
}

/// Capability id gating `state_commitment` records. Kept in one place so the
/// gate in [`validate_record_under`] and the registry entry cannot drift.
pub const STATE_COMMITMENT_CAPABILITY_ID: &str = "state_commitment";

/// Capability id gating endorsement enforcement at the commit path (ADR-008
/// handoff 4). Kept in one place so the node-level gate and the registry
/// entry cannot drift.
pub const ENDORSEMENT_CAPABILITY_ID: &str = "endorsement";

/// The v1 capability registry (ADR-010 decision 2).
pub const CAPABILITY_V1: &[CapabilityDescriptor] = &[
    CapabilityDescriptor {
        id: "canonical_schema_v1",
        version: 1,
    },
    CapabilityDescriptor {
        id: STATE_COMMITMENT_CAPABILITY_ID,
        version: 1,
    },
    CapabilityDescriptor {
        id: "pdc",
        version: 1,
    },
    CapabilityDescriptor {
        id: "endorsement",
        version: 1,
    },
    CapabilityDescriptor {
        id: "bft_consensus",
        version: 1,
    },
];

/// Capabilities active from genesis: the behaviors the v1 ledger already
/// validates. `pdc`, `endorsement`, and `bft_consensus` activate later.
pub const GENESIS_CAPABILITIES: &[&str] = &["canonical_schema_v1", STATE_COMMITMENT_CAPABILITY_ID];

/// Deterministic immutable hash of a capability version. This is the third
/// component of a capability's registry identity (ADR-010 decision 4).
#[must_use]
pub fn capability_hash(id: &str, version: u32) -> String {
    sha256(format!("{id}|v{version}").as_bytes())
}

/// Look up a registered capability by id.
#[must_use]
pub fn lookup_capability(id: &str) -> Option<&'static CapabilityDescriptor> {
    CAPABILITY_V1.iter().find(|c| c.id == id)
}

/// What a peer advertises in its `Hello` handshake (ADR-010 decision 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityAdvertisement {
    /// Capability identifier.
    pub id: String,
    /// Capability version the peer supports.
    pub version: u32,
}

/// A signed, append-only control-plane record activating a capability at a
/// future height (ADR-010 decision 4).
///
/// The record is validated under the capability set active **before** the
/// transition; the new set starts exactly at the declared height and never
/// changes rules midway through its own block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityActivation {
    /// Capability identifier.
    pub capability_id: String,
    /// Immutable capability version being activated.
    pub version: u32,
    /// Must equal `capability_hash(capability_id, version)`.
    pub hash: String,
    /// Height at which the new set takes effect; strictly greater than the
    /// height of the block carrying this activation.
    pub activation_height: u64,
    /// The required governance signature set.
    ///
    /// ponytail: v1 requires presence only, consistent with canonical records;
    /// cryptographic verification lands with the endorsement engine (#37).
    pub signatures: Vec<RecordSignature>,
}

/// The active capability set at one block height.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    /// Capability id → active (version, hash).
    active: BTreeMap<String, (u32, String)>,
}

impl CapabilitySet {
    /// The capability set active from genesis.
    #[must_use]
    pub fn genesis() -> Self {
        let mut active = BTreeMap::new();
        for id in GENESIS_CAPABILITIES {
            if let Some(descriptor) = lookup_capability(id) {
                active.insert(
                    (*id).to_owned(),
                    (descriptor.version, capability_hash(id, descriptor.version)),
                );
            }
        }
        Self { active }
    }

    /// `true` when `id` is in the active set.
    #[must_use]
    pub fn is_active(&self, id: &str) -> bool {
        self.active.contains_key(id)
    }

    /// The active `(version, hash)` for `id`, when active.
    #[must_use]
    pub fn active_version(&self, id: &str) -> Option<(u32, &str)> {
        self.active
            .get(id)
            .map(|(version, hash)| (*version, hash.as_str()))
    }

    /// Number of active capabilities (useful for tests and logging).
    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// `true` when the set has no active capabilities.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    fn insert_activation(&mut self, activation: &CapabilityActivation) {
        self.active.insert(
            activation.capability_id.clone(),
            (activation.version, activation.hash.clone()),
        );
    }
}

/// The activation history derived from committed blocks, with a deterministic
/// lookup by height for admission, validation, and replay (ADR-010 decision 5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityHistory {
    /// Activations in application order; the declared height lives on the
    /// record itself.
    activations: Vec<CapabilityActivation>,
}

impl CapabilityHistory {
    /// The capability set effective at `height`: genesis capabilities plus
    /// every activation declared at or before `height`.
    #[must_use]
    pub fn effective_set(&self, height: u64) -> CapabilitySet {
        let mut set = CapabilitySet::genesis();
        for activation in &self.activations {
            if activation.activation_height <= height {
                set.insert_activation(activation);
            }
        }
        set
    }

    /// Validate an activation and fold it into the history. `containing` is the
    /// index of the block carrying the record; the activation only takes
    /// effect at its declared (strictly future) height.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] when the capability is
    /// unregistered, the version/hash identity does not match the registry,
    /// the activation height is not future relative to its block, the
    /// capability was already activated (append-only), or the signature set is
    /// empty.
    pub fn apply(
        &mut self,
        activation: CapabilityActivation,
        containing: u64,
    ) -> Result<(), CoreError> {
        let descriptor = lookup_capability(&activation.capability_id).ok_or_else(|| {
            CoreError::InvalidTransaction(format!(
                "capability activation: unknown capability '{}'",
                activation.capability_id
            ))
        })?;
        if activation.version != descriptor.version {
            return Err(CoreError::InvalidTransaction(format!(
                "capability activation: '{}' version {} is not registered",
                activation.capability_id, activation.version
            )));
        }
        let expected_hash = capability_hash(&activation.capability_id, activation.version);
        if activation.hash != expected_hash {
            return Err(CoreError::InvalidTransaction(format!(
                "capability activation: hash mismatch for '{}/v{}'",
                activation.capability_id, activation.version
            )));
        }
        if activation.activation_height <= containing {
            return Err(CoreError::InvalidTransaction(format!(
                "capability activation: height {} must be strictly future of its block {}",
                activation.activation_height, containing
            )));
        }
        if self
            .activations
            .iter()
            .any(|existing| existing.capability_id == activation.capability_id)
        {
            return Err(CoreError::InvalidTransaction(format!(
                "capability activation: '{}' is already activated (append-only)",
                activation.capability_id
            )));
        }
        if activation.signatures.is_empty() {
            return Err(CoreError::InvalidTransaction(
                "capability activation: at least one governance signature is required".into(),
            ));
        }
        self.activations.push(activation);
        Ok(())
    }

    /// Replay: derive the history from committed blocks, validating every
    /// canonical record and activation under the capability set effective at
    /// its block's height. Rebuilding from the same blocks always derives the
    /// same history (ADR-010 decision 5).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] for the first invalid record
    /// or activation encountered in block order.
    pub fn build_from_blocks(blocks: &[Block]) -> Result<Self, CoreError> {
        let mut history = Self::default();
        for block in blocks {
            history.validate_block(block)?;
        }
        Ok(history)
    }

    /// Validate one block at `block.index` under the set effective at that
    /// height and fold its activations. This is the deterministic replay step.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] for the first invalid
    /// canonical record or activation in the block.
    pub fn validate_block(&mut self, block: &Block) -> Result<(), CoreError> {
        let set = self.effective_set(block.index);
        for tx in &block.transactions {
            match &tx.kind {
                TransactionKind::CanonicalRecord(record) => {
                    validate_record_under(&set, record)?;
                }
                TransactionKind::CapabilityActivation(activation) => {
                    self.apply(activation.clone(), block.index)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Validate a canonical record under an explicit capability set.
///
/// Applies the v1 schema rules plus the height-selected capability gates:
/// `state_commitment` records require the `state_commitment` capability to be
/// active at the record's height. Endorsement enforcement (ADR-008) lives at
/// the network commit path, where the provider is configured; the remaining
/// gated behaviors attach here when they land (#46).
///
/// # Errors
///
/// Returns [`CoreError::InvalidTransaction`] for schema violations or a
/// required capability not active at this height.
pub fn validate_record_under(
    set: &CapabilitySet,
    record: &crate::canonical::CanonicalRecord,
) -> Result<(), CoreError> {
    validate_record(record)?;
    if record.schema_id == STATE_COMMITMENT_CAPABILITY_ID
        && !set.is_active(STATE_COMMITMENT_CAPABILITY_ID)
    {
        return Err(CoreError::InvalidTransaction(
            "canonical record state_commitment: capability 'state_commitment' \
             is not active at this height"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::CanonicalRecord;
    use crate::transaction::{Transaction, TransactionKind};
    use crate::Block;
    use serde_json::json;
    use std::collections::BTreeMap;

    const HEX64: &str = "abababababababababababababababababababababababababababababababab";

    fn activation(id: &str, height: u64) -> CapabilityActivation {
        CapabilityActivation {
            capability_id: id.into(),
            version: 1,
            hash: capability_hash(id, 1),
            activation_height: height,
            signatures: vec![RecordSignature {
                signer: "org-issuer".into(),
                signature_bytes: vec![0x42],
            }],
        }
    }

    fn activation_tx(id: &str, height: u64) -> Transaction {
        Transaction::with_id(
            format!("cap:{id}:{height}"),
            TransactionKind::CapabilityActivation(activation(id, height)),
        )
    }

    fn state_commitment_tx() -> Transaction {
        let payload: BTreeMap<String, serde_json::Value> = serde_json::from_value(json!({
            "merkle_root": HEX64,
            "counterparties": ["org-a", "org-b"],
        }))
        .unwrap();
        let mut record = CanonicalRecord::new(0, "state_commitment", payload, "org-issuer");
        record.signatures.push(RecordSignature {
            signer: "org-issuer".into(),
            signature_bytes: vec![0x42],
        });
        record.signatures.push(RecordSignature {
            signer: "org-a".into(),
            signature_bytes: vec![0x43],
        });
        record.commitment = record.commitment().ok();
        Transaction::with_id("sc:1".to_owned(), TransactionKind::CanonicalRecord(record))
    }

    #[test]
    fn test_registry_identity_is_unique_and_deterministic() {
        assert_eq!(CAPABILITY_V1.len(), 5);
        let mut ids: Vec<&str> = CAPABILITY_V1.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CAPABILITY_V1.len(), "capability ids are unique");
        assert_eq!(capability_hash("pdc", 1), capability_hash("pdc", 1));
        assert_ne!(capability_hash("pdc", 1), capability_hash("pdc", 2));
    }

    #[test]
    fn test_genesis_set() {
        let genesis = CapabilitySet::genesis();
        assert!(genesis.is_active("canonical_schema_v1"));
        assert!(genesis.is_active("state_commitment"));
        assert!(!genesis.is_active("pdc"));
        assert!(!genesis.is_active("endorsement"));
        assert!(!genesis.is_active("bft_consensus"));
        assert_eq!(
            genesis.active_version("state_commitment"),
            Some((1, capability_hash("state_commitment", 1).as_str()))
        );
    }

    #[test]
    fn test_effective_set_selects_by_height() {
        let mut history = CapabilityHistory::default();
        history
            .apply(activation("bft_consensus", 10), 5)
            .expect("valid activation");
        assert!(!history.effective_set(9).is_active("bft_consensus"));
        assert!(history.effective_set(10).is_active("bft_consensus"));
        assert!(history.effective_set(11).is_active("bft_consensus"));
    }

    #[test]
    fn test_activation_validation_matrix() {
        // Unknown capability.
        let mut history = CapabilityHistory::default();
        let error = history
            .apply(activation("unknown_capability", 10), 5)
            .expect_err("unknown capability must fail");
        assert!(error.to_string().contains("unknown capability"), "{error}");

        // Version mismatch.
        let mut bad = activation("pdc", 10);
        bad.version = 2;
        let mut history = CapabilityHistory::default();
        assert!(history.apply(bad, 5).is_err());

        // Hash mismatch.
        let mut bad = activation("pdc", 10);
        bad.hash = "deadbeef".into();
        let mut history = CapabilityHistory::default();
        assert!(history.apply(bad, 5).is_err());

        // Same-block transition (not strictly future).
        let mut history = CapabilityHistory::default();
        let error = history
            .apply(activation("pdc", 5), 5)
            .expect_err("same-block activation must fail");
        assert!(error.to_string().contains("future"), "{error}");

        // Missing signatures.
        let mut bad = activation("pdc", 10);
        bad.signatures.clear();
        let mut history = CapabilityHistory::default();
        assert!(history.apply(bad, 5).is_err());

        // Append-only: duplicate id rejected.
        let mut history = CapabilityHistory::default();
        history
            .apply(activation("pdc", 10), 5)
            .expect("first is fine");
        let error = history
            .apply(activation("pdc", 12), 6)
            .expect_err("duplicate activation must fail");
        assert!(error.to_string().contains("already activated"), "{error}");
    }

    #[test]
    fn test_state_commitment_gate_requires_capability() {
        let empty_set = CapabilitySet::default();
        let record = match &state_commitment_tx().kind {
            TransactionKind::CanonicalRecord(record) => record.clone(),
            other => panic!("expected canonical record, got {other:?}"),
        };
        let error = validate_record_under(&empty_set, &record)
            .expect_err("inactive capability must reject state commitments");
        assert!(error.to_string().contains("state_commitment"), "{error}");

        let genesis = CapabilitySet::genesis();
        assert!(validate_record_under(&genesis, &record).is_ok());
    }

    fn block(index: u64, previous_hash: String, transactions: Vec<Transaction>) -> Block {
        let mut block = Block::new(index, transactions, previous_hash);
        block.mine(1);
        block
    }

    #[test]
    fn test_replay_derives_the_same_capability_history() {
        let genesis = block(0, "0".into(), vec![]);
        let b1 = block(
            1,
            genesis.hash.clone(),
            vec![state_commitment_tx(), activation_tx("bft_consensus", 4)],
        );
        let b2 = block(2, b1.hash.clone(), vec![]);
        let b3 = block(3, b2.hash.clone(), vec![]);
        let b4 = block(4, b3.hash.clone(), vec![]);
        let chain = vec![genesis, b1, b2, b3, b4];

        let first = CapabilityHistory::build_from_blocks(&chain).expect("valid chain");
        let second = CapabilityHistory::build_from_blocks(&chain).expect("valid chain");
        assert_eq!(first, second, "replay must derive the same history");

        assert!(!first.effective_set(3).is_active("bft_consensus"));
        assert!(first.effective_set(4).is_active("bft_consensus"));
        assert_eq!(
            first
                .effective_set(4)
                .active_version("bft_consensus")
                .map(|(v, _)| v),
            Some(1)
        );
    }

    #[test]
    fn test_replay_rejects_same_block_transition() {
        let genesis = block(0, "0".into(), vec![]);
        // Activation declared for the same block that carries it.
        let bad = block(1, genesis.hash.clone(), vec![activation_tx("pdc", 1)]);
        let error =
            CapabilityHistory::build_from_blocks(&[genesis, bad]).expect_err("must be rejected");
        assert!(error.to_string().contains("future"), "{error}");
    }

    #[test]
    fn test_advertisement_roundtrip() {
        let advert = CapabilityAdvertisement {
            id: "pdc".into(),
            version: 1,
        };
        let json = serde_json::to_string(&advert).unwrap();
        let decoded: CapabilityAdvertisement = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, advert);
    }

    #[test]
    fn test_activation_roundtrip_serialization() {
        let json = serde_json::to_string(&activation("pdc", 10)).unwrap();
        let decoded: CapabilityActivation = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.capability_id, "pdc");
        assert_eq!(decoded.activation_height, 10);
    }
}
