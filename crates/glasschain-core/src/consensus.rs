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
        if self.block_index != block.index {
            return Err(CoreError::InvalidBlock(format!(
                "quorum certificate: block index {} does not match {}",
                self.block_index, block.index
            )));
        }
        if self.block_hash != block.hash {
            return Err(CoreError::InvalidBlock(format!(
                "quorum certificate: block hash mismatch for block {}",
                block.index
            )));
        }
        if self.is_degenerate() {
            return Ok(());
        }
        if self.aggregate_signature.is_empty() {
            return Err(CoreError::InvalidBlock(
                "quorum certificate: non-degenerate certificate carries no aggregate signature"
                    .into(),
            ));
        }
        if self.algorithm != SignatureAlgorithm::Bls12381 {
            return Err(CoreError::InvalidBlock(format!(
                "quorum certificate: aggregate signature algorithm must be Bls12381, got {:?}",
                self.algorithm
            )));
        }
        Ok(())
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
