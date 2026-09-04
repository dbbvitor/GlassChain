use crate::block::Block;
use crate::capability::{validate_record_under, CapabilityHistory};
use crate::endorsement::PolicyHistory;
use crate::error::CoreError;
use crate::transaction::{Transaction, TransactionKind};
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
            write_set: Vec::new(),
            previous_hash: "0".to_owned(),
            nonce: 0,
            hash: String::new(),
            certificate: None,
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
    /// ignored to provide idempotency across federated nodes. Canonical v1
    /// records and capability activations are validated against the
    /// capability set effective at the next height before admission
    /// (ADR-010 decision 5).
    ///
    /// # Errors
    /// Returns `Err(CoreError::InvalidTransaction)` if `tx.id` is empty, a
    /// canonical record fails validation, or a capability activation is not
    /// admissible at the next height.
    pub fn add_transaction(&mut self, tx: Transaction) -> Result<(), CoreError> {
        if tx.id.is_empty() {
            return Err(CoreError::InvalidTransaction(
                "transaction id must not be empty".into(),
            ));
        }
        // ponytail: O(chain) capability-history rebuild per admission; the
        // idempotency scan below is O(chain) anyway — cache when blocks grow.
        let next_height = self.chain.last().map_or(1, |b| b.index + 1);
        match &tx.kind {
            TransactionKind::CanonicalRecord(ref record) => {
                let history = CapabilityHistory::build_from_blocks(&self.chain)?;
                validate_record_under(&history.effective_set(next_height), record)?;
            }
            TransactionKind::CapabilityActivation(ref activation) => {
                let mut history = CapabilityHistory::build_from_blocks(&self.chain)?;
                history.apply(activation.clone(), next_height)?;
            }
            TransactionKind::PolicyUpdate(ref update) => {
                // Structural v1 policy metadata; authorization (signatures
                // under the current effective policy) is verified at the
                // network commit path, where the provider lives.
                if update.channel.is_empty() {
                    return Err(CoreError::InvalidTransaction(
                        "endorsement policy update: channel must not be empty".into(),
                    ));
                }
                update.policies.validate()?;
            }
            _ => {}
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
        // Commit gate: re-validate every canonical record and capability
        // activation in the block under the capability set effective at its
        // height, and every policy update under the replayed policy history
        // (including the same-block policy/write conflict rule), so a crafted
        // block never commits invalid content.
        let mut history = CapabilityHistory::build_from_blocks(&self.chain)?;
        history.validate_block(&block)?;
        let mut policies = PolicyHistory::build_from_blocks(&self.chain)?;
        policies.validate_block(&block)?;
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
    /// Consensus admission for a block inside this ledger: `PoW` difficulty
    /// **or** a non-degenerate BFT certificate whose structure matches the
    /// block (ADR-014). Deep certificate verification is the node's job
    /// (it owns the derived validator set); the ledger checks shape only.
    #[must_use]
    pub fn block_consensus_admissible(block: &Block, difficulty: usize) -> bool {
        block.has_valid_pow(difficulty)
            || block.certificate.as_ref().is_some_and(|certificate| {
                !certificate.is_degenerate() && certificate.validate(block).is_ok()
            })
    }

    /// # Errors
    ///
    /// Returns [`CoreError::InvalidBlock`] when any block fails chaining,
    /// consensus admission, or capability validation.
    pub fn validate_chain(&self) -> Result<(), CoreError> {
        for i in 1..self.chain.len() {
            let current = &self.chain[i];
            let previous = &self.chain[i - 1];
            current.chains_to(previous)?;
            if !Self::block_consensus_admissible(current, self.difficulty) {
                return Err(CoreError::InvalidBlock(format!(
                    "block {} does not satisfy PoW difficulty {} and carries no valid certificate",
                    current.index, self.difficulty
                )));
            }
        }
        // Height-selected capability validation and history derivation
        // (ADR-010 decision 5): each block is validated under the capability
        // set effective at its height.
        CapabilityHistory::build_from_blocks(&self.chain)
            .map_err(|e| CoreError::InvalidBlock(format!("capability history is invalid: {e}")))?;
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
        let mut history = CapabilityHistory::default();
        for i in 1..candidate.len() {
            if candidate[i].chains_to(&candidate[i - 1]).is_err() {
                return false;
            }
            if !Self::block_consensus_admissible(&candidate[i], self.difficulty) {
                return false;
            }
            // Commit gate for peer blocks: canonical records and capability
            // activations are folded per block under the set effective at that
            // height.
            if let Err(e) = history.validate_block(&candidate[i]) {
                log::warn!("Rejecting candidate chain: invalid block content: {e}");
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
    use crate::canonical::{CanonicalRecord, RecordSignature};
    use crate::capability::{capability_hash, CapabilityActivation};
    use crate::transaction::{
        InventoryUpdate, PurchaseOrder, SupplyOffer, Transaction, TransactionKind,
    };
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

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

    /// A canonical lot record wrapped in a transaction; `valid: false` drops a
    /// required field so v1 validation rejects it.
    fn canonical_tx(valid: bool) -> Transaction {
        let payload: BTreeMap<String, Value> = serde_json::from_value(json!({
            "lot_id": "lot-1",
            "product_id": "SKU-1",
            "batch_number": "BATCH-001",
        }))
        .expect("payload map");
        let mut record = CanonicalRecord::new(0, "lot", payload, "org-issuer");
        record.signatures.push(RecordSignature {
            algorithm: crate::wire::SignatureAlgorithm::Ed25519,
            signer: "org-issuer".into(),
            signature_bytes: vec![0x42],
        });
        record.commitment = record.commitment().ok();
        if !valid {
            record.payload.remove("batch_number");
        }
        Transaction::with_id("canonical:1", TransactionKind::CanonicalRecord(record))
    }

    /// A capability activation declaring `activation_height`.
    fn activation_tx(id: &str, activation_height: u64) -> Transaction {
        Transaction::with_id(
            format!("cap:{id}:{activation_height}"),
            TransactionKind::CapabilityActivation(CapabilityActivation {
                capability_id: id.into(),
                version: 1,
                hash: capability_hash(id, 1),
                activation_height,
                signatures: vec![RecordSignature {
                    algorithm: crate::wire::SignatureAlgorithm::Ed25519,
                    signer: "org-issuer".into(),
                    signature_bytes: vec![0x42],
                }],
            }),
        )
    }

    #[test]
    fn test_add_transaction_accepts_valid_canonical_record() {
        let mut ledger = Ledger::new(1);
        ledger
            .add_transaction(canonical_tx(true))
            .expect("valid record");
        assert_eq!(ledger.pending_transactions.len(), 1);
    }

    #[test]
    fn test_add_transaction_rejects_invalid_canonical_record() {
        let mut ledger = Ledger::new(1);
        let error = ledger
            .add_transaction(canonical_tx(false))
            .expect_err("invalid record must be rejected at admission");
        assert!(error.to_string().contains("batch_number"), "{error}");
        assert!(ledger.pending_transactions.is_empty());
    }

    #[test]
    fn test_commit_mined_block_rejects_invalid_canonical_record() {
        let mut ledger = Ledger::new(1);
        let previous = ledger.chain[0].clone();
        let mut block = Block::new(1, vec![canonical_tx(false)], previous.hash.clone());
        block.mine(1);
        let error = ledger
            .commit_mined_block(block, &previous.hash)
            .expect_err("invalid record must be rejected at commit");
        assert!(error.to_string().contains("batch_number"), "{error}");
        assert_eq!(ledger.chain.len(), 1, "block must not be appended");
    }

    #[test]
    fn test_try_replace_chain_rejects_invalid_canonical_record() {
        let mut ledger = Ledger::new(1);
        ledger
            .add_transaction(canonical_tx(true))
            .expect("valid record");
        ledger.mine_pending_transactions().expect("mine");
        assert!(ledger.validate_chain().is_ok());

        // A longer candidate whose extra block carries an invalid record.
        let genesis = ledger.chain[0].clone();
        let mut bad = Block::new(1, vec![canonical_tx(false)], genesis.hash.clone());
        bad.mine(1);
        let candidate = vec![genesis, bad];
        assert!(
            !ledger.try_replace_chain(candidate),
            "candidate with invalid canonical record must be rejected"
        );
        assert_eq!(ledger.chain.len(), 2, "local chain stays authoritative");
    }

    #[test]
    fn test_add_transaction_rejects_non_future_activation() {
        let mut ledger = Ledger::new(1);
        // Tip is genesis (index 0); the next block is 1, so height 1 is not
        // strictly future.
        let error = ledger
            .add_transaction(activation_tx("pdc", 1))
            .expect_err("non-future activation must be rejected");
        assert!(error.to_string().contains("future"), "{error}");
        assert!(ledger.pending_transactions.is_empty());
    }

    #[test]
    fn test_activation_commits_and_flips_the_effective_set() {
        let mut ledger = Ledger::new(1);
        ledger
            .add_transaction(activation_tx("bft_consensus", 3))
            .expect("future activation admitted");
        ledger.mine_pending_transactions().expect("mine block 1");
        ledger.mine_pending_transactions().expect("mine block 2");
        ledger.mine_pending_transactions().expect("mine block 3");

        assert!(ledger.validate_chain().is_ok());
        let history = CapabilityHistory::build_from_blocks(&ledger.chain).expect("valid history");
        assert!(!history.effective_set(2).is_active("bft_consensus"));
        assert!(history.effective_set(3).is_active("bft_consensus"));
    }

    #[test]
    fn test_commit_mined_block_rejects_same_block_activation() {
        let mut ledger = Ledger::new(1);
        let previous = ledger.chain[0].clone();
        // Activation declares height 1 while riding in block 1: not future.
        let mut block = Block::new(1, vec![activation_tx("pdc", 1)], previous.hash.clone());
        block.mine(1);
        let error = ledger
            .commit_mined_block(block, &previous.hash)
            .expect_err("same-block activation must be rejected at commit");
        assert!(error.to_string().contains("future"), "{error}");
        assert_eq!(ledger.chain.len(), 1, "block must not be appended");
    }

    #[test]
    fn test_try_replace_chain_rejects_invalid_activation() {
        let mut ledger = Ledger::new(1);
        ledger
            .add_transaction(canonical_tx(true))
            .expect("valid record");
        ledger.mine_pending_transactions().expect("mine");

        let genesis = ledger.chain[0].clone();
        let mut bad = Block::new(1, vec![activation_tx("pdc", 1)], genesis.hash.clone());
        bad.mine(1);
        let candidate = vec![genesis, bad];
        assert!(
            !ledger.try_replace_chain(candidate),
            "candidate with an invalid activation must be rejected"
        );
        assert_eq!(ledger.chain.len(), 2, "local chain stays authoritative");
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

        assert_eq!(ledger.committed_supply_offers().count(), 2);
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

    #[test]
    fn test_add_transaction_rejects_empty_id() {
        let mut ledger = Ledger::new(1);
        let tx = Transaction::with_id(
            "",
            TransactionKind::InventoryUpdate(InventoryUpdate {
                product_id: "SKU-001".into(),
                owner_id: "node-1".into(),
                quantity_delta: 50,
                reason: "test".into(),
            }),
        );
        assert!(matches!(
            ledger.add_transaction(tx),
            Err(CoreError::InvalidTransaction(_))
        ));
        assert!(ledger.pending_transactions.is_empty());
    }

    #[test]
    fn test_add_committed_transaction_is_idempotent() {
        let mut ledger = Ledger::new(1);
        let tx = Transaction::with_id(
            "tx-committed",
            TransactionKind::InventoryUpdate(InventoryUpdate {
                product_id: "SKU-001".into(),
                owner_id: "node-1".into(),
                quantity_delta: 50,
                reason: "test".into(),
            }),
        );
        ledger.add_transaction(tx.clone()).unwrap();
        ledger.mine_pending_transactions().unwrap();
        assert_eq!(ledger.chain[1].transactions.len(), 1);

        // Re-adding an already-committed transaction must be a silent no-op.
        assert!(ledger.add_transaction(tx).is_ok());
        assert!(ledger.pending_transactions.is_empty());
        assert_eq!(ledger.chain.len(), 2);
        assert_eq!(ledger.chain[1].transactions.len(), 1);
    }

    #[test]
    fn test_prepare_mining_snapshots_and_drains_pending() {
        let mut ledger = Ledger::new(2);
        let first = inventory_tx("node-1");
        let second = inventory_tx("node-2");
        ledger.add_transaction(first.clone()).unwrap();
        ledger.add_transaction(second.clone()).unwrap();

        let (index, prev_hash, txns, difficulty) = ledger.prepare_mining().unwrap();

        assert_eq!(index, 1);
        assert_eq!(prev_hash, ledger.chain[0].hash);
        assert_eq!(txns, vec![first, second]);
        assert_eq!(difficulty, 2);
        assert!(ledger.pending_transactions.is_empty());
    }

    #[test]
    fn test_commit_mined_block_appends_when_tip_matches() {
        let mut ledger = Ledger::new(1);
        ledger.add_transaction(inventory_tx("node-1")).unwrap();

        let (index, prev_hash, txns, difficulty) = ledger.prepare_mining().unwrap();
        let mut block = Block::new(index, txns, prev_hash.clone());
        block.mine(difficulty);

        assert!(ledger.commit_mined_block(block, &prev_hash).unwrap());
        assert_eq!(ledger.chain.len(), 2);
        assert_eq!(ledger.chain[1].index, 1);
        assert!(ledger.pending_transactions.is_empty());
        assert!(ledger.validate_chain().is_ok());
    }

    #[test]
    fn test_commit_mined_block_stale_tip_restores_transactions() {
        let mut ledger = Ledger::new(1);
        let tx = inventory_tx("node-1");
        ledger.add_transaction(tx.clone()).unwrap();

        let (index, prev_hash, txns, difficulty) = ledger.prepare_mining().unwrap();
        let mut stale_block = Block::new(index, txns, prev_hash.clone());
        stale_block.mine(difficulty);

        // The tip advances to a different block while the one above is mining.
        ledger.add_transaction(inventory_tx("node-2")).unwrap();
        ledger.mine_pending_transactions().unwrap();

        assert!(!ledger.commit_mined_block(stale_block, &prev_hash).unwrap());
        assert_eq!(ledger.chain.len(), 2);
        // stale_block's transactions were restored to the pending pool.
        assert_eq!(ledger.pending_transactions, vec![tx]);
    }

    #[test]
    fn test_validate_chain_rejects_block_pow_failure() {
        let mut ledger = Ledger::new(1);
        ledger.add_transaction(inventory_tx("node-1")).unwrap();
        ledger.mine_pending_transactions().unwrap();

        // Re-validate under a stricter difficulty. Block 1 still chains to
        // genesis but no longer satisfies the PoW target, so the mid-chain
        // PoW branch must fire.
        let mut b1 = ledger.chain[1].clone();
        while b1.hash.starts_with("00") {
            b1.nonce = b1.nonce.wrapping_add(1);
            b1.hash = b1.calculate_hash();
        }
        ledger.chain[1] = b1;
        ledger.difficulty = 2;

        let err = ledger.validate_chain().unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidBlock(msg) if msg.contains("PoW difficulty 2")
        ));
    }

    #[test]
    fn test_validate_chain_rejects_invalid_genesis() {
        let mut ledger = Ledger::new(1);
        ledger.chain[0].hash = "tampered".into();
        let err = ledger.validate_chain().unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidBlock(msg) if msg.contains("genesis block hash is invalid")
        ));
    }

    #[test]
    fn test_validate_chain_rejects_genesis_pow_failure() {
        let mut ledger = Ledger::new(1);
        // Adjust genesis's nonce so its hash is valid but does not satisfy a
        // stricter difficulty-2 target.
        let mut genesis = ledger.chain[0].clone();
        while genesis.hash.starts_with("00") {
            genesis.nonce = genesis.nonce.wrapping_add(1);
            genesis.hash = genesis.calculate_hash();
        }
        ledger.chain[0] = genesis;
        ledger.difficulty = 2;

        let err = ledger.validate_chain().unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidBlock(msg) if msg.contains("genesis block does not satisfy PoW difficulty")
        ));
    }

    #[test]
    fn test_try_replace_chain_rejects_invalid_chain_link() {
        let mut ledger = Ledger::new(1);
        let genesis = ledger.chain[0].clone();

        // b1 carries a wrong previous_hash, so chains_to fails even though the
        // candidate is longer than the local chain.
        let mut b1 = Block::new(1, vec![], "not-the-genesis-hash".into());
        b1.mine(1);
        let mut b2 = Block::new(2, vec![], b1.hash.clone());
        b2.mine(1);

        assert!(!ledger.try_replace_chain(vec![genesis, b1, b2]));
    }

    #[test]
    fn test_try_replace_chain_rejects_pow_failure() {
        let mut ledger = Ledger::new(1);
        ledger.difficulty = 2;
        let genesis = ledger.chain[0].clone();

        // b1 chains correctly to genesis but its hash does not satisfy the
        // stricter difficulty-2 target.
        let mut b1 = Block::new(1, vec![], genesis.hash.clone());
        while b1.hash.starts_with("00") {
            b1.nonce = b1.nonce.wrapping_add(1);
            b1.hash = b1.calculate_hash();
        }
        let mut b2 = Block::new(2, vec![], b1.hash.clone());
        b2.mine(1);

        assert!(!ledger.try_replace_chain(vec![genesis, b1, b2]));
    }
}
