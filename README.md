# GlassChain

> **A federated distributed ledger for transparent supply-chain transactions, written in Rust.**

GlassChain connects buyers and sellers across a peer-to-peer network, giving every participant a real-time, tamper-evident view of supply offers, purchase orders, inventory levels, and lead times. Built-in **smart contracts** let buyers set conditions for automatic purchasing — no manual intervention required.

---

## Features

| Feature | Description |
|---|---|
| **Distributed Ledger** | SHA-256–chained blocks with Proof-of-Work consensus and longest-chain resolution |
| **Supply-Chain Transactions** | `SupplyOffer`, `PurchaseOrder`, `InventoryUpdate` as first-class transaction types |
| **Smart Contracts** | Buyer-defined `ContractCreation` transactions that auto-execute `PurchaseOrder`s when a matching `SupplyOffer` appears |
| **Federated Network** | TCP-based P2P protocol with framed JSON messages; nodes discover peers and sync chains automatically |
| **Transparency** | All offers, orders, lead times and inventory changes are permanently recorded on-chain |

---

## Workspace Structure

```
GlassChain/
├── Cargo.toml                      # Workspace manifest
└── crates/
    ├── glasschain-core/            # Block, Transaction, Ledger, crypto
    ├── glasschain-contracts/       # Smart-contract engine
    ├── glasschain-network/         # P2P protocol, Node, PeerConnection
    └── glasschain-node/            # CLI node binary
```

---

## Getting Started

### Prerequisites

* Rust 1.75+ (install via [rustup](https://rustup.rs))

### Build

```bash
git clone https://github.com/dbbvitor/GlassChain.git
cd GlassChain
cargo build --release
```

The binary is at `target/release/glasschain-node`.

### Run a single node

```bash
cargo run --release -p glasschain-node -- --id node-1 --listen 0.0.0.0:8000
```

### Run a two-node local network

```bash
# Terminal 1 – first node
cargo run --release -p glasschain-node -- --id node-1 --listen 0.0.0.0:8000

# Terminal 2 – second node, pointing to node-1
cargo run --release -p glasschain-node -- --id node-2 --listen 0.0.0.0:8001 --peer 127.0.0.1:8000
```

---

## Interactive Commands

Once a node is running, an interactive REPL is available:

```
supply   <seller> <product> <qty> <price> <lead_days> <currency>
    Post a supply offer to the ledger.

order    <buyer> <seller> <product> <qty> <price> <currency>
    Post a manual purchase order.

contract <contract_id> <buyer> <product> <max_price> <min_qty> <max_qty> <max_lead> <currency>
    Create a smart contract for automatic purchasing.

inventory <owner> <product> <delta> <reason>
    Post an inventory update (positive = stock in, negative = stock out).

mine     Mine a block containing all pending transactions.
chain    Print the chain summary.
pending  Print pending transactions.
peers    Print known peers.
quit     Shut down.
```

### Example session

```
> contract c001 acme-corp SKU-WIDGET 15.00 100 500 10 USD
Smart contract created.

> supply supplier-x SKU-WIDGET 300 12.50 7 USD
Supply offer submitted.
[event] Contract c001 auto-executed, qty=300

> mine
Block mined.

> chain
Chain length: 3 blocks
  [   0] 0029f4a1e3b2 | txns=0 | prev=0…
  [   1] 007c3fd8a100 | txns=1 | prev=0029…
  [   2] 004d1ea09b32 | txns=3 | prev=007c…
```

---

## Network Protocol

All peer-to-peer messages are framed with a **4-byte big-endian length prefix** followed by a **UTF-8 JSON payload**:

```
┌──────────────────┬──────────────────────────────────┐
│  4 B (u32 BE)    │  N bytes – JSON message payload  │
└──────────────────┴──────────────────────────────────┘
```

### Message types

| Message | Direction | Purpose |
|---|---|---|
| `Hello` | both | Handshake; announces node ID and chain length |
| `Transaction` | broadcast | Propagate a new pending transaction |
| `Block` | broadcast | Announce a newly mined block |
| `RequestChain` | → peer | Ask for the full chain |
| `Chain` | ← peer | Response containing all blocks |
| `RequestPeers` | → peer | Ask for peer list |
| `Peers` | ← peer | Response with known peer addresses |
| `Goodbye` | both | Graceful disconnect |

---

## Smart Contracts

A buyer creates a smart contract by submitting a `ContractCreation` transaction containing `PurchaseConditions`:

```json
{
  "max_price_per_unit": 15.00,
  "min_quantity": 100,
  "max_quantity": 500,
  "max_lead_time_days": 10,
  "preferred_seller_id": null,
  "currency": "USD",
  "auto_execute": true
}
```

When a `SupplyOffer` is received on any node, the `ContractEngine` evaluates every active contract. If all conditions are met **and** `auto_execute` is `true`, a `PurchaseOrder` and a `ContractExecution` record are generated automatically and broadcast to the network.

---

## Running Tests

```bash
cargo test
```

18 unit tests (core) + 9 smart-contract engine tests + 5 network integration tests covering:
- Block creation, hashing, PoW mining, and tamper detection
- Ledger chain validation and longest-chain replacement
- Smart-contract condition matching, fulfilment, and cancellation
- Network node startup, event emission, and two-node chain synchronisation

---

## License

[MIT](LICENSE)
