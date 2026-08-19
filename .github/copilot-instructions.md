# GitHub Copilot instructions — GlassChain

**The canonical instructions for this repository are in [`AGENTS.md`](../AGENTS.md).**
Read that file first and follow it. This file is a condensed pointer; when the two
disagree, `AGENTS.md` wins and should be the file you update.

## What this repository is

GlassChain is a federated distributed ledger for transparent supply-chain
transactions, written in Rust. It has SHA-256 chained blocks with Proof-of-Work
consensus, supply-chain transaction types, a contract/watcher automation engine, a
TLS-encrypted TCP + libp2p P2P layer, and a Tonic/Prost gRPC API. It is a Cargo
workspace of 11 crates (~16k lines of Rust) on the Rust **1.95** toolchain pinned
by `rust-toolchain.toml`, edition 2021, async on Tokio.

## Build and validate

CI (`.github/workflows/ci.yml`) runs strict formatting and lint gates, a
check/test matrix on Ubuntu, macOS, and Windows, coverage, and a RustSec audit on
every push and PR. A cold build takes minutes — always validate locally first.
Run these from the repository root:

1. `cargo fmt --all --check`
2. `cargo check --workspace --all-targets --all-features --locked`
3. `cargo test --workspace --all-targets --all-features --locked`
4. `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`

Notes that will save you a failed run:

- A first clean build downloads and compiles `wasmtime`, `libp2p`, and `tonic`;
  expect several minutes. Subsequent builds are fast.
- `cargo fmt --all --check` currently passes. Clippy with `-D warnings` exposes
  existing Cargo metadata and pedantic/nursery diagnostics. Do not weaken the CI
  gates; format only files you touched when making unrelated changes.
- Never run `cargo run -p glasschain-node` in an automated step: it starts an
  interactive REPL that blocks on stdin. Use the integration tests in
  `crates/glasschain-network/tests/` instead.

## Where code lives

`crates/glasschain-core` (blocks, ledger, transactions, provider traits, SNCM
schema) depends on nothing internal. Everything else builds on it:
`contracts`, `storage`, `vm`, `identity`, `indexer` → `network` → `rpc` → `sdk` →
`node` and `cli`. **The workspace has no circular dependencies; do not introduce
one.** If a lower crate needs behavior from a higher one, define a trait in
`glasschain-core` and inject the implementation.

Changing the gRPC surface requires editing both
`crates/glasschain-rpc/proto/glasschain/v1/glasschain.proto` and
`crates/glasschain-rpc/src/server.rs`; `build.rs` regenerates the bindings.

See the "Where to make changes" table in `AGENTS.md` for a per-topic file map, and
[`PLUGIN_KIT.md`](../PLUGIN_KIT.md) for the provider-trait reference.

## Code conventions

- **No `unsafe`** — `unsafe_code = "deny"` workspace-wide, and there is currently
  none. New crates must add `[lints] workspace = true`.
- Errors: per-crate `error.rs` with a `thiserror` enum; propagate with `?`. No
  `unwrap()`/`expect()` in library code — they are allowed only inside `#[test]`.
- Logging: the `log` crate in libraries (`env_logger` only in binaries), with
  inline format captures such as `log::warn!("bad addr {addr:?}: {e}")`.
- Serialization is `serde` derive; the peer wire protocol is JSON.
- Currency is always an integer in minor units (`1500` = `$15.00`) — never a float.
- Identifiers must be at least 2 characters (`id`, `tx`, `rx` are fine; `x` is not).
- Add or update tests for every behavior change. Unit tests go in `#[cfg(test)] mod
  tests` next to the code; integration tests go in `crates/glasschain-network/tests/`.

## Security

Peer transport is TLS-encrypted by default with certificate-fingerprint
verification and an in-memory TOFU peer registry. Do not weaken or bypass it.
`GLASSCHAIN_INSECURE_TLS=1` and the `insecure-tls` feature are local-debugging
escape hatches only — never make them the default and never add new env-var kill
switches for security controls. Never commit keys, certificates, or `.pem` files;
identity material is generated at runtime. Signing is ed25519, hashing is SHA-256.

The address-bound in-memory TOFU model, the lack of a shared CA, and the lack of
trust persistence across restarts are known and accepted limitations. Do not
silently "fix" them inside an unrelated change.

## Working artifacts and PRs

Put plans, task breakdowns, and durable findings in `.agents/plans/`,
`.agents/tasks/`, and `.agents/memories/` — never source code, secrets, or build
output. Commit subjects follow a loose Conventional Commits style (`feat:`,
`fix:`, `refactor:`) under ~72 characters. Update `README.md` when CLI flags, REPL
commands, the wire protocol, or the gRPC surface change, and `PLUGIN_KIT.md` when
a provider trait changes. Never commit anything from `target/`.

## Trust these instructions

Rely on the commands and paths above and in `AGENTS.md` rather than re-exploring
the repository. Search only when this information is incomplete or turns out to be
wrong — and when it is, update `AGENTS.md` as part of your change.
