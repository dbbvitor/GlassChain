// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Kani proof harnesses for the identity crate's zero-trust byte surfaces
//! (wayfinder #165). Run with
//! `cargo kani -p glasschain-identity --default-unwind 16`.
//!
//! ## Scope
//!
//! The trust-boundary logic that is byte arithmetic rather than a crypto
//! primitive, allocation-free, no clock, no network:
//!
//! * OCSP serial comparison (`minimal_be`): strips exactly the leading zero
//!   bytes, idempotent, non-empty iff a non-zero byte exists — two serials
//!   compare equal iff their minimal forms match.
//! * DER TLV framing (`read_tlv`, the OCSP staple parser's first stage):
//!   arbitrary bytes never panic and never read past the buffer; a decoded
//!   element's tag is the input's first byte and its contents sit at the end
//!   of the consumed region.
//!
//! The signed message encoders (`org_possession_message`, `tofu_pin_message`,
//! `msp_registration_message`) were tried and deferred: CBMC does not finish
//! on their symbolic-length `Vec<u8>` builders (the same symbolic-heap limit
//! recorded in `.agents/memories/kani-deferral.md`). Their exact bytes are
//! pinned by unit tests and mutation coverage instead.
//!
//! The crypto underneath (`ed25519-dalek`, `ring`, `x509-cert`, `webpki`) is
//! outside Kani's reach and stays test + mutation covered. The zero-trust
//! policy is Verus first, Kani for what Verus cannot express (ADR-019).

use crate::ocsp::{minimal_be, read_tlv};

/// Leading zero bytes are stripped and nothing else: the result is a suffix
/// of the input, is idempotent, and is empty exactly when the input is
/// all-zero.
#[kani::proof]
fn minimal_be_strips_only_leading_zeros() {
    let bytes: [u8; 8] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let input = &bytes[..len];

    let minimal = minimal_be(input);
    assert!(minimal.len() <= input.len());
    assert!(input.ends_with(minimal));
    assert_eq!(minimal_be(minimal), minimal);
    if minimal.is_empty() {
        assert!(input.iter().all(|byte| *byte == 0));
    } else {
        assert_ne!(minimal[0], 0);
    }
}

/// Arbitrary bytes never make `read_tlv` panic or report a region outside
/// the buffer: on success the tag is the input's first byte, the contents
/// end exactly at the consumed offset, and that offset never exceeds the
/// input length.
#[kani::proof]
fn read_tlv_never_overreads() {
    let bytes: [u8; 8] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let input = &bytes[..len];

    if let Ok((element, consumed)) = read_tlv(input) {
        assert_eq!(element.tag, input[0]);
        assert!(consumed <= input.len());
        if consumed <= input.len() {
            assert!(input[..consumed].ends_with(element.contents));
        }
    }
}
