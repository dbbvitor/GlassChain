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
//!     endorsement_policy: None,
//!     retention_secs: 72 * 60 * 60,
//! };
//! let mut channel = Channel::new(config);
//! assert!(channel.is_member("fabricante-abc"));
//! ```

use crate::error::IdentityError;
use glasschain_core::crypto::sha256;
use glasschain_core::endorsement::PolicyExpression;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Organizations that are policy-level members of **every** collection by
/// default (ADR-003 decision 2).
///
/// Regulators already receive full pricing through NF-e, so per-collection
/// audit grants would only create recall blind spots.
pub const DEFAULT_REGULATOR_ORGS: &[&str] = &["anvisa", "mapa"];

/// Configuration for creating a new [`Channel`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Unique channel name (e.g. `"pharma-anvisa-channel"`).
    pub name: String,
    /// Node/participant IDs that are members of this channel. Regulator
    /// organizations are added to every collection automatically — they are
    /// not listed here.
    pub member_ids: Vec<String>,
    /// Human-readable channel description.
    pub description: String,
    /// The collection's optional endorsement policy declaration (ADR-008).
    ///
    /// Membership is a **reading/writing/receipt** control; it is never an
    /// endorsement. This field is this node's LOCAL DECLARATION of the
    /// collection's policy — the authoritative, enforced source is the
    /// committed `PolicyUpdate` carrying a collection-scoped
    /// `collection_policy` (ADR-008), which the endorsement engine evaluates
    /// at the commit path over verified principals (ticket #45). `None`
    /// declares the collection imposes no extra policy.
    #[serde(default)]
    pub endorsement_policy: Option<PolicyExpression>,
    /// The collection's private-payload retention window in seconds
    /// (ADR-003 decision 4): the transient pre-commit store holds payloads
    /// for this long before they become purge candidates. Default: 72 hours.
    /// Purge removes payloads; the chain's hash commitments persist forever.
    #[serde(default = "default_retention_secs")]
    pub retention_secs: u64,
}

/// The default retention window: 72 hours (ADR-003 decision 4).
#[must_use]
pub const fn default_retention_secs() -> u64 {
    72 * 60 * 60
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
    /// Create a new channel from the given configuration. Regulator
    /// organizations are members of every collection by default (ADR-003
    /// decision 2), so [`Self::is_member`] accepts them without being listed.
    #[must_use]
    pub fn new(config: ChannelConfig) -> Self {
        let mut member_set: HashSet<String> = config.member_ids.iter().cloned().collect();
        for regulator in DEFAULT_REGULATOR_ORGS {
            member_set.insert((*regulator).to_owned());
        }
        Self {
            config,
            committed_hashes: Vec::new(),
            private_data: Vec::new(),
            member_set,
        }
    }

    /// The collection's optional endorsement policy. Membership never
    /// satisfies a policy by itself — see [`ChannelConfig::endorsement_policy`].
    #[must_use]
    pub const fn endorsement_policy(&self) -> Option<&PolicyExpression> {
        self.config.endorsement_policy.as_ref()
    }

    /// The effective member organizations: the configured members plus the
    /// default regulators.
    #[must_use]
    pub fn member_orgs(&self) -> Vec<&str> {
        let mut orgs: Vec<&str> = self.member_set.iter().map(String::as_str).collect();
        orgs.sort_unstable();
        orgs
    }

    /// Return `true` if `node_id` is a member of this channel.
    #[must_use]
    pub fn is_member(&self, node_id: &str) -> bool {
        self.member_set.contains(node_id)
    }

    /// Submit private data to the channel.
    ///
    /// On success, returns the SHA-256 hash of the payload (to be included in
    /// the main-chain transaction as a "hash commitment").
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::Channel)` if the submitter is not a channel member.
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
    #[must_use]
    pub fn get_private_data(&self, hash: &str, requestor_id: &str) -> Option<&PrivateDataEntry> {
        if !self.is_member(requestor_id) {
            return None;
        }
        self.private_data.iter().find(|e| e.payload_hash == hash)
    }

    /// Return all on-chain hash commitments for this channel.
    #[must_use]
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
            endorsement_policy: None,
            retention_secs: default_retention_secs(),
        })
    }

    #[test]
    fn test_regulators_are_default_members() {
        let ch = test_channel();
        for regulator in DEFAULT_REGULATOR_ORGS {
            assert!(
                ch.is_member(regulator),
                "regulator {regulator} must be a default member"
            );
        }
        assert!(!ch.is_member("outsider"));
        let orgs = ch.member_orgs();
        assert!(orgs.contains(&"anvisa") && orgs.contains(&"mapa"));
    }

    #[test]
    fn test_endorsement_policy_is_separate_from_membership() {
        let mut ch = test_channel();
        // A member is not an endorsement: no policy is configured, so the
        // collection imposes none, and membership alone grants no approval.
        assert!(ch.endorsement_policy().is_none());
        assert!(ch.is_member("fabricante-abc"));
        // With a policy configured, membership still does not satisfy it —
        // the policy is evaluated by the endorsement engine over verified
        // principals, not by the member set.
        ch.config.endorsement_policy =
            Some(glasschain_core::endorsement::PolicyExpression::SignedBy(
                glasschain_core::endorsement::Principal::new("org-a"),
            ));
        assert!(ch.endorsement_policy().is_some());
        assert!(
            !ch.member_orgs().is_empty(),
            "membership and policy are separate controls"
        );
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
