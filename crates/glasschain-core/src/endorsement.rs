//! Identity-neutral endorsement policy seam (ADR-008).
//!
//! The v1 policy language is a deterministic Fabric-style signature-policy
//! tree: [`PolicyExpression::SignedBy`] and [`PolicyExpression::NOutOf`], with
//! local `and`/`or` builders that serialize to `NOutOf`. The persisted/wire
//! representation is data, never executable policy code.
//!
//! [`EndorsementProvider`] evaluates an expression against a request's
//! signers; the *implementation* derives principals from verified credentials,
//! counts at most one signature per distinct principal, and rejects a
//! caller-supplied label that conflicts with the verified identity.

use crate::block::Block;
use crate::error::CoreError;
use crate::providers::EndorsementProvider;
use crate::transaction::{Transaction, TransactionKind};
use crate::write_set::{PersistentWrite, WriteVisibility};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// A v1 policy principal: a verified MSP organization member identifier
/// (ADR-008 decision 2).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Principal(String);

impl Principal {
    /// Create a principal. Empty principals are invalid (see
    /// [`PolicyExpression::validate`]).
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The principal identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Principal {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for Principal {
    fn from(name: String) -> Self {
        Self(name)
    }
}

/// A deterministic signature-policy expression (ADR-008 decision 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyExpression {
    /// One signature from the named principal.
    SignedBy(Principal),
    /// `required` satisfied sub-rules. Local `AND`/`OR` convenience builders
    /// serialize to this shape; there is no implicit `ANY`/`ALL`/`MAJORITY`
    /// language.
    NOutOf {
        /// Number of sub-rules that must evaluate true.
        required: usize,
        /// Sub-expressions, in deterministic order.
        rules: Vec<Self>,
    },
}

impl PolicyExpression {
    /// A single-principal policy.
    #[must_use]
    pub fn signed_by(principal: impl Into<Principal>) -> Self {
        Self::SignedBy(principal.into())
    }

    /// All sub-rules must hold: serializes to `NOutOf(rules.len(), rules)`.
    #[must_use]
    pub const fn and(rules: Vec<Self>) -> Self {
        Self::NOutOf {
            required: rules.len(),
            rules,
        }
    }

    /// At least one sub-rule must hold: serializes to `NOutOf(1, rules)`.
    #[must_use]
    pub const fn or(rules: Vec<Self>) -> Self {
        Self::NOutOf { required: 1, rules }
    }

    /// Validate the expression as v1 policy metadata: every leaf names a
    /// non-empty principal, every `NOutOf` requires at least one rule, and the
    /// required count is achievable (`1..=rules.len()`). A channel without an
    /// explicit default is not allow-all — v1 policies name at least one
    /// principal and require at least one signature.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] for the first violated rule.
    pub fn validate(&self) -> Result<(), CoreError> {
        match self {
            Self::SignedBy(principal) => {
                if principal.as_str().is_empty() {
                    return Err(CoreError::InvalidTransaction(
                        "endorsement policy: principal must not be empty".into(),
                    ));
                }
            }
            Self::NOutOf { required, rules } => {
                if rules.is_empty() {
                    return Err(CoreError::InvalidTransaction(
                        "endorsement policy: NOutOf needs at least one rule".into(),
                    ));
                }
                if *required == 0 || *required > rules.len() {
                    return Err(CoreError::InvalidTransaction(format!(
                        "endorsement policy: required {required} must be in 1..={}",
                        rules.len()
                    )));
                }
                for rule in rules {
                    rule.validate()?;
                }
            }
        }
        Ok(())
    }

    /// The signature count this expression requires at its root.
    #[must_use]
    pub const fn required_count(&self) -> usize {
        match self {
            Self::SignedBy(_) => 1,
            Self::NOutOf { required, .. } => *required,
        }
    }

    /// Pure, deterministic tree evaluation over a set of **distinct** verified
    /// principals. Counting distinct principals (never duplicate or replayed
    /// signatures) is the caller's responsibility — this function assumes its
    /// input is already a set.
    ///
    /// An `NOutOf` with no rules never evaluates true (the allow-all shape is
    /// rejected by [`Self::validate`]; this guard keeps unvalidated
    /// expressions from accidentally passing too).
    #[must_use]
    pub fn evaluate(&self, principals: &HashSet<Principal>) -> bool {
        match self {
            Self::SignedBy(principal) => principals.contains(principal),
            Self::NOutOf { required, rules } => {
                if rules.is_empty() {
                    return false;
                }
                rules
                    .iter()
                    .filter(|rule| rule.evaluate(principals))
                    .count()
                    >= *required
            }
        }
    }
}

/// The scoped target an endorsement request covers (ADR-008 decision 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedTarget {
    /// Channel scope.
    pub channel: String,
    /// Contract scope.
    pub contract: String,
    /// Fully scoped persistent keys the transaction writes. A transaction
    /// touching multiple keys must satisfy all of their effective policies.
    pub keys: Vec<String>,
    /// PDC collection when the write targets private data.
    pub collection: Option<String>,
}

/// Policy layers for one channel/contract (ADR-008 decision 1).
///
/// Precedence: channel default → optional stricter contract default → optional
/// PDC collection policy → optional key-level policy. Every applicable layer
/// must be satisfied, so a more specific policy can only add constraints —
/// it can never weaken a base policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedPolicies {
    /// Mandatory channel default. A channel without an explicit default is
    /// not allow-all (v1 policies name at least one principal and require at
    /// least one signature).
    pub channel_default: PolicyExpression,
    /// Optional stricter contract default.
    pub contract_default: Option<PolicyExpression>,
    /// Optional collection endorsement policy for PDC writes.
    pub collection_policy: Option<PolicyExpression>,
    /// Optional per-key policies for fully scoped persistent keys.
    pub key_policies: Vec<(String, PolicyExpression)>,
}

impl ScopedPolicies {
    /// Every policy applicable to `target`, in precedence order.
    #[must_use]
    pub fn applicable(&self, target: &ScopedTarget) -> Vec<PolicyExpression> {
        let mut policies = vec![self.channel_default.clone()];
        if let Some(policy) = &self.contract_default {
            policies.push(policy.clone());
        }
        if target.collection.is_some() {
            if let Some(policy) = &self.collection_policy {
                policies.push(policy.clone());
            }
        }
        for key in &target.keys {
            if let Some((_, policy)) = self.key_policies.iter().find(|(k, _)| k == key) {
                policies.push(policy.clone());
            }
        }
        policies
    }

    /// Validate every layer as v1 policy metadata.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] for the first invalid layer.
    pub fn validate(&self) -> Result<(), CoreError> {
        self.channel_default.validate()?;
        if let Some(policy) = &self.contract_default {
            policy.validate()?;
        }
        if let Some(policy) = &self.collection_policy {
            policy.validate()?;
        }
        for (key, policy) in &self.key_policies {
            if key.is_empty() {
                return Err(CoreError::InvalidTransaction(
                    "endorsement policy: key-level policy names an empty key".into(),
                ));
            }
            policy.validate()?;
        }
        Ok(())
    }
}

/// One claimed endorser: the principal the signer claims, its public key, and
/// a detached signature over the request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndorserIdentity {
    /// The principal the signer claims. Rejected when it conflicts with the
    /// principal bound to `public_key` (ADR-008 decision 2).
    pub claimed_principal: Principal,
    /// Raw 32-byte ed25519 public key.
    pub public_key: Vec<u8>,
    /// Signature over [`EndorsementRequest::payload`].
    pub signature: Vec<u8>,
}

/// An endorsement evaluation request: the exact payload being endorsed (the
/// transaction and its committed write set), the scoped target it touches, and
/// the claimed signers (ADR-008 handoff 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndorsementRequest {
    /// The scoped keys and channel/contract/collection the payload writes.
    pub target: ScopedTarget,
    /// Canonical bytes every endorser signs.
    pub payload: Vec<u8>,
    /// Claimed signers with signatures.
    pub signers: Vec<EndorserIdentity>,
}

/// The outcome of evaluating one policy expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndorsementEvaluation {
    /// `true` when the expression is satisfied.
    pub satisfied: bool,
    /// Distinct verified principals whose signatures were counted (at most one
    /// per principal).
    pub distinct_principals: Vec<Principal>,
    /// Required count at the expression root.
    pub required: usize,
}

/// The per-transaction endorsement carrier (ADR-008 §4): the scoped target
/// the signers authorized, and the signers over the transaction's canonical
/// bytes.
///
/// The transaction's committed partial write set must stay inside the
/// declared scope — signers authorize the exact transaction and the write set
/// it commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionEndorsement {
    /// The scoped keys and channel/contract/collection the signers authorized.
    pub target: ScopedTarget,
    /// Signers over [`TransactionEndorsement::payload`].
    pub signers: Vec<EndorserIdentity>,
}

impl TransactionEndorsement {
    /// The canonical bytes every endorser signs: the transaction serialized
    /// with its endorsement carriers cleared, so signatures are never
    /// self-referential. The transaction id, kind, and declared targets are
    /// covered; the committed write set is bound by the scope check in
    /// [`evaluate_transaction_endorsements`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serialization`] when the transaction cannot be
    /// serialized (not reachable for in-memory transactions).
    pub fn payload(tx: &Transaction) -> Result<Vec<u8>, CoreError> {
        let unsigned = Transaction {
            endorsements: Vec::new(),
            ..tx.clone()
        };
        Ok(serde_json::to_vec(&unsigned)?)
    }

    /// `true` when this carrier's declared scope covers `write`.
    #[must_use]
    pub fn covers(&self, write: &PersistentWrite) -> bool {
        write.channel == self.target.channel
            && write.contract == self.target.contract
            && self.target.keys.iter().any(|key| key == &write.key)
            && match (&write.visibility, &self.target.collection) {
                (WriteVisibility::Public, None) => true,
                (WriteVisibility::Pdc(name), Some(collection)) => name == collection,
                _ => false,
            }
    }
}

/// A committed, versioned, append-only endorsement-policy update for one
/// scope (ADR-008 decision 4).
///
/// The update activates only after its containing block commits;
/// authorization rides the transaction's endorsement carriers evaluated
/// against the *current* effective policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyUpdate {
    /// Channel scope (non-empty).
    pub channel: String,
    /// Contract scope; empty names a channel-wide update.
    pub contract: String,
    /// The full replacement policy set for the scope, validated as v1 policy
    /// metadata. A key-level policy is cleared by omitting it — the clearing
    /// authorization is this transaction's endorsement under the current
    /// effective policy.
    pub policies: ScopedPolicies,
}

/// The fail-closed v1 default for scopes without a committed policy: the
/// fixed `network-governance` principal must sign (ADR-008 decision 1 — a
/// channel without an explicit default is not allow-all).
pub const NETWORK_GOVERNANCE_PRINCIPAL: &str = "network-governance";

/// The endorsement-policy history derived from committed blocks, mirroring
/// `CapabilityHistory` (ADR-008 decision 4).
///
/// Policy metadata is versioned, append-only, and replayed deterministically
/// from the chain. Historical blocks keep the policy effective at their
/// height by construction — replay folds updates in block order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyHistory {
    updates: Vec<PolicyUpdate>,
}

impl PolicyHistory {
    /// The effective policy set for `(channel, contract)`: the last committed
    /// update for the exact scope, else the last channel-wide update, else the
    /// fail-closed default.
    #[must_use]
    pub fn policies_for(&self, channel: &str, contract: &str) -> ScopedPolicies {
        let exact = self
            .updates
            .iter()
            .rev()
            .find(|update| update.channel == channel && update.contract == contract);
        let channel_wide = self
            .updates
            .iter()
            .rev()
            .find(|update| update.channel == channel && update.contract.is_empty());
        exact
            .or(channel_wide)
            .map_or_else(Self::default_policies, |update| update.policies.clone())
    }
    /// The fail-closed v1 default policy set.
    // ponytail: fixed network-governance principal keeps un-configured scopes
    // fail-closed; deployments commit a PolicyUpdate naming real principals.
    #[must_use]
    pub fn default_policies() -> ScopedPolicies {
        ScopedPolicies {
            channel_default: PolicyExpression::signed_by(NETWORK_GOVERNANCE_PRINCIPAL),
            contract_default: None,
            collection_policy: None,
            key_policies: Vec::new(),
        }
    }

    /// The committed updates in application order.
    #[must_use]
    pub fn updates(&self) -> &[PolicyUpdate] {
        &self.updates
    }

    /// Validate one update as v1 policy metadata and fold it into the history.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] for an empty channel or
    /// invalid policy metadata (allow-all shapes are rejected).
    pub fn apply(&mut self, update: PolicyUpdate) -> Result<(), CoreError> {
        if update.channel.is_empty() {
            return Err(CoreError::InvalidTransaction(
                "endorsement policy update: channel must not be empty".into(),
            ));
        }
        update.policies.validate()?;
        self.updates.push(update);
        Ok(())
    }

    /// Replay: derive the history from committed blocks, validating every
    /// update in block order. Rebuilding from the same blocks always derives
    /// the same history. Cryptographic endorsement evaluation happens at the
    /// network commit path, where the provider lives; this replay validates
    /// metadata and the same-block rule only.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] for the first invalid update
    /// or same-block policy/write conflict encountered in block order.
    pub fn build_from_blocks(blocks: &[Block]) -> Result<Self, CoreError> {
        let mut history = Self::default();
        for block in blocks {
            history.validate_block(block)?;
        }
        Ok(history)
    }

    /// Replay one block: validate its policy updates in order and enforce the
    /// same-block rule (ADR-008 decision 4) — a block that changes a key's
    /// policy **and** writes the same key is rejected, because the write would
    /// commit under the old policy while the block installs a new one. The new
    /// policy applies deterministically from the next block.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] for the first invalid update
    /// or same-block conflict.
    pub fn validate_block(&mut self, block: &Block) -> Result<(), CoreError> {
        let mut pending: Vec<&PolicyUpdate> = Vec::new();
        let mut changed_keys: Vec<(&str, &str, &str)> = Vec::new();
        for tx in &block.transactions {
            if let TransactionKind::PolicyUpdate(update) = &tx.kind {
                for (key, _) in &update.policies.key_policies {
                    changed_keys.push((&update.channel, &update.contract, key.as_str()));
                }
                pending.push(update);
            }
        }
        // Written keys: the block's committed write set plus every declared
        // endorsement target on non-policy transactions. A `PolicyUpdate`'s
        // own carrier target names the keys being authorized, not written.
        let written = block
            .write_set
            .iter()
            .map(|write| {
                (
                    write.channel.as_str(),
                    write.contract.as_str(),
                    write.key.as_str(),
                )
            })
            .chain(
                block
                    .transactions
                    .iter()
                    .filter(|tx| !matches!(tx.kind, TransactionKind::PolicyUpdate(_)))
                    .flat_map(|tx| {
                        tx.endorsements.iter().flat_map(|endorsement| {
                            endorsement.target.keys.iter().map(move |key| {
                                (
                                    endorsement.target.channel.as_str(),
                                    endorsement.target.contract.as_str(),
                                    key.as_str(),
                                )
                            })
                        })
                    }),
            );
        for (channel, contract, key) in written {
            if changed_keys.contains(&(channel, contract, key)) {
                return Err(CoreError::InvalidTransaction(format!(
                    "endorsement policy: key '{key}' on ({channel}, {contract}) is both \
                     policy-updated and written in the same block"
                )));
            }
        }
        for update in pending {
            let owned = update.clone();
            self.apply(owned)?;
        }
        Ok(())
    }
}

/// The v1 operation default for a transaction whose record family demands
/// stronger endorsement than the scoped policies (ADR-008 decision 3).
///
/// Custody handoffs (`delivery_receipt`) require the sender (record issuer)
/// and the receiving custodian, 2-of-2; `recall` requires the issuing
/// custodian and the authorized authority (`issued_by`), 2-of-2;
/// `quality_certification` and `audit_attestation` require the payload
/// issuer's signature. Quarantine and dispute are workflow transitions, not
/// record families — their multi-party rule is whatever the committed scoped
/// policies configure.
///
/// Known default-bearing families with a missing payload field fail closed;
/// families without a default return `Ok(None)`.
///
/// # Errors
///
/// Returns [`CoreError::InvalidTransaction`] when a default-bearing record
/// family is missing its payload authority field.
// ponytail: recall's 2-of-2 degenerates to self-approval when the envelope
// issuer equals the payload authority; per-channel configured multi-party
// policies for record families land with channel wiring.
pub fn operation_default(tx: &Transaction) -> Result<Option<PolicyExpression>, CoreError> {
    // Governance default (ADR-012): a capability activation switches
    // network-wide, validation-affecting behaviour, so it requires the
    // network-governance principal. The fixed principal is the v1 genesis
    // fallback — fail-closed — until a deployment commits a `PolicyUpdate`
    // naming its real governance principals.
    if matches!(tx.kind, TransactionKind::CapabilityActivation(_)) {
        return Ok(Some(PolicyExpression::signed_by("network-governance")));
    }
    let TransactionKind::CanonicalRecord(record) = &tx.kind else {
        return Ok(None);
    };
    let payload_str = |key: &str| {
        record
            .payload
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    match record.schema_id.as_str() {
        "delivery_receipt" => {
            let Some(receiver) = payload_str("receiver_id") else {
                return Err(CoreError::InvalidTransaction(
                    "endorsement: custody handoff record is missing 'receiver_id'".into(),
                ));
            };
            Ok(Some(PolicyExpression::and(vec![
                PolicyExpression::signed_by(record.issuer.clone()),
                PolicyExpression::signed_by(receiver),
            ])))
        }
        "recall" => {
            let Some(authority) = payload_str("issued_by") else {
                return Err(CoreError::InvalidTransaction(
                    "endorsement: recall record is missing 'issued_by'".into(),
                ));
            };
            Ok(Some(PolicyExpression::NOutOf {
                required: 2,
                rules: vec![
                    PolicyExpression::signed_by(record.issuer.clone()),
                    PolicyExpression::signed_by(authority),
                ],
            }))
        }
        "quality_certification" | "audit_attestation" => {
            let Some(issuer) = payload_str("issuer") else {
                return Err(CoreError::InvalidTransaction(
                    "endorsement: certification/audit record is missing 'issuer'".into(),
                ));
            };
            Ok(Some(PolicyExpression::signed_by(issuer)))
        }
        "state_commitment" => {
            // ADR-012: the default matches what the record's own count-only
            // signature check already pretends to require — the issuer and
            // every named counterparty sign.
            let Some(counterparties) = record
                .payload
                .get("counterparties")
                .and_then(Value::as_array)
            else {
                return Err(CoreError::InvalidTransaction(
                    "endorsement: state commitment record is missing 'counterparties'".into(),
                ));
            };
            let mut rules = vec![PolicyExpression::signed_by(record.issuer.clone())];
            for counterparty in counterparties {
                let Some(name) = counterparty.as_str().filter(|v| !v.is_empty()) else {
                    return Err(CoreError::InvalidTransaction(
                        "endorsement: state commitment counterparty must be a non-empty \
                         organization name"
                            .into(),
                    ));
                };
                rules.push(PolicyExpression::signed_by(name));
            }
            Ok(Some(PolicyExpression::NOutOf {
                required: rules.len(),
                rules,
            }))
        }
        _ => Ok(None),
    }
}

/// Full endorsement evaluation for one candidate block's transaction (ADR-008
/// §4).
///
/// Every declared carrier is verified and every applicable policy layer —
/// channel, contract, collection, key, and the operation default — must be
/// satisfied, or the transaction is rejected with no partial state. Runs
/// against the *pre-block* policy history: a policy update in the same block
/// applies only from the next block.
///
/// `partial_writes` is the transaction's own contribution to the committed
/// write set (empty on replay paths that cannot attribute writes; the caller
/// then checks aggregate coverage against the block's declared carriers).
///
/// # Errors
///
/// Returns [`CoreError::InvalidTransaction`] when a write falls outside every
/// declared scope, a carrier signature cannot be authenticated, or any
/// applicable layer is unsatisfied.
pub fn evaluate_transaction_endorsements(
    provider: &dyn EndorsementProvider,
    history: &PolicyHistory,
    tx: &Transaction,
    partial_writes: &[PersistentWrite],
) -> Result<(), CoreError> {
    // The committed write set must stay inside the signed scope.
    for write in partial_writes {
        if !tx
            .endorsements
            .iter()
            .any(|endorsement| endorsement.covers(write))
        {
            return Err(CoreError::InvalidTransaction(format!(
                "endorsement: transaction '{}' writes key '{}' outside every declared \
                 endorsement scope",
                tx.id, write.key
            )));
        }
    }

    // A policy update is itself a signed transaction satisfying the current
    // effective policy — it must carry at least one endorsement carrier.
    if matches!(tx.kind, TransactionKind::PolicyUpdate(_)) && tx.endorsements.is_empty() {
        return Err(CoreError::InvalidTransaction(
            "endorsement: policy update carries no endorsement".into(),
        ));
    }

    // Canonical records have no channel/contract scope: their policy is the
    // operation default below, and the carriers act as the signer container.
    let is_record = matches!(tx.kind, TransactionKind::CanonicalRecord(_));
    if !is_record {
        for endorsement in &tx.endorsements {
            let policies =
                history.policies_for(&endorsement.target.channel, &endorsement.target.contract);
            let request = EndorsementRequest {
                target: endorsement.target.clone(),
                payload: TransactionEndorsement::payload(tx)?,
                signers: endorsement.signers.clone(),
            };
            for policy in policies.applicable(&endorsement.target) {
                if !provider.evaluate(&policy, &request)?.satisfied {
                    return Err(CoreError::InvalidTransaction(format!(
                        "endorsement: transaction '{}' failed policy evaluation (required {})",
                        tx.id,
                        policy.required_count()
                    )));
                }
            }
        }
    }

    // Operation defaults (ADR-008 decision 3) apply on top of the scoped
    // policies, satisfied by the union of the transaction's verified signers
    // (distinct-principal counting makes duplicates harmless).
    if let Some(default) = operation_default(tx)? {
        let signers: Vec<EndorserIdentity> = tx
            .endorsements
            .iter()
            .flat_map(|endorsement| endorsement.signers.iter().cloned())
            .collect();
        let request = EndorsementRequest {
            target: ScopedTarget {
                channel: String::new(),
                contract: String::new(),
                keys: Vec::new(),
                collection: None,
            },
            payload: TransactionEndorsement::payload(tx)?,
            signers,
        };
        if !provider.evaluate(&default, &request)?.satisfied {
            return Err(CoreError::InvalidTransaction(format!(
                "endorsement: transaction '{}' failed its operation default (required {})",
                tx.id,
                default.required_count()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::CanonicalRecord;

    fn principal(name: &str) -> Principal {
        Principal::new(name)
    }

    fn set(names: &[&str]) -> HashSet<Principal> {
        names.iter().map(|n| principal(n)).collect()
    }

    #[test]
    fn test_signed_by_evaluates() {
        let expression = PolicyExpression::signed_by("org-a");
        assert!(expression.evaluate(&set(&["org-a"])));
        assert!(!expression.evaluate(&set(&["org-b"])));
        assert_eq!(expression.required_count(), 1);
    }

    #[test]
    fn test_n_out_of_evaluates() {
        let expression = PolicyExpression::NOutOf {
            required: 2,
            rules: vec![
                PolicyExpression::signed_by("org-a"),
                PolicyExpression::signed_by("org-b"),
                PolicyExpression::signed_by("org-c"),
            ],
        };
        assert!(expression.evaluate(&set(&["org-a", "org-b"])));
        assert!(!expression.evaluate(&set(&["org-a"])));
    }

    #[test]
    fn test_nested_n_out_of() {
        let expression = PolicyExpression::NOutOf {
            required: 2,
            rules: vec![
                PolicyExpression::signed_by("regulator"),
                PolicyExpression::or(vec![
                    PolicyExpression::signed_by("custodian-a"),
                    PolicyExpression::signed_by("custodian-b"),
                ]),
            ],
        };
        assert!(expression.evaluate(&set(&["regulator", "custodian-a"])));
        assert!(expression.evaluate(&set(&["regulator", "custodian-b"])));
        assert!(!expression.evaluate(&set(&["regulator"])));
        assert!(!expression.evaluate(&set(&["custodian-a", "custodian-b"])));
    }

    #[test]
    fn test_and_or_builders_serialize_to_n_out_of() {
        let and = PolicyExpression::and(vec![
            PolicyExpression::signed_by("a"),
            PolicyExpression::signed_by("b"),
        ]);
        let PolicyExpression::NOutOf { required, rules } = &and else {
            panic!("AND must serialize to NOutOf");
        };
        assert_eq!(*required, 2);
        assert_eq!(rules.len(), 2);

        let or = PolicyExpression::or(vec![
            PolicyExpression::signed_by("a"),
            PolicyExpression::signed_by("b"),
        ]);
        let PolicyExpression::NOutOf { required, rules } = &or else {
            panic!("OR must serialize to NOutOf");
        };
        assert_eq!(*required, 1);
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn test_validation_rejects_allow_all_shapes() {
        assert!(PolicyExpression::signed_by("").validate().is_err());

        assert!(PolicyExpression::NOutOf {
            required: 0,
            rules: vec![PolicyExpression::signed_by("a")],
        }
        .validate()
        .is_err());

        assert!(PolicyExpression::NOutOf {
            required: 3,
            rules: vec![
                PolicyExpression::signed_by("a"),
                PolicyExpression::signed_by("b"),
            ],
        }
        .validate()
        .is_err());

        assert!(PolicyExpression::NOutOf {
            required: 1,
            rules: vec![],
        }
        .validate()
        .is_err());

        assert!(PolicyExpression::signed_by("org-a").validate().is_ok());
    }

    #[test]
    fn test_applicable_precedence_composition() {
        let policies = ScopedPolicies {
            channel_default: PolicyExpression::signed_by("channel-gov"),
            contract_default: Some(PolicyExpression::signed_by("contract-owner")),
            collection_policy: Some(PolicyExpression::or(vec![
                PolicyExpression::signed_by("member-a"),
                PolicyExpression::signed_by("member-b"),
            ])),
            key_policies: vec![(
                "threshold".into(),
                PolicyExpression::and(vec![
                    PolicyExpression::signed_by("channel-gov"),
                    PolicyExpression::signed_by("regulator"),
                ]),
            )],
        };

        // No collection, no keys: channel + contract layers.
        let target = ScopedTarget {
            channel: "supply".into(),
            contract: "inventory".into(),
            keys: vec![],
            collection: None,
        };
        assert_eq!(policies.applicable(&target).len(), 2);

        // PDC write adds the collection layer.
        let target = ScopedTarget {
            channel: "supply".into(),
            contract: "inventory".into(),
            keys: vec![],
            collection: Some("pricing".into()),
        };
        assert_eq!(policies.applicable(&target).len(), 3);

        // Multi-key write adds one layer per key that has a policy; keys
        // without a policy add nothing.
        let target = ScopedTarget {
            channel: "supply".into(),
            contract: "inventory".into(),
            keys: vec!["threshold".into(), "other".into()],
            collection: Some("pricing".into()),
        };
        assert_eq!(policies.applicable(&target).len(), 4);
    }

    #[test]
    fn test_scoped_policies_validate() {
        let policies = ScopedPolicies {
            channel_default: PolicyExpression::signed_by("channel-gov"),
            contract_default: None,
            collection_policy: None,
            key_policies: vec![],
        };
        assert!(policies.validate().is_ok());

        let bad = ScopedPolicies {
            channel_default: PolicyExpression::signed_by(""),
            ..policies.clone()
        };
        assert!(bad.validate().is_err());

        let bad_key = ScopedPolicies {
            key_policies: vec![(String::new(), PolicyExpression::signed_by("a"))],
            ..policies
        };
        assert!(bad_key.validate().is_err());
    }

    #[test]
    fn test_expression_roundtrip_is_deterministic() {
        let expression = PolicyExpression::NOutOf {
            required: 2,
            rules: vec![
                PolicyExpression::signed_by("org-a"),
                PolicyExpression::signed_by("org-b"),
            ],
        };
        let json = serde_json::to_string(&expression).expect("serialize");
        assert_eq!(
            json,
            r#"{"NOutOf":{"required":2,"rules":[{"SignedBy":"org-a"},{"SignedBy":"org-b"}]}}"#,
            "wire form is deterministic data, never executable policy code"
        );
        let decoded: PolicyExpression = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, expression);
    }

    // ── #45: committed policy metadata, carriers, and evaluation ─────────────

    /// Test double: trusts any signer whose claimed principal is registered.
    /// Cryptographic verification is covered by `glasschain-identity`'s MSP
    /// provider tests; this exercises the evaluation composition only.
    struct RegisteredProvider {
        principals: HashSet<Principal>,
    }

    impl EndorsementProvider for RegisteredProvider {
        fn evaluate(
            &self,
            expression: &PolicyExpression,
            request: &EndorsementRequest,
        ) -> Result<EndorsementEvaluation, CoreError> {
            expression.validate()?;
            let distinct: HashSet<Principal> = request
                .signers
                .iter()
                .map(|signer| signer.claimed_principal.clone())
                .filter(|principal| self.principals.contains(principal))
                .collect();
            let mut principals: Vec<Principal> = distinct.into_iter().collect();
            principals.sort();
            Ok(EndorsementEvaluation {
                satisfied: expression.evaluate(&principals.iter().cloned().collect()),
                distinct_principals: principals,
                required: expression.required_count(),
            })
        }

        fn name(&self) -> &'static str {
            "test-registered"
        }
    }

    fn provider(principals: &[&str]) -> RegisteredProvider {
        RegisteredProvider {
            principals: principals.iter().map(|p| Principal::new(*p)).collect(),
        }
    }

    fn signer(principal: &str) -> EndorserIdentity {
        EndorserIdentity {
            claimed_principal: Principal::new(principal),
            public_key: vec![0x42; 32],
            signature: vec![0x42; 64],
        }
    }

    fn carrier(channel: &str, contract: &str, keys: &[&str]) -> TransactionEndorsement {
        TransactionEndorsement {
            target: ScopedTarget {
                channel: channel.into(),
                contract: contract.into(),
                keys: keys.iter().map(|key| (*key).to_owned()).collect(),
                collection: None,
            },
            signers: Vec::new(),
        }
    }

    fn write(channel: &str, contract: &str, key: &str) -> PersistentWrite {
        PersistentWrite {
            channel: channel.into(),
            contract: contract.into(),
            key: key.into(),
            op: crate::write_set::WriteOp::Set(b"v".to_vec()),
            visibility: WriteVisibility::Public,
        }
    }

    #[test]
    fn test_policies_for_precedence_exact_over_channel_wide_over_default() {
        let mut history = PolicyHistory::default();
        history
            .apply(PolicyUpdate {
                channel: "supply".into(),
                contract: String::new(),
                policies: ScopedPolicies {
                    channel_default: PolicyExpression::signed_by("channel-wide"),
                    contract_default: None,
                    collection_policy: None,
                    key_policies: Vec::new(),
                },
            })
            .expect("valid channel-wide update");
        history
            .apply(PolicyUpdate {
                channel: "supply".into(),
                contract: "inventory".into(),
                policies: ScopedPolicies {
                    channel_default: PolicyExpression::signed_by("exact"),
                    contract_default: None,
                    collection_policy: None,
                    key_policies: Vec::new(),
                },
            })
            .expect("valid exact update");

        assert_eq!(
            history.policies_for("supply", "inventory").channel_default,
            PolicyExpression::signed_by("exact"),
            "the exact scope wins"
        );
        assert_eq!(
            history.policies_for("supply", "other").channel_default,
            PolicyExpression::signed_by("channel-wide"),
            "an unconfigured contract falls back to the channel-wide set"
        );
        assert_eq!(
            history.policies_for("unknown", "other").channel_default,
            PolicyExpression::signed_by(NETWORK_GOVERNANCE_PRINCIPAL),
            "an unconfigured scope falls back to the fail-closed default"
        );
    }

    #[test]
    fn test_policy_update_application_rejects_invalid_metadata() {
        let mut history = PolicyHistory::default();
        let error = history
            .apply(PolicyUpdate {
                channel: String::new(),
                contract: String::new(),
                policies: PolicyHistory::default_policies(),
            })
            .expect_err("empty channel must fail");
        assert!(error.to_string().contains("channel"), "{error}");

        let error = history
            .apply(PolicyUpdate {
                channel: "supply".into(),
                contract: String::new(),
                policies: ScopedPolicies {
                    channel_default: PolicyExpression::signed_by(""),
                    contract_default: None,
                    collection_policy: None,
                    key_policies: Vec::new(),
                },
            })
            .expect_err("allow-all policy metadata must fail");
        assert!(
            error.to_string().contains("principal"),
            "invalid expression must fail: {error}"
        );
    }

    fn unmined_block(index: u64, previous_hash: String, transactions: Vec<Transaction>) -> Block {
        Block::new(index, transactions, previous_hash)
    }

    fn policy_update_tx(channel: &str, contract: &str, key: &str) -> Transaction {
        let mut tx = Transaction::new(TransactionKind::PolicyUpdate(PolicyUpdate {
            channel: channel.into(),
            contract: contract.into(),
            policies: ScopedPolicies {
                channel_default: PolicyExpression::signed_by("channel-gov"),
                contract_default: None,
                collection_policy: None,
                key_policies: vec![(key.into(), PolicyExpression::signed_by("key-gov"))],
            },
        }));
        let mut endorsement = carrier(channel, contract, &[key]);
        endorsement.signers = vec![signer("network-governance")];
        tx.endorsements.push(endorsement);
        tx
    }

    #[test]
    fn test_same_block_policy_update_and_write_conflict_is_rejected() {
        let mut history = PolicyHistory::default();
        let genesis = unmined_block(0, "0".into(), vec![]);

        // The update's own endorsement target names the changed key; the
        // update transaction itself must not be caught by the rule.
        let update = policy_update_tx("supply", "inventory", "threshold");
        history
            .validate_block(&unmined_block(
                1,
                genesis.hash.clone(),
                vec![update.clone()],
            ))
            .expect("the policy update alone commits");

        // A second transaction writing the same key in the same block.
        let mut writer =
            Transaction::new(TransactionKind::InventoryUpdate(crate::InventoryUpdate {
                product_id: "SKU".into(),
                owner_id: "owner".into(),
                quantity_delta: 1,
                reason: "test".into(),
            }));
        let mut endorsement = carrier("supply", "inventory", &["threshold"]);
        endorsement.signers = vec![signer("channel-gov")];
        writer.endorsements.push(endorsement);

        let error = history
            .validate_block(&unmined_block(1, genesis.hash, vec![update, writer]))
            .expect_err("same-block update + write must be rejected");
        assert!(error.to_string().contains("same block"), "{error}");
    }

    #[test]
    fn test_policy_history_replay_from_blocks_is_deterministic() {
        let genesis = unmined_block(0, "0".into(), vec![]);
        let channel_wide = Transaction::new(TransactionKind::PolicyUpdate(PolicyUpdate {
            channel: "supply".into(),
            contract: String::new(),
            policies: ScopedPolicies {
                channel_default: PolicyExpression::signed_by("channel-gov"),
                contract_default: None,
                collection_policy: None,
                key_policies: Vec::new(),
            },
        }));
        let b1 = unmined_block(1, genesis.hash.clone(), vec![channel_wide]);
        let b2 = unmined_block(
            2,
            b1.hash.clone(),
            vec![policy_update_tx("supply", "inventory", "k")],
        );
        let chain = vec![genesis, b1, b2];

        let replayed = PolicyHistory::build_from_blocks(&chain).expect("valid chain");
        assert_eq!(replayed.updates().len(), 2);
        assert_eq!(
            replayed.policies_for("supply", "inventory").channel_default,
            PolicyExpression::signed_by("channel-gov"),
            "the exact-scope update applies"
        );
        assert_eq!(
            replayed,
            PolicyHistory::build_from_blocks(&chain).expect("valid chain"),
            "replay must derive the same history"
        );
    }

    fn delivery_receipt_tx(receiver: &str) -> Transaction {
        let mut payload = std::collections::BTreeMap::new();
        payload.insert(
            "receiver_id".to_owned(),
            serde_json::Value::String(receiver.to_owned()),
        );
        let mut tx = Transaction::new(TransactionKind::CanonicalRecord(CanonicalRecord::new(
            0,
            "delivery_receipt",
            payload,
            "sender-org",
        )));
        let mut endorsement = carrier("supply", "inventory", &[]);
        endorsement.signers = vec![signer("sender-org"), signer(receiver)];
        tx.endorsements.push(endorsement);
        tx
    }

    #[test]
    fn test_operation_defaults_for_record_families() {
        let receipt = delivery_receipt_tx("receiver-org");
        let default = operation_default(&receipt)
            .expect("known family validates")
            .expect("custody handoff has a default");
        let PolicyExpression::NOutOf { required, rules } = &default else {
            panic!("custody default must be 2-of-2");
        };
        assert_eq!(*required, 2);
        assert_eq!(rules.len(), 2);

        let mut payload = std::collections::BTreeMap::new();
        payload.insert(
            "issuer".to_owned(),
            serde_json::Value::String("cert-org".to_owned()),
        );
        let certification = Transaction::new(TransactionKind::CanonicalRecord(
            CanonicalRecord::new(0, "quality_certification", payload, "cert-org"),
        ));
        assert_eq!(
            operation_default(&certification).expect("known family validates"),
            Some(PolicyExpression::signed_by("cert-org")),
            "certification requires the payload issuer"
        );

        // Recall: 2-of-2 over the envelope issuer and the payload authority.
        let mut payload = std::collections::BTreeMap::new();
        payload.insert(
            "issued_by".to_owned(),
            serde_json::Value::String("authority-org".to_owned()),
        );
        payload.insert(
            "lot_ref".to_owned(),
            serde_json::Value::String("lot-1".to_owned()),
        );
        payload.insert(
            "reason".to_owned(),
            serde_json::Value::String("contamination".to_owned()),
        );
        payload.insert(
            "status".to_owned(),
            serde_json::Value::String("initiated".to_owned()),
        );
        let recall = Transaction::new(TransactionKind::CanonicalRecord(CanonicalRecord::new(
            0,
            "recall",
            payload,
            "custodian-org",
        )));
        let default = operation_default(&recall)
            .expect("known family validates")
            .expect("recall has a default");
        let PolicyExpression::NOutOf { required, .. } = &default else {
            panic!("recall default must be multi-party");
        };
        assert_eq!(*required, 2);

        // A known family missing its payload field fails closed.
        let broken_recall = Transaction::new(TransactionKind::CanonicalRecord(
            CanonicalRecord::new(0, "recall", std::collections::BTreeMap::new(), "org"),
        ));
        let error = operation_default(&broken_recall).expect_err("missing field fails closed");
        assert!(error.to_string().contains("issued_by"), "{error}");

        let lot = Transaction::new(TransactionKind::CanonicalRecord(CanonicalRecord::new(
            0,
            "lot",
            std::collections::BTreeMap::new(),
            "org",
        )));
        assert_eq!(
            operation_default(&lot).expect("unknown family is not an error"),
            None,
            "ordinary records have no default"
        );
    }

    #[test]
    fn test_evaluation_satisfied_and_unsatisfied_paths() {
        let provider = provider(&["channel-gov", "key-gov"]);
        let mut history = PolicyHistory::default();
        history
            .apply(PolicyUpdate {
                channel: "supply".into(),
                contract: "inventory".into(),
                policies: ScopedPolicies {
                    channel_default: PolicyExpression::signed_by("channel-gov"),
                    contract_default: None,
                    collection_policy: None,
                    key_policies: vec![(
                        "threshold".into(),
                        PolicyExpression::signed_by("key-gov"),
                    )],
                },
            })
            .expect("valid update");

        let mut tx = Transaction::new(TransactionKind::InventoryUpdate(crate::InventoryUpdate {
            product_id: "SKU".into(),
            owner_id: "owner".into(),
            quantity_delta: 1,
            reason: "test".into(),
        }));
        let mut endorsement = carrier("supply", "inventory", &["threshold"]);
        endorsement.signers = vec![signer("channel-gov"), signer("key-gov")];
        tx.endorsements.push(endorsement);

        let writes = vec![write("supply", "inventory", "threshold")];
        assert!(
            evaluate_transaction_endorsements(&provider, &history, &tx, &writes).is_ok(),
            "channel + key layers satisfied"
        );

        // Missing the key-level signer.
        let mut tx_missing = tx.clone();
        tx_missing.endorsements[0].signers = vec![signer("channel-gov")];
        let error = evaluate_transaction_endorsements(&provider, &history, &tx_missing, &writes)
            .expect_err("unsatisfied key policy must reject");
        assert!(error.to_string().contains("failed policy"), "{error}");

        // A write outside every declared scope.
        let outside = vec![write("supply", "inventory", "other-key")];
        let error = evaluate_transaction_endorsements(&provider, &history, &tx, &outside)
            .expect_err("out-of-scope write must reject");
        assert!(
            error.to_string().contains("outside every declared"),
            "{error}"
        );
    }

    #[test]
    fn test_distinct_signer_counting_in_evaluation() {
        let provider = provider(&["sender-org", "receiver-org"]);
        let history = PolicyHistory::default();
        let mut tx = delivery_receipt_tx("receiver-org");
        // sender-org signs twice — the duplicate must not satisfy 2-of-2.
        tx.endorsements[0].signers = vec![signer("sender-org"), signer("sender-org")];
        let error = evaluate_transaction_endorsements(&provider, &history, &tx, &[])
            .expect_err("duplicate signatures must not satisfy 2-of-2");
        assert!(error.to_string().contains("operation default"), "{error}");

        tx.endorsements[0].signers = vec![signer("sender-org"), signer("receiver-org")];
        assert!(
            evaluate_transaction_endorsements(&provider, &history, &tx, &[]).is_ok(),
            "two distinct custodians satisfy the custody default"
        );
    }

    #[test]
    fn test_policy_update_requires_endorsement_carrier() {
        let provider = provider(&["network-governance"]);
        let history = PolicyHistory::default();
        let bare = Transaction::new(TransactionKind::PolicyUpdate(PolicyUpdate {
            channel: "supply".into(),
            contract: String::new(),
            policies: PolicyHistory::default_policies(),
        }));
        let error = evaluate_transaction_endorsements(&provider, &history, &bare, &[])
            .expect_err("an unauthorized policy update must reject");
        assert!(error.to_string().contains("no endorsement"), "{error}");

        // Authorized under the pre-block (fail-closed default) policy.
        let mut authorized = policy_update_tx("supply", "", "");
        authorized.endorsements[0].signers = vec![signer("network-governance")];
        assert!(
            evaluate_transaction_endorsements(&provider, &history, &authorized, &[]).is_ok(),
            "the network-governance principal authorizes the update"
        );
    }

    #[test]
    fn test_governance_operation_default_for_capability_activation() {
        let provider = provider(&["network-governance"]);
        let history = PolicyHistory::default();
        let activation = Transaction::new(TransactionKind::CapabilityActivation(
            crate::CapabilityActivation {
                capability_id: "endorsement".into(),
                version: 1,
                hash: crate::capability_hash("endorsement", 1),
                activation_height: 2,
                signatures: vec![crate::RecordSignature {
                    signer: "governance".into(),
                    signature_bytes: vec![0x42],
                }],
            },
        ));

        // The decorative signatures field does not authorize: no carrier, no
        // governance principal — fail closed.
        let error = evaluate_transaction_endorsements(&provider, &history, &activation, &[])
            .expect_err("an unendorsed capability activation must reject");
        assert!(error.to_string().contains("operation default"), "{error}");

        // A carrier whose verified signer is the governance principal satisfies
        // the default.
        let mut authorized = activation;
        authorized.endorsements.push(carrier("", "", &[]));
        authorized.endorsements[0].signers = vec![signer("network-governance")];
        assert!(
            evaluate_transaction_endorsements(&provider, &history, &authorized, &[]).is_ok(),
            "the governance principal authorizes the activation"
        );
    }

    #[test]
    fn test_state_commitment_operation_default_requires_issuer_and_counterparties() {
        let provider = provider(&["issuer-org", "counter-a", "counter-b"]);
        let history = PolicyHistory::default();
        let payload = [
            (
                "merkle_root".to_owned(),
                serde_json::Value::String(format!("{:064x}", 1u128)),
            ),
            (
                "counterparties".to_owned(),
                serde_json::json!(["counter-a", "counter-b"]),
            ),
        ]
        .into_iter()
        .collect();
        let mut record = CanonicalRecord::new(0, "state_commitment", payload, "issuer-org");
        record.signatures.push(crate::RecordSignature {
            signer: "issuer-org".into(),
            signature_bytes: vec![0x42],
        });
        let tx = Transaction::new(TransactionKind::CanonicalRecord(record));

        // Only the issuer signs the carrier: the counterparties are missing.
        let mut partial = tx.clone();
        partial.endorsements.push(carrier("", "", &[]));
        partial.endorsements[0].signers = vec![signer("issuer-org")];
        let error = evaluate_transaction_endorsements(&provider, &history, &partial, &[])
            .expect_err("issuer alone must not satisfy the state-commitment default");
        assert!(error.to_string().contains("operation default"), "{error}");

        // Issuer + both counterparties: satisfied.
        let mut full = tx;
        full.endorsements.push(carrier("", "", &[]));
        full.endorsements[0].signers = vec![
            signer("issuer-org"),
            signer("counter-a"),
            signer("counter-b"),
        ];
        assert!(
            evaluate_transaction_endorsements(&provider, &history, &full, &[]).is_ok(),
            "issuer plus every named counterparty satisfies the default"
        );
    }

    #[test]
    fn test_covers_matches_scope_and_visibility() {
        let mut endorsement = carrier("supply", "inventory", &["k1"]);
        assert!(endorsement.covers(&write("supply", "inventory", "k1")));
        assert!(!endorsement.covers(&write("supply", "inventory", "k2")));
        assert!(!endorsement.covers(&write("other", "inventory", "k1")));

        endorsement.target.collection = Some("pricing".into());
        let pdc_write = PersistentWrite {
            visibility: WriteVisibility::Pdc("pricing".into()),
            ..write("supply", "inventory", "k1")
        };
        assert!(endorsement.covers(&pdc_write));
        assert!(
            !endorsement.covers(&write("supply", "inventory", "k1")),
            "a public write is not covered by a collection-scoped carrier"
        );
    }

    #[test]
    fn test_transaction_endorsement_serde_back_compat() {
        let tx = delivery_receipt_tx("receiver-org");
        // The carrier rides the transaction serialization.
        let json = serde_json::to_string(&tx).expect("serialize");
        assert!(json.contains("endorsements"), "carriers are serialized");
        let decoded: Transaction = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, tx);

        // Pre-endorsement JSON (no field) parses with empty carriers, and an
        // empty carrier list serializes identically to the old form.
        let legacy = r#"{"id":"tx-1","timestamp":0,"kind":{"type":"SupplyOffer","payload":{"product_id":"p","product_name":"n","seller_id":"s","quantity_available":1,"price_per_unit":1,"lead_time_days":1,"currency":"USD"}}}"#;
        let decoded: Transaction = serde_json::from_str(legacy).expect("legacy JSON parses");
        assert!(decoded.endorsements.is_empty());
        let plain = Transaction::new(TransactionKind::InventoryUpdate(crate::InventoryUpdate {
            product_id: "SKU".into(),
            owner_id: "owner".into(),
            quantity_delta: 1,
            reason: "test".into(),
        }));
        let serialized = serde_json::to_string(&plain).expect("serialize");
        assert!(
            !serialized.contains("endorsements"),
            "empty carriers are skipped so historical hashes stay stable: {serialized}"
        );
    }
}
