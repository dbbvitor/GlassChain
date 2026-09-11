//! Session-bound organization possession proofs (#110).
//!
//! A verified certificate proves issuance, not possession: the PEM bytes are
//! public and can be copied. A peer proves it holds the certificate's private
//! key by signing a domain-separated message over the **TLS session's
//! exported keying material** (RFC 5705), which both ends derive identically.
//! A captured proof therefore cannot replay on another session, and a copied
//! certificate without its key cannot produce one at all.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use x509_cert::{der::Decode, Certificate};

/// The TLS exporter label both peers derive the session binding under.
pub const SESSION_BINDING_LABEL: &[u8] = b"glasschain-org-possession";

/// Append a length-prefixed field (`u32` big-endian length).
fn push_field(message: &mut Vec<u8>, field: &[u8]) {
    #[allow(clippy::cast_possible_truncation)]
    let len = u32::try_from(field.len()).expect("possession field fits u32");
    message.extend_from_slice(&len.to_be_bytes());
    message.extend_from_slice(field);
}

/// The exact message a possession proof signs:
/// `domain || len(org) || org || len(node_id) || node_id || len(binding) || binding`.
///
/// The binding is this session's TLS exporter output, so the proof is valid
/// only on the connection it was produced on.
#[must_use]
pub fn org_possession_message(org: &str, node_id: &str, binding: &[u8]) -> Vec<u8> {
    let mut message = b"glasschain-org-possession:".to_vec();
    push_field(&mut message, org.as_bytes());
    push_field(&mut message, node_id.as_bytes());
    push_field(&mut message, binding);
    message
}

/// Verify `proof` under the ed25519 public key inside `cert_pem` over
/// [`org_possession_message`] for `(org, node_id, binding)`.
///
/// Any parse, key or signature failure is `false` — a missing, malformed or
/// mismatched proof never verifies (fail closed).
#[must_use]
pub fn verify_org_possession(
    cert_pem: &str,
    org: &str,
    node_id: &str,
    binding: &[u8],
    proof: &[u8],
) -> bool {
    let Some(key) = certificate_ed25519_public_key(cert_pem) else {
        return false;
    };
    verify_ed25519(&key, &org_possession_message(org, node_id, binding), proof)
}

/// The certificate subject's Organization name (the MSP principal for a
/// remote member), if present.
#[must_use]
pub fn certificate_organization(cert_pem: &str) -> Option<String> {
    use x509_cert::ext::pkix::name::DirectoryString;

    let der = CertificateDer::from_pem_slice(cert_pem.as_bytes()).ok()?;
    let cert = Certificate::from_der(der.as_ref()).ok()?;
    let value = cert
        .tbs_certificate()
        .subject()
        .organization()
        .ok()
        .flatten()?;
    match value {
        DirectoryString::Utf8String(text) => Some(text),
        DirectoryString::PrintableString(text) => Some(text.to_string()),
        _ => None,
    }
}

/// The raw 32-byte ed25519 public key inside a PEM certificate, if it parses
/// and carries an ed25519 SPKI. `None` on any parse or shape mismatch.
#[must_use]
pub fn certificate_ed25519_public_key(cert_pem: &str) -> Option<[u8; 32]> {
    let der = CertificateDer::from_pem_slice(cert_pem.as_bytes()).ok()?;
    let cert = Certificate::from_der(der.as_ref()).ok()?;
    let raw = cert
        .tbs_certificate()
        .subject_public_key_info()
        .subject_public_key
        .as_bytes()?;
    <[u8; 32]>::try_from(raw).ok()
}

/// Verify `proof` (a detached 64-byte ed25519 signature) over `message` under
/// `public_key`. Any malformed input is `false`.
#[must_use]
pub fn verify_ed25519(public_key: &[u8], message: &[u8], proof: &[u8]) -> bool {
    let Ok(key) = <[u8; 32]>::try_from(public_key) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&key) else {
        return false;
    };
    let Ok(signature) = <[u8; 64]>::try_from(proof) else {
        return false;
    };
    verifying_key
        .verify(message, &Signature::from_bytes(&signature))
        .is_ok()
}

/// The message a **TOFU pin rotation** proof signs (#88):
/// `domain || len(node_id) || node_id || len(tls_cert_fingerprint) ||
/// tls_cert_fingerprint`.
///
/// A peer's transport certificate may be re-issued (or regenerated on
/// restart) while its identity key stays the same. The pinned key signs this
/// message over the **new** transport fingerprint; the pin holder verifies it
/// under the key pinned at first contact, so only the holder of the original
/// identity key can re-key an address.
#[must_use]
pub fn tofu_pin_message(node_id: &str, tls_cert_fingerprint: &str) -> Vec<u8> {
    let mut message = b"glasschain-tofu-pin:".to_vec();
    push_field(&mut message, node_id.as_bytes());
    push_field(&mut message, tls_cert_fingerprint.as_bytes());
    message
}

/// The message an **MSP principal registration** proof signs (#87, D4):
/// `domain || len(org) || org || len(public_key) || public_key`.
///
/// Registration derives the principal from the certificate (chain + subject
/// CN) and requires this proof under the certificate's key, so no caller can
/// register a key it does not hold.
#[must_use]
pub fn msp_registration_message(org: &str, public_key: &[u8]) -> Vec<u8> {
    let mut message = b"glasschain-msp-registration:".to_vec();
    push_field(&mut message, org.as_bytes());
    push_field(&mut message, public_key);
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Organization;

    #[test]
    fn possession_proof_verifies_only_for_its_session_and_identity() {
        let mut org = Organization::new("PharmaCorp").expect("org");
        let identity = org.issue_identity("node-a").expect("identity").clone();
        let cert_pem = identity
            .certificate_pem
            .clone()
            .expect("issued identity carries a certificate");
        let binding = [7u8; 32];
        let proof = identity.sign_bytes(&org_possession_message("PharmaCorp", "node-a", &binding));

        assert!(
            verify_org_possession(&cert_pem, "PharmaCorp", "node-a", &binding, &proof),
            "the identity proves possession on its own session"
        );
        // A different session binding, org or node id does not verify.
        assert!(!verify_org_possession(
            &cert_pem,
            "PharmaCorp",
            "node-a",
            &[8u8; 32],
            &proof
        ));
        assert!(!verify_org_possession(
            &cert_pem,
            "OtherCorp",
            "node-a",
            &binding,
            &proof
        ));
        assert!(!verify_org_possession(
            &cert_pem,
            "PharmaCorp",
            "node-b",
            &binding,
            &proof
        ));

        // A copied certificate without its private key cannot impersonate:
        // another identity's proof does not verify under this certificate.
        let mut other_org = Organization::new("OtherCorp").expect("org");
        let impostor = other_org.issue_identity("node-a").expect("identity");
        let impostor_proof =
            impostor.sign_bytes(&org_possession_message("PharmaCorp", "node-a", &binding));
        assert!(
            !verify_org_possession(&cert_pem, "PharmaCorp", "node-a", &binding, &impostor_proof),
            "a proof from a different key must not verify"
        );
    }
}
