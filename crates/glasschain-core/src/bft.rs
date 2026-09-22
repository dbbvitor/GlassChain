// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
#[cfg(feature = "bft")]
use crate::consensus::{CommitNotification, QuorumCertificate};
#[cfg(feature = "bft")]
use crate::error::CoreError;
use serde::{Deserialize, Serialize};

#[cfg(feature = "bft")]
use bls_signatures::{PrivateKey, PublicKey, Serialize as BlsSerialize, Signature};

#[cfg(feature = "bft")]
use crate::providers::ConsensusProvider;
#[cfg(feature = "bft")]
use crate::transaction::Transaction;
#[cfg(feature = "bft")]
use crate::Block;
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

#[cfg(feature = "bft")]
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
#[cfg(feature = "bft")]
#[derive(Clone)]
pub struct BftConsensusProvider {
    /// Validators in canonical index order — the order the certificate bitmap
    /// addresses.
    validators: Vec<ValidatorInfo>,
    /// The local proposer's BLS signing key.
    signing_key: PrivateKey,
}

#[cfg(feature = "bft")]
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

    /// The local proposer's BLS signing key — the round driver rebuilds the
    /// provider when the on-chain validator registry changes (Q34) while the
    /// node's own key stays fixed.
    #[must_use]
    pub const fn signing_key(&self) -> &PrivateKey {
        &self.signing_key
    }

    /// The canonical validator order (name, key bytes) — the bitmap's index
    /// space.
    #[must_use]
    pub fn validators(&self) -> &[ValidatorInfo] {
        &self.validators
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
        let signature = self.signing_key.sign(BftVote::vote_message(&block.hash));
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

    /// Sign a vote for the current round driver (phase-tagged, set-checked on
    /// receipt by [`Self::verify_vote`]). `chain_id` is the genesis block
    /// hash — deterministic across nodes and already compared on chain
    /// replacement (#95: votes bind chain/height/round/phase).
    #[must_use]
    pub fn sign_vote(
        &self,
        chain_id: &str,
        height: u64,
        round: u32,
        phase: VotePhase,
        block_hash: &str,
    ) -> BftVote {
        BftVote::sign(
            chain_id,
            height,
            round,
            phase,
            block_hash,
            &self.signing_key,
        )
    }

    /// Verify a vote against this validator set: self-verification plus
    /// membership — the voter's key must belong to a set validator. Returns
    /// the voter's bitmap index.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] when the vote does not verify or
    /// the voter is not in the set.
    pub fn verify_vote(&self, vote: &BftVote) -> Result<usize, CoreError> {
        vote.verify()?;
        self.validators
            .iter()
            .position(|validator| validator.public_key == vote.public_key)
            .ok_or_else(|| {
                CoreError::InvalidBlock("bft: vote from a validator outside the set".into())
            })
    }

    /// Verify every vote, check membership, and aggregate: returns the signer
    /// bitmap over the set's canonical order plus the aggregate signature —
    /// the material a [`QuorumCertificate`] is built from. Duplicate voters
    /// are collapsed; every signature is still verified.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] when any vote fails verification.
    pub fn aggregate_votes(&self, votes: &[BftVote]) -> Result<(Vec<u8>, Vec<u8>), CoreError> {
        let mut signers_bitmap = vec![0u8; self.validators.len().div_ceil(8)];
        let mut signatures: Vec<Signature> = Vec::with_capacity(votes.len());
        for vote in votes {
            let index = self.verify_vote(vote)?;
            if signers_bitmap[index / 8] & (1 << (index % 8)) == 0 {
                signers_bitmap[index / 8] |= 1 << (index % 8);
                signatures.push(
                    Signature::from_bytes(vote.signature.as_slice()).map_err(|e| {
                        CoreError::InvalidBlock(format!("bft: invalid vote signature: {e}"))
                    })?,
                );
            }
        }
        let aggregate = bls_signatures::aggregate(&signatures)
            .map_err(|e| CoreError::InvalidBlock(format!("bft: vote aggregation failed: {e}")))?;
        Ok((signers_bitmap, aggregate.as_bytes()))
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
        let block_hash = bls_signatures::hash(BftVote::vote_message(&block.hash).as_slice());
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
/// `e(-G1, agg_sig) · prod_i e(pk_i, hash) == identity`. By bilinearity the
/// signer keys collapse into one G1 sum — the entire check is **two pairing
/// terms regardless of quorum size**, which is what unblocks the
/// 300-validator gate (ADR-015, issue #85: the pure-Rust backend failed on
/// the 299 × 202-term precommit re-verification herd).
///
/// `bls-signatures` ships only the *distinct-message* aggregate verify (it
/// enforces message uniqueness as its rogue-key countermeasure); the
/// same-message form is what a quorum certificate needs, and
/// proof-of-possession replaces the uniqueness requirement.
#[cfg(feature = "bft")]
fn verify_same_message_multisig(
    aggregate: &Signature,
    public_keys: &[[u8; 48]],
    hash: &blstrs::G2Projective,
) -> bool {
    use blstrs::{pairing, G1Affine, G1Projective, G2Affine, Gt};
    use group::prime::PrimeCurveAffine as _;
    use group::{Curve as _, Group as _};

    let mut signer_keys = G1Projective::identity();
    for key in public_keys {
        let parsed = G1Affine::from_compressed(key);
        let Some(pk) = <Option<G1Affine>>::from(parsed) else {
            return false;
        };
        signer_keys += pk;
    }

    let signature_bytes: [u8; 96] = match aggregate.as_bytes().try_into() {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let Some(signature) = <Option<G2Affine>>::from(G2Affine::from_compressed(&signature_bytes))
    else {
        return false;
    };
    let Some(hash) =
        <Option<G2Affine>>::from(G2Affine::from_compressed(&hash.to_affine().to_compressed()))
    else {
        return false;
    };

    // e(-G1, agg_sig) · e(sum_pk, hash) == identity.
    let lhs = pairing(&-G1Affine::generator(), &signature);
    let rhs = pairing(&G1Affine::from(signer_keys), &hash);
    lhs + rhs == Gt::identity()
}

#[cfg(feature = "bft")]
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

/// The two phases of a BFT round (ADR-002 adoption-gate build): a validator
/// prevotes a candidate hash, and — once a valid prevote quorum exists —
/// precommits it. Commit happens on a precommit quorum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum VotePhase {
    #[default]
    Prevote,
    Precommit,
}

impl VotePhase {
    /// Routing metadata: which phase the vote belongs to. Its byte is part of
    /// the sealed context envelope ([`BftVote::context_message`], #95) and
    /// phase-appropriate message flow (a prevote quorum justifies precommits)
    /// is enforced on top of the envelope binding.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Prevote => 0,
            Self::Precommit => 1,
        }
    }
}

/// One validator's BLS vote over a candidate block hash at `(height, round)`.
///
/// Every vote carries two signatures (#95): `signature` over the candidate
/// hash — the aggregate material an ADR-014 certificate is built from — and
/// `context_signature` over the chain/height/round/phase envelope
/// ([`BftVote::context_message`]). The legacy hash-only format (no context
/// signature) is no longer accepted (#99): a vote must bind its consensus
/// context to be verifiable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BftVote {
    /// Height being voted on.
    pub height: u64,
    /// Round within the height (increments on timeout — view change).
    pub round: u32,
    /// The phase this vote belongs to.
    pub phase: VotePhase,
    /// The candidate block hash being voted for.
    pub block_hash: String,
    /// The chain this vote belongs to: the genesis block hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chain_id: String,
    /// The voter's BLS public key (G1, 48 bytes).
    #[serde(with = "crate::wire::base64_bytes")]
    pub public_key: Vec<u8>,
    /// BLS signature over the candidate-hash vote message
    /// ([`BftVote::vote_message`]), base64 on the wire.
    #[serde(with = "crate::wire::base64_bytes")]
    pub signature: Vec<u8>,
    /// BLS signature over the context envelope
    /// ([`BftVote::context_message`]).
    #[serde(
        default,
        with = "crate::wire::base64_bytes",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub context_signature: Vec<u8>,
    /// The vote-signature algorithm (post-quantum plan action 2).
    #[serde(
        default,
        skip_serializing_if = "crate::wire::SignatureAlgorithm::is_ed25519"
    )]
    pub algorithm: crate::wire::SignatureAlgorithm,
}

#[cfg(feature = "bft")]
impl BftVote {
    /// The exact message a vote signature commits to: the candidate hash,
    /// domain-separated from every other signature purpose in the system.
    /// Height/round/phase are routing metadata; the aggregate over these
    /// signatures is exactly an ADR-014 certificate over the block hash.
    ///
    /// # Panics
    ///
    /// Never for real hashes: the length cast is guarded and JSON hashes are
    /// short.
    #[must_use]
    pub fn vote_message(block_hash: &str) -> Vec<u8> {
        let mut msg = b"glasschain-bft-vote:".to_vec();
        let hash = block_hash.as_bytes();
        #[allow(clippy::cast_possible_truncation)]
        let len32 = u32::try_from(hash.len()).expect("hash length fits u32");
        msg.extend_from_slice(&len32.to_be_bytes());
        msg.extend_from_slice(hash);
        msg
    }

    /// The message a vote's context signature commits to (#95):
    /// `domain || len(chain_id) || chain_id || height (u64 BE) ||
    /// round (u32 BE) || phase tag (u8) || len(hash) || hash`. A vote signed
    /// here cannot replay across heights, rounds, phases or networks.
    ///
    /// # Panics
    ///
    /// Never for real hashes/chain ids: the length casts are guarded.
    #[must_use]
    pub fn context_message(
        chain_id: &str,
        height: u64,
        round: u32,
        phase: VotePhase,
        block_hash: &str,
    ) -> Vec<u8> {
        let mut msg = b"glasschain-bft-vote-ctx:".to_vec();
        let chain = chain_id.as_bytes();
        #[allow(clippy::cast_possible_truncation)]
        let chain_len = u32::try_from(chain.len()).expect("chain id length fits u32");
        msg.extend_from_slice(&chain_len.to_be_bytes());
        msg.extend_from_slice(chain);
        msg.extend_from_slice(&height.to_be_bytes());
        msg.extend_from_slice(&round.to_be_bytes());
        msg.push(phase.tag());
        let hash = block_hash.as_bytes();
        #[allow(clippy::cast_possible_truncation)]
        let hash_len = u32::try_from(hash.len()).expect("hash length fits u32");
        msg.extend_from_slice(&hash_len.to_be_bytes());
        msg.extend_from_slice(hash);
        msg
    }

    /// Sign a vote with `signing_key`: the candidate-hash signature (the
    /// certificate aggregate material) plus the context envelope signature.
    #[must_use]
    pub fn sign(
        chain_id: &str,
        height: u64,
        round: u32,
        phase: VotePhase,
        block_hash: &str,
        signing_key: &PrivateKey,
    ) -> Self {
        Self {
            height,
            round,
            phase,
            block_hash: block_hash.to_owned(),
            chain_id: chain_id.to_owned(),
            public_key: signing_key.public_key().as_bytes(),
            signature: signing_key.sign(Self::vote_message(block_hash)).as_bytes(),
            context_signature: signing_key
                .sign(Self::context_message(
                    chain_id, height, round, phase, block_hash,
                ))
                .as_bytes(),
            algorithm: crate::wire::SignatureAlgorithm::Bls12381,
        }
    }

    /// Cryptographic self-verification; validator-set membership is the
    /// caller's check (the set is height-dependent).
    ///
    /// Both signatures must verify: the candidate-hash signature and the
    /// context envelope over this vote's own
    /// `chain_id`/height/round/phase/hash. A vote without a context
    /// signature (the pre-#95 legacy format) is rejected — replaying it at
    /// another height, round, phase or chain cannot be ruled out.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] for malformed keys/signatures, a
    /// missing context signature, or a signature that does not verify.
    pub fn verify(&self) -> Result<(), CoreError> {
        if self.algorithm != crate::wire::SignatureAlgorithm::Bls12381 {
            return Err(CoreError::InvalidBlock(format!(
                "bft: vote algorithm must be Bls12381, got {:?}",
                self.algorithm
            )));
        }
        if self.context_signature.is_empty() {
            return Err(CoreError::InvalidBlock(
                "bft: vote has no context signature — the legacy hash-only vote format is no \
                 longer accepted (#99)"
                    .into(),
            ));
        }
        let public = PublicKey::from_bytes(self.public_key.as_slice()).map_err(|e| {
            CoreError::InvalidBlock(format!("bft: vote has an invalid BLS public key: {e}"))
        })?;
        let signature = Signature::from_bytes(self.signature.as_slice()).map_err(|e| {
            CoreError::InvalidBlock(format!("bft: vote has an invalid BLS signature: {e}"))
        })?;
        let message = Self::vote_message(&self.block_hash);
        if !public.verify(signature, message) {
            return Err(CoreError::InvalidBlock(format!(
                "bft: vote signature does not verify (height {}, round {}, phase {:?})",
                self.height, self.round, self.phase
            )));
        }
        let context = Signature::from_bytes(self.context_signature.as_slice()).map_err(|e| {
            CoreError::InvalidBlock(format!("bft: vote has an invalid context signature: {e}"))
        })?;
        let envelope = Self::context_message(
            &self.chain_id,
            self.height,
            self.round,
            self.phase,
            &self.block_hash,
        );
        if !public.verify(context, envelope) {
            return Err(CoreError::InvalidBlock(format!(
                "bft: vote context signature does not verify (chain {}, height {}, round {}, phase {:?})",
                self.chain_id, self.height, self.round, self.phase
            )));
        }
        Ok(())
    }
}

/// Self-verifying evidence of validator equivocation (ADR-009 §4, #77).
///
/// One validator signed two different candidate hashes in the same
/// `(chain, height, round, phase)` context. The proof carries both votes in
/// full, so verification rides the dual-sign context envelope (#95): each
/// vote must individually verify, both must agree on the chain and the
/// proof's `(height, round, phase, public_key)` context, and they must name
/// different hashes. No reputation, no weighting, no automatic ejection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquivocationProof {
    pub height: u64,
    pub round: u32,
    pub phase: VotePhase,
    /// The equivocating validator's BLS public key.
    #[serde(with = "crate::wire::base64_bytes")]
    pub public_key: Vec<u8>,
    /// First conflicting vote, dual-signed over its own hash and the
    /// context envelope ([`BftVote::context_message`], #95).
    pub first_vote: BftVote,
    /// Second conflicting vote, dual-signed over its own hash and the same
    /// context envelope ([`BftVote::context_message`], #95).
    pub second_vote: BftVote,
}

#[cfg(feature = "bft")]
impl EquivocationProof {
    /// Self-verification: both votes verify through their dual-sign context
    /// envelopes (#95), agree on the chain and the proof's
    /// `(height, round, phase, public_key)` context, and name different
    /// hashes. Validator-set membership is the caller's check.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] when the proof does not verify.
    pub fn verify(&self) -> Result<(), CoreError> {
        self.first_vote.verify()?;
        self.second_vote.verify()?;
        if self.first_vote.block_hash == self.second_vote.block_hash {
            return Err(CoreError::InvalidBlock(
                "equivocation proof: both votes name the same hash — not equivocation".into(),
            ));
        }
        for (label, vote) in [("first", &self.first_vote), ("second", &self.second_vote)] {
            if vote.height != self.height
                || vote.round != self.round
                || vote.phase != self.phase
                || vote.public_key != self.public_key
            {
                return Err(CoreError::InvalidBlock(format!(
                    "equivocation proof: {label} vote disagrees with the proof context"
                )));
            }
        }
        if self.first_vote.chain_id != self.second_vote.chain_id {
            return Err(CoreError::InvalidBlock(
                "equivocation proof: the votes are bound to different chains".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "bft")]
mod tests {
    use super::*;
    use crate::Ledger;

    /// `count` validators with deterministic BLS keys and valid proofs of
    /// possession, plus the matching signing keys.
    fn validator_material(count: usize) -> (Vec<ValidatorInfo>, Vec<PrivateKey>) {
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
        (validators, keys)
    }

    /// `count` validators with the signing key at `local` (bitmap index).
    fn provider_signing_with(count: usize, local: usize) -> BftConsensusProvider {
        let (validators, keys) = validator_material(count);
        BftConsensusProvider::new(validators, keys[local]).expect("valid validators")
    }

    fn provider(count: usize) -> (BftConsensusProvider, Vec<PrivateKey>) {
        let (validators, keys) = validator_material(count);
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
            .map(|key| key.sign(BftVote::vote_message(&block.hash)))
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
        let wrong = keys[0].sign(BftVote::vote_message("a different block hash"));
        certificate.aggregate_signature = wrong.as_bytes();
        let error = provider
            .verify_certificate(&certificate, &block)
            .expect_err("a tampered aggregate must be rejected");
        assert!(error.to_string().contains("does not verify"), "{error}");
    }

    #[test]
    fn test_vote_round_aggregation_matches_certificate_verification() {
        // Mirrors the network round driver: prevote + precommit votes through
        // aggregate_votes must produce a certificate verify_certificate accepts.
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let (provider, _) = provider(1);
        let mut block = Block::with_write_set(1, vec![], genesis.hash, Vec::new());
        block.hash = block.calculate_hash();

        let precommit = provider.sign_vote(
            &ledger.chain[0].hash,
            1,
            0,
            VotePhase::Precommit,
            &block.hash,
        );
        assert!(precommit.verify().is_ok());

        let (bitmap, aggregate) = provider.aggregate_votes(&[precommit]).expect("aggregate");
        let certificate = QuorumCertificate {
            block_index: 1,
            block_hash: block.hash.clone(),
            signers_bitmap: bitmap,
            aggregate_signature: aggregate,
            algorithm: crate::wire::SignatureAlgorithm::Bls12381,
        };
        assert!(provider.verify_certificate(&certificate, &block).is_ok());
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

    /// A vote signed over the given chain id; helper for #95 acceptance.
    fn context_vote(
        provider: &BftConsensusProvider,
        chain_id: &str,
        height: u64,
        round: u32,
        phase: VotePhase,
        hash: &str,
    ) -> BftVote {
        provider.sign_vote(chain_id, height, round, phase, hash)
    }

    #[test]
    fn test_context_vote_rejects_tampered_height_round_phase_chain_id() {
        let (provider, _) = provider(1);
        let ledger = Ledger::new(1);
        let chain_id = ledger.chain[0].hash.clone();
        let vote = context_vote(&provider, &chain_id, 7, 2, VotePhase::Prevote, "hash-x");
        assert!(vote.verify().is_ok());

        // Tamper each context field while keeping the signatures: the
        // envelope must fail for every one of them.
        for tampered in [
            {
                let mut v = vote.clone();
                v.height = 8;
                v
            },
            {
                let mut v = vote.clone();
                v.round = 3;
                v
            },
            {
                let mut v = vote.clone();
                v.phase = VotePhase::Precommit;
                v
            },
            {
                // Nursery rust-clippy#8251-class false positive: the clone
                // is mutated and returned, `vote` is not re-usable here.
                #[allow(clippy::redundant_clone)]
                let mut v = vote.clone();
                v.chain_id = "other-chain".into();
                v
            },
        ] {
            let error = tampered
                .verify()
                .expect_err("tampered context must be rejected");
            assert!(error.to_string().contains("context signature"), "{error}");
        }
    }

    #[test]
    fn test_legacy_hash_only_vote_is_rejected_after_transition() {
        // #99: the legacy hash-only format (no context signature) is no
        // longer accepted — a vote must bind its consensus context.
        let (_, keys) = provider(1);
        let mut vote = BftVote::sign("", 7, 2, VotePhase::Prevote, "legacy-hash", &keys[0]);
        vote.chain_id = String::new();
        vote.context_signature = Vec::new();
        let error = vote
            .verify()
            .expect_err("legacy-only votes must be rejected");
        assert!(error.to_string().contains("legacy hash-only"), "{error}");
    }

    #[test]
    fn test_context_vote_without_chain_id_is_rejected() {
        // A vote that claims a context signature must bind a chain id: an
        // empty chain id with a context signature is a cross-network attack
        // shape and fails closed.
        let (provider, _) = provider(1);
        let mut vote = context_vote(&provider, "genesis-hash", 1, 0, VotePhase::Prevote, "hash");
        vote.chain_id = String::new();
        let error = vote.verify().expect_err("empty chain id must be rejected");
        assert!(error.to_string().contains("context signature"), "{error}");
    }

    #[test]
    fn test_aggregate_of_context_signed_votes_produces_valid_certificate() {
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let count = 4;
        let (provider, keys) = provider(count);
        let mut block = Block::with_write_set(1, vec![], genesis.hash.clone(), Vec::new());
        block.hash = block.calculate_hash();

        let votes: Vec<BftVote> = keys
            .iter()
            .map(|key| BftVote::sign(&genesis.hash, 1, 0, VotePhase::Precommit, &block.hash, key))
            .collect();
        for vote in &votes {
            assert!(vote.verify().is_ok(), "context-signed votes must verify");
        }
        let (bitmap, aggregate) = provider.aggregate_votes(&votes).expect("aggregate");
        let certificate = QuorumCertificate {
            block_index: 1,
            block_hash: block.hash.clone(),
            signers_bitmap: bitmap,
            aggregate_signature: aggregate,
            algorithm: crate::wire::SignatureAlgorithm::Bls12381,
        };
        assert!(provider.verify_certificate(&certificate, &block).is_ok());

        // A context-tampered vote fails before aggregation can use it.
        let mut impostor = votes[0].clone();
        impostor.round = 1;
        let mut mixed: Vec<BftVote> = votes[..count - 1].to_vec();
        mixed.push(impostor);
        let error = provider
            .aggregate_votes(&mixed)
            .expect_err("mismatched-context votes must fail aggregation");
        assert!(error.to_string().contains("does not verify"), "{error}");
    }
    #[test]
    fn provider_rejects_malformed_validator_material() {
        let signing = PrivateKey::new([9; 64]);
        let valid = ValidatorInfo {
            name: "v".to_owned(),
            public_key: signing.public_key().as_bytes(),
            pop: signing.sign(b"glasschain-bls-pop").as_bytes(),
        };

        // Wrong key length is named in the error.
        let mut bad_len = valid.clone();
        bad_len.public_key = vec![1; 47];
        let Err(err) = BftConsensusProvider::new(vec![bad_len], signing) else {
            panic!("bad length must be rejected");
        };
        assert!(err.to_string().contains("expected 48"), "{err}");

        // A 48-byte value that is not a valid BLS key.
        let mut bad_key = valid.clone();
        bad_key.public_key = vec![7; 48];
        assert!(BftConsensusProvider::new(vec![bad_key], signing).is_err());

        // An unparseable proof of possession.
        let mut bad_pop = valid;
        bad_pop.pop = vec![7; 96];
        assert!(BftConsensusProvider::new(vec![bad_pop], signing).is_err());
    }

    #[test]
    fn provider_accessors_expose_the_validator_view() {
        let (provider, keys) = provider(3);
        assert_eq!(provider.validator_count(), 3);
        assert_eq!(provider.validators().len(), 3);
        assert_eq!(provider.public_key(), keys[0].public_key().as_bytes());
        assert_eq!(provider.signing_key(), &keys[0]);
    }

    #[test]
    fn verify_vote_rejects_a_validator_outside_the_set() {
        let (provider, _keys) = provider(2);

        // An outsider's vote never matches a registered public key.
        let outsider = PrivateKey::new([77; 64]);
        let vote = BftVote {
            height: 1,
            round: 0,
            phase: VotePhase::Prevote,
            block_hash: "abc".to_owned(),
            chain_id: String::new(),
            public_key: outsider.public_key().as_bytes(),
            signature: outsider.sign(b"vote").as_bytes(),
            algorithm: crate::wire::SignatureAlgorithm::Bls12381,
            context_signature: Vec::new(),
        };
        assert!(provider.verify_vote(&vote).is_err());
    }

    #[test]
    fn verify_certificate_rejects_a_degenerate_quorum() {
        let (provider, keys) = provider(3);
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().unwrap().clone();
        let block = Block::new(1, vec![], genesis.hash);

        let degenerate = QuorumCertificate {
            block_index: 1,
            block_hash: block.hash.clone(),
            signers_bitmap: vec![0u8; 1],
            aggregate_signature: Vec::new(),
            algorithm: crate::wire::SignatureAlgorithm::Bls12381,
        };
        let err = provider
            .verify_certificate(&degenerate, &block)
            .expect_err("degenerate certificates are not final");
        assert!(err.to_string().contains("degenerate"), "{err}");

        // A genuine certificate signed by all validators verifies.
        let real = aggregated_certificate(&block, &keys, &[0, 1, 2]);
        provider.verify_certificate(&real, &block).unwrap();

        // The same certificate against a different block fails.
        let mut other = block.clone();
        other.nonce += 1;
        other.hash = other.calculate_hash();
        assert!(provider.verify_certificate(&real, &other).is_err());

        // A bitmap naming a validator beyond the set is malformed.
        let mut oversized = real;
        oversized.signers_bitmap = Vec::new();
        assert!(provider.verify_certificate(&oversized, &block).is_err());
    }

    #[test]
    fn consensus_provider_trait_validates_chaining_only() {
        let (provider, _keys) = provider(1);
        let ledger = Ledger::new(1);
        let genesis = ledger.chain[0].clone();
        let mut chained = Block::new(1, vec![], genesis.hash.clone());
        chained.mine(1);
        if let Err(e) = provider.validate_block(&chained, &genesis) {
            panic!("chained block must validate: {e}");
        }

        let mut orphan = Block::new(2, vec![], "unrelated".to_owned());
        orphan.mine(1);
        let err = provider
            .validate_block(&orphan, &chained)
            .expect_err("orphan must fail");
        assert!(err.to_string().contains("does not chain"), "{err}");
        assert_eq!(provider.name(), "bft");
    }

    /// The local signer's bitmap bit lands in the right byte and bit position
    /// when the index crosses the first byte (kills `local_index -> Some(0)`
    /// and the `index / 8` / `index % 8` / shift mutants).
    #[test]
    fn test_attest_bitmap_marks_the_local_index_beyond_the_first_byte() {
        let mut ledger = Ledger::new(1);
        let genesis = ledger.mine_pending_transactions().expect("genesis").clone();
        let provider = provider_signing_with(12, 10);
        let notification = provider.attest(genesis);
        assert_eq!(
            notification.certificate.signers_bitmap,
            vec![0, 0b0000_0100]
        );
    }

    /// The vote message is `domain || BE(u32) length || hash`, so a length or
    /// hash mutation changes the bytes (kills body-replacement and
    /// length-arithmetic mutants).
    #[test]
    fn test_vote_message_encodes_domain_length_and_hash() {
        let mut expected = b"glasschain-bft-vote:".to_vec();
        expected.extend_from_slice(&3u32.to_be_bytes());
        expected.extend_from_slice(b"abc");
        assert_eq!(BftVote::vote_message("abc"), expected);
        assert_ne!(BftVote::vote_message("abc"), BftVote::vote_message("ab1"));
        assert_ne!(BftVote::vote_message("abc"), BftVote::vote_message("abcd"));
    }

    /// `aggregate_votes` marks every distinct voter's bit and collapses
    /// duplicates (kills the bitmap byte/bit arithmetic mutants).
    #[test]
    fn test_aggregate_votes_marks_every_voter_and_dedupes() {
        let (provider, keys) = provider(12);
        let ledger = Ledger::new(1);
        let chain_id = ledger.chain[0].hash.clone();
        let first = BftVote::sign(&chain_id, 3, 1, VotePhase::Precommit, "block-x", &keys[0]);
        let tenth = BftVote::sign(&chain_id, 3, 1, VotePhase::Precommit, "block-x", &keys[10]);
        let (bitmap, _) = provider
            .aggregate_votes(&[first.clone(), tenth, first.clone()])
            .expect("aggregate");
        assert_eq!(bitmap, vec![0b0000_0001, 0b0000_0100]);

        // Deduplication: the same vote twice contributes one signature, so the
        // aggregate (and bitmap) equals the single-vote result. Kills the
        // bitmap-membership shift mutant in the dedupe check.
        let (single_bitmap, single_aggregate) = provider
            .aggregate_votes(std::slice::from_ref(&first))
            .expect("aggregate");
        let (duplicate_bitmap, duplicate_aggregate) = provider
            .aggregate_votes(&[first.clone(), first])
            .expect("aggregate");
        assert_eq!(duplicate_bitmap, single_bitmap);
        assert_eq!(duplicate_aggregate, single_aggregate);
    }

    /// An equivocation proof verifies only when both votes are internally
    /// valid, name different hashes, and agree with the proof context and
    /// chain (kills the `verify -> Ok(())` and context-comparison mutants).
    #[test]
    fn test_equivocation_proof_verifies_and_rejects_tampering() {
        let (provider, _) = provider(1);
        let ledger = Ledger::new(1);
        let chain_id = ledger.chain[0].hash.clone();
        let first = provider.sign_vote(&chain_id, 5, 2, VotePhase::Prevote, "hash-a");
        let second = provider.sign_vote(&chain_id, 5, 2, VotePhase::Prevote, "hash-b");
        let proof = EquivocationProof {
            height: 5,
            round: 2,
            phase: VotePhase::Prevote,
            public_key: first.public_key.clone(),
            first_vote: first.clone(),
            second_vote: second,
        };
        assert!(proof.verify().is_ok());

        let same_hash = EquivocationProof {
            second_vote: first,
            ..proof.clone()
        };
        assert!(
            same_hash.verify().is_err(),
            "one hash twice is not equivocation"
        );

        for (label, mutated) in [
            (
                "height",
                EquivocationProof {
                    height: 6,
                    ..proof.clone()
                },
            ),
            (
                "round",
                EquivocationProof {
                    round: 3,
                    ..proof.clone()
                },
            ),
            (
                "phase",
                EquivocationProof {
                    phase: VotePhase::Precommit,
                    ..proof.clone()
                },
            ),
            (
                "public key",
                EquivocationProof {
                    public_key: vec![0; 48],
                    ..proof.clone()
                },
            ),
        ] {
            assert!(mutated.verify().is_err(), "{label} disagreement must fail");
        }

        let other_chain = provider.sign_vote("other-chain", 5, 2, VotePhase::Prevote, "hash-b");
        let cross_chain = EquivocationProof {
            second_vote: other_chain,
            ..proof.clone()
        };
        assert!(cross_chain.verify().is_err(), "cross-chain votes must fail");

        let mut tampered = proof;
        tampered.second_vote.block_hash = "hash-c".into();
        assert!(tampered.verify().is_err(), "unsigned hash change must fail");
    }
}
