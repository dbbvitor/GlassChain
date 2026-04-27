use crate::block::Block;
use crate::error::CoreError;
use crate::transaction::Transaction;
use serde::{Deserialize, Serialize};

/// Default Proof-of-Work difficulty (number of leading zero characters required).
pub const DEFAULT_DIFFICULTY: usize = 2;

/// The `GlassChain` distributed ledger.
///
/// Maintains a validated chain of [`Block`]s and a pool of pending
/// [`Transaction`]s waiting to be committed in the next block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ledger {
    /// The ordered, validated chain of blocks.
    pub chain: Vec<Block>,
    /// Transactions received but not yet committed to a block.
    pub pending_transactions: Vec<Transaction>,
    /// `PoW` difficulty used when mining new blocks.
    pub difficulty: usize,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new(DEFAULT_DIFFICULTY)
    }
}

impl Ledger {
    /// Create a new ledger, automatically mining the genesis block.
    ///
    /// The genesis block uses a **fixed timestamp of `0`** so that every node
    /// with the same `PoW` difficulty produces an identical genesis hash.  This
    /// is required for `try_replace_chain` to accept chains from peers that
    /// were started at different wall-clock times.
    #[must_use]
    pub fn new(difficulty: usize) -> Self {
        // Build genesis directly with timestamp=0 (canonical, fixed).
        let mut genesis = Block {
            index: 0,
            timestamp: 0,
            transactions: Vec::new(),
            previous_hash: "0".to_owned(),
            nonce: 0,
            hash: String::new(),
        };
        genesis.hash = genesis.calculate_hash();
        genesis.mine(difficulty);
        Self {
            chain: vec![genesis],
            pending_transactions: Vec::new(),
            difficulty,
        }
    }

    /// Add a transaction to the pending pool.
    ///
    /// Transactions must have a non-empty `id`; duplicate IDs are silently
    /// ignored to provide idempotency across federated nodes.
    ///
    /// # Errors
    /// Returns `Err(CoreError::InvalidTransaction)` if `tx.id` is empty.
    pub fn add_transaction(&mut self, tx: Transaction) -> Result<(), CoreError> {
        if tx.id.is_empty() {
            return Err(CoreError::InvalidTransaction(
                "transaction id must not be empty".into(),
            ));
        }
        // Idempotency check: reject if already committed or pending.
        let already_committed = self
            .chain
            .iter()
            .flat_map(|b| b.transactions.iter())
            .any(|t| t.id == tx.id);
        let already_pending = self.pending_transactions.iter().any(|t| t.id == tx.id);
        if !already_committed && !already_pending {
            self.pending_transactions.push(tx);
        }
        Ok(())
    }

    /// Mine a new block containing all pending transactions.
    ///
    /// Returns the newly mined block (already appended to the chain).
    ///
    /// **Note:** this method holds no external lock – callers that need to avoid
    /// blocking an async task's mutex should use [`prepare_mining`] /
    /// [`commit_mined_block`] instead.
    ///
    /// # Errors
    /// Returns `Err(CoreError::EmptyLedger)` if the chain is somehow empty.
    ///
    /// # Panics
    /// Panics if the newly pushed block cannot be retrieved from the chain,
    /// which should never happen in practice.
    pub fn mine_pending_transactions(&mut self) -> Result<&Block, CoreError> {
        let previous = self.chain.last().ok_or(CoreError::EmptyLedger)?.clone();
        let index = previous.index + 1;
        let transactions = std::mem::take(&mut self.pending_transactions);
        let mut block = Block::new(index, transactions, previous.hash);
        block.mine(self.difficulty);
        self.chain.push(block);
        Ok(self.chain.last().expect("just pushed"))
    }

    /// Snapshot the chain tip and drain the pending pool for **out-of-lock** mining.
    ///
    /// Returns `(index, previous_hash, pending_transactions, difficulty)`.
    /// After calling this the pending pool is empty; the caller must either
    /// commit the mined block via [`commit_mined_block`] (which also handles
    /// restoring transactions on a stale tip) or push the transactions back
    /// manually.
    ///
    /// # Errors
    /// Returns `Err(CoreError::EmptyLedger)` if the chain is empty.
    pub fn prepare_mining(&mut self) -> Result<(u64, String, Vec<Transaction>, usize), CoreError> {
        let prev = self.chain.last().ok_or(CoreError::EmptyLedger)?.clone();
        let txns = std::mem::take(&mut self.pending_transactions);
        Ok((prev.index + 1, prev.hash, txns, self.difficulty))
    }

    /// Append a pre-mined block if `expected_prev_hash` still matches the chain tip.
    ///
    /// Returns `true` when the block was appended.  If the chain tip advanced
    /// while mining (a race), the block's transactions are restored to the
    /// pending pool and `false` is returned so the caller can retry.
    ///
    /// # Errors
    /// Returns `Err(CoreError::EmptyLedger)` if the chain is empty when
    /// checking the current tip hash.
    pub fn commit_mined_block(
        &mut self,
        block: Block,
        expected_prev_hash: &str,
    ) -> Result<bool, CoreError> {
        let tip_hash = self
            .chain
            .last()
            .ok_or(CoreError::EmptyLedger)?
            .hash
            .clone();
        if tip_hash != expected_prev_hash {
            // Chain advanced while we were mining; restore transactions to pool.
            for tx in block.transactions {
                let _ = self.add_transaction(tx);
            }
            log::warn!("Mined block is stale (chain tip moved); transactions restored to pool");
            return Ok(false);
        }
        self.chain.push(block);
        Ok(true)
    }

    /// Return a reference to the most recently committed block.
    #[must_use]
    pub fn latest_block(&self) -> Option<&Block> {
        self.chain.last()
    }

    /// Validate the entire chain from genesis to tip.
    ///
    /// Returns `Ok(())` if every block is internally valid, correctly
    /// chains to its predecessor, and satisfies the configured `PoW` target.
    ///
    /// # Errors
    /// Returns `Err(CoreError::InvalidBlock)` if any block fails its internal
    /// hash check, does not chain correctly to its predecessor, or does not
    /// satisfy the configured Proof-of-Work difficulty target.
    pub fn validate_chain(&self) -> Result<(), CoreError> {
        for i in 1..self.chain.len() {
            let current = &self.chain[i];
            let previous = &self.chain[i - 1];
            current.chains_to(previous)?;
            if !current.has_valid_pow(self.difficulty) {
                return Err(CoreError::InvalidBlock(format!(
                    "block {} does not satisfy PoW difficulty {}",
                    current.index, self.difficulty
                )));
            }
        }
        // Also validate the genesis block itself.
        if let Some(genesis) = self.chain.first() {
            if !genesis.is_valid() {
                return Err(CoreError::InvalidBlock(
                    "genesis block hash is invalid".into(),
                ));
            }
            if !genesis.has_valid_pow(self.difficulty) {
                return Err(CoreError::InvalidBlock(
                    "genesis block does not satisfy PoW difficulty".into(),
                ));
            }
        }
        Ok(())
    }

    /// Return `true` when the supplied chain is longer **and** valid,
    /// and replace the local chain if so (longest-chain consensus rule).
    pub fn try_replace_chain(&mut self, candidate: Vec<Block>) -> bool {
        if candidate.len() <= self.chain.len() {
            return false;
        }

        // Reject if genesis blocks differ.
        if let (Some(local_genesis), Some(cand_genesis)) = (self.chain.first(), candidate.first()) {
            if local_genesis.hash != cand_genesis.hash {
                log::warn!("Rejecting candidate chain: genesis hash mismatch");
                return false;
            }
        }

        // Validate candidate from scratch (hash integrity + PoW).
        for i in 1..candidate.len() {
            if candidate[i].chains_to(&candidate[i - 1]).is_err() {
                return false;
            }
            if !candidate[i].has_valid_pow(self.difficulty) {
                return false;
            }
        }
        if let Some(genesis) = candidate.first() {
            if !genesis.is_valid() {
                return false;
            }
            if !genesis.has_valid_pow(self.difficulty) {
                return false;
            }
        }
        log::info!(
            "Replacing local chain (length {}) with longer candidate (length {})",
            self.chain.len(),
            candidate.len()
        );
        self.chain = candidate;
        true
    }

    /// Return all supply-offer transactions committed on the ledger,
    /// useful for transparency queries (lead times, bottleneck analysis).
    pub fn committed_supply_offers(
        &self,
    ) -> impl Iterator<Item = &crate::transaction::SupplyOffer> {
        use crate::transaction::TransactionKind;
        self.chain
            .iter()
            .flat_map(|b| b.transactions.iter())
            .filter_map(|tx| {
                if let TransactionKind::SupplyOffer(ref offer) = tx.kind {
                    Some(offer)
                } else {
                    None
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{
        InventoryUpdate, PurchaseOrder, SupplyOffer, Transaction, TransactionKind,
    };

    fn supply_offer_tx(
        seller: &str,
        product: &str,
        qty: u64,
        price: u64,
        lead: u32,
    ) -> Transaction {
        Transaction::new(TransactionKind::SupplyOffer(SupplyOffer {
            product_id: product.into(),
            product_name: "Widget".into(),
            seller_id: seller.into(),
            quantity_available: qty,
            price_per_unit: price,
            lead_time_days: lead,
            currency: "USD".into(),
        }))
    }

    fn inventory_tx(owner: &str) -> Transaction {
        Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-001".into(),
            owner_id: owner.into(),
            quantity_delta: 50,
            reason: "test".into(),
        }))
    }

    #[test]
    fn test_new_ledger_has_genesis() {
        let ledger = Ledger::new(1);
        assert_eq!(ledger.chain.len(), 1);
        assert_eq!(ledger.chain[0].index, 0);
        assert_eq!(ledger.chain[0].previous_hash, "0");
    }

    #[test]
    fn test_add_and_mine_transactions() {
        let mut ledger = Ledger::new(1);
        ledger.add_transaction(inventory_tx("node-1")).unwrap();
        ledger.add_transaction(inventory_tx("node-2")).unwrap();
        assert_eq!(ledger.pending_transactions.len(), 2);

        ledger.mine_pending_transactions().unwrap();
        assert_eq!(ledger.chain.len(), 2);
        assert!(ledger.pending_transactions.is_empty());
        assert!(ledger.validate_chain().is_ok());
    }

    #[test]
    fn test_idempotent_transaction_addition() {
        let mut ledger = Ledger::new(1);
        let tx = inventory_tx("node-1");
        ledger.add_transaction(tx.clone()).unwrap();
        ledger.add_transaction(tx).unwrap(); // same tx again
        assert_eq!(ledger.pending_transactions.len(), 1);
    }

    #[test]
    fn test_validate_chain_detects_tamper() {
        let mut ledger = Ledger::new(1);
        ledger.add_transaction(inventory_tx("node-1")).unwrap();
        ledger.mine_pending_transactions().unwrap();

        // Tamper: corrupt the hash of block 1
        ledger.chain[1].hash = "tampered".into();
        assert!(ledger.validate_chain().is_err());
    }

    #[test]
    fn test_try_replace_chain_longer_wins() {
        let mut ledger_a = Ledger::new(1);
        let mut ledger_b = Ledger::new(1);

        // Independently created ledgers with the same difficulty should share
        // the same canonical genesis block (timestamp=0, deterministic hash).
        assert_eq!(ledger_a.chain[0].hash, ledger_b.chain[0].hash);

        ledger_b.add_transaction(inventory_tx("node-x")).unwrap();
        ledger_b.mine_pending_transactions().unwrap();
        ledger_b.add_transaction(inventory_tx("node-y")).unwrap();
        ledger_b.mine_pending_transactions().unwrap();

        assert!(ledger_a.try_replace_chain(ledger_b.chain.clone()));
        assert_eq!(ledger_a.chain.len(), 3);
    }

    #[test]
    fn test_try_replace_chain_shorter_ignored() {
        let mut ledger_a = Ledger::new(1);
        ledger_a.add_transaction(inventory_tx("node-1")).unwrap();
        ledger_a.mine_pending_transactions().unwrap();

        let ledger_b = Ledger::new(1); // only genesis
        assert!(!ledger_a.try_replace_chain(ledger_b.chain));
    }

    #[test]
    fn test_try_replace_chain_different_genesis_rejected() {
        let mut ledger_a = Ledger::new(1);

        // Construct an independent genesis whose hash is guaranteed to differ
        // from ledger_a's genesis by shifting the timestamp before re-mining.
        let mut alt_genesis = Block::new(0, vec![], "0".to_owned());
        alt_genesis.timestamp = alt_genesis.timestamp.wrapping_add(9_999_999);
        alt_genesis.mine(1);

        // Build two more blocks on top so the candidate chain is longer than
        // ledger_a (which has only its own genesis block).
        let mut b1 = Block::new(1, vec![], alt_genesis.hash.clone());
        b1.mine(1);
        let mut b2 = Block::new(2, vec![], b1.hash.clone());
        b2.mine(1);

        let candidate = vec![alt_genesis, b1, b2];

        // The candidate is longer and internally valid, but its genesis hash
        // does not match ledger_a's genesis — it must be rejected.
        assert!(!ledger_a.try_replace_chain(candidate));
    }

    #[test]
    fn test_committed_supply_offers() {
        let mut ledger = Ledger::new(1);
        ledger
            .add_transaction(supply_offer_tx("s1", "SKU-001", 100, 1000, 5))
            .unwrap();
        ledger
            .add_transaction(supply_offer_tx("s2", "SKU-002", 200, 500, 3))
            .unwrap();
        ledger.mine_pending_transactions().unwrap();

        let offers: Vec<_> = ledger.committed_supply_offers().collect();
        assert_eq!(offers.len(), 2);
    }

    #[test]
    fn test_purchase_order_transaction() {
        let mut ledger = Ledger::new(1);
        let po_tx = Transaction::new(TransactionKind::PurchaseOrder(PurchaseOrder {
            product_id: "SKU-001".into(),
            buyer_id: "buyer-1".into(),
            seller_id: "seller-1".into(),
            quantity: 50,
            agreed_price_per_unit: 1000,
            currency: "USD".into(),
            contract_id: None,
        }));
        ledger.add_transaction(po_tx).unwrap();
        ledger.mine_pending_transactions().unwrap();
        assert!(ledger.validate_chain().is_ok());
    }
}
