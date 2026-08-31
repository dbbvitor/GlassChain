//! Tendermint-class BFT behind the consensus seam (ticket #42), gated behind the
//! `bft` feature.
//!
//! [`BftConsensusProvider`] is the staged, **default-off** BFT implementation
//! of [`ConsensusProvider`]. It attests blocks with real ed25519 signatures
//! over the block hash and **cryptographically verifies** a
//! [`QuorumCertificate`] against its configured validator set, requiring a ⅔+
//! distinct-validator quorum (ADR-002: a commit consumer never trusts "the
//! leader said so").
//!
//! Staged scope: `attest` signs with the local key, so a produced certificate
//! carries exactly one attestation — a 1-validator set is its own quorum.
//! Gathering attestations from remote validators over the network, wire
//! transport of certificates, and commit-path certificate verification for
//! received/synced blocks are the explicit ADR-010 testnet adoption gates, not
//! part of this delivery.

use crate::block::Block;
use crate::consensus::{Attestation, CommitNotification, QuorumCertificate};
use crate::error::CoreError;
use crate::providers::ConsensusProvider;
use crate::transaction::Transaction;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// One validator in the BFT validator set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorInfo {
    /// Validator identifier (MSP principal).
    pub name: String,
    /// Raw 32-byte ed25519 public key.
    pub public_key: [u8; 32],
}

/// The Tendermint-class BFT consensus provider.
///
/// Holds the validator set against which quorum certs are produced and verified
/// plus the local proposer's ed25519 signing key.
pub struct BftConsensusProvider {
    /// Validators in index order.
    validators: Vec<ValidatorInfo>,
    /// The local proposer's signing key.
    signing_key: SigningKey,
}

impl BftConsensusProvider {
    /// Build a provider over `validators`, signing with `signing_key`.
    ///
    /// The signing key should belong to one of `validators`; an outsider key
    /// still produces attestations, but [`Self::verify_certificate`] rejects
    /// them as unknown validators (fail-closed).
    #[must_use]
    pub const fn new(validators: Vec<ValidatorInfo>, signing_key: SigningKey) -> Self {
        Self {
            validators,
            signing_key,
        }
    }

    /// Minimum number of distinct validators needed for finality (⅔+, rounded up).
    #[must_use]
    pub const fn quorum(&self) -> usize {
        self.validators.len() * 2 / 3 + 1
    }

    /// The local proposer's raw 32-byte public key.
    #[must_use]
    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Produce a block (no `PoW` — finality is the attestation set, not a nonce)
    /// and its real quorum certificate over `block.hash`.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is set to a time before the Unix epoch (via
    /// [`Block::with_write_set`]).
    #[must_use]
    pub fn attest(&self, block: Block) -> CommitNotification {
        let mut block = block;
        block.hash = block.calculate_hash();
        let local = self.signing_key.verifying_key().to_bytes();
        let signature = self.signing_key.sign(block.hash.as_bytes());
        let attestation = Attestation {
            validator: self
                .validators
                .iter()
                .find(|validator| validator.public_key == local)
                .map_or_else(
                    || hex::encode(&local[..8]),
                    |validator| validator.name.clone(),
                ),
            public_key: local.to_vec(),
            signature: signature.to_bytes().to_vec(),
        };
        // ponytail: single local attestation; a 1-validator set is its own
        // quorum. Multi-validator vote gathering over the network is the
        // ADR-010 testnet adoption gate — add a round driver there.
        let certificate = QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            attestations: vec![attestation],
        };
        CommitNotification { block, certificate }
    }

    /// Verify that `certificate` is a valid, non-degenerate ⅔+ quorum over
    /// `block.hash`: every attestation must be a real ed25519 signature over the
    /// block hash by a distinct validator in the set.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] for any structural or cryptographic
    /// mismatch.
    pub fn verify_certificate(
        &self,
        certificate: &QuorumCertificate,
        block: &Block,
    ) -> Result<(), CoreError> {
        certificate.validate(block)?;
        if certificate.is_degenerate() {
            return Err(CoreError::InvalidBlock(
                "bft: degenerate (empty) quorum certificate is not final".into(),
            ));
        }
        let mut distinct = std::collections::HashSet::new();
        for attestation in &certificate.attestations {
            let key_bytes: [u8; 32] = attestation
                .public_key
                .as_slice()
                .try_into()
                .map_err(|_| CoreError::InvalidBlock("bft: non-32-byte public key".into()))?;
            if !self
                .validators
                .iter()
                .any(|validator| validator.public_key == key_bytes)
            {
                return Err(CoreError::InvalidBlock(format!(
                    "bft: attestation from unknown validator '{}'",
                    attestation.validator
                )));
            }
            let verifier = VerifyingKey::from_bytes(&key_bytes).map_err(|_| {
                CoreError::InvalidBlock("bft: invalid ed25519 verifying key".into())
            })?;
            let sig = Signature::from_slice(&attestation.signature)
                .map_err(|_| CoreError::InvalidBlock("bft: invalid ed25519 signature".into()))?;
            verifier.verify(block.hash.as_bytes(), &sig).map_err(|_| {
                CoreError::InvalidBlock(format!(
                    "bft: signature from '{}' does not verify block {}",
                    attestation.validator, block.index
                ))
            })?;
            distinct.insert(key_bytes);
        }
        if distinct.len() < self.quorum() {
            return Err(CoreError::InvalidBlock(format!(
                "bft: quorum {} not reached ({} distinct validators attested)",
                self.quorum(),
                distinct.len()
            )));
        }
        Ok(())
    }
}

impl ConsensusProvider for BftConsensusProvider {
    fn propose_block(
        &self,
        index: u64,
        transactions: Vec<Transaction>,
        previous: &Block,
    ) -> Result<CommitNotification, CoreError> {
        let block = Block::with_write_set(index, transactions, previous.hash.clone(), Vec::new());
        let notification = self.attest(block);
        notification.validate()?;
        Ok(notification)
    }

    fn validate_block(&self, block: &Block, previous: &Block) -> Result<(), CoreError> {
        // The synchronous seam hands `validate_block` no certificate, so a BFT
        // provider can only do the structural (chain + hash) check here. The
        // real, cryptographic ⅔+ quorum verification is
        // [`Self::verify_certificate`], called wherever a certificate is
        // available (e.g. the node-level finality scenario, verifying members).
        // Wire transport of certificates and commit-path verification of
        // received/synced blocks are ADR-010 adoption-gate work; the
        // certificate travels with the commit notification, not the bare block.
        block.chains_to(previous)
    }

    fn name(&self) -> &'static str {
        "tendermint-bft"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;
    use ed25519_dalek::SigningKey;

    fn key(seed: usize) -> SigningKey {
        let byte = u8::try_from(seed).expect("seed fits u8");
        SigningKey::from_bytes(&[byte; 32])
    }

    fn provider(count: usize) -> (BftConsensusProvider, Vec<SigningKey>) {
        let keys: Vec<SigningKey> = (0..count).map(|i| key(i + 1)).collect();
        let validators = keys
            .iter()
            .enumerate()
            .map(|(i, k)| ValidatorInfo {
                name: format!("validator-{i}"),
                public_key: k.verifying_key().to_bytes(),
            })
            .collect();
        (BftConsensusProvider::new(validators, keys[0].clone()), keys)
    }

    #[test]
    fn test_propose_produces_real_quorum_certificate() {
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let (provider, _) = provider(1);
        let notification = provider
            .propose_block(1, vec![], &genesis)
            .expect("propose");
        assert!(!notification.certificate.is_degenerate());
        assert_eq!(notification.certificate.block_index, 1);
        assert_eq!(notification.certificate.block_hash, notification.block.hash);
        assert!(notification.validate().is_ok());
        assert!(provider
            .verify_certificate(&notification.certificate, &notification.block)
            .is_ok());
    }

    #[test]
    fn test_quorum_threshold() {
        let (p1, _) = provider(1);
        assert_eq!(p1.quorum(), 1);
        let (p3, _) = provider(3);
        assert_eq!(p3.quorum(), 3);
        let (p4, _) = provider(4);
        assert_eq!(p4.quorum(), 3);
    }

    #[test]
    fn test_provider_name() {
        let (provider, _) = provider(1);
        assert_eq!(provider.name(), "tendermint-bft");
    }

    #[test]
    fn test_verify_rejects_unknown_validator() {
        let (provider, _keys) = provider(3);
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let block = Block::with_write_set(1, vec![], genesis.hash.clone(), Vec::new());
        let outsider = key(0xFF);
        let mut certificate = QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            attestations: Vec::new(),
        };
        for _ in 0..provider.quorum() {
            certificate.attestations.push(Attestation {
                validator: format!("outsider-{}", outsider.verifying_key().to_bytes()[0]),
                public_key: outsider.verifying_key().to_bytes().to_vec(),
                signature: outsider.sign(block.hash.as_bytes()).to_bytes().to_vec(),
            });
        }
        let err = provider
            .verify_certificate(&certificate, &block)
            .unwrap_err();
        assert!(err.to_string().contains("unknown validator"), "{err}");
        assert!(provider.validate_block(&block, &genesis).is_err());
    }

    #[test]
    fn test_verify_rejects_wrong_signature() {
        let (provider, keys) = provider(3);
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let block = Block::with_write_set(1, vec![], genesis.hash, Vec::new());
        // Attest with a validator's key but sign the *wrong* bytes.
        let mut certificate = QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            attestations: Vec::new(),
        };
        for (i, key) in keys.iter().take(provider.quorum()).enumerate() {
            let bad_bytes = format!("wrong-for-block-{i}");
            certificate.attestations.push(Attestation {
                validator: format!("validator-{i}"),
                public_key: key.verifying_key().to_bytes().to_vec(),
                signature: key.sign(bad_bytes.as_bytes()).to_bytes().to_vec(),
            });
        }
        let err = provider
            .verify_certificate(&certificate, &block)
            .unwrap_err();
        assert!(err.to_string().contains("does not verify"), "{err}");
    }

    #[test]
    fn test_verify_rejects_under_quorum() {
        let (provider, keys) = provider(4);
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let block = Block::with_write_set(1, vec![], genesis.hash, Vec::new());
        let mut certificate = QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            attestations: Vec::new(),
        };
        // Only two distinct validators attest, but quorum(4) = 3.
        for key in keys.iter().take(2) {
            certificate.attestations.push(Attestation {
                validator: format!("v-{}", key.verifying_key().to_bytes()[0]),
                public_key: key.verifying_key().to_bytes().to_vec(),
                signature: key.sign(block.hash.as_bytes()).to_bytes().to_vec(),
            });
        }
        let err = provider
            .verify_certificate(&certificate, &block)
            .unwrap_err();
        assert!(err.to_string().contains("quorum"), "{err}");
    }

    #[test]
    fn test_verify_rejects_duplicate_attestations() {
        let (provider, keys) = provider(3);
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let block = Block::with_write_set(1, vec![], genesis.hash, Vec::new());
        // The same validator attests three times over the correct hash: distinct
        // count is still 1, below quorum(3) — duplicates never inflate quorum.
        let key = &keys[0];
        let attestation = Attestation {
            validator: "validator-0".into(),
            public_key: key.verifying_key().to_bytes().to_vec(),
            signature: key.sign(block.hash.as_bytes()).to_bytes().to_vec(),
        };
        let certificate = QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            attestations: vec![attestation.clone(), attestation.clone(), attestation],
        };
        let err = provider
            .verify_certificate(&certificate, &block)
            .unwrap_err();
        assert!(err.to_string().contains("quorum"), "{err}");
    }
}
