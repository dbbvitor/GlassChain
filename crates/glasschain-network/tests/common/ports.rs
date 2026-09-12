//! Shared loopback-port allocator for the network integration tests.
//!
//! Probing `bind(":0")` and dropping the listener races with sibling tests:
//! the kernel can hand the same just-freed ephemeral port to two probes
//! before either node binds it (`AddrInUse` under parallel test threads —
//! `sncm_compliance`, 2026-09-03). Ports are reserved from a per-process band
//! below the OS ephemeral range (which starts at 32768 on Linux, 49152 on
//! macOS/Windows): the counter hands every caller in this process a distinct
//! port, and the bind probe skips ports held by anything else (other test
//! binaries run in their own process, hence the pid-seeded band). With no
//! shared window left, `cargo test`/tarpaulin run the harnesses in parallel.
//!
//! `glasschain-rpc/tests/server_integration.rs` keeps an identical local
//! copy (it cannot reach this file); mirror any change there.

/// Allocate a unique loopback port for this test process.
#[must_use]
pub fn free_addr() -> String {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let band = u16::try_from(std::process::id() % 32).expect("pid mod 32 fits u16");
    loop {
        let offset = NEXT.fetch_add(1, Ordering::Relaxed) % 300;
        let port = 22_000 + band * 300 + offset;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return format!("127.0.0.1:{port}");
        }
    }
}
