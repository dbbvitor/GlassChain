//! BFT round state and the vote-round driver (ADR-002 adoption gate, ADR-009
//! churn, ADR-014 aggregation).
//!
//! Round flow per `(height, round)` — Tendermint-shaped, two phases:
//!
//! 1. **Prevote** — the round leader (validators[(height + round) % n])
//!    broadcasts [`Message::Proposal`]; validators verify the candidate and
//!    answer with a phase-tagged BLS [`glasschain_core::BftVote`]. The leader
//!    aggregates a prevote quorum.
//! 2. **Precommit** — the leader re-broadcasts the candidate with the prevote
//!    certificate ([`Message::Precommit`]); validators that see a valid
//!    prevote quorum **lock** the hash and precommit. A precommit quorum
//!    commits.
//!
//! View change: on phase timeout the round increments (proposer rotates);
//! locked validators prevote their locked hash, and the leader proposes its
//! locked block when it has one — the minimal locking rule that prevents
//! two conflicting quorums at one height in the dev/test setting.
//!
//! The validator set is **on-chain state** (ADR-009/ADR-010): world-state
//! entries under `governance/validator-registry/<name>`, replayed like every
//! projection; governance manages membership through endorsed writes.

use glasschain_core::{BftConsensusProvider, BftVote, EquivocationProof, VotePhase};

/// Rounds attempted per height before the driver gives up (dev knob).
pub const MAX_ROUNDS: u32 = 4;

/// Per-phase vote-collection timeout (dev knob; liveness guidance in
/// `docs/liveness.md` §4 — claimable numbers wait for the testnet).
pub const PHASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The on-chain validator-registry contract (world-state prefix
/// `ws:governance:validator-registry/<name>` → JSON `{public_key, pop}`).
pub const VALIDATOR_REGISTRY_CONTRACT: &str = "validator-registry";
pub const VALIDATOR_REGISTRY_CHANNEL: &str = "governance";

/// In-flight round state for one height.
#[derive(Debug, Default)]
pub struct BftRound {
    /// Height being decided.
    pub height: u64,
    /// Current round (increments on timeout — view change).
    pub round: u32,
    /// The hash this node has locked by precommitting (Tendermint locking
    /// rule): later rounds prevote the locked hash.
    pub locked: Option<String>,
    /// The leader's candidate for this round, cached for precommit.
    pub proposal: Option<glasschain_core::Block>,
}

impl BftRound {
    /// A fresh round at `height` round 0.
    #[must_use]
    pub const fn new(height: u64) -> Self {
        Self {
            height,
            round: 0,
            locked: None,
            proposal: None,
        }
    }

    /// The canonical validator order is derived from registry names.
    #[must_use]
    pub fn phase_message_hash(&self) -> Option<String> {
        self.proposal.as_ref().map(|block| block.hash.clone())
    }
}

/// Recorded evidence of a validator signing two different hashes in one
/// `(height, round, phase)`.
#[derive(Debug)]
pub struct DetectedEquivocation {
    pub proof: EquivocationProof,
}

/// Book-keeping for one validator's votes at `(height, round, phase)` —
/// the receipt side of #77's detection rule.
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct VoteReceipts {
    seen: std::collections::HashMap<(u64, u32, VotePhase, Vec<u8>), (String, Vec<u8>)>,
}

impl VoteReceipts {
    /// Record a verified vote; returns an equivocation proof when the same
    /// key already voted for a **different** hash in the same
    /// `(height, round, phase)`.
    #[must_use]
    pub fn record(&mut self, vote: &BftVote) -> Option<EquivocationProof> {
        let key = (vote.height, vote.round, vote.phase, vote.public_key.clone());
        match self.seen.get(&key) {
            Some((hash, signature)) if hash != &vote.block_hash => {
                let (first_hash, first_signature) = (hash.clone(), signature.clone());
                Some(EquivocationProof {
                    height: vote.height,
                    round: vote.round,
                    phase: vote.phase,
                    public_key: vote.public_key.clone(),
                    first_signature,
                    first_block_hash: first_hash,
                    second_signature: vote.signature.clone(),
                    second_block_hash: vote.block_hash.clone(),
                })
            }
            Some(_) => None,
            None => {
                self.seen
                    .insert(key, (vote.block_hash.clone(), vote.signature.clone()));
                None
            }
        }
    }
}

/// Deterministic proposer for `(height, round)`: round-robin over the
/// validator set's canonical order (ADR-009 — one org one slot, equal power).
#[must_use]
pub fn proposer_index(validators: &BftConsensusProvider, height: u64, round: u32) -> usize {
    // Deterministic round-robin; usize truncation is impossible for heights
    // below 2^32 on 32-bit targets and unreachable on 64-bit — and a wrap
    // would only shift the rotation, never break safety.
    #[allow(clippy::cast_possible_truncation)]
    let base = height as usize;
    #[allow(clippy::cast_possible_truncation)]
    let offset = round as usize;
    (base + offset) % validators.validator_count().max(1)
}

#[cfg(all(test, feature = "bft"))]
mod tests {
    use super::*;
    use bls_signatures::PrivateKey;
    use bls_signatures::Serialize as _;

    fn vote(key: &PrivateKey, height: u64, round: u32, phase: VotePhase, hash: &str) -> BftVote {
        BftVote::sign(height, round, phase, hash, key)
    }

    #[test]
    fn test_receipts_detect_conflicting_hashes() {
        let key = PrivateKey::new([3; 64]);
        let mut receipts = VoteReceipts::default();
        let first = vote(&key, 5, 0, VotePhase::Prevote, "hash-a");
        assert!(receipts.record(&first).is_none());
        // Same hash again: no equivocation.
        assert!(receipts.record(&first).is_none());
        // Different hash at the same (height, round, phase): detected.
        let second = vote(&key, 5, 0, VotePhase::Prevote, "hash-b");
        let proof = receipts.record(&second).expect("equivocation detected");
        assert!(proof.verify().is_ok(), "the proof must be self-verifying");
    }

    #[test]
    fn test_receipts_do_not_false_positive_across_phases_or_heights() {
        let key = PrivateKey::new([4; 64]);
        let mut receipts = VoteReceipts::default();
        assert!(receipts
            .record(&vote(&key, 5, 0, VotePhase::Prevote, "hash-a"))
            .is_none());
        // Different phase: a prevote and a precommit over the same hash are
        // the protocol working, not misbehavior.
        assert!(receipts
            .record(&vote(&key, 5, 0, VotePhase::Precommit, "hash-a"))
            .is_none());
        // Different height: same.
        assert!(receipts
            .record(&vote(&key, 6, 0, VotePhase::Prevote, "hash-a"))
            .is_none());
        // Different key: distinct validators may vote the same hash.
        let other = PrivateKey::new([5; 64]);
        assert!(receipts
            .record(&vote(&other, 5, 0, VotePhase::Prevote, "hash-a"))
            .is_none());
    }

    #[test]
    fn test_equivocation_proof_rejects_same_hash() {
        let key = PrivateKey::new([6; 64]);
        let proof = glasschain_core::EquivocationProof {
            height: 1,
            round: 0,
            phase: VotePhase::Prevote,
            public_key: key.public_key().as_bytes(),
            first_signature: key
                .sign(glasschain_core::BftVote::vote_message("hash-a"))
                .as_bytes(),
            first_block_hash: "hash-a".into(),
            second_signature: key
                .sign(glasschain_core::BftVote::vote_message("hash-a"))
                .as_bytes(),
            second_block_hash: "hash-a".into(),
        };
        assert!(proof.verify().is_err(), "same hash is not equivocation");
    }
}
