// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Quorum-certificate types for the consensus seam (ADR-002, ADR-014).
//!
//! A [`QuorumCertificate`] is a BLS12-381 aggregate signature over the block
//! hash from a quorum of validators: constant-size regardless of validator
//! count (ADR-014). The degenerate Proof-of-Work certificate (dev/test engine)
//! carries an empty bitmap — the valid nonce is the block's own attestation.

use crate::error::CoreError;
use crate::wire::{base64_bytes, SignatureAlgorithm};
use crate::Block;
use serde::{Deserialize, Serialize};

/// Certificate-admission logic for [`QuorumCertificate::validate`], proved in
/// production form with Verus (ADR-019). The gate binds a certificate to the
/// block it claims (index and hash) and requires a non-degenerate certificate
/// to carry a non-empty BLS12-381 aggregate; the pairing check itself stays
/// outside the proof (BLS assumed).
mod cert_proofs {
    use crate::pin::str_eq;
    use vstd::prelude::*;

    verus! {

    /// The admission decision for one certificate/block pair.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CertDecision {
        /// The certificate names this block and is structurally complete.
        Accepted,
        /// The certificate's index differs from the block's.
        IndexMismatch,
        /// The certificate's hash differs from the block's.
        HashMismatch,
        /// A non-degenerate certificate carries no aggregate signature.
        MissingAggregate,
        /// A non-degenerate certificate's aggregate is not BLS12-381.
        WrongAlgorithm,
    }

    /// The admission case order, mirroring the shipped checks: index, hash,
    /// degenerate short-circuit, aggregate presence, discriminant.
    pub open spec fn spec_decide(
        block_index: u64,
        block_hash: &str,
        cert_index: u64,
        cert_hash: &str,
        degenerate: bool,
        has_aggregate: bool,
        bls_algorithm: bool,
    ) -> CertDecision {
        if cert_index != block_index {
            CertDecision::IndexMismatch
        } else if !str_eq(cert_hash, block_hash) {
            CertDecision::HashMismatch
        } else if degenerate {
            CertDecision::Accepted
        } else if !has_aggregate {
            CertDecision::MissingAggregate
        } else if !bls_algorithm {
            CertDecision::WrongAlgorithm
        } else {
            CertDecision::Accepted
        }
    }

    /// Decide certificate admission from the extracted certificate and block
    /// fields.
    #[must_use]
    pub fn decide(
        block_index: u64,
        block_hash: &str,
        cert_index: u64,
        cert_hash: &str,
        degenerate: bool,
        has_aggregate: bool,
        bls_algorithm: bool,
    ) -> (decision: CertDecision)
        ensures
            decision == spec_decide(
                block_index,
                block_hash,
                cert_index,
                cert_hash,
                degenerate,
                has_aggregate,
                bls_algorithm,
            ),
    {
        if cert_index != block_index {
            return CertDecision::IndexMismatch;
        }
        if !str_eq(cert_hash, block_hash) {
            return CertDecision::HashMismatch;
        }
        if degenerate {
            return CertDecision::Accepted;
        }
        if !has_aggregate {
            return CertDecision::MissingAggregate;
        }
        if !bls_algorithm {
            return CertDecision::WrongAlgorithm;
        }
        CertDecision::Accepted
    }

    /// The exact acceptance predicate: a certificate is admitted when it
    /// names the block and is either the degenerate PoW form or carries a
    /// non-empty BLS12-381 aggregate.
    pub proof fn acceptance_iff(
        block_index: u64,
        block_hash: &str,
        cert_index: u64,
        cert_hash: &str,
        degenerate: bool,
        has_aggregate: bool,
        bls_algorithm: bool,
    )
        ensures
            spec_decide(
                block_index,
                block_hash,
                cert_index,
                cert_hash,
                degenerate,
                has_aggregate,
                bls_algorithm,
            ) == CertDecision::Accepted <==> (cert_index == block_index && str_eq(
                cert_hash,
                block_hash,
            ) && (degenerate || (has_aggregate && bls_algorithm))),
    {
    }

    /// Acceptance binds the certificate to the block it claims.
    pub proof fn accepted_names_the_block(
        block_index: u64,
        block_hash: &str,
        cert_index: u64,
        cert_hash: &str,
        degenerate: bool,
        has_aggregate: bool,
        bls_algorithm: bool,
    )
        ensures
            spec_decide(
                block_index,
                block_hash,
                cert_index,
                cert_hash,
                degenerate,
                has_aggregate,
                bls_algorithm,
            ) == CertDecision::Accepted ==> cert_index == block_index && str_eq(
                cert_hash,
                block_hash,
            ),
    {
    }

    /// A non-degenerate certificate is only admitted with an aggregate and
    /// the BLS12-381 discriminant — no bitmap-only or mislabelled certificate
    /// passes the structural gate.
    pub proof fn non_degenerate_acceptance_is_complete(
        block_index: u64,
        block_hash: &str,
        cert_index: u64,
        cert_hash: &str,
        has_aggregate: bool,
        bls_algorithm: bool,
    )
        ensures
            spec_decide(
                block_index,
                block_hash,
                cert_index,
                cert_hash,
                false,
                has_aggregate,
                bls_algorithm,
            ) == CertDecision::Accepted ==> has_aggregate && bls_algorithm,
    {
    }

    } // verus!
}

use cert_proofs::CertDecision;

/// A BLS12-381 aggregate signature over a block hash from a quorum of
/// validators (ADR-014).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumCertificate {
    /// Index of the block this certificate attests.
    pub block_index: u64,
    /// Hash of the attested block.
    pub block_hash: String,
    /// Signers as a bitmap over the validator set's canonical order (bit
    /// `i` = `validators[i]`), little-endian bytes. Empty for the degenerate
    /// Proof-of-Work certificate (the valid nonce is the `PoW` attestation).
    #[serde(with = "base64_bytes")]
    pub signers_bitmap: Vec<u8>,
    /// The BLS12-381 aggregate signature over `block_hash`, base64 on the
    /// wire — constant size regardless of how many validators signed.
    #[serde(with = "base64_bytes")]
    pub aggregate_signature: Vec<u8>,
    /// The aggregate-signature algorithm (post-quantum plan action 2).
    #[serde(
        default,
        skip_serializing_if = "crate::wire::SignatureAlgorithm::is_ed25519"
    )]
    pub algorithm: SignatureAlgorithm,
}

impl QuorumCertificate {
    /// The degenerate certificate for a Proof-of-Work block: `PoW`'s attestation
    /// is the mined nonce carried by the block itself.
    #[must_use]
    pub fn pow(block: &Block) -> Self {
        Self {
            block_index: block.index,
            block_hash: block.hash.clone(),
            signers_bitmap: Vec::new(),
            aggregate_signature: Vec::new(),
            algorithm: SignatureAlgorithm::Ed25519,
        }
    }

    /// `true` when this is the degenerate Proof-of-Work certificate.
    #[must_use]
    pub const fn is_degenerate(&self) -> bool {
        self.signers_bitmap.is_empty()
    }

    /// Structural validation against `block`: the certificate must name this
    /// block's index and hash, and a non-degenerate certificate must carry a
    /// BLS aggregate signature and algorithm discriminant. Cryptographic
    /// verification of the aggregate lands with the BFT engine (ADR-014).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] for the first structural mismatch.
    pub fn validate(&self, block: &Block) -> Result<(), CoreError> {
        match cert_proofs::decide(
            block.index,
            block.hash.as_str(),
            self.block_index,
            self.block_hash.as_str(),
            self.is_degenerate(),
            !self.aggregate_signature.is_empty(),
            self.algorithm == SignatureAlgorithm::Bls12381,
        ) {
            CertDecision::Accepted => Ok(()),
            CertDecision::IndexMismatch => Err(CoreError::InvalidBlock(format!(
                "quorum certificate: block index {} does not match {}",
                self.block_index, block.index
            ))),
            CertDecision::HashMismatch => Err(CoreError::InvalidBlock(format!(
                "quorum certificate: block hash mismatch for block {}",
                block.index
            ))),
            CertDecision::MissingAggregate => Err(CoreError::InvalidBlock(
                "quorum certificate: non-degenerate certificate carries no aggregate signature"
                    .into(),
            )),
            CertDecision::WrongAlgorithm => Err(CoreError::InvalidBlock(format!(
                "quorum certificate: aggregate signature algorithm must be Bls12381, got {:?}",
                self.algorithm
            ))),
        }
    }
}

/// A committed block plus the certificate attesting it: the unit every commit
/// consumer receives from the consensus seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitNotification {
    /// The committed block.
    pub block: Block,
    /// The attestation set for `block`.
    pub certificate: QuorumCertificate,
}

impl CommitNotification {
    /// The Proof-of-Work dev/test notification: a degenerate certificate
    /// derived from the block itself.
    #[must_use]
    pub fn for_pow_block(block: Block) -> Self {
        let certificate = QuorumCertificate::pow(&block);
        Self { block, certificate }
    }

    /// Validate that the certificate attests this notification's block.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] when the certificate does not match
    /// the block (see [`QuorumCertificate::validate`]).
    pub fn validate(&self) -> Result<(), CoreError> {
        self.certificate.validate(&self.block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;
    #[cfg(feature = "bft")]
    use bls_signatures::Serialize as _;

    #[test]
    fn test_pow_certificate_is_degenerate_and_validates() {
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        let notification = CommitNotification::for_pow_block(block.clone());
        assert!(notification.certificate.is_degenerate());
        assert!(notification.validate().is_ok());
        assert_eq!(notification.certificate.block_hash, block.hash);
    }

    #[test]
    fn test_certificate_rejects_wrong_block() {
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        // The certificate attests the mined block; a tampered block no longer
        // matches its hash, so the certificate must fail.
        let certificate = QuorumCertificate::pow(&block);
        let mut tampered = block;
        tampered.hash = "deadbeef".into();
        assert!(certificate.validate(&tampered).is_err());
    }

    #[test]
    fn test_notification_validate_propagates_certificate_mismatch() {
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        let certificate = QuorumCertificate::pow(&block);
        let mut tampered = block;
        tampered.hash = "deadbeef".into();
        let notification = CommitNotification {
            block: tampered,
            certificate,
        };
        assert!(
            notification.validate().is_err(),
            "a notification whose certificate does not attest its block must fail"
        );
    }

    #[test]
    fn test_certificate_rejects_wrong_index() {
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        let certificate = QuorumCertificate {
            block_index: block.index + 1,
            block_hash: block.hash.clone(),
            signers_bitmap: Vec::new(),
            aggregate_signature: Vec::new(),
            algorithm: SignatureAlgorithm::Ed25519,
        };
        assert!(certificate.validate(&block).is_err());
    }

    #[test]
    fn test_non_degenerate_certificate_structural_rules() {
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        let well_formed = QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            signers_bitmap: vec![0b0000_0001],
            aggregate_signature: vec![0x24; 96],
            algorithm: SignatureAlgorithm::Bls12381,
        };
        assert!(well_formed.validate(&block).is_ok());

        // An aggregate without the BLS discriminant is rejected: the
        // algorithm field must name the scheme that produced it.
        let mut wrong_alg = well_formed.clone();
        wrong_alg.algorithm = SignatureAlgorithm::Ed25519;
        assert!(wrong_alg.validate(&block).is_err());

        // A bitmap without an aggregate is rejected.
        let mut empty_sig = well_formed;
        empty_sig.aggregate_signature = Vec::new();
        assert!(empty_sig.validate(&block).is_err());
    }

    #[cfg(feature = "bft")]
    #[test]
    fn test_quorum_certificate_size_budget_at_300_signers() {
        // ADR-014 validation: a full 300-of-300 aggregate certificate is
        // constant-size — bitmap + one 96-byte signature, versus ~79 KB of
        // per-validator decimal arrays before Step 1 and aggregation.
        let mut seed = 0u8;
        let mut validators = Vec::new();
        let mut signer_keys = Vec::new();
        for _ in 0..300usize {
            seed = seed.wrapping_add(1);
            let secret = bls_signatures::PrivateKey::new([seed; 64]);
            let public = secret.public_key();
            let pop = secret.sign(format!(
                "glasschain-bls-pop:{}",
                hex::encode(public.as_bytes())
            ));
            validators.push(crate::bft::ValidatorInfo {
                name: format!("validator-{}", validators.len() + 1),
                public_key: public.as_bytes(),
                pop: pop.as_bytes(),
            });
            signer_keys.push(secret);
        }
        let provider = crate::bft::BftConsensusProvider::new(validators, signer_keys[0])
            .expect("valid validators");
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        let notification = provider.attest(block.clone());
        let certificate = notification.certificate;
        assert!(
            certificate.validate(&block).is_ok(),
            "the synthetic certificate must be well-formed"
        );
        let json = serde_json::to_vec(&certificate).expect("serialize");
        assert!(
            json.len() < 1_000,
            "300-signer aggregate certificate is {} B, over the 1 KB constant-size budget",
            json.len()
        );
        let round: QuorumCertificate = serde_json::from_slice(&json).expect("deserialize");
        assert_eq!(round, certificate, "base64 fields must round-trip");
    }

    #[test]
    fn test_notification_roundtrip_serialization() {
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        let notification = CommitNotification::for_pow_block(block);
        let json = serde_json::to_string(&notification).expect("serialize");
        let decoded: CommitNotification = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, notification);
    }
}
