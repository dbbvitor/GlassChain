//! CA-backed certificate chain verifier for `GlassChain`.
//!
//! [`CertChainVerifier`] replaces the `AcceptAnyCert` model for org-mode nodes:
//! any peer whose certificate was **not** issued by a trusted Root CA is
//! rejected at the application layer, even though the TLS handshake still
//! uses `AcceptAnyCert` for transport encryption.
//!
//! ## Trust model
//!
//! A verifier holds one **own-organization** anchor (the Root CA that issued
//! this node's identity) plus an optional set of **federation anchors** — the
//! Root CAs of other organizations the deployment has decided to trust, loaded
//! from configuration ([`add_federation_root_file`](Self::add_federation_root_file),
//! [`add_federation_root_pem`](Self::add_federation_root_pem)). A peer
//! certificate is accepted if it chains to *any* anchor. With no federation
//! anchors this is exactly the single-org model.
//!
//! [`VerificationLevel::Full`] — the default — performs:
//!
//! 1. The peer cert's `Issuer` DN must byte-match a trusted Root CA's `Subject` DN.
//! 2. The peer cert must be within its stated validity period.
//! 3. The matching Root CA's signature over the peer cert's TBS bytes must verify,
//!    anchoring the peer cert cryptographically to that organization's CA.
//!
//! Step 3 is what makes the check meaningful: a Distinguished Name is attacker
//! chosen data, so any party can mint a certificate whose `Issuer` DN matches
//! ours. Only the signature proves a trusted CA actually issued it.
//!
//! [`VerificationLevel::Structural`] drops step 3. It exists for tests and for
//! diagnosing DN-encoding mismatches — not as a deployment posture.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use glasschain_identity::{CertChainVerifier, Organization};
//!
//! let mut org = Organization::new("PharmaOrg").unwrap();
//! let mut verifier = verifier_with_crl(&org);
//!
//! // Optionally trust a federation peer's organization Root CA:
//! // verifier.add_federation_root_file("peer-root-ca.pem").unwrap();
//!
//! // Verify a peer's DER-encoded TLS certificate.
//! // verifier.verify_cert_der(peer_cert_der).unwrap();
//! ```

use crate::error::IdentityError;
use crate::msp::Organization;
use rustls_pki_types::{pem::PemObject, CertificateDer};
use serde::{Deserialize, Serialize};
use x509_cert::{
    der::{Decode, Encode},
    ext::pkix::name::DirectoryString,
    Certificate,
};

// ── Verification level ────────────────────────────────────────────────────────

/// Controls how strictly peer certificates are verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationLevel {
    /// Verify issuer DN + validity period only — **no cryptographic sig check**.
    ///
    /// This proves nothing about issuance: a Distinguished Name is not a secret,
    /// so anyone can self-sign a certificate that passes. Use only in tests, or
    /// to isolate a DN-encoding problem from a signature problem.
    Structural,

    /// Full cryptographic chain verification — the default.
    ///
    /// Everything [`Structural`](Self::Structural) does, plus an ECDSA/EdDSA
    /// signature check over the TBS certificate bytes using the Root CA's
    /// public key, performed by `rustls-webpki`.
    Full,
}

impl Default for VerificationLevel {
    /// Defaults to [`Full`](Self::Full) — secure by default.
    fn default() -> Self {
        Self::Full
    }
}

/// Signature algorithms accepted on a peer certificate.
///
/// Covers what `rcgen` can emit for a Root CA: it defaults to ECDSA-P256-SHA256,
/// and `GlassChain` identities use Ed25519. P-384 is included so an operator can
/// raise the CA's curve without a code change. Deliberately excludes RSA and
/// SHA-1: nothing in this workspace issues them, and narrowing the set narrows
/// the downgrade surface.
const SUPPORTED_SIG_ALGS: &[&dyn rustls_pki_types::SignatureVerificationAlgorithm] = &[
    webpki::ring::ECDSA_P256_SHA256,
    webpki::ring::ECDSA_P384_SHA384,
    webpki::ring::ED25519,
];

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

    /// The peer certificate's serial is on the issuing CA's CRL (ADR-013).
    #[error("certificate has been revoked")]
    Revoked,

    /// The issuing CA's CRL is missing, or an intermediate CA's is — revocation
    /// status could not be determined and the verifier fails closed (ADR-013).
    #[error("revocation status unknown: the issuing CA's CRL is missing from the trust store")]
    RevocationStatusUnknown,

    /// The issuing CA's CRL is present but past its `next_update` — the org
    /// broke its freshness cadence, so verification fails closed (ADR-013).
    #[error("CRL is expired: the issuing CA must publish a fresh one")]
    CrlExpired,

    /// No CRL was loaded at all. Verifiers require CRLs — fail closed.
    #[error("no CRL loaded: every trusted CA must have a current CRL in the trust store")]
    CrlMissing,

    /// The Root CA's signature over the peer certificate did not verify.
    ///
    /// Raised only at [`VerificationLevel::Full`]. A certificate can match the
    /// issuer DN and still fail here — that is precisely the forgery the
    /// structural check cannot detect.
    #[error("certificate chain verification failed: {0}")]
    SignatureInvalid(String),

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

/// One trusted Root CA: an own-organization anchor or a federation anchor.
struct TrustAnchor {
    /// Human-readable organization name, used in error/log messages.
    org_name: String,
    /// Full DER encoding of the Root CA certificate — the trust anchor for
    /// signature verification at [`VerificationLevel::Full`].
    cert_der: Vec<u8>,
}

/// Verifies that a peer TLS certificate was issued by a known Organisation Root CA.
///
/// Construct with [`from_org`](Self::from_org), [`from_pem`](Self::from_pem), or
/// [`from_der`](Self::from_der), then call [`verify_cert_der`](Self::verify_cert_der)
/// or [`verify_cert_pem`](Self::verify_cert_pem) for each incoming peer cert.
///
/// ## Security note
///
/// Defaults to [`VerificationLevel::Full`], which cryptographically anchors the
/// peer certificate to this organisation's Root CA. Lowering `level` to
/// [`VerificationLevel::Structural`] reduces the check to a Distinguished Name
/// comparison that any party can satisfy by self-signing; do that only in tests.
pub struct CertChainVerifier {
    /// Full DER encoding of the Root CA certificate.
    ///
    /// Used as the trust anchor for signature verification at
    /// [`VerificationLevel::Full`].
    root_cert_der: Vec<u8>,

    /// Human-readable organisation name, used in error messages.
    pub org_name: String,

    /// Determines how thoroughly peer certificates are checked.
    pub level: VerificationLevel,

    /// Root CAs of other organizations this deployment has explicitly decided
    /// to trust (ADR-011 federation trust store). A peer certificate chains
    /// against the own-organization anchor first, then these.
    federation_anchors: Vec<TrustAnchor>,

    /// Subordinate CA certificates from the trust store — not self-signed, so
    /// they are not trust anchors; webpki builds leaf → intermediate → anchor
    /// paths through them (ADR-013).
    intermediates: Vec<TrustAnchor>,

    /// CRLs loaded from the trust store (ADR-013). Verification fails closed:
    /// without a current CRL from the issuing CA, revocation status is unknown
    /// and the certificate is rejected (webpki `UnknownStatusPolicy::Deny`),
    /// and an expired CRL is an error too (`ExpirationPolicy::Enforce`).
    crls: Vec<webpki::CertRevocationList<'static>>,
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

        let root_der = CertificateDer::from_pem_slice(root_ca_pem.as_bytes())
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
        // Parse to validate the DER structure up front.
        Certificate::from_der(root_ca_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;

        Ok(Self {
            root_cert_der: root_ca_der.to_vec(),
            org_name: org_name.into(),
            level: VerificationLevel::default(),
            federation_anchors: Vec::new(),
            intermediates: Vec::new(),
            crls: Vec::new(),
        })
    }

    /// Override the verification level.
    ///
    /// Only useful for lowering to [`VerificationLevel::Structural`] in tests;
    /// [`VerificationLevel::Full`] is already the default.
    #[must_use]
    pub const fn with_level(mut self, level: VerificationLevel) -> Self {
        self.level = level;
        self
    }

    /// Add a federation Root CA from a PEM string (ADR-011).
    ///
    /// The first `CERTIFICATE` block found in `root_ca_pem` is used. The
    /// `org_label` is only used in error and log messages.
    ///
    /// # Errors
    ///
    /// Same as [`from_pem`](Self::from_pem).
    pub fn add_federation_root_pem(
        &mut self,
        org_label: impl Into<String>,
        root_ca_pem: &str,
    ) -> Result<(), CertVerificationError> {
        let org_label = org_label.into();
        let root_der = CertificateDer::from_pem_slice(root_ca_pem.as_bytes())
            .map_err(|e| CertVerificationError::PemError(e.to_string()))?;
        self.add_anchor(org_label, root_der.as_ref())?;
        Ok(())
    }

    /// Add federation Root CA(s) from a PEM file (ADR-011).
    ///
    /// Every `CERTIFICATE` block in the file becomes an anchor, so a bundle
    /// file holding several organizations' roots works as-is.
    ///
    /// # Errors
    ///
    /// Returns [`CertVerificationError::PemError`] if the file cannot be read
    /// or contains no valid `CERTIFICATE` block, or
    /// [`CertVerificationError::ParseError`] for malformed certificates.
    pub fn add_federation_root_file(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), CertVerificationError> {
        let path = path.as_ref();
        let label = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("federation-root")
            .to_owned();
        let mut added = 0;
        for cert in CertificateDer::pem_file_iter(path)
            .map_err(|e| CertVerificationError::PemError(e.to_string()))?
        {
            let cert = cert.map_err(|e| CertVerificationError::PemError(e.to_string()))?;
            self.add_anchor(format!("{label}#{added}"), cert.as_ref())?;
            added += 1;
        }
        if added == 0 {
            return Err(CertVerificationError::PemError(format!(
                "no CERTIFICATE block found in {}",
                path.display()
            )));
        }
        Ok(())
    }

    /// Build and append one trust-store CA from DER bytes: self-signed
    /// certificates are trust anchors, anything else is an intermediate that
    /// paths may be built through (ADR-013).
    fn add_anchor(
        &mut self,
        org_label: String,
        root_ca_der: &[u8],
    ) -> Result<(), CertVerificationError> {
        let parsed = Certificate::from_der(root_ca_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;
        let mut subject_der = Vec::new();
        parsed
            .tbs_certificate()
            .subject()
            .encode_to_vec(&mut subject_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;
        let mut issuer_der = Vec::new();
        parsed
            .tbs_certificate()
            .issuer()
            .encode_to_vec(&mut issuer_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;
        let entry = TrustAnchor {
            org_name: org_label,
            cert_der: root_ca_der.to_vec(),
        };
        if subject_der == issuer_der {
            self.federation_anchors.push(entry);
        } else {
            self.intermediates.push(entry);
        }
        Ok(())
    }

    /// Add a CRL from a PEM string (ADR-013). Verification fails closed when
    /// a peer's issuing CA has no current CRL here.
    ///
    /// # Errors
    ///
    /// Returns [`CertVerificationError::PemError`] if the PEM cannot be decoded,
    /// or [`CertVerificationError::ParseError`] if the CRL is malformed.
    pub fn add_crl_pem(&mut self, crl_pem: &str) -> Result<(), CertVerificationError> {
        let der =
            rustls_pki_types::CertificateRevocationListDer::from_pem_slice(crl_pem.as_bytes())
                .map_err(|e| CertVerificationError::PemError(e.to_string()))?;
        let crl = webpki::OwnedCertRevocationList::from_der(der.as_ref())
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;
        self.crls.push(crl.into());
        Ok(())
    }

    /// Add CRL(s) from a PEM file — every `X509 CRL` block becomes a loaded
    /// CRL (ADR-013).
    ///
    /// # Errors
    ///
    /// Returns [`CertVerificationError::PemError`] if the file cannot be read
    /// or contains no CRL block.
    pub fn add_crl_file(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), CertVerificationError> {
        let path = path.as_ref();
        let mut added = 0;
        for crl in rustls_pki_types::CertificateRevocationListDer::pem_file_iter(path)
            .map_err(|e| CertVerificationError::PemError(e.to_string()))?
        {
            let crl = crl.map_err(|e| CertVerificationError::PemError(e.to_string()))?;
            let crl = webpki::OwnedCertRevocationList::from_der(crl.as_ref())
                .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;
            self.crls.push(crl.into());
            added += 1;
        }
        if added == 0 {
            return Err(CertVerificationError::PemError(format!(
                "no X509 CRL block found in {}",
                path.display()
            )));
        }
        Ok(())
    }

    /// Number of CRLs loaded (ADR-013). Exposed so startup logging can report
    /// the trust store's revocation coverage.
    #[must_use]
    pub const fn crl_count(&self) -> usize {
        self.crls.len()
    }

    /// Number of federation anchors configured (excludes the own-organization
    /// anchor). Exposed so startup logging can report the trust store size.
    #[must_use]
    pub const fn federation_anchor_count(&self) -> usize {
        self.federation_anchors.len()
    }
}

// ── Verification ──────────────────────────────────────────────────────────────

/// Map a webpki path-building failure to the granular error type. Revocation
/// failures keep their identity: `Revoked` is the control working, the
/// unknown/expired variants are the fail-closed posture of ADR-013.
fn map_webpki_error(err: webpki::Error) -> CertVerificationError {
    match err {
        webpki::Error::CertRevoked => CertVerificationError::Revoked,
        webpki::Error::UnknownRevocationStatus => CertVerificationError::RevocationStatusUnknown,
        webpki::Error::CrlExpired { .. } => CertVerificationError::CrlExpired,
        webpki::Error::UnknownIssuer => CertVerificationError::IssuerMismatch {
            expected_org: String::new(),
            actual_issuer: "no path to a trust anchor".to_owned(),
        },
        other => CertVerificationError::SignatureInvalid(other.to_string()),
    }
}

impl CertChainVerifier {
    /// Verify a DER-encoded peer certificate against a trusted Root CA.
    ///
    /// # Checks performed
    ///
    /// 1. Parse the DER bytes as an X.509 certificate.
    /// 2. Re-encode the cert's `Issuer` DN and compare it byte-for-byte with
    ///    each trusted Root CA's `Subject` DN — the own-organization anchor
    ///    first, then the federation anchors (ADR-011), with intermediates
    ///    from the trust store available for path building (ADR-013).
    /// 3. Check that `SystemTime::now()` falls inside the cert's validity window.
    /// 4. At [`VerificationLevel::Full`], build and verify a full path to an
    ///    anchor — and check revocation: the issuing CA's CRL must be present
    ///    and current, or the certificate is rejected (fail-closed, ADR-013).
    ///
    /// Steps 2 and 3 run first so that a misconfigured peer produces a specific,
    /// actionable error rather than a generic chain-verification failure. Each
    /// anchor is tried in turn; the peer is accepted if any path verifies.
    ///
    /// # Errors
    ///
    /// Returns the appropriate [`CertVerificationError`] variant on failure.
    pub fn verify_cert_der(&self, peer_cert_der: &[u8]) -> Result<(), CertVerificationError> {
        // ── 1. Parse peer certificate ────────────────────────────────────────
        let peer_cert = Certificate::from_der(peer_cert_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;

        // ── 2. Validity period check ─────────────────────────────────────────
        Self::check_validity(&peer_cert)?;

        // ── 3. Structural mode stops here: no path build, no revocation.
        if self.level == VerificationLevel::Structural {
            log::debug!("cert_verifier: structural verification passed (no crypto, no CRL check)");
            return Ok(());
        }

        // ── 4. Revocation is fail-closed (ADR-013): no CRLs, no verification.
        if self.crls.is_empty() {
            return Err(CertVerificationError::CrlMissing);
        }

        // ── 5. Cryptographic path verification against each anchor ──────────
        let mut last_err: Option<CertVerificationError> = None;
        let own = TrustAnchor {
            org_name: self.org_name.clone(),
            cert_der: self.root_cert_der.clone(),
        };
        for anchor in std::iter::once(&own).chain(self.federation_anchors.iter()) {
            match self.verify_path(anchor, peer_cert_der) {
                Ok(()) => {
                    log::debug!(
                        "cert_verifier: {:?} verification passed for cert issued by '{}'",
                        self.level,
                        anchor.org_name,
                    );
                    return Ok(());
                }
                Err(e) => last_err = Some(e),
            }
        }

        Err(
            last_err.unwrap_or_else(|| CertVerificationError::IssuerMismatch {
                expected_org: self.org_name.clone(),
                actual_issuer: format!("{:?}", peer_cert.tbs_certificate().issuer()),
            }),
        )
    }

    /// Check that `now` falls inside the peer certificate's validity window.
    fn check_validity(peer_cert: &Certificate) -> Result<(), CertVerificationError> {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let validity = peer_cert.tbs_certificate().validity();

        // Time is Copy and exposes to_unix_duration(self) directly in x509-cert 0.3.
        let not_before = validity.not_before.to_unix_duration().as_secs();
        let not_after = validity.not_after.to_unix_duration().as_secs();

        if now_secs < not_before || now_secs > not_after {
            return Err(CertVerificationError::InvalidValidity);
        }
        Ok(())
    }

    /// Build and verify a certification path from `peer_cert_der` to `anchor`,
    /// possibly through intermediate CAs loaded from the trust store, with
    /// fail-closed revocation checking (ADR-013).
    ///
    /// Revocation semantics (all webpki, all fail-closed):
    /// - `UnknownStatusPolicy::Deny` — the issuing CA's CRL missing from the
    ///   trust store rejects the certificate;
    /// - `ExpirationPolicy::Enforce` — an expired CRL rejects;
    /// - depth `Chain` — intermediates are revocation-checked too.
    fn verify_path(
        &self,
        anchor: &TrustAnchor,
        peer_cert_der: &[u8],
    ) -> Result<(), CertVerificationError> {
        let root_der = rustls_pki_types::CertificateDer::from(anchor.cert_der.as_slice());
        let anchor = webpki::anchor_from_trusted_cert(&root_der)
            .map_err(|e| CertVerificationError::SignatureInvalid(e.to_string()))?;

        let peer_der = rustls_pki_types::CertificateDer::from(peer_cert_der);
        let end_entity = webpki::EndEntityCert::try_from(&peer_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;

        let intermediate_certs: Vec<rustls_pki_types::CertificateDer<'_>> = self
            .intermediates
            .iter()
            .map(|i| rustls_pki_types::CertificateDer::from(i.cert_der.as_slice()))
            .collect();

        let crls: Vec<&webpki::CertRevocationList<'static>> = self.crls.iter().collect();
        let revocation = webpki::RevocationOptionsBuilder::new(&crls)
            .map_err(|_| CertVerificationError::CrlMissing)?
            .with_expiration_policy(webpki::ExpirationPolicy::Enforce)
            .build();

        let now = rustls_pki_types::UnixTime::since_unix_epoch(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default(),
        );

        // `GlassChain` certificates carry no Extended Key Usage extension, and
        // RFC 5280 treats an absent EKU as unconstrained, so this argument does
        // not currently reject anything. It is `client_auth` rather than
        // `server_auth` because a peer dialling out is the case that must keep
        // working; if EKUs are ever added, they must include clientAuth.
        end_entity
            .verify_for_usage(
                SUPPORTED_SIG_ALGS,
                &[anchor],
                &intermediate_certs,
                now,
                webpki::KeyUsage::client_auth(),
                Some(revocation),
                None,
            )
            .map_err(map_webpki_error)?;
        Ok(())
    }

    /// Verify `peer_cert_der` against this organisation's Root CA and return
    /// the certificate subject's Common Name — the member identity stamped at
    /// issuance. This is the certificate-verified organization for the
    /// private-payload path (ticket #47): the self-asserted `Hello` org is
    /// trusted only when it matches this CN.
    ///
    /// # Errors
    ///
    /// Returns any [`CertVerificationError`] from the chain verification, or
    /// [`CertVerificationError::ParseError`] when the subject CN is absent or
    /// not UTF-8.
    pub fn verified_subject_cn(
        &self,
        peer_cert_der: &[u8],
    ) -> Result<String, CertVerificationError> {
        self.verify_cert_der(peer_cert_der)?;
        let peer_cert = Certificate::from_der(peer_cert_der)
            .map_err(|e| CertVerificationError::ParseError(e.to_string()))?;
        let cn = peer_cert
            .tbs_certificate()
            .subject()
            .common_name()
            .ok()
            .flatten()
            .and_then(|value| match value {
                DirectoryString::Utf8String(text) => Some(text),
                DirectoryString::PrintableString(text) => Some(text.to_string()),
                _ => None,
            })
            .ok_or_else(|| {
                CertVerificationError::ParseError("subject Common Name is absent".to_owned())
            })?;
        Ok(cn)
    }

    /// Verify a PEM-encoded peer certificate and return its subject CN — the
    /// PEM form of [`Self::verified_subject_cn`], for certificates carried on
    /// the wire (ticket #47).
    ///
    /// # Errors
    ///
    /// Returns [`CertVerificationError::PemError`] if decoding fails, or any
    /// error from [`Self::verified_subject_cn`].
    pub fn verified_subject_cn_pem(
        &self,
        peer_cert_pem: &str,
    ) -> Result<String, CertVerificationError> {
        let der = rustls_pki_types::CertificateDer::from_pem_slice(peer_cert_pem.as_bytes())
            .map_err(|e| CertVerificationError::PemError(e.to_string()))?;
        self.verified_subject_cn(der.as_ref())
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
        let der = CertificateDer::from_pem_slice(peer_cert_pem.as_bytes())
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
    /// This is the trust anchor used for signature verification at
    /// [`VerificationLevel::Full`], exposed so callers can redistribute it.
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

    /// Create two independent organisations that share a name, and therefore an
    /// byte-identical Root CA Distinguished Name, but hold unrelated CA keys.
    ///
    /// This is the forgery the structural check cannot see: an attacker picks
    /// the victim's org name, self-signs their own CA, and issues themselves a
    /// member certificate whose `Issuer` DN matches the real one exactly.
    fn impostor_pair(org_name: &str) -> (Organization, String) {
        let genuine = Organization::new(org_name).unwrap();
        let (_, impostor_cert_pem) = org_with_member(org_name, "impostor");
        (genuine, impostor_cert_pem)
    }

    /// A Full-level verifier for `org` with the org's (revocation-empty) CRL
    /// attached — the ADR-013 fail-closed default requires a current CRL.
    fn verifier_with_crl(org: &Organization) -> CertChainVerifier {
        let mut verifier = CertChainVerifier::from_org(org).unwrap();
        verifier.add_crl_pem(&org.crl_pem().unwrap()).unwrap();
        verifier
    }

    // ── 1. Construction from Organization ────────────────────────────────────

    #[test]
    fn test_from_org_constructs_verifier() {
        let org = Organization::new("PharmaOrg").unwrap();
        let verifier = verifier_with_crl(&org);

        assert_eq!(verifier.org_name(), "PharmaOrg");
        assert_eq!(
            verifier.level,
            VerificationLevel::Full,
            "verification must be cryptographic by default"
        );
        assert!(!verifier.root_cert_der.is_empty());
        assert!(!verifier.root_cert_der.is_empty());
    }

    // ── 2. Member cert issued by the same org passes ──────────────────────────

    #[test]
    fn test_member_cert_passes_verification() {
        let (org, cert_pem) = org_with_member("PharmaOrg", "node-x");
        let verifier = verifier_with_crl(&org);

        assert!(
            verifier.verify_cert_pem(&cert_pem).is_ok(),
            "a cert issued by the org's own CA should pass full verification"
        );
    }

    // ── 3. Cert from a different org is rejected ──────────────────────────────

    #[test]
    fn test_wrong_org_cert_fails() {
        let (org1, _) = org_with_member("Org1", "node-1");
        let (_, cert_pem_org2) = org_with_member("Org2", "node-2");

        let verifier = verifier_with_crl(&org1);
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
        let verifier = verifier_with_crl(&org);

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
        assert!(!verifier.root_cert_der.is_empty());
    }

    // ── 6. from_der round-trips: PEM -> DER -> verifier -> member cert OK ─────

    #[test]
    fn test_from_der_round_trips() {
        let mut org = Organization::new("RoundTripOrg").unwrap();

        // Decode the Root CA PEM to raw DER bytes.
        let root_der = CertificateDer::from_pem_slice(org.root_ca_cert_pem.as_bytes()).unwrap();

        // Build verifier from DER and confirm it has the right org name.
        let mut verifier = CertChainVerifier::from_der("RoundTripOrg", root_der.as_ref()).unwrap();
        verifier.add_crl_pem(&org.crl_pem().unwrap()).unwrap();
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
        let verifier = verifier_with_crl(&org);

        // Happy path via the PEM convenience wrapper.
        assert!(verifier.verify_cert_pem(&cert_pem).is_ok());

        // Confirm the underlying DER path also works for the same cert.
        let der = CertificateDer::from_pem_slice(cert_pem.as_bytes()).unwrap();
        assert!(verifier.verify_cert_der(der.as_ref()).is_ok());
    }

    // ── 8. A CA impersonating our DN is rejected ──────────────────────────────

    #[test]
    fn test_impostor_ca_with_identical_dn_is_rejected() {
        let (genuine, impostor_cert_pem) = impostor_pair("PharmaOrg");
        let verifier = verifier_with_crl(&genuine);

        let err = verifier
            .verify_cert_pem(&impostor_cert_pem)
            .expect_err("a cert from a foreign CA must be rejected even when the DN matches");

        assert!(
            matches!(err, CertVerificationError::SignatureInvalid(_)),
            "expected SignatureInvalid, got {err}"
        );
    }

    // ── 9. Structural mode is knowingly weaker ────────────────────────────────

    #[test]
    fn test_structural_level_accepts_impostor() {
        let (genuine, impostor_cert_pem) = impostor_pair("PharmaOrg");
        let verifier = verifier_with_crl(&genuine).with_level(VerificationLevel::Structural);

        // Documents the exact gap that VerificationLevel::Full closes. If this
        // ever starts failing, the two levels have stopped being distinguishable
        // and one of them is redundant.
        assert!(
            verifier.verify_cert_pem(&impostor_cert_pem).is_ok(),
            "structural verification compares DNs only, so the impostor passes"
        );
    }

    // ── 10. Bit-flipped signature is rejected ─────────────────────────────────

    // ── 11. Validity window excludes now ────────────────────────────────────

    #[test]
    fn test_expired_cert_is_rejected_invalid_validity() {
        let org = Organization::new("PharmaOrg").unwrap();
        let verifier = verifier_with_crl(&org);

        // Replicate the org Root CA's Subject DN so the Issuer DN byte-matches
        // (as `impostor_pair` does), but set a validity window that expired in
        // the past so step 3 of `verify_cert_der` fires before the signature
        // check does.
        let mut params = rcgen::CertificateParams::default();
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "PharmaOrg Root CA");
        dn.push(rcgen::DnType::OrganizationName, "PharmaOrg");
        params.distinguished_name = dn;
        params.not_after = rcgen::date_time_ymd(2000, 1, 1);

        let key_pair = rcgen::KeyPair::generate().unwrap();
        let expired_cert = params.self_signed(&key_pair).unwrap();

        let err = verifier
            .verify_cert_pem(&expired_cert.pem())
            .expect_err("a cert whose validity window excludes now must be rejected");

        assert!(
            matches!(err, CertVerificationError::InvalidValidity),
            "expected InvalidValidity, got {err}"
        );
    }

    // ── 12. verify_cert_pem rejects garbage PEM ──────────────────────────────

    #[test]
    fn test_verify_cert_pem_rejects_garbage_pem() {
        let (org, _) = org_with_member("PemOrg", "node-x");
        let verifier = verifier_with_crl(&org);

        let err = verifier
            .verify_cert_pem("not a pem")
            .expect_err("garbage PEM input must be rejected");

        assert!(
            matches!(err, CertVerificationError::PemError(_)),
            "expected PemError, got {err}"
        );
    }

    // ── 13. from_pem rejects PEM with no CERTIFICATE block ───────────────────

    #[test]
    fn test_from_pem_rejects_non_certificate_pem() {
        // A structurally valid PEM block, but labelled PRIVATE KEY rather than
        // CERTIFICATE: decoding must fail with PemError, not ParseError.
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let private_key_pem = key_pair.serialize_pem();

        let Err(err) = CertChainVerifier::from_pem("TestOrg", &private_key_pem) else {
            panic!("PEM containing no CERTIFICATE block must be rejected")
        };

        assert!(
            matches!(err, CertVerificationError::PemError(_)),
            "expected PemError, got {err}"
        );
    }

    // ── 14. from_der rejects malformed DER ───────────────────────────────────

    #[test]
    fn test_from_der_rejects_malformed_der() {
        let Err(err) = CertChainVerifier::from_der("TestOrg", &[0xFFu8; 32]) else {
            panic!("malformed DER bytes must be rejected")
        };

        assert!(
            matches!(err, CertVerificationError::ParseError(_)),
            "expected ParseError, got {err}"
        );
    }

    // ── 15. Federation anchors (ADR-011) ─────────────────────────────────────

    #[test]
    fn test_tampered_certificate_is_rejected() {
        let (org, cert_pem) = org_with_member("PharmaOrg", "node-tamper");
        let verifier = verifier_with_crl(&org);

        let der = CertificateDer::from_pem_slice(cert_pem.as_bytes()).unwrap();
        let mut tampered = der.as_ref().to_vec();

        // The signature BIT STRING terminates the certificate, so corrupting the
        // final byte invalidates the signature while leaving every DER length
        // prefix intact — the cert still parses, it just no longer verifies.
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;

        let err = verifier
            .verify_cert_der(&tampered)
            .expect_err("a tampered certificate must be rejected");

        assert!(
            matches!(err, CertVerificationError::SignatureInvalid(_)),
            "expected SignatureInvalid, got {err}"
        );
    }

    // ── 16. Revocation (ADR-013) ─────────────────────────────────────────────

    #[test]
    fn test_revoked_member_cert_is_rejected() {
        let (mut org, good_pem) = org_with_member("OrgR", "good-node");
        let revoked = org.issue_identity("bad-node").unwrap().clone();
        org.revoke_identity("bad-node").unwrap();

        let verifier = verifier_with_crl(&org);
        // An unrevoked member still passes...
        assert!(verifier.verify_cert_pem(&good_pem).is_ok());
        // ...and the revoked one is rejected — the control working.
        let err = verifier
            .verify_cert_pem(revoked.certificate_pem.as_ref().unwrap())
            .expect_err("a revoked certificate must be rejected");
        assert!(matches!(err, CertVerificationError::Revoked), "got {err}");
    }

    #[test]
    fn test_missing_crl_fails_closed() {
        let (org, cert_pem) = org_with_member("OrgNoCrl", "node-x");
        let verifier = CertChainVerifier::from_org(&org).unwrap();
        let err = verifier
            .verify_cert_pem(&cert_pem)
            .expect_err("verification without any CRL must fail closed");
        assert!(
            matches!(err, CertVerificationError::CrlMissing),
            "got {err}"
        );
    }

    #[test]
    fn test_expired_crl_fails_closed() {
        let (org, cert_pem) = org_with_member("OrgStale", "node-x");
        let mut verifier = CertChainVerifier::from_org(&org).unwrap();
        verifier
            .add_crl_pem(&org.crl_with_validity(60, 30).unwrap())
            .unwrap();
        let err = verifier
            .verify_cert_pem(&cert_pem)
            .expect_err("an expired CRL must fail closed");
        assert!(
            matches!(err, CertVerificationError::CrlExpired),
            "got {err}"
        );
    }

    #[test]
    fn test_intermediate_ca_chain_verifies_and_revokes() {
        let mut org = Organization::new("RootOrg").unwrap();
        let mut intermediate = org.issue_intermediate_ca("InterOrg").unwrap();
        let good = intermediate.issue_identity("node-i").unwrap();

        let mut verifier = verifier_with_crl(&org);
        // The intermediate cert is not self-signed → classified as an
        // intermediate, and webpki builds leaf → intermediate → root.
        verifier
            .add_federation_root_pem("InterOrg", intermediate.cert_pem())
            .unwrap();
        verifier
            .add_crl_pem(&intermediate.crl_pem().unwrap())
            .unwrap();

        assert!(verifier
            .verify_cert_pem(good.certificate_pem.as_ref().unwrap())
            .is_ok());

        // Revoking through the intermediate CA rejects the member.
        let bad = intermediate.issue_identity("node-bad").unwrap();
        intermediate.revoke_identity("node-bad").unwrap();
        let mut strict = verifier_with_crl(&org);
        strict
            .add_federation_root_pem("InterOrg", intermediate.cert_pem())
            .unwrap();
        strict
            .add_crl_pem(&intermediate.crl_pem().unwrap())
            .unwrap();
        let err = strict
            .verify_cert_pem(bad.certificate_pem.as_ref().unwrap())
            .expect_err("a revoked intermediate-issued cert must be rejected");
        assert!(matches!(err, CertVerificationError::Revoked), "got {err}");
    }

    #[test]
    fn test_federation_anchor_accepts_peer_from_trusted_org() {
        let (own, _) = org_with_member("OrgOwn", "own-node");
        let (peer_org, peer_cert_pem) = org_with_member("OrgPeer", "peer-node");

        let mut verifier = verifier_with_crl(&own);
        assert_eq!(verifier.federation_anchor_count(), 0);
        verifier
            .add_federation_root_pem("OrgPeer", &peer_org.root_ca_cert_pem)
            .unwrap();
        // The peer cert's issuer needs a current CRL too (ADR-013 fail-closed).
        verifier.add_crl_pem(&peer_org.crl_pem().unwrap()).unwrap();
        assert_eq!(verifier.federation_anchor_count(), 1);

        assert!(verifier.verify_cert_pem(&peer_cert_pem).is_ok());
        assert_eq!(
            verifier.verified_subject_cn_pem(&peer_cert_pem).unwrap(),
            "peer-node"
        );
    }

    #[test]
    fn test_own_org_member_still_verified_with_federation_anchors() {
        let (own, own_member_pem) = org_with_member("OrgOwn", "own-node");
        let (peer_org, _) = org_with_member("OrgPeer", "peer-node");

        let mut verifier = verifier_with_crl(&own);
        verifier
            .add_federation_root_pem("OrgPeer", &peer_org.root_ca_cert_pem)
            .unwrap();

        assert!(verifier.verify_cert_pem(&own_member_pem).is_ok());
    }

    #[test]
    fn test_untrusted_org_still_rejected_with_federation_anchors() {
        let (own, _) = org_with_member("OrgOwn", "own-node");
        let (trusted_peer, _) = org_with_member("OrgPeer", "peer-node");
        let (_, untrusted_pem) = org_with_member("OrgStranger", "stranger");

        let mut verifier = verifier_with_crl(&own);
        verifier
            .add_federation_root_pem("OrgPeer", &trusted_peer.root_ca_cert_pem)
            .unwrap();
        verifier
            .add_crl_pem(&trusted_peer.crl_pem().unwrap())
            .unwrap();

        let err = verifier
            .verify_cert_pem(&untrusted_pem)
            .expect_err("an org outside the trust store must be rejected");
        assert!(
            matches!(err, CertVerificationError::IssuerMismatch { .. }),
            "expected IssuerMismatch, got {err}"
        );
    }

    #[test]
    fn test_federation_root_file_loads_bundle() {
        let (own, _) = org_with_member("OrgOwn", "own-node");
        let (org_a, member_a) = org_with_member("OrgA", "node-a");
        let (org_b, member_b) = org_with_member("OrgB", "node-b");

        // A bundle file holding two organizations' roots.
        let bundle = format!("{}\n{}", org_a.root_ca_cert_pem, org_b.root_ca_cert_pem);
        let path = std::env::temp_dir().join(format!(
            "glasschain-trust-store-test-{}.pem",
            std::process::id()
        ));
        std::fs::write(&path, &bundle).unwrap();

        let mut verifier = verifier_with_crl(&own);
        verifier.add_federation_root_file(&path).unwrap();
        // The bundle members' issuers need current CRLs too (ADR-013).
        verifier.add_crl_pem(&org_a.crl_pem().unwrap()).unwrap();
        verifier.add_crl_pem(&org_b.crl_pem().unwrap()).unwrap();
        assert_eq!(verifier.federation_anchor_count(), 2);
        assert!(verifier.verify_cert_pem(&member_a).is_ok());
        assert!(verifier.verify_cert_pem(&member_b).is_ok());
        std::fs::remove_file(&path).ok();
    }
}
