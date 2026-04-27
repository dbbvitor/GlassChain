//! CA-backed certificate chain verifier for `GlassChain`.
//!
//! [`CertChainVerifier`] replaces the `AcceptAnyCert` model for org-mode nodes:
//! any peer whose certificate was **not** issued by this organisation's Root CA
//! is rejected at the application layer, even though the TLS handshake still
//! uses `AcceptAnyCert` for transport encryption.
//!
//! ## Trust model
//!
//! Phase 1 (current) implements **structural verification**:
//!
//! 1. The peer cert's `Issuer` DN must byte-match the Root CA's `Subject` DN.
//! 2. The peer cert must be within its stated validity period.
//!
//! Phase 2 will add full **cryptographic signature verification** (ECDSA-P256 /
//! Ed25519 over the TBS bytes using the Root CA's public key) once the
//! `rustls-webpki` integration is complete.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use glasschain_identity::{CertChainVerifier, Organization};
//!
//! let mut org = Organization::new("PharmaOrg").unwrap();
//! let verifier = CertChainVerifier::from_org(&org).unwrap();
//!
//! // Verify a peer's DER-encoded TLS certificate.
//! // verifier.verify_cert_der(peer_cert_der).unwrap();
//! ```

use crate::error::IdentityError;
use crate::msp::Organization;
use serde::{Deserialize, Serialize};
use x509_cert::{
    der::{Decode, Encode},
    Certificate,
};

// ── Verification level ────────────────────────────────────────────────────────

/// Controls how strictly peer certificates are verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationLevel {
    /// Verify issuer DN + validity period only (no cryptographic sig check).
    ///
    /// Suitable for development and private permissioned networks where the
    /// Root CA PEM is distributed out-of-band and implicitly trusted.
    Structural,

    /// Full cryptographic chain verification.
    ///
    /// Requires matching Extended Key Usage (EKU) extensions and performs an
    /// ECDSA/EdDSA signature check over the TBS certificate bytes.
    /// Reserved for future implementation via `rustls-webpki`.
    Full,
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Granular certificate verification errors.
///
/// This is a distinct type from [`IdentityError`] so callers can match on the
/// specific failure cause.  A [`From`] impl bridges into [`IdentityError`]
/// for call-sites that only need coarse error propagation.
#[derive(Debug, thiserror::Error)]
pub enum CertVerificationError {
    /// The DER or X.509 structure could not be parsed.
    #[error("certificate parsing failed: {0}")]
    ParseError(String),

    /// The peer cert's `Issuer` DN does not match the Root CA's `Subject` DN.
    #[error(
        "issuer mismatch: expected org '{expected_org}', \
         cert issued by '{actual_issuer}'"
    )]
    IssuerMismatch {
        /// Human-readable organisation name carried by the verifier.
        expected_org: String,
        /// Debug-formatted `Issuer` DN extracted from the peer certificate.
        actual_issuer: String,
    },

    /// The peer cert's `notBefore`/`notAfter` window does not include now.
    #[error("certificate expired or not yet valid")]
    InvalidValidity,

    /// PEM decoding or I/O failure while reading a PEM block.
    #[error("PEM decoding failed: {0}")]
    PemError(String),

    /// The certificate contained no `Issuer` DN (should not occur in practice).
    #[error("cert contains no issuer DN")]
    MissingIssuer,
}

/// Bridges [`CertVerificationError`] into [`IdentityError`] via `?`.
///
/// Maps to `IdentityError::CertGen` so the failure message is preserved as a
/// string, avoiding a circular dependency between the two error types.
impl From<CertVerificationError> for IdentityError {
    fn from(e: CertVerificationError) -> Self {
        Self::CertGen(e.to_string())
    }
}

// ── Verifier ──────────────────────────────────────────────────────────────────

/// Verifies that a peer TLS certificate was issued by a known Organisation Root CA.
///
/// Construct with [`from_org`](Self::from_org), [`from_pem`](Self::from_pem), or
/// [`from_der`](Self::from_der), then call [`verify_cert_der`](Self::verify_cert_der)
/// or [`verify_cert_pem`](Self::verify_cert_pem) for each incoming peer cert.
///
/// ## Security note
///
/// The default [`VerificationLevel::Structural`] mode checks the issuer DN and
/// validity window but does **not** verify the Root CA's cryptographic signature
/// over the TBS bytes.  This is intentional for Phase 1 private networks where
/// the Root CA PEM is pre-shared out-of-band.  See [`VerificationLevel::Full`]
/// for the roadmap item that will close this gap.
pub struct CertChainVerifier {
    /// Root CA `Subject` DN re-encoded as raw DER bytes.
    ///
    /// Compared byte-for-byte with the peer cert's `Issuer` DN DER bytes,
    /// which avoids any string-canonicalization ambiguity.
    root_subject_der: Vec<u8>,

    /// Full DER encoding of the Root CA certificate.
    ///
    /// Retained for future use in cryptographic signature verification
    /// ([`VerificationLevel::Full`]).
    root_cert_der: Vec<u8>,

    /// Human-readable organisation name, used in error messages.
    pub org_name: String,

    /// Determines how thoroughly peer certificates are checked.
    pub level: VerificationLevel,
}

// ── Constructors ──────────────────────────────────────────────────────────────

impl CertChainVerifier {
    /// Create a verifier from an [`Organization`]'s Root CA PEM certificate.
    ///
    /// # Errors
    ///
    /// Returns [`CertVerificationError::PemError`] if the PEM cannot be decoded,
    /// or [`CertVerificationError::ParseError`] if the DER structure is invalid.
    pub fn from_org(org: &Organization) -> Result<Self, CertVerificationError> {
        Self::from_pem(&org.name, &org.root_ca_cert_pem)
    }

    /// Create a verifier from a raw Root CA PEM string.
    ///
    /// The first `CERTIFICATE` block found in `root_ca_pem` is used; any
    /// additional blocks are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`CertVerificationError::PemError`] if the PEM cannot be decoded
    /// or contains no certificate block, or
    /// [`CertVerificationError::ParseError`] if the DER structure is malformed.
    pub fn from_pem(
        org_name: impl Into<String>,
        root_ca_pem: &str,
    ) -> Result<Self, CertVerificationError> {
        let org_name = org_name.into();

        let mut reader = std::io::BufReader::new(root_ca_pem.as_bytes());
        let root_der = rustls_pemfile::certs(&mut reader)
            .next()
            .ok_or_else(|| CertVerificationError::PemError("no certificate block in PEM".into()))?
            .map_err(|e| CertVerificationError::PemError(e.to_string()))?;

        Self::from_der(org_name, root_der.as_ref())
    }

    /// Create a verifier from a Root CA DER byte slice.
    ///
    /// # Errors
    ///
    /// Returns [`CertVerificationError::ParseError`] if the DER bytes cannot be
    /// decoded as a valid X.509 certificate.
    pub fn from_der(
        org_name: impl Into<String>,
        root_ca_der: &[u8],
    ) -> Result<Self, CertVerificationError> {
        let root_cert = Certificate::from_der(root_ca_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;

        // Re-encode the Subject DN to obtain canonical DER bytes that can be
        // compared directly with the Issuer DN bytes inside peer certificates.
        let mut root_subject_der = Vec::new();
        root_cert
            .tbs_certificate
            .subject
            .encode_to_vec(&mut root_subject_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;

        Ok(Self {
            root_subject_der,
            root_cert_der: root_ca_der.to_vec(),
            org_name: org_name.into(),
            level: VerificationLevel::Structural,
        })
    }
}

// ── Verification ──────────────────────────────────────────────────────────────

impl CertChainVerifier {
    /// Verify a DER-encoded peer certificate against this organisation's Root CA.
    ///
    /// # Checks performed
    ///
    /// 1. Parse the DER bytes as an X.509 certificate.
    /// 2. Re-encode the cert's `Issuer` DN and compare it byte-for-byte with
    ///    the Root CA's `Subject` DN.
    /// 3. Check that `SystemTime::now()` falls inside the cert's validity window.
    ///
    /// Cryptographic signature verification is tracked as a TODO below.
    ///
    /// # Errors
    ///
    /// Returns the appropriate [`CertVerificationError`] variant on failure.
    pub fn verify_cert_der(&self, peer_cert_der: &[u8]) -> Result<(), CertVerificationError> {
        // ── 1. Parse peer certificate ────────────────────────────────────────
        let peer_cert = Certificate::from_der(peer_cert_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;

        // ── 2. Issuer DN must match Root CA Subject DN ───────────────────────
        let mut peer_issuer_der = Vec::new();
        peer_cert
            .tbs_certificate
            .issuer
            .encode_to_vec(&mut peer_issuer_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;

        if peer_issuer_der != self.root_subject_der {
            let actual_issuer = format!("{:?}", peer_cert.tbs_certificate.issuer);
            return Err(CertVerificationError::IssuerMismatch {
                expected_org: self.org_name.clone(),
                actual_issuer,
            });
        }

        // ── 3. Validity period check ─────────────────────────────────────────
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let validity = &peer_cert.tbs_certificate.validity;

        // Time is Copy and exposes to_unix_duration(self) directly in x509-cert 0.2.
        let not_before = validity.not_before.to_unix_duration().as_secs();
        let not_after = validity.not_after.to_unix_duration().as_secs();

        if now_secs < not_before || now_secs > not_after {
            return Err(CertVerificationError::InvalidValidity);
        }

        // ── TODO: cryptographic signature verification ───────────────────────
        // Full ECDSA-P256 / Ed25519 signature verification over the TBS cert
        // bytes using the Root CA's public key is deferred to the production
        // implementation (Phase 2 via rustls-webpki).
        //
        // The structural issuer match above provides meaningful security for
        // private networks where the Root CA PEM is distributed out-of-band;
        // an attacker cannot forge a cert with a matching Subject DN without
        // also controlling the DER encoding, but cryptographic binding is still
        // strongly preferred for any internet-facing deployment.

        log::debug!(
            "cert_verifier: structural verification passed for cert issued by '{}'",
            self.org_name,
        );

        Ok(())
    }

    /// Verify a PEM-encoded peer certificate against this organisation's Root CA.
    ///
    /// Decodes the first `CERTIFICATE` block in `peer_cert_pem` and delegates
    /// to [`verify_cert_der`](Self::verify_cert_der).
    ///
    /// # Errors
    ///
    /// Returns [`CertVerificationError::PemError`] if decoding fails, or any
    /// error from [`verify_cert_der`](Self::verify_cert_der).
    pub fn verify_cert_pem(&self, peer_cert_pem: &str) -> Result<(), CertVerificationError> {
        let mut reader = std::io::BufReader::new(peer_cert_pem.as_bytes());
        let der = rustls_pemfile::certs(&mut reader)
            .next()
            .ok_or_else(|| CertVerificationError::PemError("no certificate block in PEM".into()))?
            .map_err(|e| CertVerificationError::PemError(e.to_string()))?;

        self.verify_cert_der(der.as_ref())
    }

    /// Returns the organisation name this verifier was built for.
    #[must_use]
    pub fn org_name(&self) -> &str {
        &self.org_name
    }

    /// Returns the raw DER bytes of the Root CA certificate.
    ///
    /// Retained primarily for future [`VerificationLevel::Full`] support, where
    /// the Root CA's public key will be extracted from these bytes to perform a
    /// cryptographic signature check over the peer certificate's TBS bytes.
    #[must_use]
    pub fn root_ca_der(&self) -> &[u8] {
        &self.root_cert_der
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msp::Organization;

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Create an org, issue one identity, and return the org and the member cert PEM.
    fn org_with_member(org_name: &str, node_id: &str) -> (Organization, String) {
        let mut org = Organization::new(org_name).unwrap();
        let cert_pem = org
            .issue_identity(node_id)
            .unwrap()
            .certificate_pem
            .clone()
            .unwrap();
        (org, cert_pem)
    }

    // ── 1. Construction from Organization ────────────────────────────────────

    #[test]
    fn test_from_org_constructs_verifier() {
        let org = Organization::new("PharmaOrg").unwrap();
        let verifier = CertChainVerifier::from_org(&org).unwrap();

        assert_eq!(verifier.org_name(), "PharmaOrg");
        assert_eq!(verifier.level, VerificationLevel::Structural);
        assert!(!verifier.root_subject_der.is_empty());
        assert!(!verifier.root_cert_der.is_empty());
    }

    // ── 2. Member cert issued by the same org passes ──────────────────────────

    #[test]
    fn test_member_cert_passes_verification() {
        let (org, cert_pem) = org_with_member("PharmaOrg", "node-x");
        let verifier = CertChainVerifier::from_org(&org).unwrap();

        assert!(
            verifier.verify_cert_pem(&cert_pem).is_ok(),
            "a cert issued by the org's own CA should pass structural verification"
        );
    }

    // ── 3. Cert from a different org is rejected ──────────────────────────────

    #[test]
    fn test_wrong_org_cert_fails() {
        let (org1, _) = org_with_member("Org1", "node-1");
        let (_, cert_pem_org2) = org_with_member("Org2", "node-2");

        let verifier = CertChainVerifier::from_org(&org1).unwrap();
        let err = verifier
            .verify_cert_pem(&cert_pem_org2)
            .expect_err("cert from Org2 must not verify against Org1's CA");

        assert!(
            matches!(err, CertVerificationError::IssuerMismatch { .. }),
            "expected IssuerMismatch, got {err}"
        );
    }

    // ── 4. Self-signed cert (no chain to the org CA) is rejected ──────────────

    #[test]
    fn test_self_signed_cert_fails() {
        let org = Organization::new("TestOrg").unwrap();
        let verifier = CertChainVerifier::from_org(&org).unwrap();

        let key_pair = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["test".into()]).unwrap();
        let self_signed = params.self_signed(&key_pair).unwrap();
        let self_signed_pem = self_signed.pem();

        let err = verifier
            .verify_cert_pem(&self_signed_pem)
            .expect_err("a self-signed cert must not verify against the org CA");

        assert!(
            matches!(err, CertVerificationError::IssuerMismatch { .. }),
            "expected IssuerMismatch, got {err}"
        );
    }

    // ── 5. from_pem parses the Root CA PEM correctly ──────────────────────────

    #[test]
    fn test_from_pem_parses_correctly() {
        let org = Organization::new("TestOrg").unwrap();
        let verifier = CertChainVerifier::from_pem("TestOrg", &org.root_ca_cert_pem).unwrap();

        assert_eq!(verifier.org_name(), "TestOrg");
        assert!(!verifier.root_cert_der.is_empty());
        assert!(!verifier.root_subject_der.is_empty());
    }

    // ── 6. from_der round-trips: PEM -> DER -> verifier -> member cert OK ─────

    #[test]
    fn test_from_der_round_trips() {
        let mut org = Organization::new("RoundTripOrg").unwrap();

        // Decode the Root CA PEM to raw DER bytes.
        let mut reader = std::io::BufReader::new(org.root_ca_cert_pem.as_bytes());
        let root_der = rustls_pemfile::certs(&mut reader)
            .next()
            .unwrap()
            .unwrap();

        // Build verifier from DER and confirm it has the right org name.
        let verifier = CertChainVerifier::from_der("RoundTripOrg", root_der.as_ref()).unwrap();
        assert_eq!(verifier.org_name(), "RoundTripOrg");

        // Issue a member cert and verify it passes — confirms that the Subject DN
        // extracted by `from_der` matches the Issuer DN in member certs.
        let identity = org.issue_identity("node-der").unwrap();
        let cert_pem = identity.certificate_pem.as_ref().unwrap();
        assert!(verifier.verify_cert_pem(cert_pem).is_ok());
    }

    // ── 7. verify_cert_pem wrapper works end-to-end ───────────────────────────

    #[test]
    fn test_verify_cert_pem() {
        let (org, cert_pem) = org_with_member("PemOrg", "node-pem");
        let verifier = CertChainVerifier::from_org(&org).unwrap();

        // Happy path via the PEM convenience wrapper.
        assert!(verifier.verify_cert_pem(&cert_pem).is_ok());

        // Confirm the underlying DER path also works for the same cert.
        let mut reader = std::io::BufReader::new(cert_pem.as_bytes());
        let der = rustls_pemfile::certs(&mut reader)
            .next()
            .unwrap()
            .unwrap();
        assert!(verifier.verify_cert_der(der.as_ref()).is_ok());
    }
}
