//! Ed25519 identity and signed-transaction wrappers.

use crate::error::IdentityError;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use glasschain_core::Transaction;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};

/// An on-ledger participant identity, consisting of an ed25519 key pair and a
/// human-readable node identifier.
///
/// The **signing key** (private) never leaves the identity object.
/// The **verifying key** (public) is exposed for sharing with peers.
pub struct Identity {
    /// Human-readable node or participant identifier.
    pub node_id: String,
    /// ed25519 signing key (contains both private and public key material).
    signing_key: SigningKey,
    /// PEM-encoded X.509 certificate signed by the organization's Root CA.
    /// `None` until the identity has been issued a certificate via
    /// [`Organization::issue_identity`].
    pub certificate_pem: Option<String>,
}

impl Identity {
    /// Generate a fresh identity with a randomly-generated ed25519 key pair.
    pub fn generate(node_id: impl Into<String>) -> Self {
        let mut rng = OsRng;
        let signing_key = SigningKey::generate(&mut rng);
        Self {
            node_id: node_id.into(),
            signing_key,
            certificate_pem: None,
        }
    }

    /// Return the raw 32-byte public key.
    #[must_use]
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Return the hex-encoded public key string.
    #[must_use]
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public_key_bytes())
    }

    /// Return the ed25519 [`VerifyingKey`] for signature verification.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// Produce an `rcgen::KeyPair` that wraps the **same** ed25519 private key
    /// used for transaction signing.
    ///
    /// This allows the MSP to issue X.509 certificates whose public key is
    /// identical to the key that signs on-ledger transactions, unifying the
    /// two key systems.
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` if the ed25519 private key cannot be
    /// encoded to PKCS#8 DER format or if `rcgen` rejects the key material.
    pub(crate) fn rcgen_key_pair(&self) -> Result<rcgen::KeyPair, crate::error::IdentityError> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        let pkcs8_doc = self
            .signing_key
            .to_pkcs8_der()
            .map_err(|e| crate::error::IdentityError::CertGen(e.to_string()))?;
        rcgen::KeyPair::try_from(pkcs8_doc.as_bytes())
            .map_err(|e| crate::error::IdentityError::CertGen(e.to_string()))
    }

    /// Sign a transaction and wrap it in a [`SignedTransaction`].
    ///
    /// The signature covers the canonical JSON serialisation of the
    /// transaction (same bytes as used in the block hash computation).
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::Serialization)` if the transaction cannot be
    /// serialised to canonical JSON.
    pub fn sign_transaction(&self, tx: Transaction) -> Result<SignedTransaction, IdentityError> {
        let canonical = serde_json::to_vec(&tx)?;
        let signature = self.signing_key.sign(&canonical);
        Ok(SignedTransaction {
            transaction: tx,
            signature_bytes: signature.to_bytes().to_vec(),
            signer_public_key: self.public_key_bytes().to_vec(),
            signer_node_id: self.node_id.clone(),
        })
    }
}

/// A [`Transaction`] bundled with an ed25519 detached signature.
///
/// Any peer can verify the signature using the included
/// `signer_public_key` after optionally checking it against the relevant
/// organization's membership list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTransaction {
    /// The transaction payload.
    pub transaction: Transaction,
    /// 64-byte ed25519 signature over the canonical JSON of `transaction`.
    pub signature_bytes: Vec<u8>,
    /// 32-byte ed25519 public key of the signer.
    pub signer_public_key: Vec<u8>,
    /// Human-readable node identifier of the signer.
    pub signer_node_id: String,
}

impl SignedTransaction {
    /// Verify the embedded signature against the transaction payload.
    ///
    /// Returns `Ok(())` when the signature is valid.
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::InvalidPublicKey)` if the stored public key bytes
    /// cannot be parsed as a valid ed25519 verifying key, or
    /// `Err(IdentityError::VerificationFailed)` if the signature bytes are malformed
    /// or do not match the transaction payload.
    pub fn verify(&self) -> Result<(), IdentityError> {
        let key_bytes: [u8; 32] = self
            .signer_public_key
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::InvalidPublicKey)?;
        let verifying_key =
            VerifyingKey::from_bytes(&key_bytes).map_err(|_| IdentityError::InvalidPublicKey)?;

        let sig_bytes: [u8; 64] = self
            .signature_bytes
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::VerificationFailed)?;
        let signature = Signature::from_bytes(&sig_bytes);

        let canonical =
            serde_json::to_vec(&self.transaction).map_err(IdentityError::Serialization)?;

        verifying_key
            .verify(&canonical, &signature)
            .map_err(|_| IdentityError::VerificationFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{InventoryUpdate, TransactionKind};

    fn sample_tx() -> Transaction {
        Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-001".into(),
            owner_id: "node-1".into(),
            quantity_delta: 50,
            reason: "test".into(),
        }))
    }

    #[test]
    fn test_identity_generate_unique_keys() {
        let a = Identity::generate("node-a");
        let b = Identity::generate("node-b");
        assert_ne!(a.public_key_bytes(), b.public_key_bytes());
    }

    #[test]
    fn test_sign_and_verify_success() {
        let identity = Identity::generate("node-1");
        let tx = sample_tx();
        let signed = identity.sign_transaction(tx).unwrap();
        assert!(signed.verify().is_ok());
    }

    #[test]
    fn test_tampered_transaction_fails_verification() {
        use glasschain_core::TransactionKind;
        let identity = Identity::generate("node-1");
        let tx = sample_tx();
        let mut signed = identity.sign_transaction(tx).unwrap();
        // Tamper: swap the transaction kind
        if let TransactionKind::InventoryUpdate(ref mut u) = signed.transaction.kind {
            u.quantity_delta = 999;
        }
        assert!(signed.verify().is_err());
    }

    #[test]
    fn test_wrong_key_fails_verification() {
        let signer = Identity::generate("node-1");
        let impostor = Identity::generate("node-2");
        let tx = sample_tx();
        let mut signed = signer.sign_transaction(tx).unwrap();
        // Replace with impostor's public key
        signed.signer_public_key = impostor.public_key_bytes().to_vec();
        assert!(signed.verify().is_err());
    }

    #[test]
    fn test_public_key_hex_is_64_chars() {
        let identity = Identity::generate("test");
        assert_eq!(identity.public_key_hex().len(), 64);
    }

    #[test]
    fn test_signed_transaction_serialization_roundtrip() {
        let identity = Identity::generate("node-1");
        let signed = identity.sign_transaction(sample_tx()).unwrap();
        let json = serde_json::to_string(&signed).unwrap();
        let decoded: SignedTransaction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.signer_node_id, "node-1");
        assert!(decoded.verify().is_ok());
    }
}
