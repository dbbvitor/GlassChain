# GlassChain

> **A federated distributed ledger for transparent supply-chain transactions, written in Rust.**

GlassChain connects buyers and sellers across a peer-to-peer network, giving participants a real-time, tamper-evident view of offers, orders, inventory events, and custody metadata. Contracts and watcher hooks can autonomously generate transactions from ledger events.

---

## Features

| Feature | Description |
|---|---|
| **Distributed Ledger** | SHA-256 chained blocks with Proof-of-Work consensus and longest-chain resolution |
| **Supply-Chain Transactions** | `SupplyOffer`, `PurchaseOrder`, `InventoryUpdate`, and `AssetRegistration` |
| **Contract Automation** | `ContractCreation` rules auto-execute purchase flows on matching offers |
| **Watcher Automation** | Commit-phase inventory hooks can enqueue autonomous reorder purchase orders |
| **Regulatory Traceability** | Anvisa/SNCM-aligned metadata model with `MetadataTrustScore` scoring |
| **Federated Network** | TCP-based P2P protocol with handshake, transaction/block broadcast, and sync |
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

Run two local peers:

```bash
# Terminal 1
cargo run --release -p glasschain-node -- --id node-1 --listen 0.0.0.0:8000

# Terminal 2
cargo run --release -p glasschain-node -- --id node-2 --listen 0.0.0.0:8001 --peer 127.0.0.1:8000
```

---

## Interactive CLI Commands

```text
supply   <seller> <product> <qty> <price> <lead_days> <currency>
order    <buyer> <seller> <product> <qty> <price> <currency>
contract <contract_id> <buyer> <product> <max_price> <min_qty> <max_qty> <max_lead> <currency>
inventory <owner> <product> <delta> <reason>
asset <originator> <product_name> <gtin> <batch> <expiry> <serial> <qty> <event_type>
mine
chain
pending
peers
quit | exit
```

`asset` prints Metadata Trust Score at submission time. Use `-` for optional metadata fields.

---

## gRPC API (Current)

Proto path: `crates/glasschain-rpc/proto/glasschain.proto`

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
  - `MineBlock`

---

## Network Protocol

All peer messages are framed as:

- 4-byte big-endian length prefix (`u32`)
- UTF-8 JSON payload

Message types:

- `Hello`
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

Example contract condition payload:

```json
{
  "max_price_per_unit": 15.0,
  "min_quantity": 100,
  "max_quantity": 500,
  "max_lead_time_days": 10,
  "preferred_seller_id": null,
  "currency": "USD",
  "auto_execute": true
}
```

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
