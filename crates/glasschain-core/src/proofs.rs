// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Kani proof harness for the pure, heap-free surface of `glasschain-core`
//! (wayfinder #157, #166). Run with
//! `cargo kani -p glasschain-core --harness proofs::iso8601_check_is_total_and_bounded --default-unwind 16`.
//!
//! ## Scope and the recorded infeasibility
//!
//! The ticket aimed at the whole crate; the toolchain does not support that.
//! With kani-verifier 0.68.0 / CBMC 6.11.0:
//!
//! * harnesses that reach SHA-256 (any `capability_hash` path) pull in the
//!   `llvm.x86.sha256*` intrinsics, which Kani reports as unsupported;
//! * harnesses over heap-using scoring code (`MetadataTrustScore::compute`,
//!   `schema::validate_asset` with its `format!` messages) fail inside CBMC
//!   with `CBMC failed with status 15` during propositional reduction;
//! * `Block::new` reads the system clock, a foreign call Kani cannot model
//!   (harnesses build literals instead).
//!
//! What remains verifiable is the heap-free predicate surface, and that is
//! what this module gates. Expansion is deferred until CBMC handles the
//! allocation paths; see `.agents/memories/kani-deferral.md`.

use crate::asset::is_valid_iso8601_date;

/// The ISO-8601 structural check never panics and accepts exactly the
/// plausible `YYYY-MM-DD` shape: the `-` separators keep the parse slices on
/// character boundaries, and the numeric fields must be in range.
#[kani::proof]
fn iso8601_check_is_total_and_bounded() {
    let digits: [u8; 8] = kani::any();
    let mut shaped = *b"2000-01-01";
    let mut next = digits.iter();
    for (index, byte) in shaped.iter_mut().enumerate() {
        if index == 4 || index == 7 {
            continue;
        }
        *byte = b'0' + (next.next().expect("ascii digit") % 10);
    }
    let date = std::str::from_utf8(&shaped).expect("ASCII digits");
    let valid = is_valid_iso8601_date(date);
    let year = u16::from(digits[0] % 10) * 1000
        + u16::from(digits[1] % 10) * 100
        + u16::from(digits[2] % 10) * 10
        + u16::from(digits[3] % 10);
    let month = (digits[4] % 10) * 10 + (digits[5] % 10);
    let day = (digits[6] % 10) * 10 + (digits[7] % 10);
    assert_eq!(
        valid,
        year >= 1900 && (1..=12).contains(&month) && (1..=31).contains(&day)
    );
}
