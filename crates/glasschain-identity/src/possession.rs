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
    let Ok(der) = CertificateDer::from_pem_slice(cert_pem.as_bytes()) else {
        return false;
    };
    let Ok(cert) = Certificate::from_der(der.as_ref()) else {
        return false;
    };
    let Some(raw) = cert
        .tbs_certificate()
        .subject_public_key_info()
        .subject_public_key
        .as_bytes()
    else {
        return false;
    };
    let Ok(key) = <[u8; 32]>::try_from(raw) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&key) else {
        return false;
    };
    let Ok(signature) = <[u8; 64]>::try_from(proof) else {
        return false;
    };
    verifying_key
        .verify(
            &org_possession_message(org, node_id, binding),
            &Signature::from_bytes(&signature),
        )
        .is_ok()
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
