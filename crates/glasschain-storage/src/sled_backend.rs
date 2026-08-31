//! Sled-backed implementation of [`StorageProvider`].
//!
//! [`SledStorageProvider`] uses two separate sled trees:
//!
//! - `blocks` – serialised [`Block`] objects keyed by their 8-byte big-endian
//!   block index.
//! - `state`  – arbitrary World State key-value pairs stored as raw bytes.
//!
//! Sled is a pure-Rust, high-performance embedded database that requires no
//! external C dependencies and is optimised for solid-state storage.

use glasschain_core::{Block, CoreError, StorageProvider};
use sled::Transactional;

/// Persistent, sled-backed implementation of [`StorageProvider`].
///
/// Create a new instance with [`SledStorageProvider::open`], passing a path
/// to a directory on disk.  The directory will be created if it does not
/// exist.
///
/// # Example
/// ```no_run
/// use glasschain_storage::SledStorageProvider;
/// use glasschain_core::StorageProvider;
///
/// let store = SledStorageProvider::open("/var/lib/glasschain/state").unwrap();
/// store.put_state("world_state_key", b"value").unwrap();
/// ```
pub struct SledStorageProvider {
    blocks: sled::Tree,
    state: sled::Tree,
    /// Keep a reference to the parent DB so it is not dropped prematurely.
    _db: sled::Db,
}

impl SledStorageProvider {
    /// Open (or create) a sled database at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Storage`] if the database cannot be opened or the
    /// internal trees cannot be created.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, CoreError> {
        let db = sled::open(path).map_err(|e| CoreError::Storage(e.to_string()))?;
        let blocks = db
            .open_tree("blocks")
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let state = db
            .open_tree("state")
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(Self {
            blocks,
            state,
            _db: db,
        })
    }

    /// Flush all pending writes to disk synchronously.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Storage`] if the underlying sled flush fails.
    pub fn flush(&self) -> Result<(), CoreError> {
        self.blocks
            .flush()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        self.state
            .flush()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(())
    }
}

impl StorageProvider for SledStorageProvider {
    fn put_block(&self, block: &Block) -> Result<(), CoreError> {
        let key = block.index.to_be_bytes();
        let value = serde_json::to_vec(block)?;
        self.blocks
            .insert(key, value)
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        log::debug!("SledStorage: persisted block {}", block.index);
        Ok(())
    }

    fn apply_block(&self, block: &Block) -> Result<(), CoreError> {
        // One atomic block-plus-state boundary (ADR-007 decision 2): tip
        // check, block insert, and write-set application run inside a single
        // sled multi-tree transaction, so a stale candidate is rejected whole
        // and a partial write set can never be acknowledged.
        let block_key = block.index.to_be_bytes();
        let block_value = serde_json::to_vec(block)?;

        // Read the current tip outside the transaction; inside, we verify the
        // tip key still holds the same bytes (sled's conflict detection
        // retries or aborts if it changed concurrently) and chain-check the
        // candidate against it.
        let tip: Option<(u64, Block)> = match self.latest_block_index()? {
            Some(tip_index) => Some((
                tip_index,
                self.get_block(tip_index)?.ok_or_else(|| {
                    CoreError::Storage(format!("block {tip_index} missing from store"))
                })?,
            )),
            None => None,
        };
        let tip_bytes = tip
            .as_ref()
            .map(|(_, tip_block)| serde_json::to_vec(tip_block))
            .transpose()?;

        // Abort payload is `CoreError` so a stale candidate surfaces as
        // `InvalidBlock` (matching every other backend) while real sled
        // failures stay `Storage`.
        let abort =
            |message: String| sled::transaction::ConflictableTransactionError::Abort(message);
        (&self.blocks, &self.state)
            .transaction(|(tx_blocks, tx_state)| {
                match (&tip, &tip_bytes) {
                    (None, None) => {
                        glasschain_core::validate_tip_chain(block, None)
                            .map_err(|e| abort(e.to_string()))?;
                    }
                    (Some((tip_index, _)), Some(bytes)) => {
                        match tx_blocks.get(tip_index.to_be_bytes().as_slice())? {
                            None => {
                                return Err(abort("tip block disappeared from store".to_owned()));
                            }
                            Some(stored) => {
                                if stored.as_ref() != bytes.as_slice() {
                                    return Err(abort(
                                        "stale tip: tip changed during apply".to_owned(),
                                    ));
                                }
                            }
                        }
                        glasschain_core::validate_tip_chain(block, tip.as_ref().map(|(_, t)| t))
                            .map_err(|e| abort(e.to_string()))?;
                    }
                    _ => {
                        return Err(abort("tip state inconsistent".to_owned()));
                    }
                }
                tx_blocks.insert(block_key.as_slice(), block_value.as_slice())?;
                for write in &block.write_set {
                    match &write.op {
                        glasschain_core::WriteOp::Set(value) => {
                            tx_state.insert(write.state_key().as_bytes(), value.as_slice())?;
                        }
                        glasschain_core::WriteOp::Delete => {
                            tx_state.remove(write.state_key().as_bytes())?;
                        }
                    }
                }
                Ok(())
            })
            .map_err(|e| match e {
                sled::transaction::TransactionError::Abort(message) => {
                    CoreError::InvalidBlock(message)
                }
                sled::transaction::TransactionError::Storage(error) => {
                    CoreError::Storage(error.to_string())
                }
            })?;
        log::debug!("SledStorage: applied block {}", block.index);
        Ok(())
    }

    fn get_block(&self, index: u64) -> Result<Option<Block>, CoreError> {
        let key = index.to_be_bytes();
        match self
            .blocks
            .get(key)
            .map_err(|e| CoreError::Storage(e.to_string()))?
        {
            Some(bytes) => {
                let block: Block = serde_json::from_slice(&bytes)?;
                Ok(Some(block))
            }
            None => Ok(None),
        }
    }

    fn latest_block_index(&self) -> Result<Option<u64>, CoreError> {
        match self
            .blocks
            .last()
            .map_err(|e| CoreError::Storage(e.to_string()))?
        {
            Some((key, _)) => {
                let arr: [u8; 8] = key
                    .as_ref()
                    .try_into()
                    .map_err(|_| CoreError::Storage("corrupt block key".into()))?;
                Ok(Some(u64::from_be_bytes(arr)))
            }
            None => Ok(None),
        }
    }

    fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError> {
        self.state
            .insert(key.as_bytes(), value)
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(())
    }

    fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
        Ok(self
            .state
            .get(key.as_bytes())
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .map(|bytes| bytes.to_vec()))
    }

    fn delete_state(&self, key: &str) -> Result<(), CoreError> {
        self.state
            .remove(key.as_bytes())
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "sled"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::Transaction;

    fn open_temp() -> (SledStorageProvider, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = SledStorageProvider::open(dir.path()).expect("open");
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
    fn test_flush_succeeds() {
        let (store, _dir) = open_temp();
        store.put_state("flush-test", b"ok").unwrap();
        store.flush().unwrap();
    }

    #[test]
    fn test_provider_name() {
        let (store, _dir) = open_temp();
        assert_eq!(store.name(), "sled");
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
