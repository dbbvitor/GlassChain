//! Channel abstraction — sub-ledgers for specific clients/suppliers.
//!
//! A [`Channel`] is a named sub-ledger that restricts which participants may
//! see or submit transactions.  Channels implement the "Private Data
//! Collections" concept from Hyperledger Fabric: the transaction hashes are
//! recorded on the main chain, while the full payloads are only distributed to
//! channel members.
//!
//! ## Architecture
//! ```text
//! Main Chain
//!   ├── Block N
//!   │     └── ChannelDataHash("pharma-channel", hash=0xABCD…)  ← on-chain proof
//!   └── Block N+1
//!
//! Pharma Channel (off-chain, member-only)
//!   └── Full transaction payloads (GTIN, batch, serial, …)
//! ```
//!
//! ## Usage
//! ```rust
//! use glasschain_identity::{Channel, ChannelConfig};
//!
//! let config = ChannelConfig {
//!     name: "pharma-channel".into(),
//!     member_ids: vec!["fabricante-abc".into(), "farmacia-sul".into()],
//!     description: "Private channel for Anvisa-regulated products".into(),
//! };
//! let mut channel = Channel::new(config);
//! assert!(channel.is_member("fabricante-abc"));
//! ```

use crate::error::IdentityError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use glasschain_core::crypto::sha256;

/// Configuration for creating a new [`Channel`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Unique channel name (e.g. `"pharma-anvisa-channel"`).
    pub name: String,
    /// Node/participant IDs that are members of this channel.
    pub member_ids: Vec<String>,
    /// Human-readable channel description.
    pub description: String,
}

/// A named sub-ledger (channel) that restricts data visibility to its members.
///
/// The main chain records only a SHA-256 hash of each channel transaction's
/// payload (the "Private Data Collection" pattern), while the full data is
/// shared off-chain only with channel members.
pub struct Channel {
    /// Channel configuration including member list.
    pub config: ChannelConfig,
    /// Hashed payloads committed to the main chain for this channel.
    committed_hashes: Vec<String>,
    /// Full payloads stored off-chain (in practice these would be encrypted
    /// and distributed only to channel members).
    private_data: Vec<PrivateDataEntry>,
    /// Fast membership lookup.
    member_set: HashSet<String>,
}

/// An off-chain private data entry with an on-chain hash commitment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateDataEntry {
    /// The channel this entry belongs to.
    pub channel_name: String,
    /// SHA-256 hash of `payload` (recorded on the main chain).
    pub payload_hash: String,
    /// Full transaction payload (shared only with channel members).
    pub payload: Vec<u8>,
    /// Node ID of the submitter.
    pub submitter_id: String,
}

impl Channel {
    /// Create a new channel from the given configuration.
    pub fn new(config: ChannelConfig) -> Self {
        let member_set = config.member_ids.iter().cloned().collect();
        Self {
            config,
            committed_hashes: Vec::new(),
            private_data: Vec::new(),
            member_set,
        }
    }

    /// Return `true` if `node_id` is a member of this channel.
    pub fn is_member(&self, node_id: &str) -> bool {
        self.member_set.contains(node_id)
    }

    /// Submit private data to the channel.
    ///
    /// Returns `Err(IdentityError::Channel)` if the submitter is not a member.
    /// On success, returns the SHA-256 hash of the payload (to be included in
    /// the main-chain transaction as a "hash commitment").
    pub fn submit_private_data(
        &mut self,
        submitter_id: &str,
        payload: Vec<u8>,
    ) -> Result<String, IdentityError> {
        if !self.is_member(submitter_id) {
            return Err(IdentityError::Channel(format!(
                "{submitter_id} is not a member of channel '{}'",
                self.config.name
            )));
        }
        let hash = sha256(&payload);
        self.committed_hashes.push(hash.clone());
        self.private_data.push(PrivateDataEntry {
            channel_name: self.config.name.clone(),
            payload_hash: hash.clone(),
            payload,
            submitter_id: submitter_id.to_owned(),
        });
        Ok(hash)
    }

    /// Retrieve private data by its payload hash.
    ///
    /// Only accessible to channel members; non-members receive `None`.
    pub fn get_private_data(
        &self,
        hash: &str,
        requestor_id: &str,
    ) -> Option<&PrivateDataEntry> {
        if !self.is_member(requestor_id) {
            return None;
        }
        self.private_data
            .iter()
            .find(|e| e.payload_hash == hash)
    }

    /// Return all on-chain hash commitments for this channel.
    pub fn committed_hashes(&self) -> &[String] {
        &self.committed_hashes
    }

    /// Add a new member to the channel.
    pub fn add_member(&mut self, node_id: impl Into<String>) {
        let nid = node_id.into();
        self.member_set.insert(nid.clone());
        if !self.config.member_ids.contains(&nid) {
            self.config.member_ids.push(nid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_channel() -> Channel {
        Channel::new(ChannelConfig {
            name: "pharma-channel".into(),
            member_ids: vec!["fabricante-abc".into(), "farmacia-sul".into()],
            description: "Test private channel".into(),
        })
    }

    #[test]
    fn test_channel_membership() {
        let ch = test_channel();
        assert!(ch.is_member("fabricante-abc"));
        assert!(ch.is_member("farmacia-sul"));
        assert!(!ch.is_member("outsider"));
    }

    #[test]
    fn test_submit_and_retrieve_private_data() {
        let mut ch = test_channel();
        let payload = b"sensitive GTIN data".to_vec();
        let hash = ch
            .submit_private_data("fabricante-abc", payload.clone())
            .unwrap();
        assert!(!hash.is_empty());

        // Member can retrieve.
        let entry = ch
            .get_private_data(&hash, "farmacia-sul")
            .expect("member should see data");
        assert_eq!(entry.payload, payload);
    }

    #[test]
    fn test_non_member_cannot_submit() {
        let mut ch = test_channel();
        let result = ch.submit_private_data("outsider", b"data".to_vec());
        assert!(result.is_err());
    }

    #[test]
    fn test_non_member_cannot_retrieve() {
        let mut ch = test_channel();
        let hash = ch
            .submit_private_data("fabricante-abc", b"secret".to_vec())
            .unwrap();
        assert!(ch.get_private_data(&hash, "outsider").is_none());
    }

    #[test]
    fn test_hash_commitment_recorded() {
        let mut ch = test_channel();
        let hash = ch
            .submit_private_data("fabricante-abc", b"payload".to_vec())
            .unwrap();
        assert!(ch.committed_hashes().contains(&hash));
    }

    #[test]
    fn test_add_member() {
        let mut ch = test_channel();
        ch.add_member("new-node");
        assert!(ch.is_member("new-node"));
    }
}
