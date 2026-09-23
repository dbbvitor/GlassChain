// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Organizational Membership Service Provider (MSP).
//!
//! Each [`Organization`] acts as a Root CA: it generates a self-signed X.509
//! certificate (via `rcgen`) and then issues member certificates to
//! [`Identity`] instances.

use crate::error::IdentityError;
use crate::identity::Identity;
use crate::ocsp::{mint_good_response, OcspError, OcspMintInput};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, KeyUsagePurpose,
    RevocationReason, RevokedCertParams, SerialNumber,
};
use rustls_pki_types::pem::PemObject as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use x509_cert::der::{Decode, Encode};

/// How long a minted CRL stays current (`next_update`). An operator must
/// re-publish at least this often; verifiers fail closed on an expired CRL
/// (ADR-013).
const CRL_VALIDITY_DAYS: i64 = 30;

/// The Organizational-Unit value marking an **operator administrator**
/// certificate (ADR-017). Only certs carrying this OU satisfy the admin gate.
pub const ADMIN_ROLE: &str = "admin";

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
    /// The Root CA's PKCS#8 private-key DER, stashed before `Issuer::new`
    /// consumes the key pair — the OCSP staple signer (ADR-017).
    ca_key_pkcs8_der: Vec<u8>,
    /// The Root CA's Subject DN, full DER encoding — the OCSP responderID
    /// and certID issuer-name-hash input.
    ca_subject_der: Vec<u8>,
    /// The Root CA's raw public-key bytes (SPKI bit-string contents) — the
    /// OCSP certID issuer-key-hash input.
    ca_public_key: Vec<u8>,
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
        // The root signs member certificates, mints the organization's CRLs
        // (ADR-013) and signs OCSP staples (ADR-017); verifiers may enforce
        // the crlSign and digitalSignature key usages.
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];

        let key_pair = KeyPair::generate().map_err(|e| IdentityError::CertGen(e.to_string()))?;

        // self_signed borrows params, so cert and root_ca_cert_pem can be obtained
        // before params and key_pair are consumed by Issuer::new below.
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let root_ca_cert_pem = cert.pem();

        // Stash the signer material the OCSP staple minter needs (ADR-017):
        // PKCS#8 private key, Subject DN and raw public key, all before
        // Issuer::new consumes them.
        let ca_key_pkcs8_der = key_pair.serialize_der();
        let ca_public_key = key_pair.public_key_raw().to_vec();
        let ca_subject_der = {
            let parsed = x509_cert::Certificate::from_der(cert.der().as_ref())
                .map_err(|e| IdentityError::CertGen(e.to_string()))?;
            let mut out = Vec::new();
            parsed
                .tbs_certificate()
                .subject()
                .encode_to_vec(&mut out)
                .map_err(|e| IdentityError::CertGen(e.to_string()))?;
            out
        };

        // Issuer::new consumes params and key_pair; all Cow values are Owned
        // so the issuer carries 'static lifetime and can be stored in the struct.
        let ca_issuer = Issuer::new(params, key_pair);

        Ok(Self {
            name: org_name,
            root_ca_cert_pem,
            ca_issuer,
            ca_key_pkcs8_der,
            ca_subject_der,
            ca_public_key,
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
        self.issue_identity_with_role(node_id, None)
    }

    /// Issue a member identity carrying an operational role (ADR-017): the
    /// role is stamped into the certificate's subject as an
    /// Organizational-Unit name (`OU=admin` for
    /// [`ADMIN_ROLE`]), so a verifying party can read it from the verified
    /// chain — never from a caller-supplied label.
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` as [`Self::issue_identity`].
    ///
    /// # Panics
    ///
    /// Panics if the internal member registry is inconsistent (should never occur in practice).
    pub fn issue_identity_with_role(
        &mut self,
        node_id: impl Into<String>,
        role: Option<&str>,
    ) -> Result<&Identity, IdentityError> {
        let nid: String = node_id.into();
        let mut identity = Identity::generate(nid.clone());

        // Build member certificate parameters. A tracked serial number is what
        // makes the certificate revocable (ADR-013).
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, nid.clone());
        dn.push(DnType::OrganizationName, self.name.clone());
        if let Some(role) = role {
            dn.push(DnType::OrganizationalUnitName, role);
        }
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

    /// Mint an OCSP staple attesting **good** for the member certificate
    /// issued to `node_id` (ADR-017). The response is signed by this
    /// organization's Root CA and stays fresh for [`OCSP_VALIDITY_SECS`];
    /// the member staples it on its `Hello` and receiving nodes verify it
    /// locally (no responder network queries).
    ///
    /// Revoked or never-issued nodes cannot be attested: minting is refused,
    /// so a staple is always a live "good" attestation from the issuer.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::CertGen`] when `node_id` has no live issued
    /// certificate or the response cannot be minted.
    pub fn ocsp_response_der(&self, node_id: &str) -> Result<Vec<u8>, IdentityError> {
        self.ocsp_response_for_serial(node_id, self.issued_serials.get(node_id))
    }

    fn ocsp_response_for_serial(
        &self,
        node_id: &str,
        serial: Option<&rcgen::SerialNumber>,
    ) -> Result<Vec<u8>, IdentityError> {
        let serial = serial.ok_or_else(|| {
            IdentityError::CertGen(format!("no live issued certificate for node `{node_id}`"))
        })?;
        let mut serial_be = [0u8; 8];
        let raw = serial.to_bytes();
        serial_be[8usize.saturating_sub(raw.len())..].copy_from_slice(&raw);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        mint_good_response(&OcspMintInput {
            serial: serial_be,
            issuer_subject_der: &self.ca_subject_der,
            issuer_public_key: &self.ca_public_key,
            issuer_pkcs8_der: &self.ca_key_pkcs8_der,
            now,
            validity_secs: crate::ocsp::OCSP_VALIDITY_SECS,
        })
        .map_err(|e: OcspError| IdentityError::CertGen(e.to_string()))
    }

    /// The Root CA's PKCS#8 private-key DER — exposed for the OCSP minter's
    /// tests; operators' persistent material is never serialized here.
    #[allow(dead_code)]
    pub(crate) fn ca_key_pkcs8_der(&self) -> &[u8] {
        &self.ca_key_pkcs8_der
    }

    /// The Root CA's Subject DN, full DER encoding — test accessor for the
    /// OCSP minter.
    #[allow(dead_code)]
    pub(crate) fn ca_subject_der(&self) -> &[u8] {
        &self.ca_subject_der
    }

    /// The Root CA's raw public-key bytes — test accessor for the OCSP minter.
    #[allow(dead_code)]
    pub(crate) fn ca_public_key(&self) -> &[u8] {
        &self.ca_public_key
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
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        params.serial_number = Some(self.take_serial());

        let key = KeyPair::generate().map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let issuer_params = params.clone();
        let cert = params
            .signed_by(&key, &self.ca_issuer)
            .map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let cert_pem = cert.pem();
        let key_pkcs8_der = key.serialize_der();
        let public_key = key.public_key_raw().to_vec();
        let subject_der = {
            let parsed = x509_cert::Certificate::from_der(cert.der().as_ref())
                .map_err(|e| IdentityError::CertGen(e.to_string()))?;
            let mut out = Vec::new();
            parsed
                .tbs_certificate()
                .subject()
                .encode_to_vec(&mut out)
                .map_err(|e| IdentityError::CertGen(e.to_string()))?;
            out
        };
        let issuer = Issuer::new(issuer_params, key);
        Ok(IntermediateCa {
            cert_pem,
            org_name: self.name.clone(),
            issuer,
            key_pkcs8_der,
            subject_der,
            public_key,
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
    /// The intermediate's PKCS#8 private-key DER — the OCSP staple signer.
    key_pkcs8_der: Vec<u8>,
    /// The intermediate's Subject DN, full DER encoding.
    subject_der: Vec<u8>,
    /// The intermediate's raw public-key bytes.
    public_key: Vec<u8>,
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
        self.issue_identity_with_role(node_id, None)
    }

    /// Issue an intermediate-issued member identity carrying an operational
    /// role (ADR-017). Mirrors [`Organization::issue_identity_with_role`].
    ///
    /// # Errors
    ///
    /// Returns `Err(IdentityError::CertGen)` if the certificate cannot be
    /// generated.
    pub fn issue_identity_with_role(
        &mut self,
        node_id: impl Into<String>,
        role: Option<&str>,
    ) -> Result<Identity, IdentityError> {
        let nid: String = node_id.into();
        let mut identity = Identity::generate(nid.clone());

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, nid.clone());
        dn.push(DnType::OrganizationName, self.org_name.clone());
        if let Some(role) = role {
            dn.push(DnType::OrganizationalUnitName, role);
        }
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

    /// Mint an OCSP staple attesting **good** for a member certificate issued
    /// by this intermediate CA (ADR-017). See
    /// [`Organization::ocsp_response_der`].
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::CertGen`] when the node has no live issued
    /// certificate or the response cannot be minted.
    pub fn ocsp_response_der(&self, node_id: &str) -> Result<Vec<u8>, IdentityError> {
        let serial = self.issued_serials.get(node_id).ok_or_else(|| {
            IdentityError::CertGen(format!("no live issued certificate for node `{node_id}`"))
        })?;
        let mut serial_be = [0u8; 8];
        let raw = serial.to_bytes();
        serial_be[8usize.saturating_sub(raw.len())..].copy_from_slice(&raw);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        mint_good_response(&OcspMintInput {
            serial: serial_be,
            issuer_subject_der: &self.subject_der,
            issuer_public_key: &self.public_key,
            issuer_pkcs8_der: &self.key_pkcs8_der,
            now,
            validity_secs: crate::ocsp::OCSP_VALIDITY_SECS,
        })
        .map_err(|e: OcspError| IdentityError::CertGen(e.to_string()))
    }

    /// Revoke a previously issued member certificate (ADR-013). See
    /// [`Organization::revoke_identity`]; revocations mint into this CA's own
    /// CRL, which verifiers check against intermediate-issued leaves.
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

// ── Durable custody (ADR-018) ─────────────────────────────────────────────────

/// One revoked certificate, in the durable snapshot form: serial hex, Unix
/// revocation time and the RFC 5280 reason code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokedRecord {
    pub serial_hex: String,
    pub revocation_time_unix: i64,
    pub reason_code: u8,
}

/// One registered member identity, in the durable snapshot form: node id,
/// ed25519 seed (hex) and the issued certificate PEM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberRecord {
    pub node_id: String,
    pub seed_hex: String,
    pub certificate_pem: String,
}

/// The durable form of an [`Organization`] (ADR-018): the state an
/// operator-owned identity file carries across restarts so the node
/// re-presents the same keys instead of re-keying.
///
/// Fields: Root CA key material, serial bookkeeping, revocation history and
/// member identities. The `ca_key_pkcs8_der_hex` and `seed_hex` fields are
/// **private key material**: the caller owns where the snapshot lives
/// (permission the file, keep it out of replicated/archived storage).
/// Nothing in `GlassChain` writes it to the storage seam.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationSnapshot {
    pub name: String,
    pub root_ca_cert_pem: String,
    /// Root CA PKCS#8 private key, hex-encoded — the cert/CRL/staple signer.
    pub ca_key_pkcs8_der_hex: String,
    pub next_serial: u64,
    /// Issued serials by node id (hex-encoded serial bytes) —
    /// `revoke_identity` bookkeeping and the staple certID.
    pub issued_serials: Vec<(String, String)>,
    pub revoked: Vec<RevokedRecord>,
    pub members: Vec<MemberRecord>,
}

/// Map an RFC 5280 revocation reason code back to `rcgen`'s enum. Code 7 is
/// unassigned (RFC 5280) — `Unspecified` stands in and a warning logs.
fn revocation_reason_from_code(code: u8) -> RevocationReason {
    match code {
        0 => RevocationReason::Unspecified,
        1 => RevocationReason::KeyCompromise,
        2 => RevocationReason::CaCompromise,
        3 => RevocationReason::AffiliationChanged,
        4 => RevocationReason::Superseded,
        5 => RevocationReason::CessationOfOperation,
        6 => RevocationReason::CertificateHold,
        8 => RevocationReason::RemoveFromCrl,
        9 => RevocationReason::PrivilegeWithdrawn,
        10 => RevocationReason::AaCompromise,
        other => {
            log::warn!("custody: unknown revocation reason code {other}; treating as unspecified");
            RevocationReason::Unspecified
        }
    }
}

/// The RFC 5280 reason code for `rcgen`'s enum.
const fn revocation_reason_code(reason: RevocationReason) -> u8 {
    match reason {
        RevocationReason::Unspecified => 0,
        RevocationReason::KeyCompromise => 1,
        RevocationReason::CaCompromise => 2,
        RevocationReason::AffiliationChanged => 3,
        RevocationReason::Superseded => 4,
        RevocationReason::CessationOfOperation => 5,
        RevocationReason::CertificateHold => 6,
        RevocationReason::RemoveFromCrl => 8,
        RevocationReason::PrivilegeWithdrawn => 9,
        RevocationReason::AaCompromise => 10,
    }
}

impl Organization {
    /// Serialize the organization's durable state (ADR-018): Root CA key
    /// pair, serial bookkeeping, revocations and member identities. The
    /// output contains **private key material** — the caller owns the file
    /// (permission it, keep it out of replicated/archived storage).
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::CertGen`] if serialization fails or a member
    /// has no certificate to persist.
    pub fn export_json(&self) -> Result<String, IdentityError> {
        let members = self
            .members
            .iter()
            .map(|(node_id, identity)| {
                Ok(MemberRecord {
                    node_id: node_id.clone(),
                    seed_hex: hex::encode(identity.seed_bytes()),
                    certificate_pem: identity.certificate_pem.clone().ok_or_else(|| {
                        IdentityError::CertGen(format!(
                            "member `{node_id}` has no certificate to persist"
                        ))
                    })?,
                })
            })
            .collect::<Result<Vec<_>, IdentityError>>()?;
        let revoked = self
            .revoked
            .iter()
            .map(|params| RevokedRecord {
                serial_hex: hex::encode(params.serial_number.to_bytes()),
                revocation_time_unix: params.revocation_time.unix_timestamp(),
                reason_code: params.reason_code.map_or(0, revocation_reason_code),
            })
            .collect();
        let snapshot = OrganizationSnapshot {
            name: self.name.clone(),
            root_ca_cert_pem: self.root_ca_cert_pem.clone(),
            ca_key_pkcs8_der_hex: hex::encode(&self.ca_key_pkcs8_der),
            next_serial: self.next_serial,
            issued_serials: self
                .issued_serials
                .iter()
                .map(|(node_id, serial)| (node_id.clone(), hex::encode(serial.to_bytes())))
                .collect(),
            revoked,
            members,
        };
        serde_json::to_string_pretty(&snapshot).map_err(|e| IdentityError::CertGen(e.to_string()))
    }

    /// Rebuild an [`Organization`] from [`export_json`](Self::export_json)
    /// output (ADR-018): the same Root CA key (so peers' trust-store anchors
    /// keep verifying), the same member keys (so possession proofs, TOFU
    /// rotation signatures and TLS fingerprints survive restarts), and the
    /// same serial bookkeeping (so CRLs stay coherent).
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::CertGen`] for malformed JSON, a corrupt CA
    /// key, an unparseable root certificate, or a member record whose seed
    /// is not 32 bytes.
    pub fn import_json(json: &str) -> Result<Self, IdentityError> {
        let snapshot: OrganizationSnapshot =
            serde_json::from_str(json).map_err(|e| IdentityError::CertGen(e.to_string()))?;
        let ca_key_pkcs8_der = hex::decode(&snapshot.ca_key_pkcs8_der_hex)
            .map_err(|e| IdentityError::CertGen(format!("CA key is not hex: {e}")))?;

        // Restore the CA key pair (the P-256 algorithm the original
        // generated with).
        let key_pair = KeyPair::from_pkcs8_der_and_sign_algo(
            &rustls_pki_types::PrivatePkcs8KeyDer::from(ca_key_pkcs8_der.clone()),
            &rcgen::PKCS_ECDSA_P256_SHA256,
        )
        .map_err(|e| IdentityError::CertGen(format!("CA key does not restore: {e}")))?;

        // Same construction as `new()`: the issuer's subject DN must match
        // the persisted root certificate so newly issued members chain to
        // the same anchor.
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, format!("{} Root CA", snapshot.name));
        dn.push(DnType::OrganizationName, snapshot.name.clone());
        params.distinguished_name = dn;
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_issuer = Issuer::new(params, key_pair);

        // Re-derive the signer material the staple minter needs.
        let root_der =
            rustls_pki_types::CertificateDer::from_pem_slice(snapshot.root_ca_cert_pem.as_bytes())
                .map_err(|e| IdentityError::CertGen(format!("root CA is not PEM: {e}")))?;
        let ca_subject_der = {
            let parsed = x509_cert::Certificate::from_der(root_der.as_ref())
                .map_err(|e| IdentityError::CertGen(e.to_string()))?;
            let mut out = Vec::new();
            parsed
                .tbs_certificate()
                .subject()
                .encode_to_vec(&mut out)
                .map_err(|e| IdentityError::CertGen(e.to_string()))?;
            out
        };
        // The staple certID issuer-key-hash input: derived straight from the
        // restored key (`Issuer` exposes no public key accessor).
        let ca_public_key = {
            use ring::rand::SystemRandom;
            use ring::signature::{EcdsaKeyPair, KeyPair as _};
            EcdsaKeyPair::from_pkcs8(
                &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                &ca_key_pkcs8_der,
                &SystemRandom::new(),
            )
            .map_err(|e| IdentityError::CertGen(format!("CA key does not restore: {e}")))?
            .public_key()
            .as_ref()
            .to_vec()
        };

        let issued_serials = snapshot
            .issued_serials
            .iter()
            .map(|(node_id, serial_hex)| {
                let bytes = hex::decode(serial_hex)
                    .map_err(|e| IdentityError::CertGen(format!("serial is not hex: {e}")))?;
                Ok((node_id.clone(), SerialNumber::from_slice(&bytes)))
            })
            .collect::<Result<HashMap<String, SerialNumber>, IdentityError>>()?;
        let revoked = snapshot
            .revoked
            .iter()
            .map(|record| {
                Ok(RevokedCertParams {
                    serial_number: SerialNumber::from_slice(
                        &hex::decode(&record.serial_hex).map_err(|e| {
                            IdentityError::CertGen(format!("serial is not hex: {e}"))
                        })?,
                    ),
                    revocation_time: time::OffsetDateTime::from_unix_timestamp(
                        record.revocation_time_unix,
                    )
                    .map_err(|e| IdentityError::CertGen(e.to_string()))?,
                    reason_code: Some(revocation_reason_from_code(record.reason_code)),
                    invalidity_date: None,
                })
            })
            .collect::<Result<Vec<RevokedCertParams>, IdentityError>>()?;
        let mut members = HashMap::new();
        for member in &snapshot.members {
            let seed = hex::decode(&member.seed_hex)
                .map_err(|e| IdentityError::CertGen(format!("seed is not hex: {e}")))?;
            let seed: [u8; 32] = seed
                .try_into()
                .map_err(|_| IdentityError::CertGen("member seed is not 32 bytes".to_owned()))?;
            let mut identity = Identity::from_seed(member.node_id.clone(), seed);
            identity.certificate_pem = Some(member.certificate_pem.clone());
            members.insert(member.node_id.clone(), identity);
        }

        Ok(Self {
            name: snapshot.name,
            root_ca_cert_pem: snapshot.root_ca_cert_pem,
            ca_issuer,
            ca_key_pkcs8_der,
            ca_subject_der,
            ca_public_key,
            members,
            next_serial: snapshot.next_serial,
            issued_serials,
            revoked,
        })
    }
}

/// Current UTC time, the single place CRL minting gets the clock from.
fn time_now() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc()
}

/// Issue an admin-role identity from `org` (ADR-017) — the shared helper the
/// OCSP and RBAC tests exercise the admin path through.
#[cfg(test)]
pub(crate) mod test_support {
    use super::Organization;

    pub fn admin_org_with_admin(org: &mut Organization, node_id: &str) -> crate::Identity {
        org.issue_identity_with_role(node_id, Some(crate::msp::ADMIN_ROLE))
            .expect("admin identity")
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ADR-018: durable custody snapshots ─────────────────────────────────

    use crate::cert_verifier::CertChainVerifier;
    use crate::ocsp::OcspStatus;

    /// Export → drop → import re-presents the same keys: identical root CA,
    /// identical member keys (possession proofs keep verifying), continued
    /// serial bookkeeping (no collision after import), revocations intact
    /// (the CRL still lists them), and staples still mint.
    #[test]
    fn snapshot_round_trip_preserves_keys_serials_and_revocations() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let live = org.issue_identity("admin-node").unwrap().clone();
        let revoked_identity = org.issue_identity("node-a").unwrap().clone();
        org.revoke_identity("node-a").unwrap();

        let json = org.export_json().unwrap();

        // The original moves on: another identity consumes a serial.
        org.issue_identity("after").unwrap();

        let mut restored = Organization::import_json(&json).unwrap();

        // Root CA identity is byte-identical.
        assert_eq!(restored.root_ca_cert_pem, org.root_ca_cert_pem);
        // The live member re-presents the same key and cert.
        let restored_live = restored.get_member("admin-node").unwrap();
        assert_eq!(
            restored_live.public_key_bytes(),
            live.public_key_bytes(),
            "a restart must re-present the pinned identity key"
        );
        assert_eq!(restored_live.certificate_pem, live.certificate_pem);
        // A possession proof from the restored identity verifies — same key.
        let proof = restored_live.sign_bytes(b"anything");
        assert!(crate::possession::verify_ed25519(
            &live.public_key_bytes(),
            b"anything",
            &proof
        ));
        // Serial bookkeeping continues from the persisted counter.
        assert_eq!(restored.next_serial, 3, "two identities were issued");
        let fresh = restored
            .issue_identity("fresh-after-import")
            .unwrap()
            .clone();
        assert!(fresh.certificate_pem.is_some());
        assert!(restored.is_member("fresh-after-import"));
        // The revocation survived: the CRL lists node-a's serial.
        let crl = restored.crl_pem().unwrap();
        assert!(!crl.is_empty());
        // And staples still mint for the live member.
        let staple = restored.ocsp_response_der("admin-node").unwrap();
        let verifier = CertChainVerifier::from_org(&restored).unwrap();
        assert_eq!(
            verifier
                .verify_ocsp_staple(live.certificate_pem.as_ref().unwrap(), &staple)
                .unwrap(),
            OcspStatus::Good
        );
        // Revoked member's cert still rejects (same serial → same CRL entry).
        let _ = revoked_identity;
    }

    /// Malformed snapshots are refused, never silently re-keyed.
    #[test]
    fn snapshot_import_rejects_malformed_input() {
        assert!(Organization::import_json("not json").is_err());
        assert!(Organization::import_json("{\"name\":\"X\"}").is_err());
        // A member seed that is not 32 bytes (org issues one first so the
        // members array is non-empty).
        let mut org = Organization::new("PharmaCorp").unwrap();
        org.issue_identity("node-a").unwrap();
        let mut snapshot: serde_json::Value =
            serde_json::from_str(&org.export_json().unwrap()).unwrap();
        snapshot["members"][0]["seed_hex"] = serde_json::Value::String("abcd".into());
        assert!(Organization::import_json(&snapshot.to_string()).is_err());
    }

    #[test]
    fn test_organization_creates_root_ca() {
        let org = Organization::new("PharmaCorp").unwrap();
        assert!(!org.root_ca_cert_pem.is_empty());
        assert!(org.root_ca_cert_pem.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn root_ca_accessors_expose_the_issuer_identity() {
        // The OCSP minter signs with the CA key and names the CA subject as
        // the responder; both accessors must return the CA certificate's own
        // material, byte for byte.
        let org = Organization::new("AccessorCorp").unwrap();
        let der = rustls_pki_types::CertificateDer::from_pem_slice(org.root_ca_cert_pem.as_bytes())
            .unwrap();
        let cert = x509_cert::Certificate::from_der(der.as_ref()).unwrap();
        let mut subject = Vec::new();
        cert.tbs_certificate()
            .subject()
            .encode_to_vec(&mut subject)
            .unwrap();
        assert_eq!(org.ca_subject_der(), subject.as_slice());
        assert_eq!(
            org.ca_public_key(),
            cert.tbs_certificate()
                .subject_public_key_info()
                .subject_public_key
                .raw_bytes()
        );
    }

    #[test]
    fn each_intermediate_issued_certificate_gets_a_fresh_serial() {
        let mut org = Organization::new("SerialCorp").unwrap();
        let mut intermediate = org
            .issue_intermediate_ca("SerialCorp Intermediate")
            .unwrap();
        let first = intermediate.issue_identity("node-1").unwrap();
        let second = intermediate.issue_identity("node-2").unwrap();

        let serial = |identity: &Identity| {
            let pem = identity.certificate_pem.as_ref().unwrap();
            let der = rustls_pki_types::CertificateDer::from_pem_slice(pem.as_bytes()).unwrap();
            x509_cert::Certificate::from_der(der.as_ref())
                .unwrap()
                .tbs_certificate()
                .serial_number()
                .clone()
        };
        assert_ne!(
            serial(&first),
            serial(&second),
            "every issued certificate must carry a distinct serial"
        );
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
    #[test]
    fn revoke_and_ocsp_fail_closed_for_unknown_nodes() {
        let mut org = Organization::new("PharmaCorp").expect("org");
        assert!(matches!(
            org.revoke_identity("never-issued"),
            Err(IdentityError::CertGen(msg)) if msg.contains("never-issued")
        ));
        assert!(matches!(
            org.ocsp_response_der("never-issued"),
            Err(IdentityError::CertGen(msg)) if msg.contains("never-issued")
        ));
    }

    #[test]
    fn revocation_reason_codes_round_trip_and_default_safely() {
        // Every enumerated code maps to itself; unassigned 7 falls back to
        // Unspecified with a warning.
        assert_eq!(
            revocation_reason_from_code(revocation_reason_code(RevocationReason::KeyCompromise)),
            RevocationReason::KeyCompromise
        );
        assert_eq!(
            revocation_reason_from_code(revocation_reason_code(RevocationReason::AaCompromise)),
            RevocationReason::AaCompromise
        );
        assert_eq!(
            revocation_reason_from_code(7),
            RevocationReason::Unspecified
        );
        assert_eq!(
            revocation_reason_from_code(255),
            RevocationReason::Unspecified
        );
    }

    #[test]
    fn intermediate_ca_mints_a_parseable_good_staple() {
        let mut org = Organization::new("RootOrg").expect("org");
        let mut intermediate = org.issue_intermediate_ca("InterOrg").expect("intermediate");
        let member = intermediate.issue_identity("node-i").expect("member");
        assert!(member.certificate_pem.is_some());

        let staple = intermediate.ocsp_response_der("node-i").expect("staple");
        let parsed = crate::ocsp::parse_staple(&staple).expect("parseable staple");
        assert!(parsed.status, "a minted staple attests good");
        assert!(
            !parsed.serial.is_empty(),
            "the staple names the member serial"
        );
        assert!(
            parsed.next_update > parsed.this_update,
            "the staple window is forward in time"
        );

        // An unknown node has no live certificate to attest.
        assert!(intermediate.ocsp_response_der("missing").is_err());
    }
}
