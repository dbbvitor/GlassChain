//! Pluggable provider traits for `GlassChain`'s core protocol layers.
//!
//! These traits define the abstract interfaces for the three pillars of the
//! distributed ledger: **Consensus**, **Storage**, and **Execution**.  Any
//! implementation can be swapped in at compile time (via feature flags) or at
//! run time (via dynamic dispatch), enabling a truly pluggable architecture.
//!
//! ## The "Provider" Pattern
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    GlassChain Node                          │
//! │   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
//! │   │ Consensus    │  │  Storage     │  │  Execution   │     │
//! │   │ Provider     │  │  Provider    │  │  Provider    │     │
//! │   │ (trait)      │  │  (trait)     │  │  (trait)     │     │
//! │   └──────┬───────┘  └──────┬───────┘  └──────┬───────┘     │
//! │          │                 │                  │             │
//! │     PoW / Raft /      In-Memory /       Script /            │
//! │     PBFT / BFT         Sled / Rocks       WASM              │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use crate::block::Block;
use crate::error::CoreError;
use crate::transaction::Transaction;

/// Abstraction over the consensus mechanism used to agree on the next block.
///
/// Implementors may use Proof-of-Work, Raft, PBFT, or any other algorithm.
/// The node calls [`ConsensusProvider::propose_block`] when it wants to add
/// pending transactions to the chain and waits for the provider to return a
/// fully validated block ready for commitment.
pub trait ConsensusProvider: Send + Sync {
    /// Propose a new block containing `transactions` that chains onto `previous`.
    ///
    /// Returns the finished block (with a valid `hash`) on success.
    /// The provider is responsible for any work required by the chosen
    /// consensus algorithm (e.g., mining a `PoW` nonce or gathering signatures).
    ///
    /// # Errors
    /// Returns `Err` if the consensus algorithm fails to produce a valid block
    /// (e.g., mining failure or quorum not reached).
    fn propose_block(
        &self,
        index: u64,
        transactions: Vec<Transaction>,
        previous: &Block,
    ) -> Result<Block, CoreError>;

    /// Validate a block proposed by a remote peer.
    ///
    /// Returns `Ok(())` when the block is structurally valid and satisfies the
    /// consensus rules (e.g., `PoW` target, quorum signatures).
    ///
    /// # Errors
    /// Returns `Err(CoreError::InvalidBlock)` if the block violates any
    /// consensus rule (wrong `previous_hash`, insufficient `PoW`, etc.).
    fn validate_block(&self, block: &Block, previous: &Block) -> Result<(), CoreError>;

    /// Human-readable identifier for this consensus implementation.
    fn name(&self) -> &str;
}

/// Abstraction over the persistent "World State" and block storage.
///
/// The World State is a key-value snapshot of the latest committed state
/// derived from all committed transactions (analogous to Ethereum's state
/// trie or Hyperledger Fabric's `CouchDB` state database).
///
/// Implementors may back this with in-memory structures, `sled`, `RocksDB`, or
/// any other store.
pub trait StorageProvider: Send + Sync {
    /// Persist a committed block.
    ///
    /// # Errors
    /// Returns `Err` if the underlying storage backend fails to persist the block.
    fn put_block(&self, block: &Block) -> Result<(), CoreError>;

    /// Retrieve a block by its sequential index.
    ///
    /// # Errors
    /// Returns `Err` if the underlying storage backend returns an error.
    fn get_block(&self, index: u64) -> Result<Option<Block>, CoreError>;

    /// Return the index of the highest committed block, or `None` if the store
    /// is empty.
    ///
    /// # Errors
    /// Returns `Err` if the underlying storage backend returns an error.
    fn latest_block_index(&self) -> Result<Option<u64>, CoreError>;

    /// Write a World State key-value pair.
    ///
    /// # Errors
    /// Returns `Err` if the underlying storage backend fails to write the value.
    fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError>;

    /// Read a World State value by key.
    ///
    /// # Errors
    /// Returns `Err` if the underlying storage backend returns an error.
    fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError>;

    /// Delete a World State key.
    ///
    /// # Errors
    /// Returns `Err` if the underlying storage backend fails to delete the key.
    fn delete_state(&self, key: &str) -> Result<(), CoreError>;

    /// Human-readable identifier for this storage implementation.
    fn name(&self) -> &str;
}

/// Abstraction over the smart-contract execution environment.
///
/// Implementors may execute contracts as native Rust closures, WASM modules
/// (via Wasmtime), or any other sandboxed runtime.  The execution provider
/// receives a serialised contract payload and the current World State accessor
/// and returns a set of state mutations to be applied atomically.
pub trait ExecutionProvider: Send + Sync {
    /// Execute a contract payload and return a list of (key, value) state
    /// mutations to be committed.
    ///
    /// `payload` is an opaque byte slice whose interpretation is provider-
    /// specific (e.g., a WASM module, a Lua script, or JSON instructions).
    /// `gas_limit` caps the computational budget; the provider should return
    /// [`CoreError::GasExhausted`] if the contract exceeds it.
    ///
    /// # Errors
    /// Returns `Err(CoreError::GasExhausted)` if the contract exceeds
    /// `gas_limit`, or `Err` for any other execution failure.
    fn execute(
        &self,
        contract_id: &str,
        payload: &[u8],
        gas_limit: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError>;

    /// Execute a contract with a pre-populated world-state snapshot.
    ///
    /// The `initial_state` entries are visible to the contract via the
    /// `get_state` / `get_state_len` host functions.  Mutations returned by
    /// the contract are applied on top of this snapshot.
    ///
    /// The default implementation ignores `initial_state` (backward-compat).
    /// Override in concrete providers to expose the snapshot to contracts.
    ///
    /// # Errors
    /// Returns `Err(CoreError::GasExhausted)` if the contract exceeds
    /// `gas_limit`, or `Err` for any other execution failure.
    fn execute_with_state(
        &self,
        contract_id: &str,
        payload: &[u8],
        initial_state: std::collections::HashMap<String, Vec<u8>>,
        gas_limit: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
        let _ = initial_state;
        self.execute(contract_id, payload, gas_limit)
    }

    /// Human-readable identifier for this execution implementation.
    fn name(&self) -> &str;
}

/// Abstraction over the peer-to-peer network transport layer.
///
/// This trait decouples the node logic from the underlying transport, making it
/// straightforward to replace the built-in TCP implementation with a `libp2p`-
/// based transport (Kademlia discovery, Noise encryption, Yamux multiplexing).
///
/// ## Plugging in `libp2p`
/// 1. Add `libp2p` as a dependency (feature-gated via `features = ["libp2p"]`).
/// 2. Implement `NetworkProvider` on a struct that wraps a `libp2p::Swarm`.
/// 3. Pass the implementation to `Node::with_network_provider`.
///
/// The current TCP implementation in `glasschain-network` satisfies this
/// contract and can be treated as the "default" adapter.
pub trait NetworkProvider: Send + Sync {
    /// Broadcast a serialised message to all known peers.
    ///
    /// The message is an opaque byte slice (typically JSON-serialised
    /// [`protocol::Message`][crate::protocol::Message]).
    /// Failures are logged but do not propagate — the network layer is
    /// best-effort.
    fn broadcast(&self, message: &[u8]);

    /// Return the list of currently connected peer addresses.
    fn connected_peers(&self) -> Vec<String>;

    /// Human-readable name for this transport implementation (e.g. `"tcp"`,
    /// `"libp2p-kademlia"`).
    fn name(&self) -> &str;
}

/// Proof-of-Work consensus provider.
///
/// This is the default `ConsensusProvider` for `GlassChain`.  It mines new
/// blocks by incrementing a nonce until the SHA-256 hash of the block header
/// starts with `difficulty` leading zero hex characters.
///
/// ## Plugging in a different algorithm
/// Replace `PowConsensusProvider` with an implementation of [`ConsensusProvider`]
/// that uses Raft, PBFT, or any other algorithm.  The `glasschain-node` binary
/// accepts any `Box<dyn ConsensusProvider>`.
pub struct PowConsensusProvider {
    /// Number of leading zero characters required in the block hash.
    pub difficulty: usize,
}

impl PowConsensusProvider {
    /// Create a new `PoW` provider with the given mining difficulty.
    #[must_use]
    pub const fn new(difficulty: usize) -> Self {
        Self { difficulty }
    }
}

impl ConsensusProvider for PowConsensusProvider {
    fn propose_block(
        &self,
        index: u64,
        transactions: Vec<Transaction>,
        previous: &Block,
    ) -> Result<Block, CoreError> {
        let mut block = Block::new(index, transactions, previous.hash.clone());
        block.mine(self.difficulty);
        Ok(block)
    }

    fn validate_block(&self, block: &Block, previous: &Block) -> Result<(), CoreError> {
        block.chains_to(previous)?;
        if !block.has_valid_pow(self.difficulty) {
            return Err(CoreError::InvalidBlock(format!(
                "block {} does not satisfy PoW difficulty {}",
                block.index, self.difficulty
            )));
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "proof-of-work"
    }
}

/// A minimal in-memory [`StorageProvider`] used for testing and the default
/// single-node configuration.
///
/// **Not** suitable for production (data is lost on process restart).
pub mod in_memory {
    use super::{Block, CoreError, StorageProvider};
    use std::collections::HashMap;
    use std::sync::RwLock;

    /// Thread-safe, in-memory implementation of [`StorageProvider`].
    #[derive(Debug, Default)]
    pub struct InMemoryStorageProvider {
        blocks: RwLock<HashMap<u64, Block>>,
        state: RwLock<HashMap<String, Vec<u8>>>,
    }

    impl InMemoryStorageProvider {
        /// Create a new empty in-memory store.
        #[must_use]
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl StorageProvider for InMemoryStorageProvider {
        fn put_block(&self, block: &Block) -> Result<(), CoreError> {
            self.blocks
                .write()
                .expect("lock poisoned")
                .insert(block.index, block.clone());
            Ok(())
        }

        fn get_block(&self, index: u64) -> Result<Option<Block>, CoreError> {
            Ok(self
                .blocks
                .read()
                .expect("lock poisoned")
                .get(&index)
                .cloned())
        }

        fn latest_block_index(&self) -> Result<Option<u64>, CoreError> {
            Ok(self
                .blocks
                .read()
                .expect("lock poisoned")
                .keys()
                .copied()
                .max())
        }

        fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError> {
            self.state
                .write()
                .expect("lock poisoned")
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
            Ok(self.state.read().expect("lock poisoned").get(key).cloned())
        }

        fn delete_state(&self, key: &str) -> Result<(), CoreError> {
            self.state.write().expect("lock poisoned").remove(key);
            Ok(())
        }

        fn name(&self) -> &'static str {
            "in-memory"
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::block::Block;

        fn genesis() -> Block {
            let mut b = Block::new(0, vec![], "0".into());
            b.mine(1);
            b
        }

        #[test]
        fn test_put_and_get_block() {
            let store = InMemoryStorageProvider::new();
            let g = genesis();
            store.put_block(&g).unwrap();
            let retrieved = store.get_block(0).unwrap().unwrap();
            assert_eq!(retrieved.index, 0);
        }

        #[test]
        fn test_latest_block_index_empty() {
            let store = InMemoryStorageProvider::new();
            assert!(store.latest_block_index().unwrap().is_none());
        }

        #[test]
        fn test_latest_block_index_after_put() {
            let store = InMemoryStorageProvider::new();
            let g = genesis();
            store.put_block(&g).unwrap();
            assert_eq!(store.latest_block_index().unwrap(), Some(0));
        }

        #[test]
        fn test_state_roundtrip() {
            let store = InMemoryStorageProvider::new();
            store.put_state("foo", b"bar").unwrap();
            assert_eq!(store.get_state("foo").unwrap(), Some(b"bar".to_vec()));
        }

        #[test]
        fn test_state_delete() {
            let store = InMemoryStorageProvider::new();
            store.put_state("k", b"v").unwrap();
            store.delete_state("k").unwrap();
            assert!(store.get_state("k").unwrap().is_none());
        }
    }
}

#[cfg(test)]
mod consensus_tests {
    use super::*;
    use crate::transaction::{InventoryUpdate, Transaction, TransactionKind};

    fn genesis() -> Block {
        let empty = Block::new(0, vec![], "0".into());
        // genesis is special – mine manually
        let mut g = empty;
        g.mine(1);
        g
    }

    fn sample_tx() -> Transaction {
        Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-001".into(),
            owner_id: "node-1".into(),
            quantity_delta: 10,
            reason: "test".into(),
        }))
    }

    #[test]
    fn test_pow_provider_name() {
        let p = PowConsensusProvider::new(2);
        assert_eq!(p.name(), "proof-of-work");
    }

    #[test]
    fn test_pow_propose_block() {
        let provider = PowConsensusProvider::new(1);
        let g = genesis();
        let block = provider.propose_block(1, vec![sample_tx()], &g).unwrap();
        assert_eq!(block.index, 1);
        assert!(block.has_valid_pow(1));
        assert!(block.is_valid());
    }

    #[test]
    fn test_pow_validate_block_valid() {
        let provider = PowConsensusProvider::new(1);
        let g = genesis();
        let block = provider.propose_block(1, vec![], &g).unwrap();
        assert!(provider.validate_block(&block, &g).is_ok());
    }

    #[test]
    fn test_pow_validate_block_wrong_prev_hash() {
        let provider = PowConsensusProvider::new(1);
        let g = genesis();
        let mut block = provider.propose_block(1, vec![], &g).unwrap();
        block.previous_hash = "bad".into();
        block.hash = block.calculate_hash(); // re-hash so it's internally valid
                                             // Chains_to should fail even with correct hash if pow is recalculated
        assert!(provider.validate_block(&block, &g).is_err());
    }
}
