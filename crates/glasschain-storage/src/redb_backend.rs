// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! redb-backed implementation of [`StorageProvider`].
//!
//! [`RedbStorageProvider`] stores everything in one redb database file inside
//! the directory passed to [`RedbStorageProvider::open`], in two tables:
//!
//! - `blocks` – serialised [`Block`] objects keyed by their block index.
//! - `state`  – arbitrary World State key-value pairs stored as raw bytes.
//!
//! redb is a pure-Rust, ACID, embedded key-value store with a stable file
//! format, MVCC readers, and crash-safe commits by default (wayfinder #173).

use glasschain_core::{Block, CoreError, StorageProvider};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

/// Serialised blocks keyed by block index.
const BLOCKS: TableDefinition<u64, &[u8]> = TableDefinition::new("blocks");
/// World-state key-value pairs.
const STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");

/// The database file created inside the directory passed to `open`.
const DB_FILE: &str = "glasschain.redb";

/// Persistent, redb-backed implementation of [`StorageProvider`].
///
/// Create a new instance with [`RedbStorageProvider::open`], passing a path to
/// a directory on disk. The directory (and the `glasschain.redb` file inside
/// it) is created if it does not exist.
///
/// # Example
/// ```no_run
/// use glasschain_storage::RedbStorageProvider;
/// use glasschain_core::StorageProvider;
///
/// let store = RedbStorageProvider::open("/var/lib/glasschain/state").unwrap();
/// store.put_state("world_state_key", b"value").unwrap();
/// ```
pub struct RedbStorageProvider {
    db: Database,
}

fn storage_err(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(error.to_string())
}

impl RedbStorageProvider {
    /// Open (or create) a redb database in `path/glasschain.redb`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Storage`] if the directory or database cannot be
    /// created, or the tables cannot be opened.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, CoreError> {
        let dir = path.as_ref();
        std::fs::create_dir_all(dir).map_err(storage_err)?;
        let db = Database::create(dir.join(DB_FILE)).map_err(storage_err)?;
        // A write transaction creates missing tables; commit once so later
        // read transactions can open both tables.
        let txn = db.begin_write().map_err(storage_err)?;
        {
            txn.open_table(BLOCKS).map_err(storage_err)?;
            txn.open_table(STATE).map_err(storage_err)?;
        }
        txn.commit().map_err(storage_err)?;
        Ok(Self { db })
    }

    /// Flush pending writes to disk synchronously.
    ///
    /// redb commits are crash-safe by default (the default durability fsyncs
    /// on commit), so there is nothing further to flush. The method remains
    /// for API parity with the durability contract in ADR-016.
    ///
    /// # Errors
    ///
    /// Never fails today; kept fallible so callers keep their error handling.
    #[allow(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "kept as an inherent `&self -> Result` method for parity with the previous backend's flush contract"
    )]
    pub const fn flush(&self) -> Result<(), CoreError> {
        Ok(())
    }

    /// Read the current tip block inside an open read or write transaction.
    fn tip_in_txn(
        blocks: &impl ReadableTable<u64, &'static [u8]>,
    ) -> Result<Option<Block>, CoreError> {
        match blocks.last().map_err(storage_err)? {
            Some((key, value)) => {
                let block: Block = serde_json::from_slice(value.value())?;
                debug_assert_eq!(block.index, key.value());
                Ok(Some(block))
            }
            None => Ok(None),
        }
    }
}

impl StorageProvider for RedbStorageProvider {
    fn put_block(&self, block: &Block) -> Result<(), CoreError> {
        let value = serde_json::to_vec(block)?;
        let txn = self.db.begin_write().map_err(storage_err)?;
        {
            let mut table = txn.open_table(BLOCKS).map_err(storage_err)?;
            table
                .insert(block.index, value.as_slice())
                .map_err(storage_err)?;
        }
        txn.commit().map_err(storage_err)?;
        log::debug!("RedbStorage: persisted block {}", block.index);
        Ok(())
    }

    fn apply_block(&self, block: &Block) -> Result<(), CoreError> {
        // One atomic block-plus-state boundary (ADR-007 decision 2): tip
        // check, block insert, and write-set application run inside a single
        // redb write transaction, so a stale candidate is rejected whole and a
        // partial write set can never be acknowledged. redb serialises write
        // transactions (single writer), so reading the tip inside the
        // transaction is race-free; an uncommitted transaction rolls back on
        // drop.
        let value = serde_json::to_vec(block)?;
        let txn = self.db.begin_write().map_err(storage_err)?;
        {
            let mut blocks = txn.open_table(BLOCKS).map_err(storage_err)?;
            let tip = Self::tip_in_txn(&blocks)?;
            glasschain_core::validate_tip_chain(block, tip.as_ref())
                .map_err(|e| CoreError::InvalidBlock(e.to_string()))?;

            let mut state = txn.open_table(STATE).map_err(storage_err)?;
            for write in &block.write_set {
                let key = write.state_key();
                match &write.op {
                    glasschain_core::WriteOp::Set(value) => {
                        state
                            .insert(key.as_str(), value.as_slice())
                            .map_err(storage_err)?;
                    }
                    glasschain_core::WriteOp::Delete => {
                        state.remove(key.as_str()).map_err(storage_err)?;
                    }
                }
            }
            blocks
                .insert(block.index, value.as_slice())
                .map_err(storage_err)?;
        }
        txn.commit().map_err(storage_err)?;
        log::debug!("RedbStorage: applied block {}", block.index);
        Ok(())
    }

    fn get_block(&self, index: u64) -> Result<Option<Block>, CoreError> {
        let txn = self.db.begin_read().map_err(storage_err)?;
        let table = txn.open_table(BLOCKS).map_err(storage_err)?;
        match table.get(index).map_err(storage_err)? {
            Some(value) => {
                let block: Block = serde_json::from_slice(value.value())?;
                Ok(Some(block))
            }
            None => Ok(None),
        }
    }

    fn latest_block_index(&self) -> Result<Option<u64>, CoreError> {
        let txn = self.db.begin_read().map_err(storage_err)?;
        let table = txn.open_table(BLOCKS).map_err(storage_err)?;
        let latest = table
            .last()
            .map_err(storage_err)?
            .map(|(key, _)| key.value());
        Ok(latest)
    }

    fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError> {
        let txn = self.db.begin_write().map_err(storage_err)?;
        {
            let mut table = txn.open_table(STATE).map_err(storage_err)?;
            table.insert(key, value).map_err(storage_err)?;
        }
        txn.commit().map_err(storage_err)?;
        Ok(())
    }

    fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
        let txn = self.db.begin_read().map_err(storage_err)?;
        let table = txn.open_table(STATE).map_err(storage_err)?;
        let value = table
            .get(key)
            .map_err(storage_err)?
            .map(|value| value.value().to_vec());
        Ok(value)
    }

    fn delete_state(&self, key: &str) -> Result<(), CoreError> {
        let txn = self.db.begin_write().map_err(storage_err)?;
        {
            let mut table = txn.open_table(STATE).map_err(storage_err)?;
            table.remove(key).map_err(storage_err)?;
        }
        txn.commit().map_err(storage_err)?;
        Ok(())
    }

    fn list_state_keys(&self, prefix: &str) -> Result<Vec<String>, CoreError> {
        let txn = self.db.begin_read().map_err(storage_err)?;
        let table = txn.open_table(STATE).map_err(storage_err)?;
        let mut keys = Vec::new();
        for entry in table.range(prefix..).map_err(storage_err)? {
            let (key, _) = entry.map_err(storage_err)?;
            let key = key.value();
            if !key.starts_with(prefix) {
                break;
            }
            keys.push(key.to_owned());
        }
        Ok(keys)
    }

    fn name(&self) -> &'static str {
        "redb"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::Transaction;

    fn open_temp() -> (RedbStorageProvider, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = RedbStorageProvider::open(dir.path()).expect("open");
        (store, dir)
    }

    fn genesis() -> Block {
        let mut b = Block::new(0, vec![], "0".into());
        b.mine(1);
        b
    }

    fn block1(genesis: &Block) -> Block {
        let mut b = Block::new(1, vec![], genesis.hash.clone());
        b.mine(1);
        b
    }

    #[test]
    fn test_put_and_get_block() {
        let (store, _dir) = open_temp();
        let g = genesis();
        store.put_block(&g).unwrap();
        let retrieved = store.get_block(0).unwrap().unwrap();
        assert_eq!(retrieved.hash, g.hash);
        assert_eq!(retrieved.index, 0);
    }

    #[test]
    fn test_apply_block_persists_and_applies_writes() {
        use glasschain_core::{PersistentWrite, WriteOp, WriteVisibility};
        let (store, _dir) = open_temp();
        let g = genesis();
        store.apply_block(&g).unwrap();

        let writes = vec![
            PersistentWrite {
                channel: "ch".into(),
                contract: "contract".into(),
                key: "a".into(),
                op: WriteOp::Set(b"1".to_vec()),
                visibility: WriteVisibility::Public,
            },
            PersistentWrite {
                op: WriteOp::Delete,
                channel: "ch".into(),
                contract: "contract".into(),
                key: "b".into(),
                visibility: WriteVisibility::Public,
            },
        ];
        let mut b = Block::with_write_set(1, vec![], g.hash, writes);
        b.mine(1);
        store.apply_block(&b).unwrap();

        assert_eq!(store.get_block(1).unwrap().unwrap().hash, b.hash);
        assert_eq!(
            store.get_state("ws:ch:contract:a").unwrap(),
            Some(b"1".to_vec())
        );
        assert!(store.get_state("ws:ch:contract:b").unwrap().is_none());
    }

    #[test]
    fn test_apply_block_rejects_stale_tip() {
        use glasschain_core::{PersistentWrite, WriteOp, WriteVisibility};
        let (store, _dir) = open_temp();
        let g = genesis();
        store.apply_block(&g).unwrap();

        let mut stale = Block::with_write_set(
            1,
            vec![],
            "not-the-tip".into(),
            vec![PersistentWrite {
                channel: "ch".into(),
                contract: "contract".into(),
                key: "k".into(),
                op: WriteOp::Set(b"v".to_vec()),
                visibility: WriteVisibility::Public,
            }],
        );
        stale.mine(1);
        assert!(matches!(
            store.apply_block(&stale),
            Err(CoreError::InvalidBlock(_))
        ));
        assert!(store.get_block(1).unwrap().is_none());
        assert!(store.get_state("ws:ch:contract:k").unwrap().is_none());
    }

    #[test]
    fn test_get_missing_block_returns_none() {
        let (store, _dir) = open_temp();
        assert!(store.get_block(99).unwrap().is_none());
    }

    #[test]
    fn test_latest_block_index_empty() {
        let (store, _dir) = open_temp();
        assert!(store.latest_block_index().unwrap().is_none());
    }

    #[test]
    fn test_latest_block_index_after_puts() {
        let (store, _dir) = open_temp();
        let g = genesis();
        let b1 = block1(&g);
        store.put_block(&g).unwrap();
        store.put_block(&b1).unwrap();
        assert_eq!(store.latest_block_index().unwrap(), Some(1));
    }

    #[test]
    fn test_state_put_get_delete() {
        let (store, _dir) = open_temp();
        store.put_state("inventory:SKU-001", b"500").unwrap();
        assert_eq!(
            store.get_state("inventory:SKU-001").unwrap(),
            Some(b"500".to_vec())
        );
        store.delete_state("inventory:SKU-001").unwrap();
        assert!(store.get_state("inventory:SKU-001").unwrap().is_none());
    }

    #[test]
    fn test_state_overwrite() {
        let (store, _dir) = open_temp();
        store.put_state("k", b"v1").unwrap();
        store.put_state("k", b"v2").unwrap();
        assert_eq!(store.get_state("k").unwrap(), Some(b"v2".to_vec()));
    }

    #[test]
    fn test_list_state_keys_prefix_and_order() {
        let (store, _dir) = open_temp();
        store.put_state("transient:b:z", b"1").unwrap();
        store.put_state("transient:a:m", b"1").unwrap();
        store.put_state("workflow:checkpoint:f1", b"1").unwrap();
        store.put_state("other", b"1").unwrap();

        assert_eq!(
            store.list_state_keys("transient:").unwrap(),
            vec!["transient:a:m".to_owned(), "transient:b:z".to_owned()],
            "only matching keys, byte-order sorted"
        );
        assert_eq!(
            store.list_state_keys("workflow:checkpoint:").unwrap(),
            vec!["workflow:checkpoint:f1".to_owned()]
        );
        assert!(store.list_state_keys("missing:").unwrap().is_empty());
    }

    #[test]
    fn test_flush_succeeds() {
        let (store, _dir) = open_temp();
        store.put_state("flush-test", b"ok").unwrap();
        store.flush().unwrap();
    }

    #[test]
    fn test_committed_blocks_survive_provider_reopen() {
        // ADR-016 regression: a block accepted by `apply_block` is discoverable
        // after another provider instance is opened over the same directory
        // (the process-restart half of the durability promise; power loss is
        // covered by quorum replication plus redb's crash-safe commits).
        // Reopening first (`drop` then reopen) matters: in-memory copies must
        // not mask what actually persisted.
        let (store, dir) = open_temp();
        let g = genesis();
        store.apply_block(&g).unwrap();

        let dir = dir.keep();
        drop(store);
        let reopened = RedbStorageProvider::open(&dir).expect("reopen");
        assert_eq!(reopened.latest_block_index().unwrap(), Some(0));
        assert_eq!(reopened.get_block(0).unwrap().unwrap().hash, g.hash);
        assert!(reopened.get_state("no-such-key").unwrap().is_none());
        reopened.flush().unwrap();
        drop(reopened);
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_provider_name() {
        let (store, _dir) = open_temp();
        assert_eq!(store.name(), "redb");
    }

    #[test]
    fn test_block_serialization_roundtrip() {
        use glasschain_core::{InventoryUpdate, TransactionKind};

        let (store, _dir) = open_temp();
        let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-001".into(),
            owner_id: "node-1".into(),
            quantity_delta: 100,
            reason: "initial stock".into(),
        }));
        let mut b = Block::new(0, vec![tx], "0".into());
        b.mine(1);
        store.put_block(&b).unwrap();
        let retrieved = store.get_block(0).unwrap().unwrap();
        assert_eq!(retrieved.transactions.len(), 1);
        assert!(retrieved.is_valid());
    }
}
