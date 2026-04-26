//! Pluggable provider traits for GlassChain's core protocol layers.
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
    /// consensus algorithm (e.g., mining a PoW nonce or gathering signatures).
    fn propose_block(
        &self,
        index: u64,
        transactions: Vec<Transaction>,
        previous: &Block,
    ) -> Result<Block, CoreError>;

    /// Validate a block proposed by a remote peer.
    ///
    /// Returns `Ok(())` when the block is structurally valid and satisfies the
    /// consensus rules (e.g., PoW target, quorum signatures).
    fn validate_block(&self, block: &Block, previous: &Block) -> Result<(), CoreError>;

    /// Human-readable identifier for this consensus implementation.
    fn name(&self) -> &str;
}

/// Abstraction over the persistent "World State" and block storage.
///
/// The World State is a key-value snapshot of the latest committed state
/// derived from all committed transactions (analogous to Ethereum's state
/// trie or Hyperledger Fabric's CouchDB state database).
///
/// Implementors may back this with in-memory structures, `sled`, RocksDB, or
/// any other store.
pub trait StorageProvider: Send + Sync {
    /// Persist a committed block.
    fn put_block(&self, block: &Block) -> Result<(), CoreError>;

    /// Retrieve a block by its sequential index.
    fn get_block(&self, index: u64) -> Result<Option<Block>, CoreError>;

    /// Return the index of the highest committed block, or `None` if the store
    /// is empty.
    fn latest_block_index(&self) -> Result<Option<u64>, CoreError>;

    /// Write a World State key-value pair.
    fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError>;

    /// Read a World State value by key.
    fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError>;

    /// Delete a World State key.
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
    fn execute(
        &self,
        contract_id: &str,
        payload: &[u8],
        gas_limit: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError>;

    /// Human-readable identifier for this execution implementation.
    fn name(&self) -> &str;
}

/// A minimal in-memory [`StorageProvider`] used for testing and the default
/// single-node configuration.
///
/// **Not** suitable for production (data is lost on process restart).
pub mod in_memory {
    use super::*;
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
            Ok(self
                .state
                .read()
                .expect("lock poisoned")
                .get(key)
                .cloned())
        }

        fn delete_state(&self, key: &str) -> Result<(), CoreError> {
            self.state
                .write()
                .expect("lock poisoned")
                .remove(key);
            Ok(())
        }

        fn name(&self) -> &str {
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
