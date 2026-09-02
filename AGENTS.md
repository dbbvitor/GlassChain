# AGENTS.md

Guidance for AI coding agents working on **GlassChain**. This file is the single
source of truth for agent instructions; `CLAUDE.md` and
`.github/copilot-instructions.md` intentionally defer to it.

Human-facing docs live in [`README.md`](README.md) (overview, CLI usage) and
[`PLUGIN_KIT.md`](PLUGIN_KIT.md) (trait-by-trait plugin reference). Read those
before making architectural changes rather than re-deriving the design from source.

---

## Project overview

GlassChain is a federated distributed ledger for transparent supply-chain
transactions, written in Rust. It provides SHA-256 chained blocks with
Proof-of-Work consensus, supply-chain transaction types (`SupplyOffer`,
`PurchaseOrder`, `InventoryUpdate`, `TraceableAssetRegistration`), a contract +
watcher automation engine, a TLS-encrypted TCP/libp2p P2P layer, and a gRPC API.

- **Type:** Cargo workspace, 12 crates, ~37k lines of Rust across 93 files
  (tests and benches included).
- **Toolchain:** Rust **1.95** (pinned in `rust-toolchain.toml`), edition 2021.
- **Runtime:** Tokio async, `tonic`/`prost` for gRPC, `wasmtime` for contract execution.
- **CI:** `.github/workflows/ci.yml` runs strict rustfmt and clippy gates, a
  check/test matrix on Ubuntu, macOS, and Windows, code coverage, and a RustSec
  dependency audit on every push and PR. Coverage uploads require the
  `CODECOV_TOKEN` repository secret. It is a safety net, not a substitute — run
  the checks below locally before declaring work done, because a cold CI build
  takes minutes.

---

## Setup and commands

Everything is plain Cargo; no bootstrap script, no codegen step to run by hand
(`glasschain-rpc` and `glasschain-network` have `build.rs` scripts that run
automatically).

```bash
# Build
cargo build                       # debug
cargo build --release

# Type-check everything, including tests and benches (fast, do this often)
cargo check --workspace --all-targets --all-features --locked

# Test — the full workspace passes on the pinned 1.95 toolchain
cargo test --workspace --all-targets --all-features --locked

# Test a single crate / a single test
cargo test -p glasschain-network
cargo test -p glasschain-core --lib -- test_name_substring

# Lint — warnings are errors in CI
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# Format check
cargo fmt --all --check
```

**Always run `cargo test --workspace --all-targets --all-features --locked` and
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
before finishing a task.** The suite is fast (~5s of test time once compiled), and
finding a failure locally is far cheaper than finding it in CI.

### Running a node

```bash
# Single node
cargo run --release -p glasschain-node -- --id node-1 --listen 0.0.0.0:8000

# Second peer
cargo run --release -p glasschain-node -- --id node-2 --listen 0.0.0.0:8001 --peer 127.0.0.1:8000

# With identity-backed TLS and the gRPC server
cargo run --release -p glasschain-node -- \
  --id node-1 --listen 0.0.0.0:8000 \
  --org PharmaCorp --identity-node-id node-1 \
  --rpc-addr 0.0.0.0:50051
```

The node binary starts an interactive REPL. **Never launch it from an agent tool
call without a timeout** — it blocks on stdin and will hang the session. Prefer
the integration tests in `crates/glasschain-network/tests/` to exercise node behavior.

The gRPC server only starts when `--rpc-addr` is passed.

### Benchmarks

Criterion benches exist but are not part of the normal loop:

```bash
cargo bench -p glasschain-vm          # vm_throughput
cargo bench -p glasschain-workflows    # watcher_throughput
```

---

## Repository layout

```text
GlassChain/
├── AGENTS.md                   # This file — canonical agent instructions
├── CLAUDE.md                   # Pointer to AGENTS.md
├── .github/copilot-instructions.md  # Pointer to AGENTS.md
├── .agents/                    # Agent working artifacts (plans, tasks, memories)
├── Cargo.toml                  # Workspace manifest + workspace-wide lint config
├── clippy.toml                 # Clippy thresholds (MSRV 1.95, complexity limits)
├── rust-toolchain.toml         # Pins Rust 1.95
├── README.md                   # Human overview, CLI reference, protocol summary
├── PLUGIN_KIT.md               # Plugin/trait developer guide — read before extending
└── crates/
    ├── glasschain-core/        # Block, Transaction, Ledger, provider traits, SNCM schema
    ├── glasschain-contracts/   # ContractEngine, WatcherService (ECA triggers)
    ├── glasschain-network/     # TCP+TLS P2P node, protocol, libp2p Swarm
    ├── glasschain-node/        # Interactive REPL binary + gRPC wiring
    ├── glasschain-storage/     # SledStorageProvider, in-memory backend
    ├── glasschain-identity/    # Identity, Organization, Channel, EndorsementEngine, MSP
    ├── glasschain-vm/          # WasmExecutionProvider, GasCosts/GasCounter
    ├── glasschain-indexer/     # IndexerProvider, ProvenanceIndex, AnalyticalFlattener, EventBus
    ├── glasschain-rpc/         # gRPC services (Tonic + Prost) + auth
    ├── glasschain-sdk/         # High-level Rust client SDK
    └── glasschain-cli/         # `glasschain` binary: identity-gen, contract-deploy, ledger-inspect
```

### Dependency direction

```text
core ← {contracts, storage, vm, identity, indexer} ← workflows ← network ← rpc ← sdk ← {node, cli}
```

`glasschain-core` depends on nothing internal. **The workspace has no circular
dependencies — do not introduce one.** If a lower crate needs behavior from a
higher one, define a trait in `glasschain-core` and inject the implementation.

### Where to make changes

| If you're changing… | Go to |
|---|---|
| Block/ledger/hashing/PoW | `glasschain-core/src/{block,ledger,crypto}.rs` |
| A transaction type or payload | `glasschain-core/src/transaction.rs` |
| Traceability metadata / trust scoring | `glasschain-core/src/asset.rs` |
| SNCM/Anvisa schema validation | `glasschain-core/src/schema.rs` |
| A pluggable seam (consensus, storage, execution, network) | `glasschain-core/src/providers.rs` |
| Contract auto-execution | `glasschain-contracts/src/engine.rs` |
| Reorder watchers (ECA) | `glasschain-workflows/src/watcher.rs` |
| Peer wire format / message types | `glasschain-network/src/protocol.rs` |
| Peer lifecycle, TLS handshake, TOFU registry | `glasschain-network/src/{node,peer}.rs` |
| gRPC surface | `glasschain-rpc/proto/glasschain/v1/glasschain.proto` **and** `src/server.rs` |
| CLI REPL commands | `glasschain-node/src/main.rs` |

Adding a gRPC method requires editing the `.proto` **and** the server impl; the
`build.rs` regenerates bindings on the next build.

---

## Code style and conventions

Most style is enforced mechanically — read `Cargo.toml`'s `[workspace.lints]` and
`clippy.toml` rather than guessing.

- **No `unsafe`.** `unsafe_code = "deny"` workspace-wide. There is currently zero
  unsafe code. If you genuinely need it, add `#[allow(unsafe_code)]` with a
  `// SAFETY:` comment explaining soundness — and expect that to be questioned.
- **Clippy runs `all` + `pedantic` + `nursery` + `cargo` at warn level.** Groups
  are set at `priority = -1`, so targeted `#[allow(clippy::...)]` on an item wins.
  Use targeted allows with a one-line justification; don't relax the workspace config.
- **New crates must opt in** to the shared config with `[lints] workspace = true`.
- **Errors:** each crate defines its own `error.rs` with a `thiserror`-derived
  enum (`CoreError`, `NetworkError`, …). Propagate with `?`; do not `unwrap()` or
  `expect()` in library code. `unwrap`/`expect`/`panic!`/`dbg!`/`println!` are
  explicitly allowed inside `#[test]` functions (see `clippy.toml`).
- **Logging:** the `log` crate (`log::info!`, `log::warn!`) in libraries;
  `env_logger` is initialized only in binaries. Use inline format captures
  (`log::warn!("bad addr {addr:?}: {e}")`) — pedantic clippy enforces this.
- **Serialization:** `serde` with `derive` everywhere; the wire protocol is JSON.
- **Money:** prices are integers in **minor currency units** (`1500` = `$15.00`).
  Never introduce floats for currency.
- **Naming:** identifiers must be ≥2 characters (`min-ident-chars-threshold = 2`);
  `id`, `tx`, `rx` are fine, `x` is not.
- **Public API:** `avoid-breaking-exported-api = false`, so clippy will suggest
  signature changes on public items. Crate roots re-export the public surface via
  `pub use` in `lib.rs` — keep that list updated when you add public types.

### Formatting caveat

`cargo fmt --all --check` is a CI gate and currently passes in the workspace. Do
not run `cargo fmt --all` as part of an unrelated change — it can produce a large
unreviewable diff. Format only the files you touched:

```bash
cargo fmt -- crates/glasschain-sdk/src/client.rs
```

### Clippy strictness

`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
is the CI gate and currently passes. Keep it clean; do not weaken the gate or
hide unrelated warnings with broad `#[allow]` attributes.

---

## Testing instructions

- Unit tests live in `#[cfg(test)] mod tests` blocks inside the module they cover.
  Integration tests live in `crates/glasschain-network/tests/`
  (`node_integration.rs`, `chaos_tests.rs`, `madsim_chaos.rs`, `sncm_compliance.rs`).
- The full workspace currently has **209 passing tests** plus doctests, with zero
  failures. The README's warning about `wasmtime` breaking `cargo test` is stale —
  it builds fine on the pinned 1.95 toolchain.
- **Add or update tests for every behavior change, even if not asked.** Follow the
  surrounding style: table-ish `assert_eq!` cases, `#[tokio::test]` for async.
- Network tests bind real localhost ports and use timeouts. If a test hangs,
  suspect a port collision or a missing `tokio::time::timeout`, not a flaky suite.
- Doctests in `///` examples are compiled and run. If you add an example that
  can't run standalone, mark the fence `ignore` or `no_run` rather than letting it break.

---

## Security considerations

Treat these as invariants, not suggestions:

- **Peer transport is TLS-encrypted by default.** Peers exchange certificates,
  verify the fingerprint against the `Hello` message, and pin it in an in-memory
  TOFU registry. Do not weaken or bypass this path.
- `GLASSCHAIN_INSECURE_TLS=1` (and the `insecure-tls` feature on
  `glasschain-network`) disables verification. It is a **local-debugging escape
  hatch only**. Never make it the default, never widen its reach, and never add
  new env-var kill switches for security controls.
- **TOFU is the deliberate current trust default.**
  `glasschain-identity`'s `CertChainVerifier` performs a real `rustls-webpki`
  chain check against an organisation Root CA and defaults to
  `VerificationLevel::Full`, but `Node.cert_verifier` remains `None` in all four
  constructors. Keep the peer handshake described as TOFU-only until a shared or
  multi-organization trust model is chosen; do not silently enable local-CA
  verification, which would reject peers from other organizations.
- **Known, accepted limitations** (documented in README — do not "fix" them
  silently as part of an unrelated change): TOFU trust is address-bound and
  in-memory, there is no shared CA across organisations, and there is no trust
  persistence across restarts.
- Never commit keys, certificates, or `.pem` files. Identity material is generated
  at runtime by `glasschain-identity`.
- Signing is ed25519 (`ed25519-dalek`), hashing is SHA-256. Don't swap primitives
  without an explicit request.

---

## Agent skills

### Issue tracker

Issues and specs live in GitHub Issues for `dbbvitor/GlassChain`; use the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Use the default canonical triage labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, and `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository using root-level `CONTEXT.md` and
`docs/adr/`. See `docs/agents/domain.md`.

Accepted architecture decisions are **shipped documentation**, not agent scratch:
they live in [`docs/adr/`](docs/adr/) alongside the in-depth human docs, never in
`.agents/plans/`.

---

## Agent working artifacts (`.agents/`)

Use `./.agents/` for anything an agent produces that is *not* shipped code:

```text
.agents/
├── README.md      # Conventions for this folder
├── plans/         # Implementation plans and specs, one file per effort
├── tasks/         # Active task breakdowns and checklists
└── memories/      # Durable findings worth carrying between sessions
```

Rules:

- Write plans to `.agents/plans/<short-slug>.md` **before** starting any change
  that spans more than a couple of files, and reference it as you implement.
- Record non-obvious discoveries (a subtle invariant, a footgun, why an approach
  failed) in `.agents/memories/<topic>.md` so the next session doesn't rediscover them.
- Use subagents for broad exploration when that reduces context churn; keep their
  write scopes disjoint.
- Keep `.agents/` files short and current. Delete or archive a plan once it ships —
  stale plans are worse than no plans. An accepted decision is not a plan —
  promote it to an ADR in [`docs/adr/`](docs/adr/) before deleting the plan that
  produced it.
- **Never put source code, secrets, or generated build output in `.agents/`.**
- If a memory turns out to be a durable project rule, promote it into this file
  instead of leaving it buried in `.agents/memories/`.

See [`.agents/README.md`](.agents/README.md) for file templates.

---

## Commit and PR conventions

- Commit messages follow a loose Conventional Commits style already used in
  history: `feat:`, `fix:`, `refactor:`, or a short imperative sentence.
  Keep the subject under ~72 characters.
- **Do not commit, push, or open a PR unless explicitly asked.** Leave changes in
  the working tree for review.
- Before proposing a change as complete, state which of these you ran and what
  they returned: `cargo check --workspace --all-targets --all-features --locked`,
  `cargo test --workspace --all-targets --all-features --locked`, and
  `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`.
- Update `README.md` when you change CLI flags, REPL commands, the wire protocol,
  or the gRPC surface. Update `PLUGIN_KIT.md` when you change a provider trait.
- `target/` is ~29 GB and gitignored. Never add build output to a commit.

---

## Gotchas

- The pinned toolchain is 1.95; `clippy.toml` sets `msrv = "1.95.0"`. Don't use
  APIs newer than that, and don't bump `rust-toolchain.toml` casually.
- `README.md` lists 9 crates but the workspace has 11 (`glasschain-sdk` and
  `glasschain-cli` were added later). `PLUGIN_KIT.md` has the current list.
- The `glasschain-node` REPL and the gRPC server are separate: no `--rpc-addr`
  means no gRPC.
- Block production is consensus-driven, not manual: the `mine`/`mine-async`
  REPL commands and the `MineBlock` RPC were retired with the quorum-certificate
  seam (ticket #38). The dev/test Proof-of-Work driver remains available
  programmatically as `Node::mine()` / `Node::mine_async()`; PoW difficulty
  comes from `DEFAULT_DIFFICULTY` in `glasschain-core::ledger`.
- Contract and watcher state are **rebuilt by replaying the committed chain** on
  restart or chain replacement. Any new automation state must be replayable the
  same way, or it will silently diverge after a sync.
- A first clean build pulls `wasmtime`, `libp2p`, and `tonic` — expect several
  minutes. Use `cargo check` while iterating.

---

## Trust these instructions

Prefer the commands and paths above over re-exploring the repository. Search the
codebase only when this file is incomplete or you find it to be wrong — and when
you do, update this file as part of your change.
