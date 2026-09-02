//! MSP-backed [`EndorsementProvider`]: verifies endorsement signatures and
//! derives principals from a registered key directory (ADR-008 handoff 2).
//!
//! A principal is the verified MSP organization member bound to a public key
//! at registration time — never a caller-supplied label. A claimed principal
//! that conflicts with the registered identity is rejected, unknown keys are
//! rejected, invalid signatures are skipped, and at most one signature counts
//! per distinct principal.
//!
//! ponytail: the directory stands in for certificate-bound MSP verification;
//! certificate-backed identity plumbing is Stage 2 (ADR-008 consequences) and
//! will register identities from issued certificates into the same directory.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use glasschain_core::{
    CoreError, EndorsementEvaluation, EndorsementProvider, EndorsementRequest, PolicyExpression,
    Principal,
};
use std::collections::{HashMap, HashSet};

use crate::Identity;

/// Ed25519-verifying endorsement provider over a registered MSP key directory.
#[derive(Debug, Default)]
pub struct MspEndorsementProvider {
    /// Public-key bytes → verified principal.
    directory: HashMap<Vec<u8>, Principal>,
}

impl MspEndorsementProvider {
    /// Create an empty directory; register members before evaluating.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an MSP member: `public_key` is the raw 32-byte ed25519 key and
    /// `principal` the verified organization member identity derived from it.
    pub fn register(&mut self, public_key: Vec<u8>, principal: Principal) {
        self.directory.insert(public_key, principal);
    }

    /// Register an [`Identity`] under a principal, binding the identity's
    /// public key to the principal it signs for.
    pub fn register_identity(&mut self, identity: &Identity, principal: Principal) {
        self.register(identity.public_key_bytes().to_vec(), principal);
    }
}

impl EndorsementProvider for MspEndorsementProvider {
    fn evaluate(
        &self,
        expression: &PolicyExpression,
        request: &EndorsementRequest,
    ) -> Result<EndorsementEvaluation, CoreError> {
        // Allow-all shapes are not valid v1 policy metadata (ADR-008 decision
        // 1); validate before counting so no caller can smuggle one in.
        expression.validate()?;

        let mut distinct: HashSet<Principal> = HashSet::new();

        for signer in &request.signers {
            let Some(verified) = self.directory.get(&signer.public_key) else {
                return Err(CoreError::InvalidTransaction(format!(
                    "endorsement: unknown signing key (hex {}...)",
                    hex::encode(&signer.public_key[..signer.public_key.len().min(4)])
                )));
            };
            if verified != &signer.claimed_principal {
                return Err(CoreError::InvalidTransaction(format!(
                    "endorsement: claimed principal '{}' conflicts with verified principal '{}'",
                    signer.claimed_principal.as_str(),
                    verified.as_str()
                )));
            }

            let Ok(key_bytes) = <[u8; 32]>::try_from(signer.public_key.as_slice()) else {
                return Err(CoreError::InvalidTransaction(
                    "endorsement: public key is not 32 bytes".into(),
                ));
            };
            let Ok(verifying_key) = VerifyingKey::from_bytes(&key_bytes) else {
                return Err(CoreError::InvalidTransaction(
                    "endorsement: public key is not a valid ed25519 key".into(),
                ));
            };
            let Ok(sig_bytes) = <[u8; 64]>::try_from(signer.signature.as_slice()) else {
                // Malformed signatures are skipped, never counted (ADR-008
                // decision 2: replayed or duplicate signatures never increase
                // the count).
                let claimed = signer.claimed_principal.as_str();
                log::warn!("endorsement: skipping malformed signature from '{claimed}'");
                continue;
            };
            let signature = Signature::from_bytes(&sig_bytes);
            if verifying_key.verify(&request.payload, &signature).is_err() {
                let claimed = signer.claimed_principal.as_str();
                log::warn!(
                    "endorsement: skipping signature that failed verification from '{claimed}'"
                );
                continue;
            }

            distinct.insert(verified.clone());
        }

        Ok(EndorsementEvaluation {
            satisfied: expression.evaluate(&distinct),
            distinct_principals: {
                let mut principals: Vec<Principal> = distinct.into_iter().collect();
                principals.sort();
                principals
            },
            required: expression.required_count(),
        })
    }

    fn name(&self) -> &'static str {
        "msp-ed25519"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{EndorserIdentity, ScopedTarget};

    fn request(payload: &[u8], signers: Vec<EndorserIdentity>) -> EndorsementRequest {
        EndorsementRequest {
            target: ScopedTarget {
                channel: "supply".into(),
                contract: "inventory".into(),
                keys: vec![],
                collection: None,
            },
            payload: payload.to_vec(),
            signers,
        }
    }

    fn signer(identity: &Identity, claimed: &str) -> EndorserIdentity {
        EndorserIdentity {
            claimed_principal: Principal::new(claimed),
            public_key: identity.public_key_bytes().to_vec(),
            signature: identity.sign_bytes(b"canonical-payload"),
        }
    }

    fn registered() -> (MspEndorsementProvider, Identity, Identity) {
        let mut provider = MspEndorsementProvider::new();
        let org_a = Identity::generate("node-a");
        let org_b = Identity::generate("node-b");
        provider.register_identity(&org_a, Principal::new("org-a"));
        provider.register_identity(&org_b, Principal::new("org-b"));
        (provider, org_a, org_b)
    }

    #[test]
    fn test_signed_by_is_satisfied() {
        let (provider, org_a, _) = registered();
        let expression = PolicyExpression::signed_by("org-a");
        let result = provider
            .evaluate(
                &expression,
                &request(b"canonical-payload", vec![signer(&org_a, "org-a")]),
            )
            .expect("valid signer");
        assert!(result.satisfied);
        assert_eq!(result.distinct_principals, vec![Principal::new("org-a")]);
        assert_eq!(result.required, 1);
    }

    #[test]
    fn test_n_out_of_across_organizations() {
        let (provider, org_a, org_b) = registered();
        let expression = PolicyExpression::NOutOf {
            required: 2,
            rules: vec![
                PolicyExpression::signed_by("org-a"),
                PolicyExpression::signed_by("org-b"),
            ],
        };
        let result = provider
            .evaluate(
                &expression,
                &request(
                    b"canonical-payload",
                    vec![signer(&org_a, "org-a"), signer(&org_b, "org-b")],
                ),
            )
            .expect("valid signers");
        assert!(result.satisfied);
        assert_eq!(result.distinct_principals.len(), 2);
    }

    #[test]
    fn test_distinct_principal_counting_duplicates_do_not_inflate() {
        let (provider, org_a, org_b) = registered();
        let expression = PolicyExpression::NOutOf {
            required: 2,
            rules: vec![
                PolicyExpression::signed_by("org-a"),
                PolicyExpression::signed_by("org-b"),
            ],
        };
        // org-a signs twice (duplicate + replay); org-b does not sign. The
        // duplicate must never satisfy org-b's principal.
        let result = provider
            .evaluate(
                &expression,
                &request(
                    b"canonical-payload",
                    vec![signer(&org_a, "org-a"), signer(&org_a, "org-a")],
                ),
            )
            .expect("valid signer");
        assert!(!result.satisfied);
        assert_eq!(result.distinct_principals, vec![Principal::new("org-a")]);

        // A second node of the same organization also counts once.
        let second_node = Identity::generate("node-a-2");
        let mut multi_node_provider = MspEndorsementProvider::new();
        multi_node_provider.register_identity(&org_a, Principal::new("org-a"));
        multi_node_provider.register_identity(&second_node, Principal::new("org-a"));
        multi_node_provider.register_identity(&org_b, Principal::new("org-b"));
        let multi_node_result = multi_node_provider
            .evaluate(
                &expression,
                &request(
                    b"canonical-payload",
                    vec![
                        signer(&org_a, "org-a"),
                        signer(&second_node, "org-a"),
                        signer(&org_b, "org-b"),
                    ],
                ),
            )
            .expect("valid signers");
        assert!(
            multi_node_result.satisfied,
            "two nodes of org-a plus org-b satisfy 2-of-2"
        );
        assert_eq!(multi_node_result.distinct_principals.len(), 2);
    }

    #[test]
    fn test_forged_organization_label_is_rejected() {
        let (provider, org_a, _) = registered();
        let expression = PolicyExpression::signed_by("org-b");
        let error = provider
            .evaluate(
                &expression,
                &request(b"canonical-payload", vec![signer(&org_a, "org-b")]),
            )
            .expect_err("forged label must be rejected");
        assert!(error.to_string().contains("conflicts"), "{error}");
    }

    #[test]
    fn test_unknown_key_is_rejected() {
        let (provider, _, _) = registered();
        let unknown = Identity::generate("unknown-node");
        let expression = PolicyExpression::signed_by("org-a");
        let error = provider
            .evaluate(
                &expression,
                &request(b"canonical-payload", vec![signer(&unknown, "org-a")]),
            )
            .expect_err("unregistered key must be rejected");
        assert!(error.to_string().contains("unknown signing key"), "{error}");
    }

    #[test]
    fn test_allow_all_expression_is_rejected() {
        let (provider, _, _) = registered();
        let allow_all = PolicyExpression::NOutOf {
            required: 0,
            rules: vec![],
        };
        let error = provider
            .evaluate(&allow_all, &request(b"payload", vec![]))
            .expect_err("allow-all expressions must be rejected at the seam");
        assert!(error.to_string().contains("rule"), "{error}");
    }

    #[test]
    fn test_invalid_signature_is_skipped() {
        let (provider, org_a, _) = registered();
        let expression = PolicyExpression::signed_by("org-a");
        let mut bad = signer(&org_a, "org-a");
        bad.signature = vec![0x42; 64]; // not a signature of the payload
        let result = provider
            .evaluate(&expression, &request(b"canonical-payload", vec![bad]))
            .expect("skipped, not fatal");
        assert!(!result.satisfied);
        assert!(result.distinct_principals.is_empty());
    }

    #[test]
    fn test_nested_expression_with_distinct_principals() {
        let (provider, org_a, org_b) = registered();
        let expression = PolicyExpression::and(vec![
            PolicyExpression::signed_by("org-a"),
            PolicyExpression::or(vec![
                PolicyExpression::signed_by("org-b"),
                PolicyExpression::signed_by("org-a"),
            ]),
        ]);
        let result = provider
            .evaluate(
                &expression,
                &request(
                    b"canonical-payload",
                    vec![signer(&org_a, "org-a"), signer(&org_b, "org-b")],
                ),
            )
            .expect("valid signers");
        assert!(result.satisfied);
    }

    #[test]
    fn test_multi_key_targets_all_layers_required() {
        let (provider, org_a, org_b) = registered();
        let policies = glasschain_core::ScopedPolicies {
            channel_default: PolicyExpression::signed_by("org-a"),
            contract_default: None,
            collection_policy: None,
            key_policies: vec![("threshold".into(), PolicyExpression::signed_by("org-b"))],
        };
        let target = ScopedTarget {
            channel: "supply".into(),
            contract: "inventory".into(),
            keys: vec!["threshold".into()],
            collection: None,
        };
        let applicable = policies.applicable(&target);
        assert_eq!(applicable.len(), 2, "channel default + key policy");

        let results: Vec<EndorsementEvaluation> = applicable
            .iter()
            .map(|policy| {
                provider
                    .evaluate(
                        policy,
                        &request(
                            b"canonical-payload",
                            vec![signer(&org_a, "org-a"), signer(&org_b, "org-b")],
                        ),
                    )
                    .expect("valid signers")
            })
            .collect();
        assert!(
            results.iter().all(|r| r.satisfied),
            "every applicable layer must be satisfied"
        );

        // Missing the key-level signer fails the transaction.
        let results: Vec<EndorsementEvaluation> = applicable
            .iter()
            .map(|policy| {
                provider
                    .evaluate(
                        policy,
                        &request(b"canonical-payload", vec![signer(&org_a, "org-a")]),
                    )
                    .expect("valid signer")
            })
            .collect();
        assert!(
            results.iter().any(|r| !r.satisfied),
            "the key-level layer must be unsatisfied"
        );
    }
}
