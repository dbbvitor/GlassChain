//! Transient pre-commit store for private data collections (ADR-003,
//! tickets #46/#47).
//!
//! A [`TransientStore`] holds private payloads **before and after commit** on
//! collection members only — the analogue of Fabric's `core/transientstore/`.
//! Entries are keyed by `(collection, commitment)` where the commitment is the
//! SHA-256 of the payload; the globally replicated chain carries exactly that
//! commitment (see [`glasschain_core::PersistentWrite::block_form`]), so a
//! member can always correlate a received payload with its committed write.
//!
//! Entries carry the collection's retention window (ADR-003 decision 4):
//! [`TransientStore::purge_expired`] removes expired payloads — they vanish,
//! while the chain's hash commitments persist forever (a late auditor can
//! prove existence and consistency but not read contents).
//!
//! The store is deliberately dumb key-value storage over the existing
//! [`StorageProvider`] seam — membership gating lives at the node boundary.
//!
//! # D5
//! [`TransientStore::purge_expired`] discovers expired payloads through
//! [`StorageProvider::list_state_keys`], so a restarted member purges
//! payloads written before the restart without a prior read; the in-memory
//! index remains a fast path, not the retention guarantee.

use glasschain_core::{CoreError, StorageProvider};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

/// The transient state-key prefix for private payloads.
pub const TRANSIENT_PREFIX: &str = "transient";

/// The wire/store envelope for one private payload: the bytes plus the
/// retention deadline (Unix seconds).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PayloadEnvelope {
    /// The private payload bytes.
    payload: Vec<u8>,
    /// Unix seconds after which the payload is a purge candidate.
    expires_at: u64,
}

/// The `StorageProvider` key for one transient payload.
fn transient_key(collection: &str, commitment: &str) -> String {
    format!("{TRANSIENT_PREFIX}:{collection}:{commitment}")
}

/// Member-side transient store for private payloads, keyed by
/// `(collection, sha256(payload))`, with per-entry retention deadlines.
#[derive(Clone)]
pub struct TransientStore {
    storage: Arc<dyn StorageProvider>,
    /// Fast-path expiry index `(key → expires_at)`, filled on `put`/`get`.
    /// Purge no longer depends on it: storage enumeration is the durable
    /// discovery path (D5).
    expiry_index: Arc<Mutex<HashMap<String, u64>>>,
}

impl TransientStore {
    /// Build a transient store over `storage`.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>) -> Self {
        Self {
            storage,
            expiry_index: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Store `payload` under `(collection, commitment)` with the retention
    /// deadline `retention_secs` from now (Unix seconds).
    ///
    /// The caller is responsible for the membership gate (node boundary) and
    /// for `commitment == sha256(payload)` integrity.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when the backend fails.
    pub fn put(
        &self,
        collection: &str,
        commitment: &str,
        payload: &[u8],
        retention_secs: u64,
    ) -> Result<(), CoreError> {
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + retention_secs;
        let envelope = serde_json::to_vec(&PayloadEnvelope {
            payload: payload.to_vec(),
            expires_at,
        })?;
        let key = transient_key(collection, commitment);
        self.storage.put_state(&key, &envelope)?;
        self.record_expiry(&key, expires_at);
        Ok(())
    }

    /// Retrieve a payload by `(collection, commitment)`, if held and not yet
    /// purged. Expired-but-not-yet-purged entries are NOT returned (retention
    /// is a read boundary, not just a background sweep).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when the backend fails.
    pub fn get(&self, collection: &str, commitment: &str) -> Result<Option<Vec<u8>>, CoreError> {
        let key = transient_key(collection, commitment);
        let Some(raw) = self.storage.get_state(&key)? else {
            return Ok(None);
        };
        let envelope: PayloadEnvelope = serde_json::from_slice(&raw)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now >= envelope.expires_at {
            return Ok(None);
        }
        self.record_expiry(&key, envelope.expires_at);
        Ok(Some(envelope.payload))
    }

    /// Purge every expired payload, discovering them from **storage** rather
    /// than only the in-memory index — a restarted member can enumerate and
    /// purge payloads written before the restart (D5).
    ///
    /// A per-key delete failure is logged and leaves that key in place, so the
    /// next sweep retries it; the sweep continues with the remaining keys
    /// instead of aborting on the first failure. Returns the number removed.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when enumeration or reading an envelope fails.
    pub fn purge_expired(&self) -> Result<usize, CoreError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let known: HashMap<String, u64> = self.lock().clone();
        let mut expired: Vec<String> = known
            .iter()
            .filter(|(_, &expires_at)| now >= expires_at)
            .map(|(key, _)| key.clone())
            .collect();
        // Durable discovery: enumerate every persisted payload; entries the
        // index does not know (written before a restart) have their deadline
        // read from the stored envelope without a prior `get`.
        let prefix = format!("{TRANSIENT_PREFIX}:");
        for key in self.storage.list_state_keys(&prefix)? {
            if known.contains_key(&key) {
                continue;
            }
            let Some(raw) = self.storage.get_state(&key)? else {
                continue;
            };
            match serde_json::from_slice::<PayloadEnvelope>(&raw) {
                Ok(envelope) if now >= envelope.expires_at => expired.push(key),
                Ok(_) => {}
                Err(e) => log::warn!("transient: unreadable envelope at {key}: {e}"),
            }
        }
        let mut purged = 0;
        for key in &expired {
            match self.storage.delete_state(key) {
                Ok(()) => {
                    self.lock().remove(key);
                    purged += 1;
                }
                Err(e) => {
                    log::warn!("transient: failed to purge {key}: {e}");
                }
            }
        }
        Ok(purged)
    }

    /// Record (or refresh) an entry's expiry in the in-memory index.
    fn record_expiry(&self, key: &str, expires_at: u64) {
        self.lock().insert(key.to_owned(), expires_at);
    }

    /// Lock the expiry index, recovering from a poisoned mutex (matching the
    /// triage view's poison-recovery form).
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.expiry_index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::providers::in_memory::InMemoryStorageProvider;

    #[test]
    fn test_put_get_delete_roundtrip() {
        let store = TransientStore::new(Arc::new(InMemoryStorageProvider::new()));
        let payload = b"price: 1500".to_vec();
        let commitment = glasschain_core::crypto::sha256(&payload);

        store.put("pricing", &commitment, &payload, 3600).unwrap();
        assert_eq!(store.get("pricing", &commitment).unwrap(), Some(payload));

        // The chain-side commitment survives independently of the store.
        assert_eq!(commitment.len(), 64, "commitment is the 64-hex sha256");
    }

    #[test]
    fn test_collections_are_namespaced() {
        let store = TransientStore::new(Arc::new(InMemoryStorageProvider::new()));
        let payload = b"payload".to_vec();
        let commitment = glasschain_core::crypto::sha256(&payload);

        store
            .put("collection-a", &commitment, &payload, 3600)
            .unwrap();
        assert_eq!(store.get("collection-b", &commitment).unwrap(), None);
        assert_eq!(
            store.get("collection-a", &commitment).unwrap(),
            Some(payload)
        );
    }

    #[test]
    fn test_retention_expiry_is_enforced_on_read_and_purge() {
        let store = TransientStore::new(Arc::new(InMemoryStorageProvider::new()));
        let payload = b"expiring".to_vec();
        let commitment = glasschain_core::crypto::sha256(&payload);

        // Zero retention: the entry is already expired.
        store.put("pricing", &commitment, &payload, 0).unwrap();
        assert_eq!(
            store.get("pricing", &commitment).unwrap(),
            None,
            "an expired payload is not readable"
        );

        // Purge removes the expired entry (the write happened, so the index
        // knows the deadline).
        assert_eq!(store.purge_expired().unwrap(), 1, "expired entry purged");
        assert_eq!(store.get("pricing", &commitment).unwrap(), None);

        // A live entry survives the purge.
        store.put("pricing", &commitment, &payload, 3600).unwrap();
        assert_eq!(store.purge_expired().unwrap(), 0);
        assert_eq!(store.get("pricing", &commitment).unwrap(), Some(payload));
    }

    /// D5: a restarted member purges payloads persisted **before** the
    /// restart, discovering them from storage without a prior read; live
    /// payloads survive and the underlying key is deleted.
    #[test]
    fn test_restart_purge_discovers_persisted_payloads_without_reading_them() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let store = TransientStore::new(Arc::clone(&storage));
        let expired_payload = b"expired-before-restart".to_vec();
        let expired_commitment = glasschain_core::crypto::sha256(&expired_payload);
        let live_payload = b"live-across-restart".to_vec();
        let live_commitment = glasschain_core::crypto::sha256(&live_payload);
        store
            .put("pricing", &expired_commitment, &expired_payload, 0)
            .unwrap();
        store
            .put("pricing", &live_commitment, &live_payload, 3600)
            .unwrap();

        // Restart: a fresh store with an empty index over the same storage.
        let restarted = TransientStore::new(Arc::clone(&storage));
        assert_eq!(
            restarted.purge_expired().unwrap(),
            1,
            "the pre-restart expired payload is discovered and purged"
        );
        assert!(
            storage
                .get_state(&transient_key("pricing", &expired_commitment))
                .unwrap()
                .is_none(),
            "the underlying key is deleted without a prior read"
        );
        assert!(
            storage
                .get_state(&transient_key("pricing", &live_commitment))
                .unwrap()
                .is_some(),
            "a live payload survives the sweep"
        );
        assert_eq!(
            restarted.get("pricing", &live_commitment).unwrap(),
            Some(live_payload)
        );
    }

    /// Storage wrapper whose next `delete_state` fails once — an interrupted
    /// sweep.
    struct FlakyDelete {
        inner: Arc<dyn StorageProvider>,
        fail_next_delete: std::sync::atomic::AtomicBool,
    }

    impl StorageProvider for FlakyDelete {
        fn put_block(&self, block: &glasschain_core::Block) -> Result<(), CoreError> {
            self.inner.put_block(block)
        }
        fn get_block(&self, index: u64) -> Result<Option<glasschain_core::Block>, CoreError> {
            self.inner.get_block(index)
        }
        fn latest_block_index(&self) -> Result<Option<u64>, CoreError> {
            self.inner.latest_block_index()
        }
        fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError> {
            self.inner.put_state(key, value)
        }
        fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
            self.inner.get_state(key)
        }
        fn delete_state(&self, key: &str) -> Result<(), CoreError> {
            if self
                .fail_next_delete
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(CoreError::Storage("simulated interrupted delete".into()));
            }
            self.inner.delete_state(key)
        }
        fn list_state_keys(&self, prefix: &str) -> Result<Vec<String>, CoreError> {
            self.inner.list_state_keys(prefix)
        }
        fn name(&self) -> &'static str {
            "flaky-delete"
        }
    }

    #[test]
    fn test_interrupted_delete_is_retried_on_the_next_sweep() {
        let inner: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let flaky: Arc<dyn StorageProvider> = Arc::new(FlakyDelete {
            inner: Arc::clone(&inner),
            fail_next_delete: std::sync::atomic::AtomicBool::new(true),
        });
        let store = TransientStore::new(Arc::clone(&flaky));
        let payload = b"retry-me".to_vec();
        let commitment = glasschain_core::crypto::sha256(&payload);
        store.put("pricing", &commitment, &payload, 0).unwrap();

        // First sweep: the delete fails, nothing is counted as purged and the
        // key is still there.
        assert_eq!(store.purge_expired().unwrap(), 0);
        assert!(inner
            .get_state(&transient_key("pricing", &commitment))
            .unwrap()
            .is_some());

        // Second sweep: the retry succeeds and the key is gone.
        assert_eq!(store.purge_expired().unwrap(), 1);
        assert!(inner
            .get_state(&transient_key("pricing", &commitment))
            .unwrap()
            .is_none());
    }

    /// D5 over the persistent backend: the payload is written, the database
    /// is **reopened**, the expired payload is purged (discovered by scan,
    /// not by a prior read) and the underlying sled key is gone.
    #[test]
    fn test_restart_purge_over_sled_backend() {
        let dir = tempfile::tempdir().expect("temp dir");
        let payload = b"sled-expired-before-restart".to_vec();
        let commitment = glasschain_core::crypto::sha256(&payload);
        {
            let storage: Arc<dyn StorageProvider> =
                Arc::new(crate::SledStorageProvider::open(dir.path()).expect("open"));
            let store = TransientStore::new(Arc::clone(&storage));
            store.put("pricing", &commitment, &payload, 0).unwrap();
        }
        // Reopen: a restarted member has an empty in-memory index.
        let storage: Arc<dyn StorageProvider> =
            Arc::new(crate::SledStorageProvider::open(dir.path()).expect("reopen"));
        let restarted = TransientStore::new(Arc::clone(&storage));
        assert_eq!(
            restarted.purge_expired().unwrap(),
            1,
            "the reopened store discovers and purges the expired payload"
        );
        assert!(
            storage
                .get_state(&transient_key("pricing", &commitment))
                .unwrap()
                .is_none(),
            "the underlying sled key is deleted"
        );
    }
}
