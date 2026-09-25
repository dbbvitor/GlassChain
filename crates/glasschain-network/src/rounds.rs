// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
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

#[cfg(feature = "bft")]
use glasschain_core::BftConsensusProvider;
use glasschain_core::{
    receipt_action, should_retain, BftVote, EquivocationProof, ReceiptAction, VotePhase,
};

/// Rounds attempted per height before the driver gives up (dev knob).
pub const MAX_ROUNDS: u32 = 4;

/// Base per-phase vote-collection timeout (dev knob; liveness guidance in
/// `docs/liveness.md` §4 — claimable numbers wait for the testnet).
pub const PHASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Phase timeout scaled to the set size.
///
/// Two O(n) costs must fit inside one window: the leader's per-vote pairing
/// verification and — dominating at scale — every validator's
/// re-verification of the prevote aggregate (an O(quorum)-pairing
/// multi-miller loop, ADR-014). With the pure-Rust backend that is seconds
/// per validator; `blst` (the documented follow-up) shrinks it ~10×.
#[must_use]
pub fn phase_timeout(validator_count: usize) -> std::time::Duration {
    PHASE_TIMEOUT + std::time::Duration::from_secs((validator_count / 10) as u64)
}

/// The on-chain validator-registry contract (world-state prefix
/// `ws:governance:validator-registry/<name>` → JSON `{public_key, pop}`).
pub const VALIDATOR_REGISTRY_CONTRACT: &str = "validator-registry";
pub const VALIDATOR_REGISTRY_CHANNEL: &str = "governance";

/// The leader's in-flight vote channel bound (#98, zero-trust §8.4).
///
/// Live quorum-sized traffic stays far below it; when full, further arrivals
/// are dropped at `handle_vote` — the queue retains the oldest messages,
/// which adversarial flooding would make stale anyway. Liveness degrades
/// under flooding; safety is kept by the absolute phase deadline and the
/// distinct-voter quorum count.
pub const VOTE_CHANNEL_CAP: usize = 1024;

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

/// Journal cap (#96): at most this many outstanding
/// `(height, round, phase, key)` receipts across all contexts.
///
/// Live quorum-sized traffic stays far below it; when the cap is reached,
/// receipts from strictly lower heights are evicted first and a still-full
/// journal drops the *new* record — an attacker's flooding context, not a
/// live vote.
pub const VOTE_RECEIPT_CAP: usize = 8192;

/// Book-keeping for one validator's votes at `(height, round, phase)` —
/// the receipt side of #77's detection rule.
///
/// Held for the node's lifetime in `NodeState` (#96): detection now works
/// across separate vote messages. Live-only: a restart loses unreported
/// evidence (accepted limitation).
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct VoteReceipts {
    seen: std::collections::HashMap<(u64, u32, VotePhase, Vec<u8>), BftVote>,
}

impl VoteReceipts {
    /// Record a verified vote; returns an equivocation proof carrying both
    /// dual-signed votes (#95) when the same key already voted for a
    /// **different** hash in the same `(height, round, phase)`. Bounded: see
    /// [`VOTE_RECEIPT_CAP`].
    #[must_use]
    pub fn record(&mut self, vote: &BftVote) -> Option<EquivocationProof> {
        let key = (vote.height, vote.round, vote.phase, vote.public_key.clone());
        // The decision table is the proved kernel (ADR-019/#176): the first
        // vote in a context inserts, a same-hash replay duplicates, and a
        // different hash in the same context is the only equivocation.
        let existing = self.seen.get(&key);
        let same_hash = existing.map(|first| first.block_hash == vote.block_hash);
        let first = existing.cloned();
        match receipt_action(first.is_some(), same_hash.unwrap_or(false)) {
            ReceiptAction::Equivocation => first.map(|first| EquivocationProof {
                height: vote.height,
                round: vote.round,
                phase: vote.phase,
                public_key: vote.public_key.clone(),
                first_vote: first,
                second_vote: vote.clone(),
            }),
            ReceiptAction::Duplicate => None,
            ReceiptAction::Insert => {
                if self.seen.len() >= VOTE_RECEIPT_CAP && !self.evict_stale(vote.height) {
                    return None;
                }
                self.seen.insert(key, vote.clone());
                None
            }
        }
    }

    /// Drop receipts for heights below `height` (a height is dead once we are
    /// voting at least one height past it — current + previous retained). The
    /// retention predicate is the proved kernel (ADR-019/#176).
    pub fn retire_below(&mut self, height: u64) -> usize {
        let before = self.seen.len();
        self.seen
            .retain(|(h, _, _, _), _| should_retain(*h, height));
        before - self.seen.len()
    }

    /// Only-lower-heights eviction inside [`VoteReceipts::record`]; returns
    /// whether anything was freed.
    fn evict_stale(&mut self, height: u64) -> bool {
        let before = self.seen.len();
        self.seen
            .retain(|(h, _, _, _), _| should_retain(*h, height));
        self.seen.len() < before
    }
}

/// Per-phase wall-clock split of one successful vote round on the leader.
///
/// Performance Step 0 instrumentation: proposal/vote/verify/commit are
/// measured separately, so a round budget can be attributed to its phases
/// instead of collapsing into one end-to-end number. Wall-clock milliseconds,
/// recorded at phase completion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BftPhaseTimings {
    /// Proposal broadcast enqueue duration (fan-out, not delivery).
    pub proposal_broadcast: u128,
    /// Prevote collection (includes the voters' block verification).
    pub prevote: u128,
    /// Prevote aggregate + certificate assembly.
    pub prevote_aggregate: u128,
    /// Precommit collection (includes prevote-certificate re-verification).
    pub precommit: u128,
    /// Precommit aggregate + final certificate assembly.
    pub precommit_aggregate: u128,
}

/// Deterministic proposer for `(height, round)`: round-robin over the
/// validator set's canonical order (ADR-009 — one org one slot, equal power).
///
/// The rotation arithmetic is the proved overflow-safe kernel (ADR-019/#176).
#[cfg(feature = "bft")]
#[must_use]
pub fn proposer_index(validators: &BftConsensusProvider, height: u64, round: u32) -> usize {
    glasschain_core::proposer_slot(height, round, validators.validator_count())
}

#[cfg(test)]
#[cfg(feature = "bft")]
mod tests {
    use super::*;
    use bls_signatures::PrivateKey;
    use bls_signatures::Serialize as _;

    fn vote(key: &PrivateKey, height: u64, round: u32, phase: VotePhase, hash: &str) -> BftVote {
        BftVote::sign("test-chain", height, round, phase, hash, key)
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
            first_vote: vote(&key, 1, 0, VotePhase::Prevote, "hash-a"),
            second_vote: vote(&key, 1, 0, VotePhase::Prevote, "hash-a"),
        };
        assert!(proof.verify().is_err(), "same hash is not equivocation");
    }

    #[test]
    fn test_equivocation_proof_rejects_cross_context_votes() {
        // Two genuine votes in different consensus contexts — different
        // heights here — are the protocol working, not equivocation. A proof
        // assembled from them must fail verification: the dual-sign context
        // envelope (#95) authenticates the shared (chain, height, round,
        // phase), not just the hashes (Frontier A conclusion).
        let key = PrivateKey::new([9; 64]);
        let proof = glasschain_core::EquivocationProof {
            height: 5,
            round: 0,
            phase: VotePhase::Prevote,
            public_key: key.public_key().as_bytes(),
            first_vote: vote(&key, 5, 0, VotePhase::Prevote, "hash-a"),
            second_vote: vote(&key, 6, 0, VotePhase::Prevote, "hash-b"),
        };
        assert!(
            proof.verify().is_err(),
            "cross-height votes are not equivocation"
        );
    }

    #[test]
    fn test_equivocation_proof_rejects_cross_chain_votes() {
        // The envelope binds the chain id: votes reused from a different
        // network (same validator, same context shape) cannot pass.
        let key = PrivateKey::new([11; 64]);
        let proof = glasschain_core::EquivocationProof {
            height: 5,
            round: 0,
            phase: VotePhase::Prevote,
            public_key: key.public_key().as_bytes(),
            first_vote: vote(&key, 5, 0, VotePhase::Prevote, "hash-a"),
            second_vote: BftVote::sign("other-chain", 5, 0, VotePhase::Prevote, "hash-b", &key),
        };
        assert!(
            proof.verify().is_err(),
            "cross-chain votes are not equivocation"
        );
    }

    #[test]
    fn test_equivocation_proof_rejects_tampered_context_signature() {
        let key = PrivateKey::new([10; 64]);
        let mut second = vote(&key, 5, 0, VotePhase::Prevote, "hash-b");
        second.context_signature = vec![0; 96];
        let proof = glasschain_core::EquivocationProof {
            height: 5,
            round: 0,
            phase: VotePhase::Prevote,
            public_key: key.public_key().as_bytes(),
            first_vote: vote(&key, 5, 0, VotePhase::Prevote, "hash-a"),
            second_vote: second,
        };
        assert!(
            proof.verify().is_err(),
            "a tampered context signature must fail"
        );
    }

    #[test]
    fn test_journal_is_flood_bounded() {
        // #96 case 3: flooding distinct height contexts cannot grow the
        // journal without limit. Distinct flood contexts saturate the cap;
        // eviction drops strictly lower heights first; with nothing lower
        // left, further flood records are declined — recorded live context
        // survives. Flood rows carry dummy signatures: `record` never
        // verifies (the handler verifies before recording), and real BLS
        // signing would make this test pay seconds per thousand rows.
        let key = PrivateKey::new([7; 64]);
        let public_key = key.public_key().as_bytes();
        let flood_vote = |height: u64| -> BftVote {
            BftVote {
                height,
                round: 0,
                phase: VotePhase::Prevote,
                block_hash: "flood-hash".into(),
                chain_id: String::new(),
                public_key: public_key.clone(),
                signature: vec![0; 96],
                context_signature: Vec::new(),
                algorithm: glasschain_core::wire::SignatureAlgorithm::Bls12381,
            }
        };
        let mut journal = VoteReceipts::default();
        for h in 0..(VOTE_RECEIPT_CAP + 100) {
            let _ = journal.record(&flood_vote(h as u64));
        }
        assert!(journal.seen.len() <= VOTE_RECEIPT_CAP, "journal overflowed");

        // Retirement drops exactly the strictly-lower heights in a small
        // journal, keeping the live (current) context.
        let mut small = VoteReceipts::default();
        let _ = small.record(&vote(&key, 5, 0, VotePhase::Prevote, "hash-a"));
        let _ = small.record(&vote(
            &PrivateKey::new([8; 64]),
            5,
            0,
            VotePhase::Prevote,
            "hash-a",
        ));
        let _ = small.record(&vote(&key, 6, 0, VotePhase::Prevote, "hash-a"));
        assert_eq!(small.seen.len(), 3);
        let detected = small.retire_below(6);
        assert_eq!(detected, 2);
        assert_eq!(small.seen.len(), 1);

        // With nothing lower left, a further flood record is declined, not
        // stored — the journal cannot grow past its bound.
        let mut saturated = VoteReceipts::default();
        for h in 0..VOTE_RECEIPT_CAP as u64 {
            let _ = saturated.record(&flood_vote(h));
        }
        assert_eq!(saturated.seen.len(), VOTE_RECEIPT_CAP);
        let mut declined = flood_vote(0);
        declined.round = 9;
        let _ = saturated.record(&declined);
        assert_eq!(saturated.seen.len(), VOTE_RECEIPT_CAP, "full journal froze");

        // A record at the current height evicts the strictly-lower flood and
        // is admitted (the eviction must report that it freed space).
        let current = flood_vote(VOTE_RECEIPT_CAP as u64);
        let _ = saturated.record(&current);
        assert_eq!(
            saturated.seen.len(),
            1,
            "a current-height record must evict stale heights and be admitted"
        );
        assert!(saturated.seen.contains_key(&(
            VOTE_RECEIPT_CAP as u64,
            0,
            VotePhase::Prevote,
            public_key
        )));
    }

    fn bft_provider(count: usize) -> BftConsensusProvider {
        let mut validators = Vec::new();
        let mut keys = Vec::new();
        for i in 0..count {
            let secret = PrivateKey::new([u8::try_from(i + 1).expect("count fits u8"); 64]);
            let public = secret.public_key();
            let pop = secret.sign(format!(
                "glasschain-bls-pop:{}",
                hex::encode(public.as_bytes())
            ));
            validators.push(glasschain_core::ValidatorInfo {
                name: format!("validator-{i}"),
                public_key: public.as_bytes(),
                pop: pop.as_bytes(),
            });
            keys.push(secret);
        }
        BftConsensusProvider::new(validators, keys[0]).expect("valid validators")
    }

    /// The phase timeout is base + one second per ten validators (integer
    /// division), so every boundary is exact.
    #[test]
    fn phase_timeout_scales_with_the_validator_count() {
        assert_eq!(phase_timeout(0), std::time::Duration::from_secs(3));
        assert_eq!(phase_timeout(9), std::time::Duration::from_secs(3));
        assert_eq!(phase_timeout(10), std::time::Duration::from_secs(4));
        assert_eq!(phase_timeout(19), std::time::Duration::from_secs(4));
        assert_eq!(phase_timeout(20), std::time::Duration::from_secs(5));
        assert_eq!(phase_timeout(25), std::time::Duration::from_secs(5));
    }

    /// `phase_message_hash` reports the cached proposal's real hash, not a
    /// constant, and `None` before a proposal exists.
    #[test]
    fn phase_message_hash_reports_the_proposal_hash() {
        let mut round = BftRound::new(7);
        assert_eq!(round.phase_message_hash(), None, "no proposal yet");
        let mut block = glasschain_core::Block::new(7, Vec::new(), "prev".into());
        block.hash = "proposal-hash".into();
        round.proposal = Some(block);
        assert_eq!(round.phase_message_hash().as_deref(), Some("proposal-hash"));
    }

    /// The proposer rotates by `(height + round) % n`, not by their
    /// difference.
    #[test]
    fn proposer_index_rotates_by_height_plus_round() {
        let provider = bft_provider(10);
        assert_eq!(proposer_index(&provider, 0, 0), 0);
        assert_eq!(proposer_index(&provider, 5, 3), 8);
        assert_eq!(proposer_index(&provider, 5, 5), 0);
        assert_eq!(proposer_index(&provider, 12, 1), 3);
        // A single validator always proposes.
        let solo = bft_provider(1);
        assert_eq!(proposer_index(&solo, 9, 4), 0);
    }
}
