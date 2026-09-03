//! Quorum certificates on the consensus seam (ADR-002, ticket #38).
//!
//! A [`CommitNotification`] is what the consensus seam hands every commit
//! consumer: the committed block plus the [`QuorumCertificate`] attesting it.
//! No consumer may depend on "the leader said so" — a verifying member checks
//! the certificate against the block.
//!
//! The retained Proof-of-Work dev/test provider supplies a **degenerate**
//! certificate: `PoW`'s attestation *is* the mined nonce, which the block itself
//! carries, so the certificate carries an empty attestation set and validates
//! structurally against the block. Real BFT attestations land with ticket #42.

use crate::block::Block;
use crate::error::CoreError;
use crate::wire::{base64_bytes, SignatureAlgorithm};
use serde::{Deserialize, Serialize};

/// One validator attestation over a block hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    /// Validator identifier (MSP principal).
    pub validator: String,
    /// Raw 32-byte ed25519 public key of the validator, base64 on the wire.
    #[serde(with = "base64_bytes")]
    pub public_key: Vec<u8>,
    /// Signature over the block hash (cryptographic verification lands with
    /// the BFT engine, ticket #42), base64 on the wire.
    #[serde(with = "base64_bytes")]
    pub signature: Vec<u8>,
    /// The algorithm that produced `signature` (post-quantum plan action 2).
    #[serde(
        default,
        skip_serializing_if = "crate::wire::SignatureAlgorithm::is_ed25519"
    )]
    pub algorithm: SignatureAlgorithm,
}

/// The attestation set for a committed block (ADR-002: quorum certificate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumCertificate {
    /// Index of the block this certificate attests.
    pub block_index: u64,
    /// Hash of the attested block.
    pub block_hash: String,
    /// Validator attestations; empty for the degenerate Proof-of-Work
    /// certificate (the valid nonce is the `PoW` attestation).
    pub attestations: Vec<Attestation>,
}

impl QuorumCertificate {
    /// The degenerate certificate for a Proof-of-Work block: `PoW`'s attestation
    /// is the mined nonce carried by the block itself.
    #[must_use]
    pub fn pow(block: &Block) -> Self {
        Self {
            block_index: block.index,
            block_hash: block.hash.clone(),
            attestations: Vec::new(),
        }
    }

    /// `true` when this is the degenerate Proof-of-Work certificate.
    #[must_use]
    pub const fn is_degenerate(&self) -> bool {
        self.attestations.is_empty()
    }

    /// Structural validation against `block`: the certificate must name this
    /// block's index and hash, and every attestation must be well-formed
    /// (non-empty validator, 32-byte key, non-empty signature). Cryptographic
    /// verification of the attestations lands with the BFT engine (#42).
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
        for attestation in &self.attestations {
            if attestation.validator.is_empty() {
                return Err(CoreError::InvalidBlock(
                    "quorum certificate: attestation validator must not be empty".into(),
                ));
            }
            if attestation.public_key.len() != 32 {
                return Err(CoreError::InvalidBlock(format!(
                    "quorum certificate: attestation from '{}' has a {}-byte key (expected 32)",
                    attestation.validator,
                    attestation.public_key.len()
                )));
            }
            if attestation.signature.len() != 64 {
                return Err(CoreError::InvalidBlock(format!(
                    "quorum certificate: attestation from '{}' has a {}-byte signature (expected 64)",
                    attestation.validator,
                    attestation.signature.len()
                )));
            }
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
            attestations: Vec::new(),
        };
        assert!(certificate.validate(&block).is_err());
    }

    #[test]
    fn test_attestation_set_structural_rules() {
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        let well_formed = Attestation {
            algorithm: crate::wire::SignatureAlgorithm::Ed25519,
            validator: "org-a".into(),
            public_key: vec![0x42; 32],
            signature: vec![0x24; 64],
        };
        let good = QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            attestations: vec![well_formed.clone()],
        };
        assert!(good.validate(&block).is_ok());

        let bad_key = QuorumCertificate {
            attestations: vec![Attestation {
                public_key: vec![0x42; 31],
                ..well_formed.clone()
            }],
            ..good.clone()
        };
        assert!(bad_key.validate(&block).is_err());

        let empty_sig = QuorumCertificate {
            attestations: vec![Attestation {
                signature: Vec::new(),
                ..well_formed
            }],
            ..good
        };
        assert!(empty_sig.validate(&block).is_err());
    }

    #[cfg(feature = "bft")]
    #[test]
    fn test_quorum_certificate_size_budget_at_201_attestations() {
        // #62 Step 1 validation: a synthetic 201-of-300 quorum certificate
        // must stay under the 40 KB base64 budget (was ~79 KB as decimal
        // arrays). The measured 11.5 KB block is the comparison point.
        let mut seed = 0u8;
        let mut keys = Vec::new();
        let mut validators = Vec::new();
        for _ in 0..300usize {
            // Repetition of seed bytes is harmless for the size budget —
            // only the attestation count matters.
            seed = seed.wrapping_add(1);
            let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
            validators.push(crate::bft::ValidatorInfo {
                name: format!("validator-{}", keys.len() + 1),
                public_key: key.verifying_key().to_bytes().to_vec(),
            });
            keys.push(key);
        }
        let provider = crate::bft::BftConsensusProvider::new(validators, keys[0].clone());
        let mut ledger = Ledger::new(1);
        let block = ledger.mine_pending_transactions().expect("mine").clone();
        let notification = provider.attest(block.clone());
        let mut certificate = notification.certificate;
        certificate.attestations.clear();
        for key in keys.iter().take(201) {
            use ed25519_dalek::Signer as _;
            let signature = key.sign(block.hash.as_bytes());
            certificate.attestations.push(Attestation {
                validator: format!("validator-{}", certificate.attestations.len() + 1),
                public_key: key.verifying_key().to_bytes().to_vec(),
                signature: signature.to_bytes().to_vec(),
                algorithm: crate::wire::SignatureAlgorithm::Ed25519,
            });
        }
        assert!(
            certificate.validate(&block).is_ok(),
            "the synthetic quorum must be well-formed"
        );
        let json = serde_json::to_vec(&certificate).expect("serialize");
        assert!(
            json.len() < 40_000,
            "201-attestation certificate is {} B, over the 40 KB budget",
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
