use crate::crypto::sha256;
use crate::error::CoreError;
use crate::transaction::Transaction;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// A single block in the `GlassChain` blockchain.
///
/// Each block commits a batch of [`Transaction`]s and cryptographically chains
/// to the previous block via `previous_hash`, forming a tamper-evident ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Block {
    /// Sequential block index (genesis block has index 0).
    pub index: u64,
    /// Unix timestamp (seconds) of when the block was mined.
    pub timestamp: u64,
    /// Ordered list of transactions committed in this block.
    pub transactions: Vec<Transaction>,
    /// Hash of the preceding block; `"0"` for the genesis block.
    pub previous_hash: String,
    /// `PoW` nonce found during mining.
    pub nonce: u64,
    /// SHA-256 hash of the block's canonical content (including `nonce`).
    pub hash: String,
}

impl Block {
    /// Compute the canonical SHA-256 hash for the current block state.
    ///
    /// The hash covers: `index`, `timestamp`, serialised `transactions`,
    /// `previous_hash`, and `nonce`.  Any change to these fields invalidates
    /// the stored hash.
    ///
    /// Fields are encoded as a JSON tuple to ensure an unambiguous canonical
    /// representation (avoids hash collisions from raw string concatenation).
    ///
    /// # Panics
    ///
    /// Panics if `serde_json::to_string` fails to serialise the block header
    /// tuple.  In practice this cannot occur because every field in the tuple
    /// is JSON-safe (integers, a `Vec<Transaction>`, and a `String`).
    #[must_use]
    pub fn calculate_hash(&self) -> String {
        let content = serde_json::to_string(&(
            self.index,
            self.timestamp,
            &self.transactions,
            &self.previous_hash,
            self.nonce,
        ))
        .expect("block header must be serializable");
        sha256(content.as_bytes())
    }

    /// Return `true` if the block's hash satisfies the Proof-of-Work target
    /// (i.e., it starts with `difficulty` leading zero characters).
    #[must_use]
    pub fn has_valid_pow(&self, difficulty: usize) -> bool {
        let target = "0".repeat(difficulty);
        self.hash.starts_with(&target)
    }

    /// Create a new, **unmined** block.
    ///
    /// The caller should invoke [`Block::mine`] before appending to the ledger.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is set to a time before the Unix epoch
    /// (i.e., [`SystemTime::now`] returns a value earlier than
    /// [`std::time::UNIX_EPOCH`]).
    #[must_use]
    pub fn new(index: u64, transactions: Vec<Transaction>, previous_hash: String) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before UNIX epoch")
            .as_secs();
        let mut block = Self {
            index,
            timestamp,
            transactions,
            previous_hash,
            nonce: 0,
            hash: String::new(),
        };
        block.hash = block.calculate_hash();
        block
    }

    /// Perform Proof-of-Work: increment `nonce` until `hash` starts with
    /// `difficulty` leading zero characters.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is set to a time before the Unix epoch.
    /// This can only be reached on the rare nonce-wrap path where the
    /// timestamp is refreshed to keep the hash space moving.
    pub fn mine(&mut self, difficulty: usize) {
        let target = "0".repeat(difficulty);
        while !self.hash.starts_with(&target) {
            // If the nonce is about to wrap, refresh the timestamp so the
            // hash space changes and mining can continue.
            if self.nonce == u64::MAX {
                self.timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock is before UNIX epoch")
                    .as_secs();
            }
            self.nonce = self.nonce.wrapping_add(1);
            self.hash = self.calculate_hash();
        }
        log::debug!(
            "Block {} mined with nonce {} → {}",
            self.index,
            self.nonce,
            &self.hash[..8]
        );
    }

    /// Return `true` when the stored hash matches a freshly computed one.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.hash == self.calculate_hash()
    }

    /// Validate that this block structurally chains to `previous`.
    ///
    /// # Errors
    ///
    /// Returns `Err(CoreError::InvalidBlock)` in any of the following cases:
    ///
    /// * `self.previous_hash` does not equal `previous.hash`.
    /// * `self.index` is not exactly `previous.index + 1`.
    /// * `self.hash` does not match a freshly recomputed hash of this block's
    ///   contents (i.e., the block has been tampered with).
    pub fn chains_to(&self, previous: &Self) -> Result<(), CoreError> {
        if self.previous_hash != previous.hash {
            return Err(CoreError::InvalidBlock(format!(
                "block {} previous_hash mismatch: expected {}, got {}",
                self.index, previous.hash, self.previous_hash
            )));
        }
        if self.index != previous.index + 1 {
            return Err(CoreError::InvalidBlock(format!(
                "block index gap: expected {}, got {}",
                previous.index + 1,
                self.index
            )));
        }
        if !self.is_valid() {
            return Err(CoreError::InvalidBlock(format!(
                "block {} hash is invalid",
                self.index
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{InventoryUpdate, Transaction, TransactionKind};

    fn sample_tx() -> Transaction {
        Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-001".into(),
            owner_id: "node-1".into(),
            quantity_delta: 100,
            reason: "initial stock".into(),
        }))
    }

    #[test]
    fn test_block_hash_changes_with_nonce() {
        let b = Block::new(0, vec![], "0".into());
        let hash_before = b.hash.clone();
        let mut b2 = b;
        b2.nonce = 1;
        b2.hash = b2.calculate_hash();
        assert_ne!(hash_before, b2.hash);
    }

    #[test]
    fn test_block_is_valid_after_creation() {
        let b = Block::new(1, vec![sample_tx()], "abc".into());
        assert!(b.is_valid());
    }

    #[test]
    fn test_block_invalid_after_tamper() {
        let mut b = Block::new(1, vec![], "abc".into());
        b.transactions.push(sample_tx()); // tamper without recalculating hash
        assert!(!b.is_valid());
    }

    #[test]
    fn test_mine_produces_leading_zeros() {
        let mut b = Block::new(0, vec![], "0".into());
        b.mine(2);
        assert!(b.hash.starts_with("00"));
        assert!(b.is_valid());
    }

    #[test]
    fn test_chains_to_valid() {
        let mut genesis = Block::new(0, vec![], "0".into());
        genesis.mine(1);
        let mut b1 = Block::new(1, vec![], genesis.hash.clone());
        b1.mine(1);
        assert!(b1.chains_to(&genesis).is_ok());
    }

    #[test]
    fn test_chains_to_invalid_prev_hash() {
        let genesis = Block::new(0, vec![], "0".into());
        let b1 = Block::new(1, vec![], "wrong_hash".into());
        assert!(b1.chains_to(&genesis).is_err());
    }

    #[test]
    fn test_chains_to_rejects_index_gap() {
        let mut genesis = Block::new(0, vec![], "0".into());
        genesis.mine(1);
        // previous_hash matches but the index jumps by 2 instead of 1
        let mut b2 = Block::new(2, vec![], genesis.hash.clone());
        b2.mine(1);
        assert!(b2.chains_to(&genesis).is_err());
    }
}
