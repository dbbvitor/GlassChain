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
        match self
            .state
            .get(key.as_bytes())
            .map_err(|e| CoreError::Storage(e.to_string()))?
        {
            Some(bytes) => Ok(Some(bytes.to_vec())),
            None => Ok(None),
        }
    }

    fn delete_state(&self, key: &str) -> Result<(), CoreError> {
        self.state
            .remove(key.as_bytes())
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(())
    }

    fn name(&self) -> &str {
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
