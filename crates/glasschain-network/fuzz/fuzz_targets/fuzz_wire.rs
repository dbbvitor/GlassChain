// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Fuzz the P2P wire decode surface (`glasschain_network::protocol::Message`).
//!
//! Peers feed arbitrary bytes into `serde_json::from_slice::<Message>`; any
//! panic (or unbounded recursion) on malformed input is a denial-of-service
//! vector. The harness asserts decode never panics; errors are fine.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<glasschain_network::protocol::Message>(data);
});
