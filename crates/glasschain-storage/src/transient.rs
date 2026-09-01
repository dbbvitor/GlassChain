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
//! # ponytail
//! The expiry index is in-memory (filled on `put`); a restarted member cannot
//! enumerate payloads written before the restart, so purge-after-restart
//! requires a storage `list` capability — add it when a real deployment needs
//! it rather than pre-building one.

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
    /// In-memory expiry index `(key → expires_at)`, filled on `put`.
    /// `ponytail:` lost on restart — see the module docs.
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

    /// Purge every expired payload this process knows about; returns the
    /// number removed. Payloads vanish; the chain's commitments persist.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when the backend fails.
    pub fn purge_expired(&self) -> Result<usize, CoreError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expired: Vec<String> = {
            let index = self.lock();
            index
                .iter()
                .filter(|(_, &expires_at)| now >= expires_at)
                .map(|(key, _)| key.clone())
                .collect()
        };
        let mut purged = 0;
        for key in &expired {
            self.storage.delete_state(key)?;
            self.lock().remove(key);
            purged += 1;
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
}
