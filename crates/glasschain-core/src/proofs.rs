// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Kani proof harnesses for the pure, sequential surface of `glasschain-core`
//! (wayfinder #157, #166). Run with
//! `cargo kani -p glasschain-core --default-unwind 16`.
//!
//! ## Scope
//!
//! Three predicates verify: the ISO-8601 structural check (heap-free), the
//! allocation-free proof-of-work prefix predicate, and the expiry-date
//! contribution to the trust score (bounded symbolic heap: one symbolic
//! dimension, concrete allocations).
//!
//! ## The recorded limits of kani-verifier 0.68.0 / CBMC 6.11.0
//!
//! * Harnesses that reach SHA-256 (any `capability_hash` path) pull in the
//!   `llvm.x86.sha256*` intrinsics, which Kani reports as unsupported.
//! * Unbounded symbolic heap — six independently symbolic `Option<String>`
//!   asset fields — aborts CBMC (`status 15` during propositional reduction).
//!   Bounding the symbolic dimension to the digits of one field is enough.
//! * `schema::validate_asset` never finishes within a practical bound: the
//!   violation messages it `format!`s explode the formula.
//! * `Block::new` reads the system clock, a foreign call Kani cannot model
//!   (harnesses call the predicate helpers directly).
//!
//! Expansion is deferred with the evidence in
//! `.agents/memories/kani-deferral.md`.

use crate::asset::{is_valid_iso8601_date, MetadataTrustScore, TraceableAsset};
use crate::block::has_leading_zeros;

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

/// Proof-of-work prefix semantics, allocation-free: difficulty zero always
/// passes, a hash shorter than the difficulty never passes, and a passing
/// prefix stays passing at any lower difficulty.
#[kani::proof]
fn proof_of_work_is_prefix_monotone() {
    let bytes: [u8; 8] = kani::any();
    let Ok(hash) = std::str::from_utf8(&bytes) else {
        return;
    };
    let low: usize = kani::any();
    let high: usize = kani::any();
    kani::assume(low <= high && high <= 8);
    assert!(has_leading_zeros(hash, 0));
    if high > hash.len() {
        assert!(!has_leading_zeros(hash, high));
    }
    if has_leading_zeros(hash, high) {
        assert!(
            has_leading_zeros(hash, low),
            "a lower difficulty must accept the same hash"
        );
        assert!(
            hash.as_bytes()[..high].iter().all(|&byte| byte == b'0'),
            "a passing hash has `high` leading zero bytes"
        );
    }
}

/// The expiry-date contribution to the trust score matches the structural
/// check: the score is 60 plus 20 exactly when the date is well formed.
#[kani::proof]
fn trust_score_expiry_contribution() {
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
    let asset = TraceableAsset {
        gtin: Some("x".into()),
        batch_number: Some("x".into()),
        expiry_date: Some(date.into()),
        serial_number: Some("x".into()),
        anvisa_registration: None,
        manufacturer_id: None,
        product_name: String::new(),
        custodian_id: String::new(),
        country_of_origin: None,
        storage_temp_celsius: None,
        quantity: 0,
    };
    let score = MetadataTrustScore::compute(&asset);
    let expected = 60 + if is_valid_iso8601_date(date) { 20 } else { 0 };
    assert_eq!(score.score, expected);
}
