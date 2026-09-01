//! Transient pre-commit store for private data collections (ADR-003,
//! ticket #46).
//!
//! A [`TransientStore`] holds private payloads **before and after commit** on
//! collection members only — the analogue of Fabric's `core/transientstore/`.
//! Entries are keyed by `(collection, commitment)` where the commitment is the
//! SHA-256 of the payload; the globally replicated chain carries exactly that
//! commitment (see [`glasschain_core::PersistentWrite::block_form`]), so a
//! member can always correlate a received payload with its committed write.
//!
//! The store is deliberately dumb key-value storage over the existing
//! [`StorageProvider`] seam — membership gating lives at the node boundary,
//! and retention/purge windows are ticket #47. A `delete` exists as the purge
//! hook: purged payloads disappear while the chain's commitments persist
//! forever.

use glasschain_core::{CoreError, StorageProvider};
use std::sync::Arc;

/// The transient state-key prefix for private payloads.
pub const TRANSIENT_PREFIX: &str = "transient";

/// The `StorageProvider` key for one transient payload.
fn transient_key(collection: &str, commitment: &str) -> String {
    format!("{TRANSIENT_PREFIX}:{collection}:{commitment}")
}

/// Member-side transient store for private payloads, keyed by
/// `(collection, sha256(payload))`.
#[derive(Clone)]
pub struct TransientStore {
    storage: Arc<dyn StorageProvider>,
}

impl TransientStore {
    /// Build a transient store over `storage`.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>) -> Self {
        Self { storage }
    }

    /// Store `payload` under `(collection, commitment)`.
    ///
    /// The caller is responsible for the membership gate (node boundary) and
    /// for `commitment == sha256(payload)` integrity.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when the backend fails.
    pub fn put(&self, collection: &str, commitment: &str, payload: &[u8]) -> Result<(), CoreError> {
        self.storage
            .put_state(&transient_key(collection, commitment), payload)
    }

    /// Retrieve a payload by `(collection, commitment)`, if held.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when the backend fails.
    pub fn get(&self, collection: &str, commitment: &str) -> Result<Option<Vec<u8>>, CoreError> {
        self.storage
            .get_state(&transient_key(collection, commitment))
    }

    /// Purge a payload (retention hook, ticket #47): the payload disappears;
    /// the chain's commitment persists forever.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when the backend fails.
    pub fn delete(&self, collection: &str, commitment: &str) -> Result<(), CoreError> {
        self.storage
            .delete_state(&transient_key(collection, commitment))
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

        store.put("pricing", &commitment, &payload).unwrap();
        assert_eq!(store.get("pricing", &commitment).unwrap(), Some(payload));

        // Purge removes the payload; the (chain) commitment is unaffected.
        store.delete("pricing", &commitment).unwrap();
        assert_eq!(store.get("pricing", &commitment).unwrap(), None);
    }

    #[test]
    fn test_collections_are_namespaced() {
        let store = TransientStore::new(Arc::new(InMemoryStorageProvider::new()));
        let payload = b"payload".to_vec();
        let commitment = glasschain_core::crypto::sha256(&payload);

        store.put("collection-a", &commitment, &payload).unwrap();
        assert_eq!(store.get("collection-b", &commitment).unwrap(), None);
        assert_eq!(
            store.get("collection-a", &commitment).unwrap(),
            Some(payload)
        );
    }
}
