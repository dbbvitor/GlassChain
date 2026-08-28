# GlassChain

> **A federated distributed ledger for transparent supply-chain transactions, written in Rust.**

GlassChain connects buyers and sellers across a peer-to-peer network, giving participants a real-time, tamper-evident view of offers, orders, inventory events, and custody metadata. Contracts and watcher hooks can autonomously generate transactions from ledger events.

---

## Features

| Feature | Description |
|---|---|
| **Distributed Ledger** | SHA-256 chained blocks with Proof-of-Work consensus and longest-chain resolution |
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
├── Cargo.toml                      # Workspace manifest
└── crates/
    ├── glasschain-core/            # Ledger, blocks, tx model, provider traits, trust scoring
    ├── glasschain-contracts/       # Contract engine + watcher service
    ├── glasschain-network/         # P2P node, protocol, peer handling
    ├── glasschain-node/            # Interactive CLI node binary
    ├── glasschain-storage/         # Storage backends/adapters
    ├── glasschain-identity/        # Identity and signing primitives
    ├── glasschain-vm/              # Wasmtime-backed execution provider
    ├── glasschain-indexer/         # Indexing, event bus, provenance model
    └── glasschain-rpc/             # gRPC service definitions and server
```

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
  - `SubscribeToEvents` (server stream)
- `NodeService`
  - `GetNodeStatus`
  - `GetPeers`

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
> Certificate-chain validation itself *is* implemented — `glasschain-identity`'s `CertChainVerifier` verifies a peer certificate against an organization Root CA using `rustls-webpki`, rejecting forged and tampered certificates — but it is intentionally not attached to the current TOFU handshake. A shared or multi-organization trust model must be chosen before enabling it.

Message types:

- `Hello` — advertises the wire `version` (mismatches are disconnected) and the
  capabilities the peer supports. A peer lacking an active capability is
  treated as a read-only observer: it can parse and validate history but may
  not propose, vote, or relay active writes.
- `Transaction`
- `Block`
- `RequestChain`
- `Chain`
- `RequestPeers`
- `Peers`
- `Goodbye`

---

## Automation Model

- **Contract Engine path:** `SupplyOffer` can trigger contract auto-execution and purchase transactions.
- **Watcher path:** committed `InventoryUpdate` events are processed in post-commit hooks and may generate autonomous reorder `PurchaseOrder` transactions.
- **Restart / sync behavior:** contract runtime state is rebuilt from the committed chain, and watcher inventory state is replayed from committed `InventoryUpdate` transactions after restore or chain replacement.
- **Identity-backed transport option:** starting the node with `--org <NAME>` and optional `--identity-node-id <ID>` derives the TLS certificate from the node's identity key. This binds the certificate fingerprint to the advertised node identity via the TOFU peer registry, but does not establish shared-CA trust between organizations.

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

Focused test run for actively wired crates:

```bash
cargo test -p glasschain-network -p glasschain-rpc -p glasschain-node
```

For full workspace tests (`cargo test`), ensure your local Rust toolchain is compatible with all transitive dependencies (notably `wasmtime` in `glasschain-vm`).

---

## License

[Apache 2.0](LICENSE)
