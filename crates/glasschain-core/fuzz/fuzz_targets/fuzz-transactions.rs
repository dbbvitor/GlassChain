// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Fuzz transaction decoding (`glasschain_core::Transaction`).
//!
//! Transactions arrive as untrusted JSON from peers and gRPC submissions;
//! any panic on malformed input is a denial-of-service vector. The harness
//! asserts decode never panics; validation errors are expected and fine.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<glasschain_core::Transaction>(data);
});
