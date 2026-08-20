//! Policy-based endorsement engine for `GlassChain`.
//!
//! This module implements the Phase 2 endorsement workflow that enables
//! permissioned governance on the `GlassChain` network.  Before a
//! [`Transaction`] is committed to the ledger, a configurable quorum of
//! authorised organisations must sign it.
//!
//! ## Workflow
//! ```text
//! EndorsementProposal
//!   ├── Transaction  (to be endorsed)
//!   └── Signatures   (collected from org members)
//!         ↓
//! EndorsementEngine::evaluate()
//!   ├── Verify each signature  (invalid → log::warn! + skip)
//!   └── Count valid sigs whose org is in policy.endorser_org_names
//!         ↓
//! EndorsementResult::Approved | Rejected
//! ```

// All public types in this module intentionally share the "Endorsement" prefix
// to be self-describing when used through the crate's flat re-export surface
// (e.g. `glasschain_identity::EndorsementPolicy`).
#![allow(clippy::module_name_repetitions)]

use crate::error::IdentityError;
use crate::identity::SignedTransaction;
use glasschain_core::Transaction;
use serde::{Deserialize, Serialize};

// ── EndorsementPolicy ────────────────────────────────────────────────────────

/// Defines the quorum rule for endorsing a [`Transaction`].
///
/// An `EndorsementPolicy` declares *which organisations* may provide valid
/// endorsements and *how many* signatures are required before a proposal may
/// be approved.
///
/// # Example
///
/// ```rust
/// use glasschain_identity::endorsement::EndorsementPolicy;
///
/// let policy = EndorsementPolicy::new(
///     "2-of-3 Pharma Orgs",
///     vec!["PharmaCorp".into(), "MedDistrib".into(), "CityPharmacy".into()],
///     2,
/// );
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndorsementPolicy {
    /// Human-readable name for this policy (e.g., `"2-of-3 Pharma Orgs"`).
    pub name: String,
    /// List of organisation names that are valid endorsers.
    pub endorser_org_names: Vec<String>,
    /// Minimum number of valid signatures required to satisfy the policy.
    pub required_count: usize,
}

impl EndorsementPolicy {
    /// Create a new [`EndorsementPolicy`].
    ///
    /// # Arguments
    ///
    /// * `name` — human-readable policy label.
    /// * `endorser_org_names` — organisations whose signatures count toward the
    ///   quorum.
    /// * `required_count` — minimum number of valid, org-matched signatures
    ///   needed to satisfy the policy.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        endorser_org_names: Vec<String>,
        required_count: usize,
    ) -> Self {
        Self {
            name: name.into(),
            endorser_org_names,
            required_count,
        }
    }

    /// Return `true` if `signatures` satisfies this policy.
    ///
    /// A signature qualifies when **both** of the following hold:
    ///
    /// 1. Its `org_name` is listed in [`EndorsementPolicy::endorser_org_names`].
    /// 2. Its embedded [`SignedTransaction`] passes cryptographic verification.
    ///
    /// The method counts all such valid, org-matched signatures and returns
    /// `true` when that count is at least [`EndorsementPolicy::required_count`].
    #[must_use]
    pub fn is_satisfied_by(&self, signatures: &[EndorsementSignature]) -> bool {
        let valid_count = signatures
            .iter()
            .filter(|sig| self.endorser_org_names.contains(&sig.org_name) && sig.verify().is_ok())
            .count();
        valid_count >= self.required_count
    }
}

// ── EndorsementSignature ─────────────────────────────────────────────────────

/// A single endorsement: a [`SignedTransaction`] paired with the submitting
/// organisation's name.
///
/// The `org_name` is validated against an [`EndorsementPolicy`]'s allow-list
/// to determine whether this signature contributes to the quorum.
#[derive(Debug, Clone)]
pub struct EndorsementSignature {
    /// The signed transaction payload.
    pub signed_transaction: SignedTransaction,
    /// Organisation name this endorser belongs to.
    pub org_name: String,
}

impl EndorsementSignature {
    /// Wrap a [`SignedTransaction`] with its originating organisation name.
    #[must_use]
    pub fn new(signed_transaction: SignedTransaction, org_name: impl Into<String>) -> Self {
        Self {
            signed_transaction,
            org_name: org_name.into(),
        }
    }

    /// Verify the cryptographic signature of the embedded [`SignedTransaction`].
    ///
    /// Delegates directly to [`SignedTransaction::verify`].
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidPublicKey`] if the stored public-key
    /// bytes are malformed, or [`IdentityError::VerificationFailed`] if the
    /// signature does not match the transaction payload.
    pub fn verify(&self) -> Result<(), IdentityError> {
        self.signed_transaction.verify()
    }
}

// ── EndorsementProposal ──────────────────────────────────────────────────────

/// A pending endorsement request, collecting signatures for a single
/// [`Transaction`] before the [`EndorsementEngine`] evaluates the result.
///
/// Once enough valid signatures have been gathered, pass the proposal to
/// [`EndorsementEngine::evaluate`] to obtain an [`EndorsementResult`].
#[derive(Debug, Clone)]
pub struct EndorsementProposal {
    /// The underlying transaction being endorsed.
    pub transaction: Transaction,
    /// Unique proposal identifier derived from the transaction ID.
    ///
    /// Format: `"endorsement-{transaction.id}"`.
    pub proposal_id: String,
    /// Collected endorsement signatures.
    pub signatures: Vec<EndorsementSignature>,
    /// Unix timestamp (seconds since the epoch) when this proposal was created.
    pub created_at: u64,
}

impl EndorsementProposal {
    /// Create a new proposal for the given transaction.
    ///
    /// * `proposal_id` is set to `"endorsement-{transaction.id}"`.
    /// * `created_at` is the current wall-clock time in Unix seconds,
    ///   falling back to `0` if the system clock predates the Unix epoch.
    #[must_use]
    pub fn new(transaction: Transaction) -> Self {
        // Compute derived fields while we still hold a borrow on `transaction`,
        // then move it into the struct below.
        let proposal_id = format!("endorsement-{}", transaction.id);
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            transaction,
            proposal_id,
            signatures: Vec::new(),
            created_at,
        }
    }

    /// Append an endorsement signature to this proposal.
    pub fn add_signature(&mut self, sig: EndorsementSignature) {
        self.signatures.push(sig);
    }

    /// Return the number of signatures collected so far.
    #[must_use]
    pub const fn signature_count(&self) -> usize {
        self.signatures.len()
    }

    /// Return the node IDs of all signers in the order they were added.
    #[must_use]
    pub fn signer_node_ids(&self) -> Vec<&str> {
        self.signatures
            .iter()
            .map(|s| s.signed_transaction.signer_node_id.as_str())
            .collect()
    }

    /// Return the organisation names of all signers in the order they were
    /// added.
    #[must_use]
    pub fn signer_org_names(&self) -> Vec<&str> {
        self.signatures
            .iter()
            .map(|s| s.org_name.as_str())
            .collect()
    }
}

// ── EndorsementResult ────────────────────────────────────────────────────────

/// The outcome of evaluating an [`EndorsementProposal`] against an
/// [`EndorsementEngine`]'s policy.
#[derive(Debug, Clone)]
pub enum EndorsementResult {
    /// The proposal collected enough valid signatures from authorised
    /// organisations to satisfy the policy.
    Approved {
        /// ID of the proposal that was evaluated.
        proposal_id: String,
        /// Number of valid endorsements counted toward the quorum.
        endorser_count: usize,
    },
    /// The proposal did not satisfy the policy.
    Rejected {
        /// ID of the proposal that was evaluated.
        proposal_id: String,
        /// Human-readable explanation of why the policy was not met.
        reason: String,
        /// Number of valid, org-matched signatures that were collected.
        collected: usize,
        /// Minimum number of signatures required by the policy.
        required: usize,
    },
}

// ── EndorsementEngine ────────────────────────────────────────────────────────

/// Evaluates [`EndorsementProposal`]s against an [`EndorsementPolicy`].
///
/// The engine verifies every collected signature, logs a warning for any that
/// fail cryptographic verification (and skips them), then determines whether
/// the remaining valid signatures from authorised organisations meet the
/// policy's quorum requirement.
///
/// # Example
///
/// ```rust,no_run
/// use glasschain_identity::endorsement::{EndorsementEngine, EndorsementPolicy};
///
/// let policy = EndorsementPolicy::new("1-of-1", vec!["MyOrg".into()], 1);
/// let engine = EndorsementEngine::new(policy);
/// // … build a proposal, add signatures, then:
/// // let result = engine.evaluate(&proposal);
/// ```
#[derive(Debug, Clone)]
pub struct EndorsementEngine {
    /// Active endorsement policy used for all evaluations.
    pub policy: EndorsementPolicy,
}

impl EndorsementEngine {
    /// Create a new engine that evaluates proposals against `policy`.
    #[must_use]
    pub const fn new(policy: EndorsementPolicy) -> Self {
        Self { policy }
    }

    /// Evaluate a proposal against the engine's policy.
    ///
    /// For each signature in the proposal:
    ///
    /// 1. The cryptographic signature is verified.  On failure a [`log::warn!`]
    ///    message is emitted and the signature is skipped.
    /// 2. If verification succeeds **and** the signer's `org_name` is listed in
    ///    [`EndorsementPolicy::endorser_org_names`], the signature is counted.
    ///
    /// Returns [`EndorsementResult::Approved`] when the count of valid,
    /// org-matched signatures is at least
    /// [`EndorsementPolicy::required_count`]; otherwise returns
    /// [`EndorsementResult::Rejected`] with a descriptive reason string.
    #[must_use]
    pub fn evaluate(&self, proposal: &EndorsementProposal) -> EndorsementResult {
        let mut valid_count: usize = 0;

        for sig in &proposal.signatures {
            match sig.verify() {
                Ok(()) => {
                    if self.policy.endorser_org_names.contains(&sig.org_name) {
                        valid_count += 1;
                    }
                }
                Err(e) => {
                    log::warn!(
                        "endorsement signature from node '{}' (org '{}') \
                         failed verification: {e}",
                        sig.signed_transaction.signer_node_id,
                        sig.org_name,
                    );
                }
            }
        }

        if valid_count >= self.policy.required_count {
            EndorsementResult::Approved {
                proposal_id: proposal.proposal_id.clone(),
                endorser_count: valid_count,
            }
        } else {
            EndorsementResult::Rejected {
                proposal_id: proposal.proposal_id.clone(),
                reason: format!(
                    "need {} endorsements from orgs {:?}, got {}",
                    self.policy.required_count, self.policy.endorser_org_names, valid_count
                ),
                collected: valid_count,
                required: self.policy.required_count,
            }
        }
    }

    /// Return `true` when the proposal would be approved under this engine's
    /// policy.
    ///
    /// Convenience wrapper around [`EndorsementEngine::evaluate`]; equivalent
    /// to:
    /// ```rust,ignore
    /// matches!(self.evaluate(proposal), EndorsementResult::Approved { .. })
    /// ```
    #[must_use]
    pub fn is_approved(&self, proposal: &EndorsementProposal) -> bool {
        matches!(self.evaluate(proposal), EndorsementResult::Approved { .. })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        EndorsementEngine, EndorsementPolicy, EndorsementProposal, EndorsementResult,
        EndorsementSignature,
    };
    use crate::identity::Identity;
    use glasschain_core::{InventoryUpdate, Transaction, TransactionKind};

    // ── helpers ───────────────────────────────────────────────────────────────

    fn sample_tx() -> Transaction {
        Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-001".into(),
            owner_id: "node-1".into(),
            quantity_delta: 10,
            reason: "endorsement test".into(),
        }))
    }

    /// Build a real, cryptographically valid [`EndorsementSignature`] for
    /// `node_id` / `org_name` by signing `tx` with a freshly generated
    /// ed25519 identity.
    fn make_sig(node_id: &str, org_name: &str, tx: Transaction) -> EndorsementSignature {
        let identity = Identity::generate(node_id);
        let signed_tx = identity.sign_transaction(tx).unwrap();
        EndorsementSignature::new(signed_tx, org_name)
    }

    // ── 1. Policy: exact count ────────────────────────────────────────────────

    #[test]
    fn test_policy_satisfied_with_exact_count() {
        // 2-of-2: exactly two valid signatures from the two listed orgs.
        let policy = EndorsementPolicy::new("2-of-2", vec!["OrgA".into(), "OrgB".into()], 2);

        let tx = sample_tx();
        let sig_a = make_sig("node-a", "OrgA", tx.clone());
        let sig_b = make_sig("node-b", "OrgB", tx);

        assert!(
            policy.is_satisfied_by(&[sig_a, sig_b]),
            "2-of-2 should be satisfied by exactly 2 valid org signatures"
        );
    }

    // ── 2. Policy: wrong org not counted ─────────────────────────────────────

    #[test]
    fn test_policy_not_satisfied_with_wrong_org() {
        // The signature is cryptographically valid but OrgC is not in the list.
        let policy = EndorsementPolicy::new("1-of-1", vec!["OrgA".into()], 1);

        let tx = sample_tx();
        let sig_c = make_sig("node-c", "OrgC", tx);

        assert!(
            !policy.is_satisfied_by(&[sig_c]),
            "signature from unlisted OrgC should not count toward the quorum"
        );
    }

    // ── 3. Policy: excess signatures ─────────────────────────────────────────

    #[test]
    fn test_policy_satisfied_with_excess_signatures() {
        // 2-of-3: three valid signatures — still satisfies the 2-of-3 policy.
        let policy = EndorsementPolicy::new(
            "2-of-3",
            vec!["OrgA".into(), "OrgB".into(), "OrgC".into()],
            2,
        );

        let tx = sample_tx();
        let sig_a = make_sig("node-a", "OrgA", tx.clone());
        let sig_b = make_sig("node-b", "OrgB", tx.clone());
        let sig_c = make_sig("node-c", "OrgC", tx);

        assert!(
            policy.is_satisfied_by(&[sig_a, sig_b, sig_c]),
            "3 valid signatures should satisfy a 2-of-3 policy"
        );
    }

    // ── 4. Policy: below threshold ────────────────────────────────────────────

    #[test]
    fn test_policy_not_satisfied_below_threshold() {
        // 2-of-3 with only 1 valid signature from a listed org → not satisfied.
        let policy = EndorsementPolicy::new(
            "2-of-3",
            vec!["OrgA".into(), "OrgB".into(), "OrgC".into()],
            2,
        );

        let tx = sample_tx();
        let sig_a = make_sig("node-a", "OrgA", tx);

        assert!(
            !policy.is_satisfied_by(&[sig_a]),
            "1 valid signature should not satisfy a 2-of-3 policy"
        );
    }

    // ── 5. Engine: approve ────────────────────────────────────────────────────

    #[test]
    fn test_engine_approve() {
        // Build a real proposal with two valid signed transactions; 2-of-2 policy
        // must evaluate to Approved { endorser_count: 2 }.
        let policy = EndorsementPolicy::new("2-of-2", vec!["OrgA".into(), "OrgB".into()], 2);
        let engine = EndorsementEngine::new(policy);

        let tx = sample_tx();
        let mut proposal = EndorsementProposal::new(tx.clone());
        proposal.add_signature(make_sig("node-a", "OrgA", tx.clone()));
        proposal.add_signature(make_sig("node-b", "OrgB", tx));

        let result = engine.evaluate(&proposal);
        assert!(
            matches!(
                result,
                EndorsementResult::Approved {
                    endorser_count: 2,
                    ..
                }
            ),
            "expected Approved with endorser_count = 2"
        );
        assert!(engine.is_approved(&proposal));
    }

    // ── 6. Engine: reject insufficient ───────────────────────────────────────

    #[test]
    fn test_engine_reject_insufficient() {
        // Only 1 valid signature out of the 2 required → Rejected.
        let policy = EndorsementPolicy::new("2-of-2", vec!["OrgA".into(), "OrgB".into()], 2);
        let engine = EndorsementEngine::new(policy);

        let tx = sample_tx();
        let mut proposal = EndorsementProposal::new(tx.clone());
        proposal.add_signature(make_sig("node-a", "OrgA", tx));

        let result = engine.evaluate(&proposal);
        assert!(
            matches!(
                result,
                EndorsementResult::Rejected {
                    collected: 1,
                    required: 2,
                    ..
                }
            ),
            "expected Rejected with collected=1, required=2"
        );
        assert!(!engine.is_approved(&proposal));
    }

    // ── 7. Proposal: signature_count ─────────────────────────────────────────

    #[test]
    fn test_proposal_add_signature() {
        let tx = sample_tx();
        let mut proposal = EndorsementProposal::new(tx.clone());

        assert_eq!(
            proposal.signature_count(),
            0,
            "new proposal has no signatures"
        );

        proposal.add_signature(make_sig("node-x", "OrgX", tx));

        assert_eq!(
            proposal.signature_count(),
            1,
            "proposal should have 1 signature after add_signature"
        );
    }

    // ── 9. Engine: a signature that fails verification is skipped ────────────

    #[test]
    fn test_engine_skips_signature_that_fails_verification() {
        // 1-of-1, but the single signature is cryptographically invalid (its
        // payload was tampered after signing). It must be skipped, not counted.
        let policy = EndorsementPolicy::new("1-of-1", vec!["OrgA".into()], 1);
        let engine = EndorsementEngine::new(policy);

        let tx = sample_tx();
        let mut proposal = EndorsementProposal::new(tx.clone());
        let mut sig = make_sig("node-a", "OrgA", tx);
        if let TransactionKind::InventoryUpdate(ref mut u) = sig.signed_transaction.transaction.kind
        {
            u.quantity_delta = 999;
        }
        proposal.add_signature(sig);

        let result = engine.evaluate(&proposal);
        assert!(
            matches!(
                result,
                EndorsementResult::Rejected {
                    collected: 0,
                    required: 1,
                    ..
                }
            ),
            "expected Rejected with collected=0 (invalid sig skipped), got {result:?}"
        );
        assert!(!engine.is_approved(&proposal));
    }

    // ── 10. Policy: cryptographically invalid signature is not counted ───────

    #[test]
    fn test_policy_ignores_signature_that_fails_verification() {
        let policy = EndorsementPolicy::new("1-of-1", vec!["OrgA".into()], 1);

        let tx = sample_tx();
        let mut sig = make_sig("node-a", "OrgA", tx);
        // Org is listed but the embedded signature no longer verifies.
        if let TransactionKind::InventoryUpdate(ref mut u) = sig.signed_transaction.transaction.kind
        {
            u.quantity_delta = 999;
        }

        assert!(
            !policy.is_satisfied_by(&[sig]),
            "a cryptographically invalid signature must not count toward the quorum"
        );
    }

    // ── 8. EndorsementResult: Debug variants ─────────────────────────────────

    #[test]
    fn test_endorsement_result_variants() {
        // Approved variant
        let approved = EndorsementResult::Approved {
            proposal_id: "proposal-abc".into(),
            endorser_count: 3,
        };
        let debug_approved = format!("{approved:?}");
        assert!(
            debug_approved.contains("Approved"),
            "debug should contain 'Approved'"
        );
        assert!(
            debug_approved.contains("proposal-abc"),
            "debug should contain proposal id"
        );
        assert!(
            debug_approved.contains('3'),
            "debug should contain endorser_count"
        );

        // Rejected variant
        let rejected = EndorsementResult::Rejected {
            proposal_id: "proposal-xyz".into(),
            reason: "not enough valid signatures".into(),
            collected: 1,
            required: 2,
        };
        let debug_rejected = format!("{rejected:?}");
        assert!(
            debug_rejected.contains("Rejected"),
            "debug should contain 'Rejected'"
        );
        assert!(
            debug_rejected.contains("proposal-xyz"),
            "debug should contain proposal id"
        );
        assert!(
            debug_rejected.contains("not enough valid signatures"),
            "debug should contain reason"
        );
    }
}
