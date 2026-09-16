// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Shared loopback-port allocator for the network integration tests.
//!
//! `free_addr` asks the OS for an ephemeral port (`127.0.0.1:0`) and **keeps
//! the socket**, parked in the node crate's pre-bound registry;
//! `Node::start` adopts that exact socket instead of re-binding.
//!
//! The probe→drop→rebind pattern this replaces is the classic Windows CI
//! flake (`AddrInUse`, WSAEADDRINUSE 10048, `chaos_tests` 2026-09-16): a
//! port probed free and then dropped stays free for seconds in a long test,
//! and in that window the OS can hand it to an unrelated outbound
//! connection as its source port, or a `TIME_WAIT` residue blocks the rebind
//! outright. Windows refuses both; Linux and macOS mask them. Holding the
//! socket closes the window — the address is continuously in use from
//! allocation until the node adopts it, and the OS never assigns a held
//! port to anything else.
//!
//! `glasschain-rpc/tests/server_integration.rs` keeps an identical local
//! copy (it cannot reach this file); mirror any change there.

/// Allocate a unique loopback address, keeping the bound socket parked for
/// the node that will use it.
#[must_use]
pub fn free_addr() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("bound address").to_string();
    glasschain_network::stash_prebound_listener(&addr, listener);
    addr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_calls_return_distinct_addrs() {
        assert_ne!(free_addr(), free_addr());
    }

    #[test]
    fn stashed_addr_stays_bound_until_adopted() {
        let addr = free_addr();
        // The held socket *is* the allocation: the port cannot fall to
        // anyone else in between (the Windows probe→drop→rebind flake).
        assert!(std::net::TcpListener::bind(&addr).is_err());
    }
}
