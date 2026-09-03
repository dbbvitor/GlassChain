//! Organizational Membership Service Provider (MSP).
//!
//! Each [`Organization`] acts as a Root CA: it generates a self-signed X.509
//! certificate (via `rcgen`) and then issues member certificates to
//! [`Identity`] instances.

use crate::error::IdentityError;
use crate::identity::Identity;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, KeyUsagePurpose,
    RevokedCertParams, SerialNumber,
};
use std::collections::HashMap;

/// How long a minted CRL stays current (`next_update`). An operator must
/// re-publish at least this often; verifiers fail closed on an expired CRL
/// (ADR-013).
const CRL_VALIDITY_DAYS: i64 = 30;

/// An organization that manages member identities and acts as the Root CA for
/// the `GlassChain` MSP.
///
/// Every transaction submitted to the ledger can be cryptographically tied
/// back to an organization, providing permissioned governance on top of the
/// open P2P protocol.
pub struct Organization {
    /// Human-readable organization name (used in the Root CA Distinguished Name).
    pub name: String,
    /// PEM-encoded Root CA certificate (shared with peers for verification).
    pub root_ca_cert_pem: String,
    /// Root CA issuer — bundles the CA params and key pair so member certificates
    /// can be signed without needing the original `Certificate` object.
    ca_issuer: Issuer<'static, KeyPair>,
    /// Registered member identities, keyed by `node_id`.
    members: HashMap<String, Identity>,
    /// Serial number counter for issued certificates.
    next_serial: u64,
    /// Issued certificate serials by node id — the bookkeeping `revoke_identity`
    /// needs (ADR-013).
    issued_serials: HashMap<String, rcgen::SerialNumber>,
    /// Revoked certificates awaiting inclusion in the next minted CRL.
    revoked: Vec<RevokedCertParams>,
}

impl Organization {
    /// Create a new organization, generating a self-signed Root CA certificate.
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` if the root CA key pair or
    /// self-signed certificate cannot be generated.
    pub fn new(name: impl Into<String>) -> Result<Self, IdentityError> {
        let org_name = name.into();

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, format!("{org_name} Root CA"));
        dn.push(DnType::OrganizationName, org_name.clone());
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        // The root signs member certificates and mints the organization's
        // CRLs (ADR-013); verifiers may enforce the crlSign key usage.
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

        let key_pair = KeyPair::generate().map_err(|e| IdentityError::CertGen(e.to_string()))?;

        // self_signed borrows params, so cert and root_ca_cert_pem can be obtained
        // before params and key_pair are consumed by Issuer::new below.
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let root_ca_cert_pem = cert.pem();

        // Issuer::new consumes params and key_pair; all Cow values are Owned
        // so the issuer carries 'static lifetime and can be stored in the struct.
        let ca_issuer = Issuer::new(params, key_pair);

        Ok(Self {
            name: org_name,
            root_ca_cert_pem,
            ca_issuer,
            members: HashMap::new(),
            next_serial: 1,
            issued_serials: HashMap::new(),
            revoked: Vec::new(),
        })
    }

    /// Issue a new member identity for the given node.
    ///
    /// Generates an ed25519 key pair and signs the member certificate with the
    /// organization's Root CA.  The resulting [`Identity`] is stored in the
    /// member registry and returned to the caller.
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` if the member key pair or certificate
    /// cannot be generated.
    ///
    /// # Panics
    ///
    /// Panics if the internal member registry is inconsistent (should never occur in practice).
    pub fn issue_identity(
        &mut self,
        node_id: impl Into<String>,
    ) -> Result<&Identity, IdentityError> {
        let nid: String = node_id.into();
        let mut identity = Identity::generate(nid.clone());

        // Build member certificate parameters. A tracked serial number is what
        // makes the certificate revocable (ADR-013).
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, nid.clone());
        dn.push(DnType::OrganizationName, self.name.clone());
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::NoCa;
        let serial = self.take_serial();
        params.serial_number = Some(serial.clone());

        // Derive the member certificate key pair from the identity's own ed25519
        // signing key so that certificate and transaction signatures share the
        // same public key.
        let member_key = identity
            .rcgen_key_pair()
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;

        // Sign the member certificate with the Root CA issuer.
        // rcgen 0.14: signed_by takes (&public_key, &Issuer) instead of
        // (&key_pair, &cert, &ca_key).
        let member_cert = params
            .signed_by(&member_key, &self.ca_issuer)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;

        identity.certificate_pem = Some(member_cert.pem());

        self.issued_serials.insert(nid.clone(), serial);
        self.members.insert(nid.clone(), identity);
        Ok(self.members.get(&nid).expect("just inserted"))
    }

    /// Revoke a previously issued member certificate (ADR-013).
    ///
    /// The revocation takes effect when the organization mints and publishes
    /// its next CRL ([`crl_pem`](Self::crl_pem)); verifiers that load that CRL
    /// reject the certificate from then on. Blocks and transactions signed
    /// before revocation stay valid — revocation is a go-forward control.
    ///
    /// # Errors
    ///
    /// Returns `Err` when no certificate was issued for `node_id`.
    pub fn revoke_identity(&mut self, node_id: &str) -> Result<(), IdentityError> {
        let serial = self.issued_serials.remove(node_id).ok_or_else(|| {
            IdentityError::CertGen(format!("no issued certificate for node `{node_id}`"))
        })?;
        self.revoked.push(RevokedCertParams {
            serial_number: serial,
            revocation_time: time_now(),
            reason_code: Some(rcgen::RevocationReason::KeyCompromise),
            invalidity_date: None,
        });
        Ok(())
    }

    /// Mint the organization's CRL over its revoked member certificates
    /// (ADR-013). The CRL is signed by the Root CA and stays current for
    /// [`CRL_VALIDITY_DAYS`] days — publish a fresh one before it expires,
    /// because verifiers fail closed on an expired CRL.
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` if the CRL cannot be built or
    /// signed.
    pub fn crl_pem(&self) -> Result<String, IdentityError> {
        self.crl_with_validity(0, CRL_VALIDITY_DAYS)
    }

    /// Mint a CRL whose `next_update` is `days` from now; negative days mint
    /// an already-expired CRL (tests exercise the fail-closed path with it).
    pub(crate) fn crl_with_validity(
        &self,
        backdated_days: i64,
        validity_days: i64,
    ) -> Result<String, IdentityError> {
        let now = time_now();
        let params = rcgen::CertificateRevocationListParams {
            this_update: now - time::Duration::days(backdated_days),
            next_update: now - time::Duration::days(backdated_days)
                + time::Duration::days(validity_days),
            crl_number: SerialNumber::from(1u64),
            issuing_distribution_point: None,
            revoked_certs: self.revoked.clone(),
            key_identifier_method: rcgen::KeyIdMethod::Sha256,
        };
        let crl = params
            .signed_by(&self.ca_issuer)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;
        crl.pem().map_err(|e| IdentityError::CertGen(e.to_string()))
    }

    /// Look up a member identity by node ID.
    #[must_use]
    pub fn get_member(&self, node_id: &str) -> Option<&Identity> {
        self.members.get(node_id)
    }

    /// Return all member node IDs.
    #[must_use]
    pub fn member_ids(&self) -> Vec<&str> {
        self.members
            .keys()
            .map(std::string::String::as_str)
            .collect()
    }

    /// Verify that a node ID is a registered member.
    #[must_use]
    pub fn is_member(&self, node_id: &str) -> bool {
        self.members.contains_key(node_id)
    }
    /// Mint an intermediate CA certificate signed by this organization's Root
    /// CA (ADR-013). The returned [`IntermediateCa`] can issue member
    /// identities of its own and mint its own CRL; verifiers build the two-hop
    /// path leaf → intermediate → root when the intermediate certificate is
    /// present in the trust store.
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` if the key pair or certificate
    /// cannot be generated.
    pub fn issue_intermediate_ca(
        &mut self,
        cn: impl Into<String>,
    ) -> Result<IntermediateCa, IdentityError> {
        let cn = cn.into();
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, &cn);
        dn.push(DnType::OrganizationName, self.name.clone());
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.serial_number = Some(self.take_serial());

        let key = KeyPair::generate().map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let issuer_params = params.clone();
        let cert = params
            .signed_by(&key, &self.ca_issuer)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let cert_pem = cert.pem();
        let issuer = Issuer::new(issuer_params, key);
        Ok(IntermediateCa {
            cert_pem,
            org_name: self.name.clone(),
            issuer,
            next_serial: 1,
            issued_serials: HashMap::new(),
            revoked: Vec::new(),
        })
    }

    fn take_serial(&mut self) -> SerialNumber {
        let serial = SerialNumber::from(self.next_serial);
        self.next_serial += 1;
        serial
    }
}

/// A subordinate CA signed by an organization's Root CA (ADR-013).
///
/// Issues member identities whose certificates chain leaf → intermediate →
/// root, and mints its own CRL over those members.
pub struct IntermediateCa {
    /// PEM-encoded intermediate CA certificate — a trust-store entry.
    cert_pem: String,
    /// Organization name stamped into issued member certificates.
    org_name: String,
    issuer: Issuer<'static, KeyPair>,
    next_serial: u64,
    issued_serials: HashMap<String, SerialNumber>,
    revoked: Vec<RevokedCertParams>,
}

impl IntermediateCa {
    /// The intermediate CA certificate, PEM-encoded — add this to the trust
    /// store so verifiers can build two-hop paths.
    #[must_use]
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Issue a member identity signed by this intermediate CA. Mirrors
    /// [`Organization::issue_identity`]; the identity is returned owned, not
    /// stored in the organization's registry.
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` if the certificate cannot be
    /// generated.
    pub fn issue_identity(
        &mut self,
        node_id: impl Into<String>,
    ) -> Result<Identity, IdentityError> {
        let nid: String = node_id.into();
        let mut identity = Identity::generate(nid.clone());

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, nid.clone());
        dn.push(DnType::OrganizationName, self.org_name.clone());
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::NoCa;
        let serial = SerialNumber::from(self.next_serial);
        self.next_serial += 1;
        params.serial_number = Some(serial.clone());

        let member_key = identity
            .rcgen_key_pair()
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let member_cert = params
            .signed_by(&member_key, &self.issuer)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;
        identity.certificate_pem = Some(member_cert.pem());

        self.issued_serials.insert(nid, serial);
        Ok(identity)
    }

    /// Revoke a previously issued member certificate (ADR-013). See
    /// [`Organization::revoke_identity`]; revocations mint into this CA's own
    /// CRL, which verifiers check against intermediate-issued leafs.
    ///
    /// # Errors
    ///
    /// Returns `Err` when no certificate was issued for `node_id`.
    pub fn revoke_identity(&mut self, node_id: &str) -> Result<(), IdentityError> {
        let serial = self.issued_serials.remove(node_id).ok_or_else(|| {
            IdentityError::CertGen(format!("no issued certificate for node `{node_id}`"))
        })?;
        self.revoked.push(RevokedCertParams {
            serial_number: serial,
            revocation_time: time_now(),
            reason_code: Some(rcgen::RevocationReason::KeyCompromise),
            invalidity_date: None,
        });
        Ok(())
    }

    /// Mint this intermediate CA's CRL (ADR-013). See
    /// [`Organization::crl_pem`].
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` if the CRL cannot be built or
    /// signed.
    pub fn crl_pem(&self) -> Result<String, IdentityError> {
        let now = time_now();
        let params = rcgen::CertificateRevocationListParams {
            this_update: now,
            next_update: now + time::Duration::days(CRL_VALIDITY_DAYS),
            crl_number: SerialNumber::from(1u64),
            issuing_distribution_point: None,
            revoked_certs: self.revoked.clone(),
            key_identifier_method: rcgen::KeyIdMethod::Sha256,
        };
        let crl = params
            .signed_by(&self.issuer)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;
        crl.pem().map_err(|e| IdentityError::CertGen(e.to_string()))
    }
}

/// Current UTC time, the single place CRL minting gets the clock from.
fn time_now() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc()
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
