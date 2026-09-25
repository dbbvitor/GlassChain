// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Pure consensus-round kernels, proved in production form with Verus
//! (ADR-019; residues tracked in #176): the proposer rotation arithmetic and
//! the vote-receipt decision table.
//!
//! The round state itself — `BftRound` and the `VoteReceipts` journal — stays
//! in `glasschain-network/src/rounds.rs`, which delegates every pure decision
//! to this module. That is the TOFU precedent (ADR-019): the `HashMap`-backed
//! state never enters a spec, only the decision does.

use vstd::prelude::*;

verus! {

/// The rotation modulus: the validator count, or one for the empty set (a
/// single implicit proposer), so the modulo is total.
pub open spec fn spec_slot_modulus(validator_count: usize) -> int {
    if validator_count == 0 { 1 } else { validator_count as int }
}

/// The proposer slot for `(height, round)`: round-robin over the validator
/// set's canonical order (ADR-009 — one org one slot, equal power).
///
/// The arithmetic is overflow-safe: each term is reduced modulo the set size
/// inside `u128` before they are added, so no `height`/`round` pair can wrap
/// the rotation (a `u64::MAX` height still lands on a valid slot).
#[must_use]
pub fn proposer_slot(height: u64, round: u32, validator_count: usize) -> (slot: usize)
    ensures
        slot as int == spec_proposer_slot(height, round, validator_count),
        validator_count > 0 ==> slot < validator_count,
{
    let slots = validator_count.max(1) as u128;
    let slot = ((u128::from(height) % slots) + (u128::from(round) % slots)) % slots;
    // `slot < validator_count.max(1) <= usize::MAX`, proved above.
    #[allow(clippy::cast_possible_truncation)]
    let slot = slot as usize;
    slot
}

/// The specification of [`proposer_slot`]: `(height mod n + round mod n) mod n`
/// over the rotation modulus.
pub open spec fn spec_proposer_slot(height: u64, round: u32, validator_count: usize) -> int {
    (height as int % spec_slot_modulus(validator_count) + round as int
        % spec_slot_modulus(validator_count)) % spec_slot_modulus(validator_count)
}

/// The slot model is the rotating `(height + round) mod n` of the round-robin
/// order — the identity the crash-recovery and view-change paths assume.
pub proof fn proposer_slot_is_rotation(height: u64, round: u32, validator_count: usize)
    requires validator_count > 0,
    ensures
        spec_proposer_slot(height, round, validator_count) == (height as int + round as int)
            % validator_count as int,
{
    vstd::arithmetic::div_mod::lemma_add_mod_noop(
        height as int,
        round as int,
        validator_count as int,
    );
}

/// The action the network's `VoteReceipts::record` takes for a vote.
///
/// The decision depends on whether a receipt for the same
/// `(height, round, phase, key)` context already exists and whether it carries
/// the same block hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptAction {
    /// First vote in this context: record it.
    Insert,
    /// Same context and same hash: a duplicate, nothing to do.
    Duplicate,
    /// Same context, different hash: equivocation evidence (#77).
    Equivocation,
}

/// The specification of [`receipt_action`], in the same case order.
pub open spec fn spec_receipt_action(existing: bool, same_hash: bool) -> ReceiptAction {
    if !existing {
        ReceiptAction::Insert
    } else if same_hash {
        ReceiptAction::Duplicate
    } else {
        ReceiptAction::Equivocation
    }
}

/// Decide the journal action for one verified vote. The caller owns the map
/// lookup; this is the decision table the shipped `record` routes through.
#[must_use]
pub const fn receipt_action(existing: bool, same_hash: bool) -> (action: ReceiptAction)
    ensures action == spec_receipt_action(existing, same_hash),
{
    if !existing {
        ReceiptAction::Insert
    } else if same_hash {
        ReceiptAction::Duplicate
    } else {
        ReceiptAction::Equivocation
    }
}

/// An equivocation proof is emitted exactly for a second, different hash from
/// the same key in the same context — and never for a replay of the same vote.
pub proof fn equivocation_iff_conflicting(existing: bool, same_hash: bool)
    ensures
        spec_receipt_action(existing, same_hash) == ReceiptAction::Equivocation
            <==> (existing && !same_hash),
        same_hash ==> spec_receipt_action(existing, same_hash) != ReceiptAction::Equivocation,
{
}

/// Whether a receipt at `receipt_height` survives retirement below
/// `retire_below`: the current height and everything above it are retained
/// (the journal keeps current + previous, #96).
pub open spec fn spec_should_retain(receipt_height: u64, retire_below: u64) -> bool {
    receipt_height >= retire_below
}

/// The retirement predicate the network's `VoteReceipts` delegates to.
#[must_use]
pub const fn should_retain(receipt_height: u64, retire_below: u64) -> (keep: bool)
    ensures keep == spec_should_retain(receipt_height, retire_below),
{
    receipt_height >= retire_below
}

/// Retirement is exact: every receipt at or above the bound stays, every
/// receipt below it is dropped, and the count arithmetic the driver reports
/// cannot disagree with the filter.
pub proof fn retirement_is_exact(receipt_height: u64, retire_below: u64)
    ensures
        receipt_height >= retire_below ==> spec_should_retain(receipt_height, retire_below),
        receipt_height < retire_below ==> !spec_should_retain(receipt_height, retire_below),
{
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot rotates by `(height + round) % n`, not by their difference.
    #[test]
    fn proposer_slot_rotates_by_height_plus_round() {
        assert_eq!(proposer_slot(0, 0, 10), 0);
        assert_eq!(proposer_slot(5, 3, 10), 8);
        assert_eq!(proposer_slot(5, 5, 10), 0);
        assert_eq!(proposer_slot(12, 1, 10), 3);
        // A single (or empty) validator set always proposes.
        assert_eq!(proposer_slot(9, 4, 1), 0);
        assert_eq!(proposer_slot(9, 4, 0), 0);
    }

    /// Extreme heights and rounds cannot wrap or panic the rotation: the slot
    /// is the mathematical `(height + round) % n` computed without overflow.
    #[test]
    fn proposer_slot_survives_the_extremes() {
        let expected = ((u128::from(u64::MAX) + u128::from(u32::MAX)) % 3) as usize;
        assert_eq!(proposer_slot(u64::MAX, u32::MAX, 3), expected);
        assert!(proposer_slot(u64::MAX, u32::MAX, 3) < 3);
        assert_eq!(
            proposer_slot(u64::MAX, 0, 7),
            (u128::from(u64::MAX) % 7) as usize
        );
    }

    /// The decision table: first vote inserts, same hash duplicates, a
    /// different hash in the same context is the only equivocation.
    #[test]
    fn receipt_action_is_the_equivocation_table() {
        assert_eq!(receipt_action(false, false), ReceiptAction::Insert);
        assert_eq!(receipt_action(false, true), ReceiptAction::Insert);
        assert_eq!(receipt_action(true, true), ReceiptAction::Duplicate);
        assert_eq!(receipt_action(true, false), ReceiptAction::Equivocation);
    }

    /// Retirement keeps the bound and everything above, drops everything below.
    #[test]
    fn should_retain_is_a_lower_bound() {
        assert!(should_retain(6, 6));
        assert!(should_retain(7, 6));
        assert!(!should_retain(5, 6));
        assert!(should_retain(0, 0));
    }
}
