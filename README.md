# GlassChain

> A federated distributed ledger for transparent supply-chain transactions, written in Rust.

GlassChain shares a tamper-evident record of offers, orders, inventory, and
custody across a federated peer-to-peer network. It is a 12-crate Rust workspace,
pinned to Rust 1.95 and async on Tokio.

---

## What GlassChain does

- **Distributed ledger** — SHA-256 chained blocks behind a pluggable
  `ConsensusProvider` seam; Proof-of-Work is the working default (dev/test).
- **Transactions & schema v1** — `SupplyOffer`, `PurchaseOrder`,
  `InventoryUpdate`, `TraceableAssetRegistration`, contract records, and 13
  strict `CanonicalRecord` families validated against a network-wide registry.
- **Contract + watcher automation** — deterministic contract rules auto-execute
  purchase flows; commit-phase watchers can enqueue autonomous reorders.
- **Private data collections** — pricing, quantities, and raw evidence travel
  point-to-point between collection members; the chain carries only SHA-256
  commitments, never the payload.
- **Identity & endorsement** — MSP identities, organization CAs, endorsement
  policies over verified principals, CRL-based revocation (ADR-013).
- **Federated network** — TLS-encrypted TCP P2P with certificate-fingerprint
  pinning and an in-memory TOFU peer registry.
- **gRPC API** — Tonic/Prost server (ledger queries, tx submission, asset
  history, event streams), plus Rust transaction-building SDK helpers and a
    `glasschain` CLI. The SDK is not yet a complete network client.

## Capability status — real today vs. staged

| Capability | Status |
|---|---|
| Ledger, PoW driver, transaction model, schema v1 | **Shipped, on by default** |
| Contract engine, watcher automation, workflows | **Shipped** |
| Private data collections (PDC) | **Shipped** (ADR-003) |
| gRPC server | **Shipped, opt-in** — starts only with `--rpc-addr` |
| TLS transport + fingerprint TOFU | **On by default** |
| Cross-organization certificate verification | **Opt-in** — `--org` + `--trust-store`; off by default |
| Tendermint-class BFT consensus | **Staged, default-off** (`bft` cargo feature) |
| libp2p transport | **Experimental, currently unwired** |
| Block mining from the node binary | **In no shipped binary** — the PoW driver is programmatic (`Node::mine()`); transactions stay `pending` until a mining peer broadcasts a block |
| Regulatory certification | **None** |
| Performance | Local harness results published; production scale and best-in-class performance not established |

---

## Quick start (local development)

### Prerequisites

- Rust via [rustup](https://rustup.rs) — `rust-toolchain.toml` pins 1.95.
- `protoc` — needed to build `glasschain-rpc` (compiles the `.proto`; not vendored).

### Build

```bash
git clone https://github.com/dbbvitor/GlassChain.git
cd GlassChain
cargo build --release
```

### Run two local peers (bound to localhost)

Run each command in a separate terminal. These peers accept transactions but do
not produce blocks autonomously; the programmatic driver is used by the tests.

```bash
cargo run --release -p glasschain-node -- --id node-1 --listen 127.0.0.1:8000

cargo run --release -p glasschain-node -- --id node-2 --listen 127.0.0.1:8001 \
  --peer 127.0.0.1:8000
```

⚠️ `glasschain-node` drops into an **interactive REPL** that blocks on stdin —
fine in a terminal, never in scripts or automation (CI uses the integration
tests in `crates/glasschain-network/tests/`). gRPC starts only with `--rpc-addr`
(e.g. `127.0.0.1:50051`); identity-backed TLS uses `--org PharmaCorp
[--identity-node-id node-1]`; cross-org verification adds `--trust-store <PATH>`
(requires `--org`). Full flag/REPL reference: [`docs/operations.md`](docs/operations.md).

### Validate

Run the repository's validation gates locally (CI also checks platform compatibility):

```bash
cargo fmt --all --check
cargo check --workspace --all-targets --all-features --locked
cargo test  --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

---

## Read this before trusting it with real data

- **PoW is the dev/test default.** Its nonce proves work, not validator
  agreement; the API's degenerate certificate is not a BFT quorum proof.
- **BFT is staged, default-off** (`bft` feature); production adoption waits on the
  ADR-010 §7 gates: testnet, API/stability evidence, licensing review and security
  audit. The current local tests do not establish production readiness.
- **Peer organizations are self-asserted by default.** Verification is opt-in
  (`--org` + `--trust-store`, fail-closed CRLs); otherwise TOFU fingerprint pinning is the only trust boundary — in-memory, address-bound, no shared CA.
- **Not regulatory-certified.** The schema is *aligned* with Anvisa/SNCM
  reporting concepts, but there is no certification, audit, or deployment.
- **No best-in-class claims** — only internal harness results are published, with explicit caveats.
- `GLASSCHAIN_INSECURE_TLS=1` / `insecure-tls` bypass TLS verification — local debugging only.

---

## Repository navigation

Dependencies flow downward only — `glasschain-core` depends on nothing internal;
the workspace has no cycles:
`core ← {contracts, storage, vm, identity, indexer} ← workflows ← network ← rpc ← sdk ← {node, cli}`

| Area | Crates |
|---|---|
| **Ledger & data** | `glasschain-core` (blocks, transactions, schema v1, provider seams) · `glasschain-storage` (in-memory / Sled / transient) · `glasschain-indexer` (event bus, provenance index) |
| **Automation** | `glasschain-contracts` (deterministic registry & matching) · `glasschain-vm` (Wasmtime + gas) · `glasschain-workflows` (flows, watchers) |
| **Identity & privacy** | `glasschain-identity` (MSP, CAs, channels, endorsement, private collections) |
| **Network & interfaces** | `glasschain-network` (P2P, wire protocol) · `glasschain-rpc` (gRPC) · `glasschain-sdk` (client) · `glasschain-cli` (CLI) · `glasschain-node` (REPL node) |

---

## Documentation

[`docs/`](docs/README.md) holds the in-depth, code-accurate documentation —
explicit about what is designed but not yet wired.

| Document | Covers |
|---|---|
| [Architecture](docs/architecture.md) | Crate map, dependency rule, provider seams, a transaction end to end |
| [Data model](docs/data-model.md) | Transaction kinds, 13 schema v1 families, blocks, write sets, capabilities |
| [Consensus](docs/consensus.md) | What runs today (PoW), the staged BFT engine, adoption gates |
| [Privacy & identity](docs/privacy-and-identity.md) | MSP, certificate verification, endorsement policy, private data collections |
| [Workflows & contracts](docs/workflows-and-contracts.md) | Contract/workflow split, WASM host ABI, watcher automation |
| [Operations](docs/operations.md) | Build, flags, REPL, gRPC, storage, wire protocol, operator warnings |
| [Liveness](docs/liveness.md) | Validator-set planning: failure domains, jurisdiction, participation |

- **ADRs** — 14 accepted records in [`docs/adr/`](docs/adr/) (index in [`docs/README.md`](docs/README.md)); read the one covering your area first.
- **Benchmarks** — [`docs/benchmarks/consensus-capacity.md`](docs/benchmarks/consensus-capacity.md) is the authoritative performance record (with explicit caveats).
- **Plans** — [roadmap and plan index](.agents/plans/README.md): requirements,
  zero trust, performance, post-quantum readiness and source-comment debt.
- **Visual demo** — a [planned browser web app](.agents/plans/gui-demo-benchmark.md)
  will demonstrate synthetic transactions, traceability and measured throughput.
  WebGPU is an optional rendering candidate with a non-GPU fallback; nothing is built yet.
- **Contributors** — [`AGENTS.md`](AGENTS.md) / [`.github/copilot-instructions.md`](.github/copilot-instructions.md) hold the invariants; [`PLUGIN_KIT.md`](PLUGIN_KIT.md) is the provider-trait & plugin reference.

---

## License

[Apache 2.0](LICENSE)