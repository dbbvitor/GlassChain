# GlassChain Plugin Developer Kit

This document describes how to build and plug custom implementations of
GlassChain's core protocol layers.  Every major component in GlassChain is
defined as a **Rust trait**; swapping in your own implementation requires no
forks and no changes to the rest of the stack.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        GlassChain Node                              │
│                                                                     │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │
│  │  Consensus   │  │   Storage    │  │  Execution   │              │
│  │  Provider    │  │   Provider   │  │  Provider    │              │
│  │  (trait)     │  │   (trait)    │  │  (trait)     │              │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘              │
│         │                 │                  │                      │
│   PoW / Raft /      In-Memory /         Script /                    │
│   PBFT / BFT          Sled / Rocks        WASM                      │
│                                                                     │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │
│  │  Network     │  │   Indexer    │  │  Event Bus   │              │
│  │  Provider    │  │   Provider   │  │  Provider    │              │
│  │  (trait)     │  │   (trait)    │  │  (trait)     │              │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘              │
│         │                 │                  │                      │
│   TCP / libp2p      In-Memory /         In-Memory /                 │
│                       PostgreSQL           Kafka                    │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 1. Consensus Plugin (`ConsensusProvider`)

**Crate:** `glasschain-core`  
**Trait:** `glasschain_core::ConsensusProvider`

### Trait contract

```rust
pub trait ConsensusProvider: Send + Sync {
    fn propose_block(
        &self,
        index: u64,
        transactions: Vec<Transaction>,
        previous: &Block,
    ) -> Result<Block, CoreError>;

    fn validate_block(&self, block: &Block, previous: &Block) -> Result<(), CoreError>;

    fn name(&self) -> &str;
}
```

### Built-in implementation

`PowConsensusProvider` — SHA-256 Proof-of-Work with configurable difficulty.

### Implementing Raft consensus

```rust
use glasschain_core::{Block, ConsensusProvider, CoreError, Transaction};

pub struct RaftConsensusProvider {
    // ... raft state machine ...
}

impl ConsensusProvider for RaftConsensusProvider {
    fn propose_block(
        &self,
        index: u64,
        transactions: Vec<Transaction>,
        previous: &Block,
    ) -> Result<Block, CoreError> {
        // 1. Submit the proposed block to the Raft cluster leader.
        // 2. Wait for quorum acknowledgement.
        // 3. Return the committed block.
        todo!()
    }

    fn validate_block(&self, block: &Block, previous: &Block) -> Result<(), CoreError> {
        // Verify the block's Raft commit certificate / quorum signatures.
        todo!()
    }

    fn name(&self) -> &str { "raft" }
}
```

---

## 2. Storage Plugin (`StorageProvider`)

**Crate:** `glasschain-core`  
**Trait:** `glasschain_core::StorageProvider`

### Trait contract

```rust
pub trait StorageProvider: Send + Sync {
    fn put_block(&self, block: &Block) -> Result<(), CoreError>;
    fn get_block(&self, index: u64) -> Result<Option<Block>, CoreError>;
    fn latest_block_index(&self) -> Result<Option<u64>, CoreError>;

    fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError>;
    fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError>;
    fn delete_state(&self, key: &str) -> Result<(), CoreError>;

    fn name(&self) -> &str;
}
```

### Built-in implementations

| Name | Crate | Notes |
|:-----|:------|:------|
| `InMemoryStorageProvider` | `glasschain-core` | Testing/dev only |
| `SledStorageProvider` | `glasschain-storage` | Pure Rust, single-node production |

### Implementing a RocksDB adapter

```rust
use glasschain_core::{Block, CoreError, StorageProvider};
use rocksdb::{DB, Options};

pub struct RocksDbStorageProvider { db: DB }

impl RocksDbStorageProvider {
    pub fn open(path: &str) -> Result<Self, CoreError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = DB::open(&opts, path)
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(Self { db })
    }
}

impl StorageProvider for RocksDbStorageProvider {
    fn put_block(&self, block: &Block) -> Result<(), CoreError> {
        let key = block.index.to_be_bytes();
        let val = serde_json::to_vec(block)?;
        self.db.put(key, val)
            .map_err(|e| CoreError::Storage(e.to_string()))
    }
    // ... implement remaining methods ...
    fn name(&self) -> &str { "rocksdb" }
}
```

---

## 3. Execution Plugin (`ExecutionProvider`)

**Crate:** `glasschain-core`  
**Trait:** `glasschain_core::ExecutionProvider`

### Trait contract

```rust
pub trait ExecutionProvider: Send + Sync {
    fn execute(
        &self,
        contract_id: &str,
        payload: &[u8],
        gas_limit: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError>;

    fn name(&self) -> &str;
}
```

### Built-in implementation

`WasmExecutionProvider` (crate `glasschain-vm`) — Wasmtime with fuel-based gas metering.

### Contract module interface

WASM contracts must export:

```wat
(module
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (import "env" "get_state_len" (func $get_state_len (param i32 i32) (result i32)))
  (export "execute" (func $main))
  (export "memory" (memory 0))
  ...
)
```

### Writing a contract in Rust

```rust
// Compile with: cargo build --target wasm32-unknown-unknown --release

#[link(wasm_import_module = "env")]
extern "C" {
    fn set_state(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32);
}

#[no_mangle]
pub extern "C" fn execute() {
    let key = b"status";
    let val = b"active";
    unsafe {
        set_state(
            key.as_ptr() as i32, key.len() as i32,
            val.as_ptr() as i32, val.len() as i32,
        );
    }
}
```

### Gas metering

Each WASM instruction consumes 1 unit of **fuel**.  Set `gas_limit` to the
maximum number of instructions the contract is permitted to execute.
Exceeding the limit returns `CoreError::GasExhausted { used, limit }`.

Recommended limits by contract type:

| Contract type | Recommended `gas_limit` |
|:--------------|:------------------------|
| Simple state write | 10,000 |
| Inventory reorder logic | 50,000 |
| Complex aggregation | 500,000 |
| Maximum allowed | 10,000,000 |

---

## 4. Network Plugin (`NetworkProvider`)

**Crate:** `glasschain-core`  
**Trait:** `glasschain_core::NetworkProvider`

### Trait contract

```rust
pub trait NetworkProvider: Send + Sync {
    fn broadcast(&self, message: &[u8]);
    fn connected_peers(&self) -> Vec<String>;
    fn name(&self) -> &str;
}
```

### Built-in implementation

The default TCP transport in `glasschain-network` implements this interface.

### Implementing a libp2p adapter

```rust
use libp2p::{Swarm, gossipsub::Gossipsub};
use glasschain_core::NetworkProvider;

pub struct LibP2pNetworkProvider {
    // Wrap a running libp2p Swarm.
    peers: std::sync::Arc<std::sync::RwLock<Vec<String>>>,
    sender: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

impl NetworkProvider for LibP2pNetworkProvider {
    fn broadcast(&self, message: &[u8]) {
        let _ = self.sender.send(message.to_vec());
    }
    fn connected_peers(&self) -> Vec<String> {
        self.peers.read().unwrap().clone()
    }
    fn name(&self) -> &str { "libp2p-gossipsub" }
}
```

**Recommended libp2p configuration:**
- Transport security: **Noise** protocol
- Peer discovery: **Kademlia DHT**
- Multiplexing: **Yamux**
- Messaging: **GossipSub** for block/transaction broadcast

---

## 5. Indexer Plugin (`IndexerProvider`)

**Crate:** `glasschain-indexer`  
**Trait:** `glasschain_indexer::IndexerProvider`

### Trait contract

```rust
pub trait IndexerProvider: Send + Sync {
    fn index_block(&self, block: &Block) -> Result<(), IndexerError>;
    fn get_block(&self, index: u64) -> Result<Option<IndexedBlock>, IndexerError>;
    fn get_transaction(&self, id: &str) -> Result<Option<IndexedTransaction>, IndexerError>;
    fn transactions_in_block(&self, block_index: u64) -> Result<Vec<IndexedTransaction>, IndexerError>;
    fn block_count(&self) -> Result<u64, IndexerError>;
    fn name(&self) -> &str;
}
```

### Built-in implementation

`InMemoryIndexer` — zero-dependency, for testing.

### Implementing a PostgreSQL adapter (SQLx)

```rust
use sqlx::PgPool;
use glasschain_core::Block;
use glasschain_indexer::{IndexedBlock, IndexedTransaction, IndexerError, IndexerProvider};

pub struct PgIndexer { pool: PgPool }

impl IndexerProvider for PgIndexer {
    fn index_block(&self, block: &Block) -> Result<(), IndexerError> {
        // Use sqlx::query! macros for compile-time checked SQL.
        // sqlx::query!(
        //   "INSERT INTO blocks (index, hash, timestamp, tx_count)
        //    VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        //   block.index as i64, block.hash, block.timestamp as i64,
        //   block.transactions.len() as i32
        // ).execute(&self.pool).await?;
        todo!()
    }
    // ...
    fn name(&self) -> &str { "postgresql" }
}
```

**Schema:**
```sql
CREATE TABLE blocks (
  index      BIGINT PRIMARY KEY,
  hash       TEXT NOT NULL,
  timestamp  BIGINT NOT NULL,
  tx_count   INT NOT NULL
);
CREATE TABLE transactions (
  id         TEXT PRIMARY KEY,
  block_idx  BIGINT REFERENCES blocks(index),
  kind       TEXT NOT NULL,
  timestamp  BIGINT NOT NULL,
  payload    JSONB NOT NULL
);
```

---

## 6. Event Bus Plugin (`EventBusProvider`)

**Crate:** `glasschain-indexer`  
**Trait:** `glasschain_indexer::EventBusProvider`

### Trait contract

```rust
pub trait EventBusProvider: Send + Sync {
    fn publish(&self, event: IndexerEvent) -> Result<(), EventBusError>;
    fn publish_block(&self, block: &Block) -> Result<(), EventBusError>;  // default impl
    fn name(&self) -> &str;
}
```

### Built-in implementation

`InMemoryEventBus` — tokio broadcast channel, for testing.

### Implementing a Kafka adapter (rdkafka)

Add to `Cargo.toml`:
```toml
[dependencies]
rdkafka = { version = "0.39", features = ["cmake-build"] }
```

```rust
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use glasschain_indexer::event_bus::{EventBusError, EventBusProvider, IndexerEvent};

pub struct KafkaEventBus {
    producer: FutureProducer,
    topic: String,
}

impl KafkaEventBus {
    pub fn new(brokers: &str, topic: &str) -> Self {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .set("message.timeout.ms", "5000")
            .create()
            .expect("kafka producer");
        Self { producer, topic: topic.into() }
    }
}

impl EventBusProvider for KafkaEventBus {
    fn publish(&self, event: IndexerEvent) -> Result<(), EventBusError> {
        let payload = serde_json::to_string(&event)
            .map_err(EventBusError::Serialization)?;
        // For a synchronous wrapper, use a blocking runtime:
        let record = FutureRecord::to(&self.topic)
            .payload(&payload)
            .key(&event.transaction_id);
        // In an async context: self.producer.send(record, Duration::from_secs(5)).await
        Ok(())
    }
    fn name(&self) -> &str { "kafka" }
}
```

---

## 7. Identity Plugin (`IdentityProvider` — Phase 2 MSP)

**Crate:** `glasschain-identity`

GlassChain's identity system supports:
- **ed25519 key pairs** for transaction signing (`Identity`)
- **X.509 certificates** signed by an organizational Root CA (`Organization`)
- **Channels** (sub-ledgers with restricted membership)
- **Private Data Collections** (hash on-chain, payload off-chain)

### Signing transactions

```rust
use glasschain_identity::{Identity, Organization};
use glasschain_core::{Transaction, TransactionKind, InventoryUpdate};

let mut org = Organization::new("MyOrg").unwrap();
let identity = org.issue_identity("node-1").unwrap();

let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
    product_id: "SKU-001".into(),
    owner_id: "node-1".into(),
    quantity_delta: 100,
    reason: "initial stock".into(),
}));
let signed = identity.sign_transaction(tx).unwrap();
signed.verify().unwrap();  // OK
```

### Creating a private channel

```rust
use glasschain_identity::{Channel, ChannelConfig};

let mut channel = Channel::new(ChannelConfig {
    name: "pharma-private".into(),
    member_ids: vec!["manufacturer".into(), "pharmacy".into()],
    description: "Anvisa-regulated product channel".into(),
});

// Submit private data (only visible to members).
let hash = channel.submit_private_data("manufacturer", b"GTIN+serial+batch".to_vec()).unwrap();
// `hash` is recorded on the main chain as proof.
```

---

## 8. Watcher Service Plugin (Phase 4 — ECA Triggers)

**Crate:** `glasschain-contracts`  
**Struct:** `glasschain_contracts::WatcherService`

```rust
use glasschain_contracts::{WatcherService, InventoryTrigger};

let mut watcher = WatcherService::new();
watcher.add_trigger(InventoryTrigger {
    trigger_id: "reorder-sku-001".into(),
    product_id: "SKU-001".into(),
    owner_id: "pharmacy-1".into(),
    reorder_threshold: 100,      // fire when inventory ≤ 100
    reorder_quantity: 500,       // order 500 units
    seller_id: "supplier-x".into(),
    price_per_unit: 9.99,
    currency: "USD".into(),
    active: true,
});

// Call on every InventoryUpdate transaction.
let orders = watcher.on_inventory_update(&update);
// Submit `orders` to the ledger.
```

---

## Quick-Start Checklist

To plug in a custom component:

1. **Pick the trait** from the table above.
2. **Create a new crate** or add a module to an existing one.
3. **Implement the trait** on your struct.
4. **Wire it in** by passing your implementation to the node/builder.
5. **Add tests** — use the in-memory defaults as reference implementations.

All built-in implementations live in their respective crates and can be
used as templates.

---

## Repository Layout

```
GlassChain/
├── crates/
│   ├── glasschain-core/        # Block, Transaction, Ledger, provider traits
│   ├── glasschain-contracts/   # ContractEngine, WatcherService
│   ├── glasschain-network/     # TCP P2P node
│   ├── glasschain-node/        # Interactive REPL binary
│   ├── glasschain-storage/     # SledStorageProvider
│   ├── glasschain-identity/    # Identity, Organization, Channel (MSP)
│   ├── glasschain-vm/          # WasmExecutionProvider (Wasmtime + gas)
│   ├── glasschain-indexer/     # IndexerProvider, ProvenanceIndex, EventBus
│   └── glasschain-rpc/         # gRPC service layer (Tonic + Prost)
└── PLUGIN_KIT.md               # This document
```
