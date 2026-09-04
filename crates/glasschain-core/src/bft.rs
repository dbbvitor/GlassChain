use crate::consensus::{CommitNotification, QuorumCertificate};
use crate::error::CoreError;
use crate::providers::ConsensusProvider;
use crate::transaction::Transaction;
use crate::Block;
use bls_signatures::{PrivateKey, PublicKey, Serialize as BlsSerialize, Signature};

/// One validator in the BFT validator set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorInfo {
    /// Validator identifier (MSP principal).
    pub name: String,
    /// BLS12-381 public key (G1, 48 bytes) — used only for quorum-certificate
    /// aggregation; transaction and identity signatures stay ed25519 (ADR-014).
    pub public_key: Vec<u8>,
    /// Proof of possession: an individual BLS signature over
    /// `"glasschain-bls-pop:<hex(public_key)>"`, verified at registration.
    /// Rogue-key defense for plain n-of-n aggregation (ADR-014 decision 4).
    pub pop: Vec<u8>,
}

impl ValidatorInfo {
    /// The distinct message a validator's proof of possession must sign.
    fn pop_message(&self) -> String {
        format!(
            "glasschain-bls-pop:{}",
            hex::encode(self.public_key.as_slice())
        )
    }
}

/// The Tendermint-class BFT consensus provider.
///
/// Holds the validator set against which quorum certs are produced and verified
/// plus the local proposer's BLS signing key. The validator set is static
/// configuration until the ADR-009 rotation machinery lands with the BFT
/// adoption gate (ADR-010).
pub struct BftConsensusProvider {
    /// Validators in canonical index order — the order the certificate bitmap
    /// addresses.
    validators: Vec<ValidatorInfo>,
    /// The local proposer's BLS signing key.
    signing_key: PrivateKey,
}

impl BftConsensusProvider {
    /// Build a provider over `validators`, signing with `signing_key`.
    ///
    /// Every validator's proof of possession is verified at registration:
    /// plain n-of-n aggregation is rogue-key-vulnerable without it (ADR-014
    /// decision 4), and one invalid key corrupts every aggregate it joins.
    ///
    /// The signing key should belong to one of `validators`; an outsider key
    /// still produces attestations, but [`Self::verify_certificate`] rejects
    /// them as degenerate (fail-closed).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] when a validator's key or proof of
    /// possession is malformed or does not verify.
    pub fn new(validators: Vec<ValidatorInfo>, signing_key: PrivateKey) -> Result<Self, CoreError> {
        for validator in &validators {
            if validator.public_key.len() != 48 {
                return Err(CoreError::InvalidBlock(format!(
                    "bft: validator '{}' has a {}-byte BLS public key (expected 48)",
                    validator.name,
                    validator.public_key.len()
                )));
            }
            let public = PublicKey::from_bytes(validator.public_key.as_slice()).map_err(|e| {
                CoreError::InvalidBlock(format!(
                    "bft: validator '{}' has an invalid BLS public key: {e}",
                    validator.name
                ))
            })?;
            let pop = Signature::from_bytes(validator.pop.as_slice()).map_err(|e| {
                CoreError::InvalidBlock(format!(
                    "bft: validator '{}' has an invalid proof of possession: {e}",
                    validator.name
                ))
            })?;
            if !public.verify(pop, validator.pop_message()) {
                return Err(CoreError::InvalidBlock(format!(
                    "bft: validator '{}' failed its proof of possession (rogue-key defense)",
                    validator.name
                )));
            }
        }
        Ok(Self {
            validators,
            signing_key,
        })
    }

    /// The local proposer's BLS public key bytes.
    #[must_use]
    pub fn public_key(&self) -> Vec<u8> {
        self.signing_key.public_key().as_bytes()
    }

    /// The validator set size.
    #[must_use]
    pub const fn validator_count(&self) -> usize {
        self.validators.len()
    }

    /// ⅔ of the validator set, rounded up — the quorum threshold.
    #[must_use]
    pub const fn quorum(&self) -> usize {
        self.validators.len() * 2 / 3 + 1
    }

    /// The bitmap index of the local proposer, if it is in the validator set.
    fn local_index(&self) -> Option<usize> {
        let local = self.signing_key.public_key().as_bytes();
        self.validators
            .iter()
            .position(|validator| validator.public_key == local)
    }

    /// Attest `block`: sign its hash with the local BLS key. A one-validator
    /// set is its own quorum; multi-validator vote gathering over the network
    /// is the ADR-010 testnet adoption gate — add a round driver there, then
    /// aggregate the collected signatures into the certificate here.
    ///
    /// # Panics
    ///
    /// Never in practice: `calculate_hash` cannot fail for JSON-serializable
    /// blocks and `PrivateKey::sign` is infallible.
    #[must_use]
    pub fn attest(&self, mut block: Block) -> CommitNotification {
        block.hash = block.calculate_hash();
        let signature = self.signing_key.sign(block.hash.as_bytes());
        let mut signers_bitmap = vec![0u8; self.validators.len().div_ceil(8)];
        if let Some(index) = self.local_index() {
            signers_bitmap[index / 8] |= 1 << (index % 8);
        }
        let certificate = QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            signers_bitmap,
            aggregate_signature: signature.as_bytes(),
            algorithm: crate::wire::SignatureAlgorithm::Bls12381,
        };
        CommitNotification { block, certificate }
    }

    /// Verify a quorum certificate: the bitmap must name a quorum of known
    /// validators and the aggregate must verify against their keys in one
    /// pairing check (ADR-014).
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

        // Bitmap: bit i = validators[i]. Bits beyond the set are malformed.
        let signer_bits: Vec<usize> = certificate
            .signers_bitmap
            .iter()
            .enumerate()
            .flat_map(|(byte, bits)| (0..8).map(move |bit| (byte * 8 + bit, bits >> bit & 1 == 1)))
            .filter(|(_, set)| *set)
            .map(|(index, _)| index)
            .collect();
        if signer_bits.len() < self.quorum() {
            return Err(CoreError::InvalidBlock(format!(
                "bft: quorum {} not reached ({} validators in bitmap)",
                self.quorum(),
                signer_bits.len()
            )));
        }
        if signer_bits
            .iter()
            .any(|index| *index >= self.validators.len())
        {
            return Err(CoreError::InvalidBlock(
                "bft: certificate bitmap names validators outside the set".into(),
            ));
        }

        let public_keys: Vec<[u8; 48]> = signer_bits
            .iter()
            .filter_map(|index| self.validators.get(*index))
            .map(|validator| {
                let mut key = [0u8; 48];
                key.copy_from_slice(validator.public_key.as_slice());
                key
            })
            .collect();

        let aggregate =
            Signature::from_bytes(certificate.aggregate_signature.as_slice()).map_err(|e| {
                CoreError::InvalidBlock(format!("bft: invalid BLS aggregate signature: {e}"))
            })?;
        let block_hash = bls_signatures::hash(block.hash.as_bytes());
        if !verify_same_message_multisig(&aggregate, &public_keys, &block_hash) {
            return Err(CoreError::InvalidBlock(format!(
                "bft: aggregate signature does not verify over block {} ({} signers)",
                block.index,
                signer_bits.len()
            )));
        }
        Ok(())
    }
}

/// The IETF `PopScheme` multisig check (ADR-014): every signer's key is
/// individually proof-of-possessed, so the same-message aggregate verifies as
/// `e(-G1, agg_sig) * prod_i e(pk_i, hash) == identity`.
///
/// `bls-signatures`' pure-Rust backend only ships the *distinct-message*
/// aggregate verify (it enforces message uniqueness as its rogue-key
/// countermeasure); the same-message form is what a quorum certificate needs,
/// and proof-of-possession replaces the uniqueness requirement.
fn verify_same_message_multisig(
    aggregate: &Signature,
    public_keys: &[[u8; 48]],
    hash: &bls12_381::G2Projective,
) -> bool {
    use bls12_381::{multi_miller_loop, G1Affine, G2Affine, G2Prepared, Gt};

    let signature = G2Affine::from(*aggregate);
    let g1_neg = -G1Affine::generator();
    let hash_prepared = G2Prepared::from(G2Affine::from(hash));

    let mut terms = vec![(g1_neg, G2Prepared::from(signature))];
    for key in public_keys {
        let parsed = G1Affine::from_compressed(key);
        let Some(pk) = <Option<G1Affine>>::from(parsed) else {
            return false;
        };
        terms.push((pk, hash_prepared.clone()));
    }
    let refs: Vec<(&G1Affine, &G2Prepared)> = terms.iter().map(|(a, b)| (a, b)).collect();
    multi_miller_loop(&refs).final_exponentiation() == Gt::identity()
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
        // Structural chaining only: certificate verification runs wherever a
        // certificate is available (`verify_certificate`), and peer-path BFT
        // admission is the ADR-010 adoption-gate work.
        block.chains_to(previous).map_err(|e| {
            CoreError::InvalidBlock(format!(
                "bft: candidate block {} does not chain to {}: {e}",
                block.index, previous.index
            ))
        })
    }

    fn name(&self) -> &'static str {
        "bft"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;

    /// `count` validators with deterministic BLS keys and valid proofs of
    /// possession, plus the matching signing keys.
    fn provider(count: usize) -> (BftConsensusProvider, Vec<PrivateKey>) {
        let mut validators = Vec::new();
        let mut keys = Vec::new();
        for i in 0..u8::try_from(count).expect("test validator count fits u8") {
            let secret = PrivateKey::new([i + 1; 64]);
            let public = secret.public_key();
            let pop = secret.sign(format!(
                "glasschain-bls-pop:{}",
                hex::encode(public.as_bytes())
            ));
            validators.push(ValidatorInfo {
                name: format!("validator-{i}"),
                public_key: public.as_bytes(),
                pop: pop.as_bytes(),
            });
            keys.push(secret);
        }
        (
            BftConsensusProvider::new(validators, keys[0]).expect("valid validators"),
            keys,
        )
    }

    /// A certificate signed by every key in `keys` over `block`'s hash, with
    /// `signers` bitmap positions set.
    fn aggregated_certificate(
        block: &Block,
        keys: &[PrivateKey],
        signers: &[usize],
    ) -> QuorumCertificate {
        let signatures: Vec<Signature> = keys
            .iter()
            .map(|key| key.sign(block.hash.as_bytes()))
            .collect();
        let mut signers_bitmap = vec![0u8; keys.len().div_ceil(8)];
        for &index in signers {
            signers_bitmap[index / 8] |= 1 << (index % 8);
        }
        QuorumCertificate {
            block_index: block.index,
            block_hash: block.hash.clone(),
            signers_bitmap,
            aggregate_signature: bls_signatures::aggregate(&signatures)
                .expect("aggregate")
                .as_bytes(),
            algorithm: crate::wire::SignatureAlgorithm::Bls12381,
        }
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
    fn test_aggregated_quorum_verifies_in_one_pairing() {
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let count = 10;
        let (provider, keys) = provider(count);
        let block = Block::with_write_set(1, vec![], genesis.hash, Vec::new());
        let mut block = block;
        block.hash = block.calculate_hash();

        // All 10 sign; the certificate carries one aggregate.
        let certificate = aggregated_certificate(&block, &keys, &(0..count).collect::<Vec<_>>());
        assert!(provider.verify_certificate(&certificate, &block).is_ok());

        // 2-of-10 is below the ⅔ quorum.
        let below = aggregated_certificate(&block, &keys, &[0, 1]);
        let error = provider
            .verify_certificate(&below, &block)
            .expect_err("below-quorum certificates must be rejected");
        assert!(error.to_string().contains("quorum"), "{error}");
    }

    #[test]
    fn test_bitmap_outside_the_set_is_rejected() {
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let (provider, keys) = provider(3);
        let mut block = Block::with_write_set(1, vec![], genesis.hash, Vec::new());
        block.hash = block.calculate_hash();
        let mut certificate = aggregated_certificate(&block, &keys, &[0, 1, 2]);
        certificate.signers_bitmap.push(0b0000_0001);
        let error = provider
            .verify_certificate(&certificate, &block)
            .expect_err("bits beyond the set must be rejected");
        assert!(error.to_string().contains("outside the set"), "{error}");
    }

    #[test]
    fn test_tampered_aggregate_is_rejected() {
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let (provider, keys) = provider(4);
        let mut block = Block::with_write_set(1, vec![], genesis.hash, Vec::new());
        block.hash = block.calculate_hash();
        let mut certificate = aggregated_certificate(&block, &keys, &[0, 1, 2, 3]);
        // A decode-valid aggregate over the WRONG message: flips land in the
        // subgroup check at decode ("Group decode error") — this exercises
        // the pairing failure instead.
        let wrong = keys[0].sign("a different message entirely");
        certificate.aggregate_signature = wrong.as_bytes();
        let error = provider
            .verify_certificate(&certificate, &block)
            .expect_err("a tampered aggregate must be rejected");
        assert!(error.to_string().contains("does not verify"), "{error}");
    }

    #[test]
    fn test_registration_rejects_invalid_proof_of_possession() {
        // The rogue-key defense (ADR-014 decision 4): a validator whose PoP
        // does not verify is rejected at registration, before it can join any
        // aggregate.
        let secret = PrivateKey::new([9; 64]);
        let public = secret.public_key();
        let impostors = vec![ValidatorInfo {
            name: "impostor".into(),
            public_key: public.as_bytes(),
            // A PoP over the WRONG message.
            pop: secret.sign("glasschain-bls-pop:other").as_bytes(),
        }];
        let Err(error) = BftConsensusProvider::new(impostors, secret) else {
            panic!("an invalid PoP must be rejected at registration");
        };
        assert!(error.to_string().contains("proof of possession"), "{error}");
    }

    #[test]
    fn test_outside_proposer_fails_closed() {
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let (provider, _) = provider(2);
        let outsider = PrivateKey::new([200; 64]);
        let outsider_provider = {
            let validators = vec![ValidatorInfo {
                name: "validator-0".into(),
                public_key: outsider.public_key().as_bytes(),
                pop: outsider
                    .sign(format!(
                        "glasschain-bls-pop:{}",
                        hex::encode(outsider.public_key().as_bytes())
                    ))
                    .as_bytes(),
            }];
            BftConsensusProvider::new(validators, outsider).expect("valid validators")
        };
        let _ = provider;
        let notification =
            outsider_provider.attest(Block::with_write_set(1, vec![], genesis.hash, Vec::new()));
        // The outsider IS a one-validator set in its own provider; a mixed
        // set is what fails closed. Verify against the REAL set: the
        // outsider's certificate carries an empty bitmap there.
        assert!(!notification.certificate.is_degenerate());
        let error = provider
            .verify_certificate(&notification.certificate, &notification.block)
            .expect_err("an outsider's certificate must not verify in the real set");
        assert!(
            error.to_string().contains("outside the set") || error.to_string().contains("quorum"),
            "{error}"
        );
    }
}
