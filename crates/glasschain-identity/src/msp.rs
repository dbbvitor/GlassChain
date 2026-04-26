//! Organizational Membership Service Provider (MSP).
//!
//! Each [`Organization`] acts as a Root CA: it generates a self-signed X.509
//! certificate (via `rcgen`) and then issues member certificates to
//! [`Identity`] instances.

use crate::error::IdentityError;
use crate::identity::Identity;
use rcgen::{Certificate, CertificateParams, DistinguishedName, DnType, KeyPair};
use std::collections::HashMap;

/// An organization that manages member identities and acts as the Root CA for
/// the GlassChain MSP.
///
/// Every transaction submitted to the ledger can be cryptographically tied
/// back to an organization, providing permissioned governance on top of the
/// open P2P protocol.
pub struct Organization {
    /// Human-readable organization name (used in the Root CA Distinguished Name).
    pub name: String,
    /// PEM-encoded Root CA certificate (shared with peers for verification).
    pub root_ca_cert_pem: String,
    /// Root CA Certificate object (kept to sign member certs).
    ca_cert: Certificate,
    /// Root CA key pair (kept to sign member certs; never serialized).
    ca_key: KeyPair,
    /// Registered member identities, keyed by `node_id`.
    members: HashMap<String, Identity>,
}

impl Organization {
    /// Create a new organization, generating a self-signed Root CA certificate.
    pub fn new(name: impl Into<String>) -> Result<Self, IdentityError> {
        let org_name = name.into();

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, format!("{} Root CA", org_name));
        dn.push(DnType::OrganizationName, org_name.clone());
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);

        let key_pair =
            KeyPair::generate().map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;

        let root_ca_cert_pem = cert.pem();

        Ok(Self {
            name: org_name,
            root_ca_cert_pem,
            ca_cert: cert,
            ca_key: key_pair,
            members: HashMap::new(),
        })
    }

    /// Issue a new member identity for the given node.
    ///
    /// Generates an ed25519 key pair and signs the member certificate with the
    /// organization's Root CA.  The resulting [`Identity`] is stored in the
    /// member registry and returned to the caller.
    pub fn issue_identity(
        &mut self,
        node_id: impl Into<String>,
    ) -> Result<&Identity, IdentityError> {
        let nid: String = node_id.into();
        let mut identity = Identity::generate(nid.clone());

        // Build member certificate parameters.
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, nid.clone());
        dn.push(DnType::OrganizationName, self.name.clone());
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::NoCa;

        // Use a fresh key pair for the member certificate.
        let member_key: KeyPair =
            KeyPair::generate().map_err(|e| IdentityError::CertGen(e.to_string()))?;

        // Sign the member certificate with the Root CA.
        let member_cert = params
            .signed_by(&member_key, &self.ca_cert, &self.ca_key)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;

        identity.certificate_pem = Some(member_cert.pem());

        self.members.insert(nid.clone(), identity);
        Ok(self.members.get(&nid).expect("just inserted"))
    }

    /// Look up a member identity by node ID.
    pub fn get_member(&self, node_id: &str) -> Option<&Identity> {
        self.members.get(node_id)
    }

    /// Return all member node IDs.
    pub fn member_ids(&self) -> Vec<&str> {
        self.members.keys().map(|s| s.as_str()).collect()
    }

    /// Verify that a node ID is a registered member.
    pub fn is_member(&self, node_id: &str) -> bool {
        self.members.contains_key(node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_organization_creates_root_ca() {
        let org = Organization::new("PharmaCorp").unwrap();
        assert!(!org.root_ca_cert_pem.is_empty());
        assert!(org.root_ca_cert_pem.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn test_issue_identity_creates_member_cert() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let identity = org.issue_identity("node-1").unwrap();
        assert_eq!(identity.node_id, "node-1");
        assert!(identity.certificate_pem.is_some());
        let cert_pem = identity.certificate_pem.as_ref().unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn test_member_lookup() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        org.issue_identity("node-1").unwrap();
        assert!(org.is_member("node-1"));
        assert!(!org.is_member("unknown-node"));
    }

    #[test]
    fn test_multiple_members() {
        let mut org = Organization::new("MedCorp").unwrap();
        org.issue_identity("distributor-1").unwrap();
        org.issue_identity("pharmacy-1").unwrap();
        assert_eq!(org.member_ids().len(), 2);
    }

    #[test]
    fn test_sign_and_verify_with_issued_identity() {
        use glasschain_core::{InventoryUpdate, Transaction, TransactionKind};

        let mut org = Organization::new("TestOrg").unwrap();
        let identity = org.issue_identity("node-a").unwrap();

        let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "PROD-001".into(),
            owner_id: "node-a".into(),
            quantity_delta: 10,
            reason: "test".into(),
        }));

        let signed = identity.sign_transaction(tx).unwrap();
        assert!(signed.verify().is_ok());
        assert!(org.is_member(&signed.signer_node_id));
    }
}
