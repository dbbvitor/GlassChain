# GlassChain

> **A federated distributed ledger for transparent supply-chain transactions, written in Rust.**

GlassChain connects buyers and sellers across a peer-to-peer network, giving participants a real-time, tamper-evident view of offers, orders, inventory events, and custody metadata. Contracts and watcher hooks can autonomously generate transactions from ledger events.

---

## Features

| Feature | Description |
|---|---|
| **Distributed Ledger** | SHA-256 chained blocks committed behind a `ConsensusProvider` seam that carries a quorum certificate. Proof-of-Work is the working default; longest-chain fork resolution was retired for a no-fork model |
| **Supply-Chain Transactions** | `SupplyOffer`, `PurchaseOrder`, `InventoryUpdate`, `AssetRegistration`, and canonical v1 records (`CanonicalRecord`) |
| **Canonical Schema v1** | 13 strict record families (lots, shipments, recall, certification, audit, state commitments, …) validated against an immutable network-wide registry before admission and commit |
| **Contract Automation** | `ContractCreation` rules auto-execute purchase flows on matching offers |
| **Watcher Automation** | Commit-phase inventory hooks can enqueue autonomous reorder purchase orders |
| **Regulatory Traceability** | Anvisa/SNCM-aligned metadata model with `MetadataTrustScore` scoring |
| **Federated Network** | Active TLS-encrypted TCP P2P protocol with certificate exchange, transaction/block broadcast, and sync; libp2p is experimental and currently unwired |
| **gRPC API** | Tonic/Prost server for ledger queries, tx submission, asset history, and event streams |
| **Indexer + Provenance** | In-memory indexing and custody-chain primitives for analytics/audit workflows |

---

## Workspace Structure

```text
GlassChain/
├── Cargo.toml                      # Workspace manifest (12 crates)
├── docs/                           # In-depth documentation and ADRs
└── crates/
    ├── glasschain-core/            # Ledger, blocks, tx model, canonical schema, provider traits
    ├── glasschain-contracts/       # Deterministic contract layer: registry, matching, approval gate
    ├── glasschain-workflows/       # I/O-driven automation: flows, checkpoints, watcher service
    ├── glasschain-vm/              # Wasmtime-backed execution provider and gas metering
    ├── glasschain-identity/        # MSP, identities, certificate verification, channels
    ├── glasschain-storage/         # Storage backends: in-memory, Sled, transient (private payloads)
    ├── glasschain-indexer/         # Indexing, event bus, provenance and lineage
    ├── glasschain-network/         # P2P node, wire protocol, peer handling
    ├── glasschain-rpc/             # gRPC service definitions and server
    ├── glasschain-sdk/             # Client SDK
    ├── glasschain-cli/             # Client CLI (contract, identity, inspect)
    └── glasschain-node/            # Interactive REPL node binary
```

Dependencies flow downward only — `glasschain-core` depends on nothing internal,
and the workspace has no cycles. See [`docs/architecture.md`](docs/architecture.md)
for the full graph and the provider seams that make it work.

### Packaging: contracts vs workflows (ticket #49)

The workspace mirrors Corda's CorDapp split — contract code and workflow code
are separate deployable modules (crates) with no dependency cycle:

- **Contract layer — verification-only, deterministic:** `glasschain-contracts`
  (the contract registry, condition matching, and the WASM approval gate — a
  `BTreeMap` registry, so emission order is deterministic across processes)
  with `glasschain-vm` executing guest code deterministically. **The
  deterministic-contract invariant:** the contract layer's evaluation and
  emission are pure functions of their inputs — no wall clock, no randomness,
  no network, no persistence — so replaying the same inputs is byte-identical
  and cross-node agreement is safe. (The chain model in `glasschain-core`
  stamps block/transaction timestamps at creation, outside this invariant.)
- **Workflow layer — I/O-driven:** `glasschain-workflows` (flow state
  machines, checkpoint persistence, and the `WatcherService` event automation
  that observes committed state and emits transactions).

Workflow code may depend on contract code (`glasschain-workflows` →
`glasschain-contracts` → `glasschain-core`); never the reverse.

---

## Getting Started

### Prerequisites

- Rust toolchain via [rustup](https://rustup.rs)

### Build

```bash
git clone https://github.com/dbbvitor/GlassChain.git
cd GlassChain
cargo build --release
```

Run a node:

```bash
cargo run --release -p glasschain-node -- --id node-1 --listen 0.0.0.0:8000
```

Run a node with an identity-backed TLS certificate:

```bash
cargo run --release -p glasschain-node -- \
  --id node-1 \
  --listen 0.0.0.0:8000 \
  --org PharmaCorp \
  --identity-node-id node-1
```

Starting with `--org` also attaches an MSP endorsement provider (see [Automation Model](#automation-model)): the node's identity is registered under a principal named after the organization, and enforcement begins once the `endorsement` capability is activated in-band.

Run two local peers:

```bash
# Terminal 1
cargo run --release -p glasschain-node -- --id node-1 --listen 0.0.0.0:8000

# Terminal 2
cargo run --release -p glasschain-node -- --id node-2 --listen 0.0.0.0:8001 --peer 127.0.0.1:8000
```

Peer transport is **TLS-encrypted by default in release builds**. By default, nodes exchange certificates during connection setup and pin the presented peer certificate for that session. The current trust model is deliberately TOFU-only; the node binary also supports **identity-backed TLS certificates** issued from the `glasschain-identity` crate, but organization CA verification is not attached to the handshake. The optional `GLASSCHAIN_INSECURE_TLS=1` escape hatch is intended only for local debugging.

---

## Interactive CLI Commands

```text
supply   <seller> <product_id> <product_name> <qty> <price> <lead_days> <currency>
order    <buyer> <seller> <product> <qty> <price> <currency>
contract <contract_id> <buyer> <product> <max_price> <min_qty> <max_qty> <max_lead> <currency>
inventory <owner> <product> <delta> <reason>
asset <originator> <product_name> <gtin> <batch> <expiry> <serial> <qty> <event_type>
chain
pending
peers
quit | exit
```

Block production is driven by the consensus layer, not by manual commands: the
`mine`/`mine-async` REPL commands and the `MineBlock` RPC were retired with the
quorum-certificate seam (ADR-002); the dev/test Proof-of-Work driver remains
available programmatically as `Node::mine()`.

### Consensus engines (ADR-002)

- **Proof-of-Work (default, dev/test):** every commit notification carries the
  degenerate quorum certificate — the valid nonce *is* the attestation.
- **Tendermint-class BFT (staged, default-off):** behind the `bft` cargo
  feature (`glasschain-core/bft`, `glasschain-network/bft`). When a
  `BftConsensusProvider` is attached and the `bft_consensus` capability is
  active at the candidate height, blocks are attested with real ed25519
  validator signatures over the block hash and committed with a
  cryptographically verified ≥⅔+ distinct-validator quorum; finality is
  single-slot and deterministic at commit. The commit consumer is identical for
  both engines. Activation is a capability activation per ADR-010: a signed
  control-plane record at a future height.

> **BFT adoption gates (ADR-010 §7 — all must pass before production use):** a
> GlassChain testnet at the target validator count, API/stability evidence,
> licensing/stewardship review, and a security audit. Malachite remains the
> staged engine candidate behind the seam; `tendermint-rs` is type/light-client
> tooling only, never the engine. The shipped engine's `attest` signs with the
> local key only — a 1-validator set is its own quorum — while
> `verify_certificate` verifies full ≥⅔+ quorums from its configured validator
> set. Gathering remote attestations, wire transport of certificates, BFT-block
> peer admission, BFT-chain sync (`try_replace_chain`) and restart persistence
> (both currently `PoW`-coupled), and validator-set changes are part of that
> staged work.

Every committed block carries the canonical write set of the accepted
persistent VM writes, covered by the block hash (ADR-007): public writes carry
their value, PDC-scoped writes carry only the collection name and the value's
SHA-256 commitment — the private value never enters the replicated block. The
block and its write set persist and apply through one atomic commit boundary
(`StorageProvider::apply_block`), and state rebuilds replay committed write
sets in block order without re-executing guest code.

---

## gRPC API (Current)

Proto path: `crates/glasschain-rpc/proto/glasschain/v1/glasschain.proto`  
Package: `glasschain.v1`

The `glasschain-rpc` crate exposes the current gRPC server implementation. The `glasschain-node` CLI binary does **not** start that server by default, but it can start `GlasschainServer` when `--rpc-addr` is provided. Without `--rpc-addr`, the node binary runs the interactive REPL and P2P networking layer only.

- `LedgerService`
  - `GetBlock`
  - `StreamBlocks`
  - `SubmitTransaction`
  - `GetChainStatus`
  - `QueryAssetHistory`
  - `GetVerifiableLineage`
  - `SubscribeToEvents` (server stream)
- `NodeService`
  - `GetNodeStatus`
  - `GetPeers`
- `IdentityService`
  - `ExchangeCertificate`
  - `VerifyEndorsement`

Analytics read path: `QueryAssetHistory` and `GetVerifiableLineage` are answered from the in-memory provenance index and analytical flattener — not by scanning the raw chain. `QueryAssetHistory` matches exact canonical asset ids (`GTIN:<gtin>[:SN:<sn>|:BATCH:<b>]`, `SN:<sn>`), boundary-anchored so a short GTIN never cross-matches a longer one, and each result's `payload_json` carries the custody event rather than the raw transaction JSON.

---

## Network Protocol

All peer messages are framed as:

- 4-byte big-endian length prefix (`u32`)
- UTF-8 JSON payload

Before the framed protocol begins, peers exchange certificates and then upgrade the connection to TLS. The presented peer certificate fingerprint is verified against the `Hello` message and recorded in a TOFU (Trust On First Use) registry. On subsequent connections the registry rejects any peer whose node ID or certificate fingerprint has changed for the same listen address. When started with `--org <NAME>`, the node issues a TLS certificate derived from its identity key material, so the same key backs both transaction signing and transport encryption.

Current trust model:
- **Default mode:** encrypted transport, per-session certificate pinning, and TOFU identity persistence across reconnects.
- **Identity-backed mode:** same deliberate TOFU default, plus the TLS certificate is derived from the node's identity key so that transport and transaction identity share one key pair.
- **libp2p mode:** experimental and currently unwired; reserved for the selective-disclosure roadmap.
- **Insecure mode:** only when explicitly enabled with `GLASSCHAIN_INSECURE_TLS=1` or the matching build feature.

> **Note:** TOFU trust is address-bound and in-memory. There is no shared CA between organizations and no trust persistence across process restarts. A peer that changes its listen address is treated as a new peer. These are known limitations, not bugs.
>
> For certificate-verified cross-organization trust, see [ADR-011](docs/adr/adr-011-federation-trust-store.md): start the node with `--trust-store <PATH>` (a PEM file or directory of the peer organizations' Root CA certificates, requires `--org`). Peers whose organization is not in the trust store stay connected but are not trusted on org-gated paths (private payloads). Without `--trust-store`, peer organizations are not certificate-verified and the startup log says so.
>
> Certificate-chain validation itself *is* implemented — `glasschain-identity`'s `CertChainVerifier` verifies a peer certificate against an organization Root CA using `rustls-webpki`, rejecting forged and tampered certificates — but it is intentionally not attached to the current TOFU handshake. A shared or multi-organization trust model must be chosen before enabling it.

Message types:

- `Hello` — advertises the wire `version` (mismatches are disconnected; current
  version is `glasschain/4`, bumped when the private-payload protocol gained
  pull-based reconciliation),
  the sender's `org` (the collection-membership principal), and the
  capabilities the peer supports. A peer lacking an active capability is
  treated as a read-only observer: it can parse and validate history but may
  not propose, vote, or relay active writes.
- `Transaction`
- `Block`
- `PrivatePayload` — a private data collection payload, sent **point-to-point
  between collection members only** (never broadcast). See "Private Data
  Collections" below.
- `RequestChain`
- `Chain`
- `RequestPeers`
- `Peers`
- `Goodbye`

---

## Private Data Collections (ADR-003)

Private payloads (pricing, quantities, counterparties, raw evidence) never
enter global replication. One global chain carries the facts; collections
carry the payloads.

**Collections** (`glasschain-identity`): a collection names its member
organizations and an optional endorsement-policy declaration. Membership —
who may read and receive payloads, and who may submit payloads locally — is a
separate control from endorsement: being a member never satisfies a policy.
The authoritative collection endorsement policy is a committed `PolicyUpdate`
carrying a collection-scoped `collection_policy`, evaluated by the endorsement
engine at the commit path over verified principals (ADR-008, ticket #45);
a node's local `ChannelConfig.endorsement_policy` is its declaration of that
policy, not an independent enforcement source. Regulator organizations
(Anvisa, MAPA) are members of every collection by default.

**The boundary model** — private cleartext exists only (a) inside the
writer's execution, (b) in `Message::PrivatePayload` between members, and (c)
in members' transient stores. Two author responsibilities keep it that way: a
guest must compute private values at runtime (a value embedded in a contract's
WASM data segment rides the committed contract definition), and private input
must enter through the payload path, not through public record fields.

- **Admission:** `submit_private_payload` requires the local org to be a
  collection member and the `pdc` capability to be active at the next height;
  PDC-scoped VM writes are dropped whole while the capability is inactive.
- **Transport:** payloads are sent point-to-point to peers whose advertised
  `org` is a member (a self-asserted string until certificate-verified
  delivery lands in #47); a payload pushed to a non-member, an
  unauthenticated peer, or with a commitment mismatch is rejected on receipt.
- **Storage/commit:** the block's write set carries the collection name and
  `sha256(value)` — never the value (`PersistentWrite::block_form`, ADR-007);
  the world-state mirror holds only commitments.
- **Replay:** state rebuilds from committed (redacted) write sets, so no
  replay path can resurrect cleartext.

**Non-member verification:** every node — member or not — holds the block
commitments. A non-member can verify that a private-data write occurred and
that its commitment is unaltered (`commitment == sha256(payload)`) without
ever reading the payload.

**Distribution (ticket #47):** a peer that was offline at dissemination time
reconciles by scanning the committed chain for the collection's PDC
commitments and requesting every payload its transient store is missing from
a member peer (`reconcile_private_payloads`); the answer travels the same
member-gated `PrivatePayload` path. Payloads are held for the collection's
retention window (`ChannelConfig.retention_secs`, default 72h) and purged on
sweep (`purge_expired_private_payloads`; retention is also enforced on read) —
payloads vanish, the chain's commitments persist forever, so a late auditor
can prove existence and consistency but not read contents. The purge sweep's
expiry index is in-memory: a restarted member cannot enumerate payloads
written before the restart.
When a node runs certificate verification (`set_cert_verifier`), the payload
path trusts only certificate-verified organizations: the sender's
Hello-carried organization certificate must verify against the org Root CA
with a subject CN equal to the claimed org; TOFU-only nodes still accept the
self-asserted org.

Staged remainder: gossipsub-based payload distribution requires per-member
encryption (gossipsub has no member admission control, so publishing
cleartext payloads to a topic would weaken the boundary); the libp2p swarm
stays the staged substrate.

### Capacity gate (ticket #48)

`cargo test -p glasschain-network --test consensus_capacity -- --ignored --nocapture`
runs the compact workload (anchored lots, certification anchors,
`state_commitment` batch records) at 200 and 300 validators in a star
topology, reporting block latency/size, certificate size, propagation
fan-out, pending-pool backpressure, partition recovery, and private-data
dissemination (measured separately). Recorded evidence with the staged-engine
caveats: `docs/benchmarks/consensus-capacity.md`. No production capacity
claim is made — ADR-010 §7's testnet/API/audit gates still apply.

---

## Automation Model

- **Contract Engine path:** `SupplyOffer` can trigger contract auto-execution and purchase transactions.
- **Watcher path:** committed `InventoryUpdate` events are processed in post-commit hooks and may generate autonomous reorder `PurchaseOrder` transactions.
- **Restart / sync behavior:** contract runtime state is rebuilt from the committed chain, and watcher inventory state is replayed from committed `InventoryUpdate` transactions after restore or chain replacement.
- **Identity-backed transport option:** starting the node with `--org <NAME>` and optional `--identity-node-id <ID>` derives the TLS certificate from the node's identity key. This binds the certificate fingerprint to the advertised node identity via the TOFU peer registry, but does not establish shared-CA trust between organizations.
- **Endorsement enforcement option:** starting with `--org <NAME>` also attaches an MSP endorsement provider (the node's own identity is registered under a principal named after the organization). Attaching the provider is necessary but not sufficient: enforcement additionally requires the `endorsement` capability to be active at the candidate height, activated in-band via a committed `CapabilityActivation` record. Without `--org`, no provider is attached and endorsement gates short-circuit; the startup log states explicitly which mode the node is in.

Example contract condition payload:

```json
{
  "max_price_per_unit": 1500,
  "min_quantity": 100,
  "max_quantity": 500,
  "max_lead_time_days": 10,
  "preferred_seller_id": null,
  "currency": "USD",
  "auto_execute": true
}
```

`max_price_per_unit` uses minor currency units (e.g. `1500` = `$15.00`).

---

## Testing

The full workspace suite passes on the pinned 1.95 toolchain — **546 tests**, with
clippy clean at `-D warnings`:

```bash
cargo fmt --all --check
cargo check   --workspace --all-targets --all-features --locked
cargo test    --workspace --all-targets --all-features --locked
cargo clippy  --workspace --all-targets --all-features --locked -- -D warnings
```

All four are CI gates on Ubuntu, macOS, and Windows. Consensus changes must be
validated in **both** feature configurations (default and `--all-features`),
because the `bft` feature gates real code paths.

A focused run while iterating:

```bash
cargo test -p glasschain-network -p glasschain-rpc -p glasschain-node
```

See [`docs/operations.md`](docs/operations.md) for benchmarks and the capacity gate.

---

## Documentation

[`docs/`](docs/README.md) holds the in-depth technical documentation — written
against the shipped code, and explicit about what is designed but not yet wired.

| Document | What it covers |
|---|---|
| [Architecture](docs/architecture.md) | Crate map, dependency rule, provider seams, a transaction traced end to end |
| [Data model](docs/data-model.md) | Transaction kinds, all 13 canonical schema v1 record families, blocks, write sets, capabilities |
| [Consensus](docs/consensus.md) | What runs today, the staged BFT engine, the adoption gates, the membership ladder |
| [Privacy and identity](docs/privacy-and-identity.md) | MSP, certificate verification, endorsement policy, private data collections |
| [Workflows and contracts](docs/workflows-and-contracts.md) | The contract/workflow split, the WASM host ABI, flows, watcher automation |
| [Operations](docs/operations.md) | Build, flags, REPL, gRPC, storage, wire protocol, operator security warnings |

[`docs/adr/`](docs/adr/) holds the nine accepted architecture decision records.
Read the one covering your area before designing a change.

---

## Roadmap

The live programme is tracked in
[`.agents/plans/requirements-alignment.md`](.agents/plans/requirements-alignment.md)
— a 26-requirement traceability matrix against the Hybrid Distributed Inventory
System specification. Canonical schema v1, VM write sets, capabilities, the
endorsement engine and its enforcement gates, quorum certificates, staged BFT,
private data collections with reconciliation, the analytics read path, and the
workflow flows have all shipped.

Next up:

| Work | Status |
|---|---|
| [Federation trust model](https://github.com/dbbvitor/GlassChain/issues/57) — certificate verification is inert in production and fails open | Needs a decision |
| [Certificate revocation](https://github.com/dbbvitor/GlassChain/issues/58) — no CRL or OCSP | Unplanned gap |
| [Endorsement provider at node startup](https://github.com/dbbvitor/GlassChain/issues/59) — the engine is inert outside tests | Ready |
| [Record signature binding](https://github.com/dbbvitor/GlassChain/issues/60) — record and capability-activation signatures are count-only | Needs a decision |
| [`glasschain-demo`](https://github.com/dbbvitor/GlassChain/issues/61) — a visual demo and benchmark harness | Planned |
| [Performance](https://github.com/dbbvitor/GlassChain/issues/62) — measure real BFT rounds, then wire encoding, batch verification, BLS | Ordered, step 0 blocking |
| BFT production adoption | Blocked on the four ADR-010 gates |

### Performance — the constraint is the advantage

Latency and scalability are treated as **sell factors**, subject to zero trust
between validators, Brazilian legal compliance, and ICP-Brasil interoperability.

On scalability the peer group is Hyperledger Fabric and Corda, and this is where
the constraints pay: Fabric's default ordering service is Raft — **crash-fault
tolerant only**, which assumes orderers do not lie, exactly the assumption a
consortium of commercial rivals cannot make. So the claim is not "limited to 300
validators" but **300 mutually-distrusting validators with deterministic
finality, plus an authenticated light-client ladder to the 70M-participant
horizon**.

On latency the bar is sub-second, and it is reachable **inside** the consensus
family ADR-002 already chose — the in-family reference point is Malachite's own
~780 ms finalization at 100 validators with 1 MB blocks, against our 11.5 KB
blocks. No family swap is needed to be best in class.

**What is honestly missing is the measurement.** The committed capacity gate
measures the dev/test Proof-of-Work engine and contains no attestation rounds, so
there is currently no BFT finality number — producing one is the blocking first
step. After it: the peer wire protocol is JSON, which renders byte arrays as
decimal digits and inflates a quorum certificate about five-fold, and certificates
are not persisted with blocks yet, so the encoding is cheap to fix now and
expensive later. Then batch signature verification, then BLS aggregation to make
light-client proofs cheap. Speculative fast paths and a DAG mempool are
candidates behind those, not ahead of them.

[`docs/consensus.md` §10](docs/consensus.md) has the detail;
[`.agents/plans/performance.md`](.agents/plans/performance.md) has the ordered
path and what the constraints permit.

### `glasschain-demo` — seeing it work

A planned desktop application, built on [gpui](https://crates.io/crates/gpui),
that drives a live in-process federation with synthetic supply-chain traffic and
renders it: custody transfers and endorsements as they happen, private-payload
exchange, a recall cascading across organizations, live throughput plots, and a
lot traced from shelf back to origin in a few clicks. Synthetic records go
through real validation, real endorsement, and the real PDC boundary — a demo
that can produce a record the node would reject is a demo that lies.

It will live outside the Cargo workspace with its own lockfile and CI job, so a
pre-1.0 GUI dependency can never affect the four mandatory gates. Its numbers
are illustrative and are explicitly **not** the ADR-010 adoption-gate benchmark;
[`docs/benchmarks/`](docs/benchmarks/consensus-capacity.md) stays authoritative.
Plan: [`.agents/plans/gui-demo-benchmark.md`](.agents/plans/gui-demo-benchmark.md).

---

## License

[Apache 2.0](LICENSE)
