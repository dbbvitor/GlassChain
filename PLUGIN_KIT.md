# GlassChain Plugin Developer Kit

This document describes how to build and plug custom implementations of
GlassChain's core protocol layers.  Every major component in GlassChain is
defined as a **Rust trait**; swapping in your own implementation requires no
forks and no changes to the rest of the stack.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          GlassChain Node                                │
│                                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                  │
│  │  Consensus   │  │   Storage    │  │  Execution   │                  │
│  │  Provider    │  │   Provider   │  │  Provider    │                  │
│  │  (trait)     │  │   (trait)    │  │  (trait)     │                  │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘                  │
│         │                 │                  │                          │
│   PoW / Raft /      In-Memory /         Script /                        │
│   PBFT / BFT          Sled / Rocks        WASM                          │
│                                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                  │
│  │  Network     │  │   Indexer    │  │  Event Bus   │                  │
│  │  Provider    │  │   Provider   │  │  Provider    │                  │
│  │  (trait)     │  │   (trait)    │  │  (trait)     │                  │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘                  │
│         │                 │                  │                          │
│  TCP + libp2p       In-Memory /         In-Memory /                     │
│  (Gossipsub/Kad)      PostgreSQL           Kafka                        │
│                                                                         │
│  ┌──────────────┐  ┌──────────────┐                                     │
│  │  Identity /  │  │  Endorsement │                                     │
│  │  MSP         │  │  Engine      │                                     │
│  │  (Phase 2)   │  │  (Phase 2)   │                                     │
│  └──────────────┘  └──────────────┘                                     │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Plugin Summary Table

| # | Plugin | Trait / Struct | Crate | Phase |
|---|--------|---------------|-------|-------|
| 1 | Consensus | `ConsensusProvider` | `glasschain-core` | Core |
| 2 | Storage | `StorageProvider` | `glasschain-core` | Core |
| 3 | Execution (WASM) | `ExecutionProvider` | `glasschain-core` | 4 |
| 4 | Network (TCP) | `NetworkProvider` | `glasschain-core` | Core |
| 4b | Network (libp2p, experimental) | `LibP2pNode` | `glasschain-network` | 1 (unwired) |
| 5 | Indexer | `IndexerProvider` | `glasschain-indexer` | 5 |
| 6 | Event Bus | `EventBusProvider` | `glasschain-indexer` | 5 |
| 7 | Identity / MSP | `Identity`, `Organization` | `glasschain-identity` | 2 |
| 7b | Endorsement Engine | `EndorsementEngine` | `glasschain-identity` | 2 |
| 7c | Endorsement Seam | `EndorsementProvider` | `glasschain-core` | 2 |
| 8 | Watcher (ECA) | `WatcherService` | `glasschain-workflows` | 4 |
| 9 | Schema Validation | `validate_asset` | `glasschain-core` | 3 |
| 10 | Gas Metering | `GasCounter`, `GasCosts` | `glasschain-vm` | 4 |
| 11 | Client SDK | `GlasschainClient` | `glasschain-sdk` | 6 |
| 12 | CLI | `glasschain` binary | `glasschain-cli` | 6 |

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
    ) -> Result<CommitNotification, CoreError>;

    fn validate_block(&self, block: &Block, previous: &Block) -> Result<(), CoreError>;

    fn name(&self) -> &str;
}
```

`propose_block` returns a [`CommitNotification`]: the finished block plus its
quorum certificate (the attestation set). No commit consumer may depend on
"the leader said so" — the certificate travels with every commit. The retained
Proof-of-Work provider supplies a **degenerate** certificate (the valid nonce
is the attestation, carried by the block itself).

### Built-in implementations

- `PowConsensusProvider` — SHA-256 Proof-of-Work with configurable difficulty
  (default, dev/test). Degenerate certificate.
- `BftConsensusProvider` (ticket #42, behind the `bft` cargo feature,
  default-off) — Tendermint-class BFT: `attest(block)` produces a block plus a
  real quorum certificate (ed25519 signatures over the block hash, ≥⅔+
  distinct validators), `verify_certificate(cert, block)` cryptographically
  verifies a certificate against the validator set. Node-side selection is
  capability-gated: the engine engages only when `bft_consensus` is active at
  the candidate height (ADR-010). Multi-validator network vote gathering is the
  ADR-010 testnet adoption gate.

### Implementing Raft consensus

```rust
use glasschain_core::{
    Block, CommitNotification, ConsensusProvider, CoreError, QuorumCertificate, Transaction,
};

pub struct RaftConsensusProvider {
    // ... raft state machine ...
}

impl ConsensusProvider for RaftConsensusProvider {
    fn propose_block(
        &self,
        index: u64,
        transactions: Vec<Transaction>,
        previous: &Block,
    ) -> Result<CommitNotification, CoreError> {
        // 1. Submit the proposed block to the Raft cluster leader.
        // 2. Wait for quorum acknowledgement.
        // 3. Return the committed block with the quorum certificate
        //    (`QuorumCertificate` with the validator attestation set).
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

    /// Atomically persist `block` and apply its canonical write set to the
    /// world state (ADR-007 decision 2).  The implementation must, inside
    /// one atomic section: verify the block chains to the stored tip (empty
    /// store accepts only the genesis), persist the block, and apply every
    /// write (`Set` → put_state, `Delete` → delete_state, keyed by
    /// `ws:<channel>:<contract>:<key>`).  A stale candidate is rejected
    /// whole — block and write set together — with `InvalidBlock`.
    ///
    /// The trait ships a sequential default (correct for single-writer
    /// processes, not atomic); override it with a real atomic section
    /// (e.g. a sled multi-tree transaction).
    fn apply_block(&self, block: &Block) -> Result<(), CoreError> { … }

    fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError>;
    fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError>;
    fn delete_state(&self, key: &str) -> Result<(), CoreError>;

    fn name(&self) -> &str;
}
```

### Built-in implementations

| Name | Crate | Notes |
|:-----|:------|:------|
| `InMemoryStorageProvider` | `glasschain-core` | Testing / dev only |
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
    // Prefer rocksdb's `WriteBatch` in `apply_block` so the tip check, block
    // insert, and write-set application commit atomically (the trait default
    // is sequential and therefore not a real atomic boundary).
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
pub struct ExecutionLimits {
    pub fuel_limit: u64,
    pub operation_gas_limit: u64,
}

pub trait ExecutionProvider: Send + Sync {
    fn execute(
        &self,
        contract_id: &str,
        payload: &[u8],
        limits: ExecutionLimits,
    ) -> Result<ExecutionResult, CoreError>;

    fn name(&self) -> &str;
}
```

`ExecutionResult` (in `glasschain-core::write_set`) separates the two output
kinds (ADR-007):

- `ephemeral: Vec<(String, Vec<u8>)>` — invocation-local output (`set_state`
  semantics); approval gates read `approve` from here and persist nothing;
- `writes: Vec<PersistentWrite>` — explicit persistent set/delete operations
  carrying `channel`, `contract`, `key`, `op` (`Set`/`Delete`), and
  `visibility` (`Public` or `Pdc(name)`).

Providers that only produce legacy ephemeral pairs can convert with
`Vec::<(String, Vec<u8>)>::into()`. `ExecutionResult::canonicalize()` validates
scope non-emptiness and rejects duplicate scoped keys, returning a
deterministically sorted copy for committed-block inclusion (ticket #41).

### Built-in implementation

`WasmExecutionProvider` (crate `glasschain-vm`) — Wasmtime with independent instruction-fuel and host-operation gas budgets. `set_state` remains ephemeral; the separate `env::persist_state` host operation produces scoped persistent writes. Budget exhaustion identifies which meter failed.

### Contract module interface

WASM contracts must export an `execute` function and a `memory` export:

```wat
(module
  (import "env" "set_state"     (func $set_state     (param i32 i32 i32 i32)))
  (import "env" "get_state_len" (func $get_state_len (param i32 i32) (result i32)))
  (import "env" "get_state"     (func $get_state     (param i32 i32 i32 i32) (result i32)))
  ;; persist_state(channel_ptr, channel_len, contract_ptr, contract_len,
  ;;               key_ptr, key_len, val_ptr, val_len, op, visibility,
  ;;               pdc_ptr, pdc_len) -> i32
  ;; op: 0 = set, 1 = delete; visibility: 0 = public, 1 = named PDC.
  ;; Returns 0 on success; -1 unknown op, -2 unknown visibility,
  ;; -3 empty PDC name, -4 malformed pointers.
  (import "env" "persist_state" (func $persist_state (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
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
    let key = b"approve";
    let val = b"1";
    unsafe {
        set_state(
            key.as_ptr() as i32, key.len() as i32,
            val.as_ptr() as i32, val.len() as i32,
        );
    }
}
```

---

## 4. Network Plugin — TCP (`NetworkProvider`)

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

The default TLS+TCP transport in `glasschain-network` (`Node`) implements this interface.
All messages are length-prefixed JSON over TLS with TOFU certificate pinning.

---

## 4b. Network Plugin — libp2p Swarm (`LibP2pNode`) — Experimental

**Crate:** `glasschain-network`
**Struct:** `glasschain_network::LibP2pNode`

The `LibP2pNode` is an experimental, currently unwired P2P transport reserved for the selective-disclosure roadmap. It is not connected to any GlassChain binary yet. When wired, it will provide:
- **Kademlia DHT** for decentralized peer discovery (no bootstrap server required)
- **Gossipsub** for efficient fan-out propagation of transactions and blocks
- **Identify** for protocol negotiation and address advertisement
- **mDNS** for automatic local-network peer discovery

### Architecture

```
LibP2pNode
    │
    ├── GlasschainBehaviour (NetworkBehaviour derive)
    │       ├── gossipsub::Behaviour  ──► topic: "glasschain/transactions"
    │       │                         ──► topic: "glasschain/blocks"
    │       ├── kad::Behaviour        ──► Kademlia DHT (Server mode)
    │       ├── identify::Behaviour   ──► /glasschain/1.0.0
    │       └── mdns::tokio::Behaviour──► local peer discovery
    │
    ├── command_tx  ──► tokio mpsc ──► background SwarmTask
    └── event_rx    ◄── tokio mpsc ◄── background SwarmTask
```

### Quick Start

```rust
use glasschain_network::{LibP2pConfig, LibP2pNode};
use libp2p::Multiaddr;

// Build configuration
let config = LibP2pConfig {
    listen_addr: "/ip4/0.0.0.0/tcp/9000".parse::<Multiaddr>().unwrap(),
    bootstrap_peers: vec![],   // add (PeerId, Multiaddr) pairs for seed nodes
};

// Start the node (spawns the event loop in a background task)
let node = LibP2pNode::new(config)?;
println!("Local peer ID: {}", node.local_peer_id);

// Publish a transaction to all peers
node.publish_transaction(my_transaction).await;

// Dial a specific peer
node.dial("/ip4/192.168.1.100/tcp/9000".parse()?).await;

// Receive events
if let Some(event) = node.try_recv_event().await {
    match event {
        SwarmNodeEvent::TransactionReceived(tx) => { /* process tx */ }
        SwarmNodeEvent::BlockReceived(block)    => { /* process block */ }
        SwarmNodeEvent::PeerConnected(peer_id)  => { /* update peer list */ }
        _ => {}
    }
}
```

### Gossipsub Topics

| Constant | Value | Content |
|:---------|:------|:--------|
| `TOPIC_TRANSACTIONS` | `"glasschain/transactions"` | `Transaction` (JSON) |
| `TOPIC_BLOCKS` | `"glasschain/blocks"` | `Block` (JSON) |

### Kademlia Bootstrap

For production multi-region deployments, seed the Kademlia routing table
with well-known bootstrap peers:

```rust
use libp2p::{Multiaddr, PeerId};
use std::str::FromStr;

let bootstrap_peers = vec![
    (
        PeerId::from_str("12D3KooWBootstrap1...")?,
        "/dns4/seed1.glasschain.io/tcp/9000".parse::<Multiaddr>()?,
    ),
    (
        PeerId::from_str("12D3KooWBootstrap2...")?,
        "/dns4/seed2.glasschain.io/tcp/9000".parse::<Multiaddr>()?,
    ),
];

let config = LibP2pConfig { listen_addr, bootstrap_peers };
```

### `SwarmCommand` reference

| Variant | Effect |
|:--------|:-------|
| `Dial(Multiaddr)` | Initiate outbound connection |
| `PublishTransaction(Transaction)` | Gossipsub publish to transactions topic |
| `PublishBlock(Block)` | Gossipsub publish to blocks topic |
| `AddKnownPeer(PeerId, Multiaddr)` | Add address to Kademlia routing table |
| `Shutdown` | Terminate the swarm event loop |

### `SwarmNodeEvent` reference

| Variant | Emitted when |
|:--------|:-------------|
| `PeerConnected(PeerId)` | TLS/Noise handshake completed |
| `PeerDisconnected(PeerId)` | Connection closed |
| `TransactionReceived(Transaction)` | Gossipsub message on transactions topic |
| `BlockReceived(Block)` | Gossipsub message on blocks topic |
| `RoutingTableUpdated` | Kademlia routing table changed |
| `Error(String)` | Non-fatal error in the event loop |

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

### `AnalyticalFlattener` (Phase 5) ✨

The `AnalyticalFlattener` transforms nested `TraceableAssetRegistration` transactions
into flat `FlatAssetRecord` structs suitable for direct insertion into SQL or
ClickHouse without further JSON parsing:

```rust
use glasschain_indexer::{AnalyticalFlattener, FlatAssetRecord};

let mut flattener = AnalyticalFlattener::new();
flattener.ingest_indexed_block(&indexed_block, &transactions);

// Query by GTIN
for record in flattener.records_by_gtin("07891234567890") {
    println!("batch={:?} trust={}", record.batch_number, record.trust_score);
}

// Export to CSV for ClickHouse ingestion
println!("{}", AnalyticalFlattener::to_csv_header());
for record in flattener.records() {
    println!("{}", AnalyticalFlattener::to_csv_row(record));
}

// Get verifiable lineage
let lineage = VerifiableLineage::build("GTIN:07891234567890", &provenance_index, &flattener);
println!("complete={} avg_trust={:.1}", lineage.is_complete, lineage.trust_score_avg);
```

### `FlatAssetRecord` SQL schema

```sql
CREATE TABLE asset_records (
    block_index              BIGINT,
    block_hash               TEXT,
    block_timestamp          BIGINT,
    transaction_id           TEXT PRIMARY KEY,
    transaction_timestamp    BIGINT,
    -- GS1 / SNCM identity fields
    gtin                     TEXT,
    batch_number             TEXT,
    expiry_date              TEXT,
    serial_number            TEXT,
    anvisa_registration      TEXT,
    manufacturer_id          TEXT,
    -- Asset context
    product_name             TEXT NOT NULL,
    custodian_id             TEXT NOT NULL,
    country_of_origin        TEXT,
    storage_temp_celsius     TEXT,
    quantity                 BIGINT,
    -- Event context
    event_type               TEXT NOT NULL,
    originator_id            TEXT NOT NULL,
    purchase_order_ref       TEXT,
    -- Computed trust
    trust_score              SMALLINT,
    is_standard_compliant    BOOLEAN,
    missing_core_fields      TEXT    -- comma-separated list
);
```

### Implementing a PostgreSQL adapter (SQLx)

```rust
use sqlx::PgPool;
use glasschain_core::Block;
use glasschain_indexer::{IndexedBlock, IndexedTransaction, IndexerError, IndexerProvider};

pub struct PgIndexer { pool: PgPool }

impl IndexerProvider for PgIndexer {
    fn index_block(&self, block: &Block) -> Result<(), IndexerError> {
        // sqlx::query!(
        //   "INSERT INTO blocks (index, hash, timestamp, tx_count)
        //    VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        //   block.index as i64, &block.hash,
        //   block.timestamp as i64, block.transactions.len() as i32
        // ).execute(&self.pool).await?;
        todo!()
    }
    fn name(&self) -> &str { "postgresql" }
    // ... implement remaining methods
}
```

---

## 6. Event Bus Plugin (`EventBusProvider`)

**Crate:** `glasschain-indexer`
**Trait:** `glasschain_indexer::EventBusProvider`

### Trait contract

```rust
pub trait EventBusProvider: Send + Sync {
    fn publish(&self, event: IndexerEvent) -> Result<(), EventBusError>;
    fn publish_block(&self, block: &Block) -> Result<(), EventBusError>; // default impl
    fn name(&self) -> &str;
}
```

### Built-in implementation

`InMemoryEventBus` — tokio broadcast channel, for testing.
Supports `subscribe()` → `broadcast::Receiver<IndexerEvent>` for async consumers.

### Implementing a Kafka adapter (rdkafka)

```toml
[dependencies]
rdkafka = { version = "0.39", features = ["cmake-build"] }
```

```rust
use rdkafka::producer::{FutureProducer, FutureRecord};
use glasschain_indexer::event_bus::{EventBusError, EventBusProvider, IndexerEvent};

pub struct KafkaEventBus { producer: FutureProducer, topic: String }

impl EventBusProvider for KafkaEventBus {
    fn publish(&self, event: IndexerEvent) -> Result<(), EventBusError> {
        let payload = serde_json::to_string(&event)
            .map_err(EventBusError::Serialization)?;
        let record = FutureRecord::to(&self.topic)
            .payload(&payload)
            .key(&event.transaction_id);
        // In async context: self.producer.send(record, Duration::from_secs(5)).await
        Ok(())
    }
    fn name(&self) -> &str { "kafka" }
}
```

---

## 7. Identity / MSP Plugin (Phase 2)

**Crate:** `glasschain-identity`

GlassChain's identity system supports:
- **ed25519 key pairs** for transaction signing (`Identity`)
- **X.509 certificates** signed by an organizational Root CA (`Organization`)
- **Channels** (sub-ledgers with restricted membership)
- **Private Data Collections** (hash on-chain, payload off-chain)

### Signing transactions

```rust
use glasschain_identity::{Identity, Organization};
use glasschain_core::{InventoryUpdate, Transaction, TransactionKind};

let mut org = Organization::new("PharmaOrg").unwrap();
let identity = org.issue_identity("node-1").unwrap();

let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
    product_id: "SKU-001".into(),
    owner_id:   "node-1".into(),
    quantity_delta: 100,
    reason: "initial stock".into(),
}));

let signed = identity.sign_transaction(tx).unwrap();
signed.verify().unwrap();  // cryptographic proof OK
```

### MSP trust hierarchy

```
Organization Root CA (rcgen X.509)
  └── issues certificate → Identity (ed25519 key pair + cert PEM)
        └── signs          → SignedTransaction (tx + 64-byte ed25519 signature)
              └── verified by any peer with → VerifyingKey (32 bytes)
```

### Creating a private data channel

```rust
use glasschain_identity::{Channel, ChannelConfig};

let mut channel = Channel::new(ChannelConfig {
    name: "pharma-private".into(),
    member_ids: vec!["manufacturer".into(), "pharmacy".into()],
    description: "Anvisa-regulated product channel".into(),
});

// Submit private data — only members can retrieve it.
let hash = channel.submit_private_data(
    "manufacturer",
    b"GTIN:07891234567890:BATCH:LOT001:SN:SN-001".to_vec(),
).unwrap();

// `hash` is embedded in the main-chain transaction as a commitment proof.
// Data stays off-chain on authorized nodes only.
```

---

## 7b. Endorsement Engine (Phase 2) ✨

**Crate:** `glasschain-identity`
**Types:** `EndorsementPolicy`, `EndorsementEngine`, `EndorsementProposal`

The endorsement engine enforces **N-of-M multi-organization approval** before a
transaction is considered valid for the ordering phase — mirroring Hyperledger
Fabric's endorsement policy model.

### Policy definition

```rust
use glasschain_identity::{EndorsementEngine, EndorsementPolicy};

// Require signatures from at least 2 of the 3 listed organizations.
let policy = EndorsementPolicy::new(
    "2-of-3 Pharma Orgs",
    vec!["PharmaOrg".into(), "DistributorCo".into(), "PharmacyChain".into()],
    2,
);
let engine = EndorsementEngine::new(policy);
```

### Building a proposal

```rust
use glasschain_identity::{
    EndorsementProposal, EndorsementSignature, Identity, Organization,
};
use glasschain_core::{Transaction, TransactionKind, PurchaseOrder};

// Each endorser organization signs the same transaction.
let tx = Transaction::new(TransactionKind::PurchaseOrder(PurchaseOrder { /* ... */ }));
let mut proposal = EndorsementProposal::new(tx.clone());

for (org_name, identity) in &endorsers {
    let signed = identity.sign_transaction(tx.clone()).unwrap();
    proposal.add_signature(EndorsementSignature::new(signed, org_name.clone()));
}
```

### Evaluating the proposal

```rust
use glasschain_identity::EndorsementResult;

match engine.evaluate(&proposal) {
    EndorsementResult::Approved { proposal_id, endorser_count } => {
        println!("✅ Approved: {endorser_count} valid endorsements for {proposal_id}");
        // Submit to ordering phase.
    }
    EndorsementResult::Rejected { reason, collected, required, .. } => {
        println!("❌ Rejected: {reason} (got {collected}, need {required})");
    }
}
```

### Endorsement flow

```
Transaction proposed
       │
       ▼
EndorsementProposal::new(tx)
       │
       ├── Org A: identity.sign_transaction(tx) → EndorsementSignature
       ├── Org B: identity.sign_transaction(tx) → EndorsementSignature
       └── Org C: identity.sign_transaction(tx) → EndorsementSignature (optional)
       │
       ▼
EndorsementEngine::evaluate(&proposal)
       │
       ├── verify each signature (ed25519)
       ├── check org_name ∈ policy.endorser_org_names
       └── count valid endorsers ≥ required_count?
              │
     ┌────────┴────────┐
     ▼                 ▼
 Approved          Rejected
```

---

## 7c. Endorsement Plugin (`EndorsementProvider`) — ADR-008 seam

**Crate:** `glasschain-core`
**Trait:** `glasschain_core::EndorsementProvider`
**Types:** `PolicyExpression`, `Principal`, `ScopedTarget`, `ScopedPolicies`, `EndorsementRequest`, `EndorserIdentity`, `EndorsementEvaluation`

Identity-neutral business-authorization seam (ADR-008). The expression is a
deterministic Fabric-style signature-policy tree (`SignedBy` / `NOutOf` with
`and`/`or` builders); the wire form is data, never executable policy code.
Scope precedence is channel default → contract default → collection policy →
key policy, and every applicable layer must be satisfied
(`ScopedPolicies::applicable`), so a more specific policy can only add
constraints.

### Trait contract

```rust
pub trait EndorsementProvider: Send + Sync {
    fn evaluate(
        &self,
        expression: &PolicyExpression,
        request: &EndorsementRequest,
    ) -> Result<EndorsementEvaluation, CoreError>;

    fn name(&self) -> &str;
}
```

Implementations must derive each signer's principal from the authenticated
key (never the caller-supplied label), reject a claimed principal that
conflicts with the verified identity, and count at most one signature per
distinct principal.

### Built-in implementation

`MspEndorsementProvider` (crate `glasschain-identity`) — ed25519 verification
over a registered key→principal directory:

```rust
use glasschain_identity::{Identity, MspEndorsementProvider};
use glasschain_core::{EndorsementProvider, PolicyExpression, Principal};

let identity = Identity::generate("node-1");
let mut provider = MspEndorsementProvider::new();
provider.register_identity(&identity, Principal::new("MyOrg"));

let request = /* EndorsementRequest signed by identity.sign_bytes(&payload) */;
let result = provider.evaluate(&PolicyExpression::signed_by("MyOrg"), &request)?;
```

### Commit-path enforcement (ADR-008 §4)

The node invokes the provider at transaction and block admission — but only
once the `endorsement` capability is active at the candidate height (ADR-010)
and a provider is attached via `Node::set_endorsement_provider`.

- **Carrier:** every endorsed transaction carries
  `Vec<TransactionEndorsement>` (`Transaction.endorsements`): the scoped
  `target` the signers authorized plus `EndorserIdentity` signatures over the
  transaction's canonical bytes (serialized with the carriers cleared, so
  signatures are never self-referential).
- **Scope binding:** the transaction's committed partial write set must stay
  inside a declared carrier's scope (`channel`, `contract`, `keys`,
  `collection`), and every applicable layer — channel, contract, collection,
  key, plus the operation default — must be satisfied, or the transaction is
  rejected with no partial state.
- **Operation defaults (ADR-008 decision 3):** custody handoffs
  (`delivery_receipt`) require sender + receiving custodian 2-of-2;
  `quality_certification`/`audit_attestation` require the payload issuer.
  Recall/quarantine/dispute have no generic default — their multi-party rule
  is whatever the committed scoped policies configure.
- **Policy metadata:** `PolicyUpdate` transactions commit versioned,
  append-only policy sets per `(channel, contract)` scope, replayed
  deterministically from the chain (`PolicyHistory`). An update is itself a
  signed transaction satisfying the *current* effective policy and activates
  only after its block commits; a block that changes a key's policy and
  writes the same key is rejected (the new policy applies from the next
  block). Scopes without a committed policy fall back to the fail-closed
  `network-governance` principal.
- **RPC:** `IdentityService.VerifyEndorsement` evaluates an
  `EndorsementRequest` JSON proposal against the committed policies and
  returns the real combined evaluation.

---

## 8. Watcher Service Plugin — ECA Triggers

**Crate:** `glasschain-workflows` (moved from `glasschain-contracts` in the
packaging split, ticket #49 — I/O-driven automation lives in the workflow
layer)
**Struct:** `glasschain_workflows::WatcherService`

The `WatcherService` implements an **Event-Condition-Action** (ECA) model:
inventory falls below a threshold → autonomously generate a `PurchaseOrder` transaction.

```rust
use glasschain_workflows::{InventoryTrigger, WatcherService};

let mut watcher = WatcherService::new();

watcher.add_trigger(InventoryTrigger {
    trigger_id:       "reorder-amoxicilina".into(),
    product_id:       "07891234567890".into(),
    owner_id:         "pharmacy-central".into(),
    threshold:        50,    // fire when inventory ≤ 50 units
    reorder_quantity: 500,   // order this many units
    enabled:          true,
});

// Call this on every InventoryUpdate transaction after it is committed.
let orders = watcher.on_inventory_update(
    "07891234567890",   // product_id
    "pharmacy-central", // owner_id
    -30,               // quantity delta (deduction)
);

for po_tx in orders {
    // Submit po_tx to the ledger — signed by the node's organizational key.
    node.submit_transaction(po_tx).await?;
}
```

**Trigger ID uniqueness:** each firing appends a monotonic counter (`trigger-id-fire-N`) so
repeated firings produce unique transaction IDs, enabling idempotent replay.

---

## 9. Schema Validation — SNCM Nudge Engine (Phase 3) ✨

**Crate:** `glasschain-core`
**Function:** `glasschain_core::validate_asset`

The SNCM schema validator enforces the Brazilian Anvisa RDC 157/2017 traceability
requirements.  It uses a **nudge model**: non-compliant assets are accepted but
flagged, while compliant assets earn a **30% gas fee discount**.

### Schema fields

| Field | Mandatory | Description |
|:------|:----------|:------------|
| `gtin` | ✅ | GTIN-14 or EAN-13 (13–14 numeric digits) |
| `batch_number` | ✅ | Production lot number |
| `expiry_date` | ✅ | ISO-8601 `YYYY-MM-DD` |
| `serial_number` | ✅ | Unique serialization number per unit |
| `anvisa_registration` | ⚠️ recommended | MS registration code |
| `manufacturer_id` | ⚠️ recommended | CNPJ or legal entity ID |

### Validation flow

```rust
use glasschain_core::{TraceableAsset, validate_asset, ViolationSeverity};

let asset = TraceableAsset {
    gtin:          Some("07891234567890".into()),
    batch_number:  Some("LOTE-2025-001".into()),
    expiry_date:   Some("2027-12-31".into()),
    serial_number: Some("SN-00000001".into()),
    ..
};

let report = validate_asset(&asset);

if report.is_compliant {
    println!("✅ SNCM compliant — gas multiplier: {:.0}%", report.gas_fee_multiplier * 100.0);
    // 0.7 × = 30% discount
} else {
    for v in &report.violations {
        match v.severity {
            ViolationSeverity::Critical => eprintln!("❌ CRITICAL: {}", v.message),
            ViolationSeverity::Warning  => eprintln!("⚠️  WARNING: {}", v.message),
        }
    }
    // Asset is still accepted — nudge model, not rejection.
}
```

### Combined with `MetadataTrustScore`

The trust score (0–100) from `MetadataTrustScore` and the SNCM validation report
complement each other:

| Mechanism | Purpose | Gas effect |
|:----------|:--------|:-----------|
| `MetadataTrustScore` | Quality signal for indexers / AI models | `fee_multiplier()`: 0.5× or 1.0× |
| `validate_asset` | SNCM regulatory compliance gate | `gas_fee_multiplier`: 0.7× or 1.0× |

---

## 10. Gas Metering (Phase 4) ✨

**Crate:** `glasschain-vm`
**Types:** `ExecutionLimits`, `GasCosts`, `GasCounter`, `GasReport`

Each execution has two independent budgets:

- **Fuel limit**: one Wasmtime fuel unit per WASM instruction.
- **Operation-gas limit**: host state operations charged by `GasCounter`.

Exhausting either budget returns `CoreError::GasExhausted` with a meter
  discriminator. The execution result is the typed `ExecutionResult` (ephemeral
  output plus the persistent write set); a `GasReport` is a standalone type and
  is not returned by `ExecutionProvider`.

```rust
use glasschain_core::ExecutionLimits;

let limits = ExecutionLimits::new(
    50_000, // fuel_limit
    50_000, // operation_gas_limit
);
executor.execute("contract-id", &wasm, limits)?;
```

### Per-operation cost table (`GasCosts`)

```rust
use glasschain_vm::gas::GasCosts;

let costs = GasCosts::default_costs();
// base_execution: 1_000
// state_read:     50  + 1 per byte
// state_write:    200 + 2 per byte
// max_call_depth: 8 (reserved for future recursive calls)
```

The live Wasmtime provider charges `base_execution` once, state reads by the
bytes returned, state writes by the bytes written, and `get_state_len` by the
flat read cost. Custom schedules remain available through `GasCounter` for
future network or fee-policy work.

### `GasCounter` and deferred depth guard

`GasCounter` is the operation-gas implementation used privately by
`WasmExecutionProvider`. Its `push_call` / `pop_call` methods remain available
for direct users, but recursive contract calls do not exist in the current
runtime, so the depth guard is intentionally deferred. `GasReport` can be used
by direct `GasCounter` callers for standalone accounting.

### Recommended limits

The current callers start with equal values for both budgets:

| Contract type | `fuel_limit` | `operation_gas_limit` |
|:--------------|-------------:|----------------------:|
| Simple state write | 10,000 | 10,000 |
| Inventory reorder logic | 50,000 | 50,000 |
| Complex aggregation | 500,000 | 500,000 |
| Autonomous PurchaseOrder generation | 100,000 | 100,000 |
| Maximum allowed | 10,000,000 | 10,000,000 |

---

## 11. Client SDK (`GlasschainClient`) — Phase 6 ✨

**Crate:** `glasschain-sdk`
**Struct:** `glasschain_sdk::GlasschainClient`

The SDK abstracts all gRPC/MSP complexity, allowing a supplier to register an
asset in fewer than 10 lines of Rust:

```rust
use glasschain_sdk::GlasschainClient;
use glasschain_core::TraceableAsset;

// 1. Build the asset
let asset = TraceableAsset {
    gtin:          Some("07891234567890".into()),
    batch_number:  Some("LOTE-2025-001".into()),
    expiry_date:   Some("2027-06-30".into()),
    serial_number: Some("SN-001".into()),
    product_name:  "Dipirona 500mg".into(),
    custodian_id:  "fab-xyz".into(),
    quantity:      1000,
    ..Default::default()
};

// 2. Build the transaction JSON (ready for gRPC SubmitTransaction)
let tx_json = GlasschainClient::build_asset_registration_tx(
    "fab-xyz",   // originator_id
    asset,
    "MANUFACTURE",
)?;

// 3. Submit via gRPC (the returned tx_json goes in SubmitTransactionRequest.transaction_json)
println!("{tx_json}");
```

### Available builders

| Method | Creates |
|:-------|:--------|
| `build_asset_registration_tx` | `AssetRegistration` transaction |
| `build_supply_offer_tx` | `SupplyOffer` transaction |
| `build_purchase_order_tx` | `PurchaseOrder` transaction |
| `build_inventory_update_tx` | `InventoryUpdate` transaction |
| `compute_trust_score` | Computes `MetadataTrustScore` without creating a transaction |

### `GlasschainClientConfig`

```rust
use glasschain_sdk::GlasschainClientConfig;

let config = GlasschainClientConfig::new("http://node.glasschain.io:50051")
    .with_node_id("my-node-id");
```

---

## 12. CLI Utility (`glasschain`) — Phase 6 ✨

**Crate:** `glasschain-cli`
**Binary:** `glasschain`

```
glasschain <COMMAND> [OPTIONS]

Commands:
  identity-gen     Generate a new node identity (with optional org Root CA)
  contract-deploy  Deploy a smart contract to a GlassChain node
  ledger-inspect   Inspect the ledger state (blocks, assets, chain status)
  help             Print this message or the help of the given subcommand

Options:
  --log-level <LEVEL>  Log verbosity [default: info]
  -h, --help           Print help
  -V, --version        Print version
```

### `identity-gen`

```bash
# Generate a standalone node identity
glasschain identity-gen --node-id node-pharma-sp

# Generate an org-issued identity (creates Root CA + member cert)
glasschain identity-gen \
    --node-id    distributor-1 \
    --org        "PharmaDistributors Brasil" \
    --output     distributor-1-identity.json
```

### `contract-deploy`

```bash
# Preview the contract transaction without submitting
glasschain contract-deploy \
    --contract-id  auto-reorder-amoxicilina \
    --buyer-id     pharmacy-central \
    --product-id   07891234567890 \
    --max-price    9500 \
    --min-qty      100 \
    --max-qty      5000 \
    --max-lead-days 7 \
    --currency     BRL \
    --dry-run
```

### `ledger-inspect`

```bash
# Show chain status
glasschain ledger-inspect --endpoint http://localhost:50051

# Inspect a specific block
glasschain ledger-inspect --endpoint http://localhost:50051 --block 42

# Query asset history by GTIN
glasschain ledger-inspect --endpoint http://localhost:50051 --gtin 07891234567890
```

---

## gRPC API Reference (Phase 1 / Phase 5)

**Crate:** `glasschain-rpc`
**Proto:** `proto/glasschain/v1/glasschain.proto`

Three services are exposed on a single port (default `0.0.0.0:50051`):

### `LedgerService`

| RPC | Description |
|:----|:------------|
| `GetBlock` | Retrieve a block by chain index |
| `StreamBlocks` | Live-stream blocks from a start index |
| `SubmitTransaction` | Submit a signed or unsigned transaction |
| `GetChainStatus` | Chain length, tip hash, pending count |
| `QueryAssetHistory` | All `AssetRegistration` txs for a GTIN/serial |
| `SubscribeToEvents` | Live stream of `NodeEvent`s |
| `GetVerifiableLineage` | Full custody chain for an asset ID ✨ Phase 5 |

### `NodeService`

| RPC | Description |
|:----|:------------|
| `GetNodeStatus` | Node ID, address, version, chain length, peer count |
| `GetPeers` | List of known peer addresses |

(`MineBlock` was retired with the quorum-certificate seam — block production is
driven by the consensus layer, not by an RPC. The dev/test Proof-of-Work driver
remains available programmatically as `Node::mine()`.)

### `IdentityService` ✨ Phase 2

| RPC | Description |
|:----|:------------|
| `ExchangeCertificate` | Register an org Root CA for peer verification |
| `VerifyEndorsement` | Validate an `EndorsementProposal` JSON payload |

### Starting the gRPC server

```bash
# Start node with gRPC enabled
glasschain-node \
    --id       node-1 \
    --listen   0.0.0.0:8000 \
    --rpc-addr 0.0.0.0:50051 \
    --org      "PharmaOrg"
```

---

## Quick-Start Checklist

To plug in a custom component:

1. **Pick the trait** from the plugin summary table at the top.
2. **Create a new crate** or add a module to an existing one.
3. **Implement the trait** on your struct.
4. **Wire it in** by passing your implementation to the node/builder.
5. **Add tests** — use the in-memory defaults as reference implementations.
6. **Optionally** add `[lints] workspace = true` to inherit the workspace lint config.

All built-in implementations live in their respective crates and can be
used as templates.

---

## Repository Layout

```
GlassChain/
├── Cargo.toml                  # Workspace manifest (12 crates)
├── PLUGIN_KIT.md               # This document
├── README.md                   # Project overview and quick-start
└── crates/
    ├── glasschain-core/        # Block, Transaction, Ledger, provider traits, schema
    ├── glasschain-contracts/   # ContractEngine, approval gate (deterministic layer)
    ├── glasschain-workflows/   # Flow state machines, checkpoints, WatcherService
    ├── glasschain-network/     # TCP+TLS P2P node + experimental unwired libp2p Swarm
    ├── glasschain-node/        # Interactive REPL binary + gRPC wiring
    ├── glasschain-storage/     # SledStorageProvider (persistent on-disk backend)
    ├── glasschain-identity/    # Identity, Organization, Channel, EndorsementEngine
    ├── glasschain-vm/          # WasmExecutionProvider + GasCosts/GasCounter (Phase 4)
    ├── glasschain-indexer/     # IndexerProvider, ProvenanceIndex, AnalyticalFlattener
    ├── glasschain-rpc/         # gRPC services: Ledger, Node, Identity (Tonic + Prost)
    ├── glasschain-sdk/         # High-level Rust client SDK (Phase 6)
    └── glasschain-cli/         # CLI binary: identity-gen, contract-deploy, ledger-inspect
```

### Dependency graph

```
glasschain-core
    ├── glasschain-contracts  (uses core + vm)
    ├── glasschain-storage    (uses core)
    ├── glasschain-vm         (uses core)
    └── glasschain-identity   (uses core)
            │
            ▼
    glasschain-indexer        (uses core)
            │
            ▼
    glasschain-network        (uses core + contracts + identity + indexer)
            │
            ▼
    glasschain-rpc            (uses core + network + identity)
            │
            ▼
    glasschain-sdk            (uses core + identity + rpc types)
            │
            ▼
    glasschain-node           (uses all above — the executable)
    glasschain-cli            (uses core + identity + sdk — the CLI binary)
```

No circular dependencies exist in the workspace.