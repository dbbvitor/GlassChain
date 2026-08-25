# Low-risk cleanup

**Status:** complete — validated 2026-08-18
**Date:** 2026-08-18

## Goal
Apply cleanup that does not remove roadmap, security, or public API capabilities.

## Scope
- Remove unused direct dependencies and oversized Tokio/Criterion feature sets.
- Make the CLI’s currently synchronous command path synchronous.
- Deduplicate `Node` constructor setup.
- Remove clearly unused WASM host fields and duplicate JSON parsing.

## Out of scope
Keep planned or security-sensitive modules: libp2p, analytics/provenance, endorsement, channels, schema validation, consensus seams, gas accounting, madsim, `PeerConnection`, and `ledger-inspect`.

## Validation
- `cargo fmt --check` on touched Rust files where practical
- `cargo check --workspace --all-targets`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets`
