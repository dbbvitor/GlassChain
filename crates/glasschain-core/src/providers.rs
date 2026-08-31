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
use crate::consensus::CommitNotification;
use crate::endorsement::{EndorsementEvaluation, EndorsementRequest, PolicyExpression};
use crate::error::CoreError;
use crate::transaction::Transaction;
use crate::write_set::ExecutionResult;

/// Abstraction over the consensus mechanism used to agree on the next block.
///
/// Implementors may use Proof-of-Work, Raft, PBFT, or any other algorithm.
/// The node calls [`ConsensusProvider::propose_block`] when it wants to add
/// pending transactions to the chain and waits for the provider to return a
/// [`CommitNotification`]: the finished block plus the quorum certificate
/// attesting it (ADR-002). No commit consumer may depend on "the leader said
/// so" — every notification carries the attestation set.
pub trait ConsensusProvider: Send + Sync {
    /// Propose a new block containing `transactions` that chains onto `previous`.
    ///
    /// Returns the finished block (with a valid `hash`) and its quorum
    /// certificate on success. The provider is responsible for any work
    /// required by the chosen consensus algorithm (e.g., mining a `PoW` nonce
    /// or gathering validator signatures). The retained Proof-of-Work provider
    /// supplies a degenerate certificate: the valid nonce is the attestation.
    ///
    /// # Errors
    /// Returns `Err` if the consensus algorithm fails to produce a valid block
    /// (e.g., mining failure or quorum not reached).
    fn propose_block(
        &self,
        index: u64,
        transactions: Vec<Transaction>,
        previous: &Block,
    ) -> Result<CommitNotification, CoreError>;

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

    /// Atomically persist `block` **and** apply its canonical write set to the
    /// world state, rejecting a stale candidate whole.
    ///
    /// This is the one atomic block-plus-state commit boundary (ADR-007
    /// decision 2): under the backend's atomic section the implementation
    /// verifies that `block` chains to the current tip (`previous_hash` and
    /// `index`; the store may be empty only for the genesis block), persists
    /// the block, and applies every [`PersistentWrite`](crate::PersistentWrite)
    /// in the block (sets write the value, deletes remove the key, keyed by
    /// [`PersistentWrite::state_key`](crate::PersistentWrite::state_key)).  On
    /// any tip mismatch the **whole** candidate — block and write set — is
    /// rejected with [`CoreError::InvalidBlock`]; a partial write set is never
    /// acknowledged.
    ///
    /// The default implementation is a sequential fallback (block first, then
    /// one `put_state`/`delete_state` per write): it is correct for
    /// single-writer processes but **not atomic**.  Implementors should
    /// override it with a real atomic section (e.g. a sled multi-tree
    /// transaction).
    ///
    /// # Errors
    /// Returns [`CoreError::InvalidBlock`] if `block` does not chain to the
    /// stored tip, and `Err` if the backend fails.
    fn apply_block(&self, block: &Block) -> Result<(), CoreError> {
        let tip = match self.latest_block_index()? {
            Some(tip_index) => {
                let Some(tip) = self.get_block(tip_index)? else {
                    return Err(CoreError::Storage(format!(
                        "block {tip_index} missing from store"
                    )));
                };
                Some(tip)
            }
            None => None,
        };
        validate_tip_chain(block, tip.as_ref())?;
        self.put_block(block)?;
        for write in &block.write_set {
            match &write.op {
                crate::write_set::WriteOp::Set(value) => {
                    self.put_state(&write.state_key(), value)?;
                }
                crate::write_set::WriteOp::Delete => {
                    self.delete_state(&write.state_key())?;
                }
            }
        }
        Ok(())
    }

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

/// The block-plus-state boundary's chain check (ADR-007 decision 2): a
/// candidate must chain to the stored tip.
///
/// `None` means an empty store, which accepts only the genesis block.  Every
/// [`StorageProvider::apply_block`] implementation routes through this so
/// stale candidates are rejected whole, with one error shape across backends.
///
/// # Errors
///
/// Returns [`CoreError::InvalidBlock`] when the candidate does not chain to
/// the tip.
pub fn validate_tip_chain(block: &Block, tip: Option<&Block>) -> Result<(), CoreError> {
    match tip {
        None => {
            if block.index != 0 {
                return Err(CoreError::InvalidBlock(format!(
                    "block {} does not chain to the empty store",
                    block.index
                )));
            }
        }
        Some(tip) => {
            if block.index != tip.index + 1 || block.previous_hash != tip.hash {
                return Err(CoreError::InvalidBlock(format!(
                    "stale tip: block {} does not chain to stored tip {}",
                    block.index, tip.index
                )));
            }
        }
    }
    Ok(())
}

/// Independent instruction and host-operation budgets for one contract execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// Maximum instruction-fuel budget for the execution provider.
    pub fuel_limit: u64,
    /// Maximum gas charged for host state operations.
    pub operation_gas_limit: u64,
}

impl ExecutionLimits {
    /// Create execution limits with independent fuel and operation budgets.
    #[must_use]
    pub const fn new(fuel_limit: u64, operation_gas_limit: u64) -> Self {
        Self {
            fuel_limit,
            operation_gas_limit,
        }
    }
}

/// Abstraction over a smart-contract execution backend.
///
/// Implementors may execute contracts as native Rust closures, WASM modules
/// (via Wasmtime), or any other sandboxed runtime.  The execution provider
/// receives a serialised contract payload and a world-state snapshot, and
/// returns the typed [`ExecutionResult`]: ephemeral output separated from the
/// explicit persistent write set (ADR-007 decision 1).
pub trait ExecutionProvider: Send + Sync {
    /// Execute a contract payload and return the typed [`ExecutionResult`]:
    /// invocation-local ephemeral output plus explicit persistent writes.
    ///
    /// `payload` is an opaque byte slice whose interpretation is provider-
    /// specific (e.g., a WASM module, a Lua script, or JSON instructions).
    /// `limits` independently caps instruction execution and host operations.
    /// The provider should return [`CoreError::GasExhausted`] when either
    /// budget is exceeded.
    ///
    /// # Errors
    /// Returns `Err(CoreError::GasExhausted)` if either limit is exceeded, or
    /// `Err` for any other execution failure.
    fn execute(
        &self,
        contract_id: &str,
        payload: &[u8],
        limits: ExecutionLimits,
    ) -> Result<ExecutionResult, CoreError>;

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
    /// Returns `Err(CoreError::GasExhausted)` if either limit is exceeded, or
    /// `Err` for any other execution failure.
    fn execute_with_state(
        &self,
        contract_id: &str,
        payload: &[u8],
        initial_state: std::collections::HashMap<String, Vec<u8>>,
        limits: ExecutionLimits,
    ) -> Result<ExecutionResult, CoreError> {
        let _ = initial_state;
        self.execute(contract_id, payload, limits)
    }

    /// Human-readable identifier for this execution implementation.
    fn name(&self) -> &str;
}

/// Abstraction over business-authorization (endorsement) enforcement.
///
/// This trait is identity-neutral: the expression, request, and result types
/// live in `glasschain-core`, while an implementation derives principals from
/// verified credentials (ADR-008). Application authorization via this seam is
/// separate from consensus finality.
pub trait EndorsementProvider: Send + Sync {
    /// Evaluate one [`PolicyExpression`] against the request's signers.
    ///
    /// Implementations must:
    /// - derive each signer's principal from the authenticated key, never from
    ///   the caller-supplied label alone;
    /// - reject a claimed principal that conflicts with the verified identity;
    /// - count at most one signature per distinct principal — duplicate,
    ///   multi-node, and replayed signatures never increase the count.
    ///
    /// # Errors
    ///
    /// Returns `Err` when a signer cannot be authenticated (unknown key), a
    /// claimed principal conflicts with the verified identity, or the
    /// expression is not valid v1 policy metadata (allow-all shapes are
    /// rejected). Signatures that fail cryptographic verification are skipped,
    /// not fatal.
    fn evaluate(
        &self,
        expression: &PolicyExpression,
        request: &EndorsementRequest,
    ) -> Result<EndorsementEvaluation, CoreError>;

    /// Human-readable identifier for this endorsement implementation.
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
    ) -> Result<CommitNotification, CoreError> {
        let mut block = Block::new(index, transactions, previous.hash.clone());
        block.mine(self.difficulty);
        // PoW's attestation is the valid nonce carried by the block itself:
        // the certificate is degenerate but still explicit on the seam.
        Ok(CommitNotification::for_pow_block(block))
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
    use super::{validate_tip_chain, Block, CoreError, StorageProvider};
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

        // The atomic section must hold both write locks until every write is
        // applied and the block insert is complete — the guards deliberately
        // live to the end of the function (the lint mis-reads the loop's last
        // use as a tighter drop point).
        #[allow(clippy::significant_drop_tightening)]
        fn apply_block(&self, block: &Block) -> Result<(), CoreError> {
            // One atomic block-plus-state boundary (ADR-007 decision 2): the
            // tip check, block insert, and write-set application all happen
            // under the same pair of write locks. Lock order (blocks, then
            // state) is fixed so concurrent applies serialize and a stale
            // candidate is rejected whole.
            let mut blocks = self.blocks.write().expect("lock poisoned");
            let tip = match blocks.keys().copied().max() {
                Some(tip_index) => Some(blocks.get(&tip_index).cloned().ok_or_else(|| {
                    CoreError::Storage(format!("block {tip_index} missing from store"))
                })?),
                None => None,
            };
            validate_tip_chain(block, tip.as_ref())?;
            let mut state = self.state.write().expect("lock poisoned");
            blocks.insert(block.index, block.clone());
            for write in &block.write_set {
                write.apply_to_cache(&mut state);
            }
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
        use crate::write_set::{PersistentWrite, WriteOp, WriteVisibility};

        fn genesis() -> Block {
            let mut b = Block::new(0, vec![], "0".into());
            b.mine(1);
            b
        }

        fn write(channel: &str, contract: &str, key: &str, value: &[u8]) -> PersistentWrite {
            PersistentWrite {
                channel: channel.into(),
                contract: contract.into(),
                key: key.into(),
                op: WriteOp::Set(value.to_vec()),
                visibility: WriteVisibility::Public,
            }
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

        #[test]
        fn test_apply_block_applies_write_set_atomically() {
            let store = InMemoryStorageProvider::new();
            let g = genesis();
            store.apply_block(&g).unwrap();

            let writes = vec![
                write("ch", "contract", "a", b"1"),
                PersistentWrite {
                    op: WriteOp::Delete,
                    ..write("ch", "contract", "b", b"gone")
                },
            ];
            let mut b = Block::with_write_set(1, vec![], g.hash, writes);
            b.mine(1);
            store.apply_block(&b).unwrap();

            assert_eq!(store.get_block(1).unwrap().unwrap().hash, b.hash);
            assert_eq!(
                store.get_state("ws:ch:contract:a").unwrap(),
                Some(b"1".to_vec()),
                "set writes must land in the world state"
            );
            assert!(
                store.get_state("ws:ch:contract:b").unwrap().is_none(),
                "delete writes must remove the key"
            );
        }

        #[test]
        fn test_apply_block_rejects_stale_tip_whole() {
            let store = InMemoryStorageProvider::new();
            let g = genesis();
            store.apply_block(&g).unwrap();

            // The candidate chains to a hash that is not the stored tip.
            let mut stale = Block::with_write_set(
                1,
                vec![],
                "not-the-tip".into(),
                vec![write("ch", "contract", "k", b"v")],
            );
            stale.mine(1);
            let err = store
                .apply_block(&stale)
                .expect_err("stale tip must be rejected");
            assert!(matches!(err, CoreError::InvalidBlock(_)));

            assert!(
                store.get_block(1).unwrap().is_none(),
                "the stale block must not be persisted"
            );
            assert!(
                store.get_state("ws:ch:contract:k").unwrap().is_none(),
                "the stale write set must not be applied"
            );
        }

        #[test]
        fn test_apply_block_rejects_index_gap_whole() {
            let store = InMemoryStorageProvider::new();
            let g = genesis();
            store.apply_block(&g).unwrap();

            // Correct previous_hash but a gap in the index sequence.
            let mut gap =
                Block::with_write_set(2, vec![], g.hash, vec![write("ch", "contract", "k", b"v")]);
            gap.mine(1);
            let err = store
                .apply_block(&gap)
                .expect_err("index gap must be rejected");
            assert!(matches!(err, CoreError::InvalidBlock(_)));
            assert!(store.get_state("ws:ch:contract:k").unwrap().is_none());
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
        let notification = provider.propose_block(1, vec![sample_tx()], &g).unwrap();
        assert_eq!(notification.block.index, 1);
        assert!(notification.block.has_valid_pow(1));
        assert!(notification.block.is_valid());
        assert!(
            notification.certificate.is_degenerate(),
            "PoW supplies the degenerate certificate"
        );
        assert!(
            notification.validate().is_ok(),
            "the PoW notification validates structurally"
        );
    }

    #[test]
    fn test_pow_validate_block_valid() {
        let provider = PowConsensusProvider::new(1);
        let g = genesis();
        let notification = provider.propose_block(1, vec![], &g).unwrap();
        assert!(provider.validate_block(&notification.block, &g).is_ok());
    }

    #[test]
    fn test_pow_validate_block_wrong_prev_hash() {
        let provider = PowConsensusProvider::new(1);
        let g = genesis();
        let mut block = provider.propose_block(1, vec![], &g).unwrap().block;
        block.previous_hash = "bad".into();
        block.hash = block.calculate_hash(); // re-hash so it's internally valid
                                             // Chains_to should fail even with correct hash if pow is recalculated
        assert!(provider.validate_block(&block, &g).is_err());
    }

    #[test]
    fn test_pow_validate_block_rejects_insufficient_pow() {
        let g = genesis();
        // Block chains correctly to genesis but its hash does not satisfy a
        // stricter PoW target (difficulty 2).
        let mut block = Block::new(1, vec![], g.hash.clone());
        while block.hash.starts_with("00") {
            block.nonce = block.nonce.wrapping_add(1);
            block.hash = block.calculate_hash();
        }
        let strict = PowConsensusProvider::new(2);
        let err = strict.validate_block(&block, &g).unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidBlock(msg) if msg.contains("PoW difficulty 2")
        ));
    }
}
