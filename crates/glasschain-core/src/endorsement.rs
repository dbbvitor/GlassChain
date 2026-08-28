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

use crate::error::CoreError;
use serde::{Deserialize, Serialize};
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
