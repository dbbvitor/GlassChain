//! Decentralized Identity and Membership Service Provider (MSP) for `GlassChain`.
//!
//! This crate implements **Phase 2** of the `GlassChain` architecture plan:
//! a permissioned governance model where **organizations**, not just nodes,
//! are the primary actors on the ledger.
//!
//! ## Design
//!
//! Every participant on the `GlassChain` network is identified by an
//! [`Identity`], which wraps an **ed25519 key pair** and an optional
//! **X.509 certificate** issued by the organization's Root CA (via `rcgen`).
//!
//! Every on-chain [`Transaction`] can be wrapped in a [`SignedTransaction`],
//! which carries a detached ed25519 signature that any peer can verify with
//! the signer's public key.
//!
//! Organizations manage their members through the [`Organization`] struct,
//! which acts as the **Membership Service Provider (MSP)** — the entity that
//! issues certificates and establishes trust boundaries.
//!
//! ## Trust hierarchy
//! ```text
//! Organization Root CA (rcgen X.509)
//!   └── Issues certificate to → Identity (ed25519 key pair + cert)
//!         └── Signs → SignedTransaction
//!               └── Verified by any peer with → Identity::public_key
//! ```
//!
//! ## Usage
//! ```rust
//! use glasschain_identity::{Identity, Organization, SignedTransaction};
//! use glasschain_core::{Transaction, TransactionKind, InventoryUpdate};
//!
//! // Organization sets up its Root CA.
//! let mut org = Organization::new("Pharma-Corp").unwrap();
//!
//! // Issue an identity (key pair + signed certificate).
//! let identity = org.issue_identity("node-1").unwrap();
//!
//! // Sign a transaction.
//! let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
//!     product_id: "SKU-001".into(),
//!     owner_id: "node-1".into(),
//!     quantity_delta: 100,
//!     reason: "initial stock".into(),
//! }));
//! let signed = identity.sign_transaction(tx).unwrap();
//!
//! // Verify the signature with the signer's public key.
//! assert!(signed.verify().is_ok());
//! ```

pub mod cert_verifier;
pub mod channel;
pub mod endorsement;
pub mod error;
pub mod identity;
pub mod msp;
pub mod msp_policy;

pub use cert_verifier::{CertChainVerifier, CertVerificationError, VerificationLevel};
pub use channel::{default_retention_secs, Channel, ChannelConfig, DEFAULT_REGULATOR_ORGS};
pub use endorsement::{
    EndorsementEngine, EndorsementPolicy, EndorsementProposal, EndorsementResult,
    EndorsementSignature,
};
pub use error::IdentityError;
pub use identity::{Identity, SignedTransaction};
pub use msp::Organization;
pub use msp_policy::MspEndorsementProvider;
