//! MSP-backed [`EndorsementProvider`]: verifies endorsement signatures and
//! derives principals from a registered key directory (ADR-008 handoff 2).
//!
//! A principal is the verified MSP organization member bound to a public key
//! at registration time — never a caller-supplied label. A claimed principal
//! that conflicts with the registered identity is rejected, unknown keys are
//! rejected, invalid signatures are skipped, and at most one signature counts
//! per distinct principal.
//!
//! # Certificate-bound registration and height-based authorization (#87, D4)
//!
//! [`MspEndorsementProvider::register_certificate`] derives the principal from
//! a certificate verified against the organization anchor (chain, subject CN,
//! validity, CRL — all at **registration** time) plus a proof of possession of
//! the certificate's signing key. Each entry records the height it becomes
//! valid at; [`MspEndorsementProvider::revoke`] records the height it stops
//! being valid. Evaluation at a height checks those bounds only — no wall
//! clock and no mutable CRL are consulted after registration, so a committed
//! endorsement verifies identically on replay (revocation is go-forward).
//!
//! The registry is configured out-of-band (like the trust store) and is
//! assumed identical across validators until a chain-derived registry exists
//! (adjacent to issue #74). [`MspEndorsementProvider::register`] remains the
//! trusted local provisioning path (valid from height 0, unverified) for
//! embedders and tests.

use crate::cert_verifier::{CertChainVerifier, CertVerificationError};
use crate::possession::{
    certificate_ed25519_public_key, certificate_organization, msp_registration_message,
    verify_ed25519,
};
use crate::Identity;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use glasschain_core::{
    CoreError, EndorsementEvaluation, EndorsementProvider, EndorsementRequest, PolicyExpression,
    Principal,
};
use std::collections::{HashMap, HashSet};

/// Errors from certificate-bound principal registration.
#[derive(Debug, thiserror::Error)]
pub enum MspRegistrationError {
    /// The identity carries no organization-issued certificate.
    #[error("identity '{0}' has no organization certificate")]
    MissingCertificate(String),
    /// The certificate failed chain/validity/revocation verification.
    #[error("certificate verification failed: {0}")]
    Certificate(#[from] CertVerificationError),
    /// The certificate's subject Organization does not match the claimed one.
    #[error("certificate organization '{subject}' does not match '{org}'")]
    OrgMismatch {
        /// The claimed organization.
        org: String,
        /// The certificate's subject Organization name.
        subject: String,
    },
    /// The certificate carries no 32-byte ed25519 public key.
    #[error("certificate carries no ed25519 public key")]
    NoPublicKey,
    /// The proof of possession does not verify under the certificate's key.
    #[error("proof of possession does not verify")]
    ProofInvalid,
}

/// One registered key's authorization record. Height bounds are the only
/// validity inputs at evaluation time (#87).
#[derive(Debug, Clone)]
struct RegisteredPrincipal {
    principal: Principal,
    /// Height from which the key is authorized for new endorsements.
    valid_from: u64,
    /// Height from which the key is no longer authorized (go-forward
    /// revocation). `None` while the key is live.
    revoked_at: Option<u64>,
}

/// Ed25519-verifying endorsement provider over a registered MSP key directory.
#[derive(Debug, Default)]
pub struct MspEndorsementProvider {
    /// Public-key bytes → authorization record.
    directory: HashMap<Vec<u8>, RegisteredPrincipal>,
}

impl MspEndorsementProvider {
    /// Create an empty directory; register members before evaluating.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an MSP member: `public_key` is the raw 32-byte ed25519 key and
    /// `principal` the verified organization member identity derived from it.
    ///
    /// **Trusted local provisioning only** — no certificate is checked. Remote
    /// principals go through [`Self::register_certificate`].
    pub fn register(&mut self, public_key: Vec<u8>, principal: Principal) {
        self.directory.insert(
            public_key,
            RegisteredPrincipal {
                principal,
                valid_from: 0,
                revoked_at: None,
            },
        );
    }

    /// Register an [`Identity`] under a principal, binding the identity's
    /// public key to the principal it signs for. Trusted local provisioning
    /// (see [`Self::register`]).
    pub fn register_identity(&mut self, identity: &Identity, principal: Principal) {
        self.register(identity.public_key_bytes().to_vec(), principal);
    }

    /// Register a remote principal from its organization certificate (#87):
    /// the certificate must verify against `verifier` (chain, validity, CRL —
    /// fail closed), its subject **Organization** must equal `org`, and
    /// `proof` must be a signature by the certificate's key over
    /// [`msp_registration_message`]. The entry becomes valid at
    /// `valid_from_height` for new endorsements; committed history at earlier
    /// heights is unaffected.
    ///
    /// # Errors
    ///
    /// Returns [`MspRegistrationError`] for a missing certificate, any
    /// verification failure, an org mismatch, a certificate without an
    /// ed25519 key, or an invalid possession proof.
    pub fn register_certificate(
        &mut self,
        cert_pem: &str,
        proof: &[u8],
        org: &str,
        verifier: &CertChainVerifier,
        valid_from_height: u64,
    ) -> Result<(), MspRegistrationError> {
        verifier.verify_cert_pem(cert_pem)?;
        let subject = certificate_organization(cert_pem).unwrap_or_else(|| "(absent)".to_owned());
        if subject != org {
            return Err(MspRegistrationError::OrgMismatch {
                org: org.to_owned(),
                subject,
            });
        }
        let public_key =
            certificate_ed25519_public_key(cert_pem).ok_or(MspRegistrationError::NoPublicKey)?;
        if !verify_ed25519(
            &public_key,
            &msp_registration_message(org, &public_key),
            proof,
        ) {
            return Err(MspRegistrationError::ProofInvalid);
        }
        self.directory.insert(
            public_key.to_vec(),
            RegisteredPrincipal {
                principal: Principal::new(org),
                valid_from: valid_from_height,
                revoked_at: None,
            },
        );
        Ok(())
    }

    /// Convenience wrapper for the local node's own identity: signs the
    /// registration proof with the identity's key and registers the
    /// certificate-bound principal. The identity must carry a certificate.
    ///
    /// # Errors
    ///
    /// Returns [`MspRegistrationError`] as [`Self::register_certificate`].
    pub fn register_own_identity(
        &mut self,
        identity: &Identity,
        org: &str,
        verifier: &CertChainVerifier,
        valid_from_height: u64,
    ) -> Result<(), MspRegistrationError> {
        let cert_pem = identity
            .certificate_pem
            .as_deref()
            .ok_or_else(|| MspRegistrationError::MissingCertificate(identity.node_id.clone()))?;
        let proof =
            identity.sign_bytes(&msp_registration_message(org, &identity.public_key_bytes()));
        self.register_certificate(cert_pem, &proof, org, verifier, valid_from_height)
    }

    /// Revoke a key from `at_height` onward (go-forward, ADR-013): new
    /// endorsements at or after that height are rejected, while committed
    /// history before it keeps verifying. Returns `false` for an unknown key.
    pub fn revoke(&mut self, public_key: &[u8], at_height: u64) -> bool {
        if let Some(entry) = self.directory.get_mut(public_key) {
            entry.revoked_at = Some(at_height);
            true
        } else {
            false
        }
    }
}

impl EndorsementProvider for MspEndorsementProvider {
    fn evaluate(
        &self,
        expression: &PolicyExpression,
        request: &EndorsementRequest,
        height: u64,
    ) -> Result<EndorsementEvaluation, CoreError> {
        // Allow-all shapes are not valid v1 policy metadata (ADR-008 decision
        // 1); validate before counting so no caller can smuggle one in.
        expression.validate()?;

        let mut distinct: HashSet<Principal> = HashSet::new();

        for signer in &request.signers {
            let Some(entry) = self.directory.get(&signer.public_key) else {
                return Err(CoreError::InvalidTransaction(format!(
                    "endorsement: unknown signing key (hex {}...)",
                    hex::encode(&signer.public_key[..signer.public_key.len().min(4)])
                )));
            };
            if entry.principal != signer.claimed_principal {
                return Err(CoreError::InvalidTransaction(format!(
                    "endorsement: claimed principal '{}' conflicts with verified principal '{}'",
                    signer.claimed_principal.as_str(),
                    entry.principal.as_str()
                )));
            }
            // Height-based authorization (#87): the committed decision, not a
            // current-time check. A key is valid from its registration height
            // and stops being valid at its revocation height.
            if height < entry.valid_from {
                return Err(CoreError::InvalidTransaction(format!(
                    "endorsement: principal '{}' is not authorized at height {height} \
                     (valid from {})",
                    entry.principal.as_str(),
                    entry.valid_from
                )));
            }
            if entry.revoked_at.is_some_and(|revoked| height >= revoked) {
                return Err(CoreError::InvalidTransaction(format!(
                    "endorsement: principal '{}' was revoked at height {}",
                    entry.principal.as_str(),
                    entry.revoked_at.expect("checked by is_some_and")
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

            distinct.insert(entry.principal.clone());
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
    use crate::Organization;
    use glasschain_core::{EndorserIdentity, ScopedTarget};

    /// The height the tests evaluate at (after every registration below).
    const AT: u64 = 10;

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
            algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
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
                AT,
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
                AT,
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
                AT,
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
                AT,
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
                AT,
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
                AT,
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
            .evaluate(&allow_all, &request(b"payload", vec![]), AT)
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
            .evaluate(&expression, &request(b"canonical-payload", vec![bad]), AT)
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
                AT,
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
                        AT,
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
                        AT,
                    )
                    .expect("valid signer")
            })
            .collect();
        assert!(
            results.iter().any(|r| !r.satisfied),
            "the key-level layer must be unsatisfied"
        );
    }

    // ── #87/D4: certificate-bound registration and height authorization ─────

    fn cert_bound_provider() -> (
        MspEndorsementProvider,
        Identity,
        Organization,
        CertChainVerifier,
    ) {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let identity = org.issue_identity("node-a").unwrap().clone();
        let mut verifier = CertChainVerifier::from_org(&org).unwrap();
        verifier.add_crl_pem(&org.crl_pem().unwrap()).unwrap();
        (MspEndorsementProvider::new(), identity, org, verifier)
    }

    #[test]
    fn test_certificate_bound_registration_and_possession() {
        let (mut provider, identity, _org, verifier) = cert_bound_provider();
        provider
            .register_own_identity(&identity, "PharmaCorp", &verifier, 5)
            .expect("certificate-bound registration");

        let expression = PolicyExpression::signed_by("PharmaCorp");
        // Valid at and after the registration height.
        assert!(
            provider
                .evaluate(
                    &expression,
                    &request(b"canonical-payload", vec![signer(&identity, "PharmaCorp")]),
                    5
                )
                .expect("valid signer")
                .satisfied
        );
        // Not valid before it (a replay of the key at an earlier height).
        let error = provider
            .evaluate(
                &expression,
                &request(b"canonical-payload", vec![signer(&identity, "PharmaCorp")]),
                4,
            )
            .expect_err("not yet authorized");
        assert!(error.to_string().contains("not authorized"), "{error}");
    }

    #[test]
    fn test_certificate_registration_rejects_wrong_org_and_bad_proof() {
        let (mut provider, identity, _org, verifier) = cert_bound_provider();
        // Wrong org: the certificate's subject must equal the claimed org.
        let error = provider
            .register_own_identity(&identity, "OtherCorp", &verifier, 0)
            .expect_err("wrong org must be rejected");
        assert!(
            matches!(error, MspRegistrationError::OrgMismatch { .. }),
            "{error}"
        );

        // Proof by a different key does not register the certificate's key.
        let mut impostor = Organization::new("OtherCorp").unwrap();
        let impostor = impostor.issue_identity("PharmaCorp").unwrap().clone();
        let cert_pem = identity.certificate_pem.clone().unwrap();
        let forged = impostor.sign_bytes(&msp_registration_message(
            "PharmaCorp",
            &identity.public_key_bytes(),
        ));
        let error = provider
            .register_certificate(&cert_pem, &forged, "PharmaCorp", &verifier, 0)
            .expect_err("a proof from another key must be rejected");
        assert!(
            matches!(error, MspRegistrationError::ProofInvalid),
            "{error}"
        );
    }

    #[test]
    fn test_revocation_is_go_forward_only() {
        let (mut provider, identity, _org, verifier) = cert_bound_provider();
        provider
            .register_own_identity(&identity, "PharmaCorp", &verifier, 5)
            .expect("registration");
        assert!(provider.revoke(&identity.public_key_bytes(), 20));

        let expression = PolicyExpression::signed_by("PharmaCorp");
        // Committed history before the revocation height still verifies.
        assert!(
            provider
                .evaluate(
                    &expression,
                    &request(b"canonical-payload", vec![signer(&identity, "PharmaCorp")]),
                    10
                )
                .expect("historical authorization stays valid")
                .satisfied
        );
        // New authorization at or after the revocation height is refused.
        let error = provider
            .evaluate(
                &expression,
                &request(b"canonical-payload", vec![signer(&identity, "PharmaCorp")]),
                20,
            )
            .expect_err("revoked key must be refused for new authorization");
        assert!(error.to_string().contains("revoked"), "{error}");

        // Unknown keys cannot be revoked.
        assert!(!provider.revoke(&[9u8; 32], 20));
    }
}
