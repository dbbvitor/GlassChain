//! Issuer-signed OCSP responses stapled at session establishment (ADR-017).
//!
//! An organization's Root CA mints a signed status response for one member
//! certificate ([`Organization::ocsp_response_der`]); the member staples it on
//! its `Hello` (the message that carries the org certificate — the TLS
//! transport certificate is a self-signed transport-only cert, so there is no
//! CA-issued certificate inside the TLS handshake itself). The receiving node
//! verifies the staple **locally** against its trust store
//! ([`CertChainVerifier::verify_ocsp_staple`]) — no outbound responder
//! queries, no egress (ADR-017 decision 1). Absent, invalid or expired
//! staples fall back to the fail-closed CRL path (ADR-013); a staple
//! asserting `revoked` fails the session closed.
//!
//! Encoding: a minimal DER encoder/decoder over the OCSP subset this project
//! emits — `OCSPResponse { successful, responseBytes { basic,
//! BasicOCSPResponse } }` with one `SingleResponse` per mint, `responderID
//! byName`, `certStatus good` (`[0] IMPLICIT NULL`) or `revoked`
//! (`[1] IMPLICIT RevokedInfo`), SHA-256 certID hashes and
//! `ecdsa-with-SHA256` signatures (rcgen's default P-256 CA key).

use sha2::{Digest, Sha256};

/// Raw SHA-256 digest of `data` — the certID hash for OCSP.
fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// The `id-pkix-ocsp-basic` response type.
const OID_BASIC: &[u8] = &[0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x01];
/// `ecdsa-with-SHA256` signature algorithm.
const OID_ECDSA_WITH_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];
/// `id-sha256` digest algorithm for certID hashes.
const OID_SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];

/// How long a minted staple stays current (`nextUpdate`). Operators mint a
/// fresh one per session; verifiers treat an expired staple as absent
/// (CRL fallback, ADR-017).
pub const OCSP_VALIDITY_SECS: u64 = 60 * 60 * 6;

/// The outcome of staple verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcspStatus {
    /// The issuer attests the certificate is good and current.
    Good,
    /// The issuer attests the certificate was revoked — fail the session
    /// closed.
    Revoked,
}

/// Errors from parsing or verifying a stapled OCSP response.
///
/// Every variant leaves the caller where it started: the CRL path (already
/// fail-closed, ADR-013) is the only fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OcspError {
    /// The DER bytes are not the OCSP shape this project emits.
    #[error("malformed OCSP staple")]
    Malformed,
    /// The response's `producedAt` is too old or `nextUpdate` has passed.
    #[error("OCSP staple is expired")]
    Expired,
    /// The staple references a different certificate.
    #[error("OCSP staple does not match the presented certificate")]
    CertMismatch,
    /// The issuer's signature over the response does not verify.
    #[error("OCSP staple signature does not verify")]
    SignatureInvalid,
}

// ── Minimal DER ──────────────────────────────────────────────────────────────

/// DER length encoding for contents of `len` bytes.
fn write_len(out: &mut Vec<u8>, len: usize) {
    #[allow(clippy::cast_possible_truncation)]
    if len < 0x80 {
        out.push(len as u8);
    } else if len <= 0xFF {
        out.push(0x81);
        #[allow(clippy::cast_possible_truncation)]
        out.push(len as u8);
    } else {
        out.push(0x82);
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(len as u16).to_be_bytes());
    }
}

/// Encode one TLV element (tag + length + contents) into `out`.
fn write_tlv(out: &mut Vec<u8>, tag: u8, contents: &[u8]) {
    out.push(tag);
    write_len(out, contents.len());
    out.extend_from_slice(contents);
}

/// Encode a DER SEQUENCE whose body is `body` (tag `0x30`).
fn sequence(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    write_tlv(&mut out, 0x30, body);
    out
}

/// Encode a DER OBJECT IDENTIFIER from its decoded arc bytes.
fn oid(oid_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    write_tlv(&mut out, 0x06, oid_bytes);
    out
}

/// Encode a DER OCTET STRING.
fn octet_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    write_tlv(&mut out, 0x04, bytes);
    out
}

/// Encode a DER INTEGER (unsigned, big-endian, minimal).
fn integer(bytes: &[u8]) -> Vec<u8> {
    let first_nonzero = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
    let stripped = &bytes[first_nonzero..];
    let needs_pad = stripped.first().is_some_and(|b| b & 0x80 != 0);
    let contents = if needs_pad {
        let mut padded = vec![0u8];
        padded.extend_from_slice(stripped);
        padded
    } else if stripped.is_empty() {
        vec![0u8]
    } else {
        stripped.to_vec()
    };
    let mut out = Vec::new();
    write_tlv(&mut out, 0x02, &contents);
    out
}

/// Encode a DER ENUMERATED value (tag `0x0A`).
fn enumerated(value: u8) -> Vec<u8> {
    let mut out = Vec::new();
    write_tlv(&mut out, 0x0A, &[value]);
    out
}

/// Encode a DER BIT STRING over raw bits (`unused_bits` trailing, raw bytes).
fn bit_string(bytes: &[u8]) -> Vec<u8> {
    let mut contents = vec![0u8];
    contents.extend_from_slice(bytes);
    let mut out = Vec::new();
    write_tlv(&mut out, 0x03, &contents);
    out
}

/// Encode a DER `GeneralizedTime` `YYYYMMDDHHMMSSZ`.
fn generalized_time(unix_secs: u64) -> Vec<u8> {
    let secs = time::OffsetDateTime::from_unix_timestamp(unix_secs.cast_signed())
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    let text = format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}Z",
        secs.year(),
        i32::from(u8::from(secs.month())),
        secs.day(),
        secs.hour(),
        secs.minute(),
        secs.second()
    );
    let mut out = Vec::new();
    write_tlv(&mut out, 0x18, text.as_bytes());
    out
}

/// One decoded TLV element: `(tag, contents_range)` over a DER buffer.
struct Element<'a> {
    tag: u8,
    contents: &'a [u8],
}

/// Decode exactly one TLV element starting at `data[0..]`, returning it and
/// the byte offset just past it. Any truncation or non-minimal length is
/// malformed DER.
fn read_tlv(data: &[u8]) -> Result<(Element<'_>, usize), OcspError> {
    if data.len() < 2 {
        return Err(OcspError::Malformed);
    }
    let tag = data[0];
    let first = data[1] & 0x7F;
    let (len, header): (usize, usize) = if data[1] & 0x80 == 0 {
        (usize::from(first), 2)
    } else if first == 0 {
        return Err(OcspError::Malformed);
    } else if first == 1 {
        let Some(&len) = data.get(2) else {
            return Err(OcspError::Malformed);
        };
        if len < 0x80 {
            return Err(OcspError::Malformed);
        }
        (usize::from(len), 3)
    } else if first == 2 {
        let Some(bytes) = data.get(2..4) else {
            return Err(OcspError::Malformed);
        };
        let len = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
        if len < 0x100 {
            return Err(OcspError::Malformed);
        }
        (len, 4)
    } else {
        return Err(OcspError::Malformed);
    };
    let contents = data
        .get(header..header.checked_add(len).ok_or(OcspError::Malformed)?)
        .ok_or(OcspError::Malformed)?;
    Ok((Element { tag, contents }, header + len))
}

/// Decode the *contents* of a SEQUENCE body into its elements, consuming the
/// whole buffer. (Callers pass the body — the outer `0x30` TLV is unwrapped
/// by [`sequence_body`] where a full encoding is the input.)
fn read_sequence(data: &[u8]) -> Result<Vec<Element<'_>>, OcspError> {
    let mut elements = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let (element, end) = read_tlv(rest)?;
        elements.push(element);
        rest = &rest[end..];
    }
    Ok(elements)
}

/// Unwrap one full TLV element whose tag must be `0x30` (a SEQUENCE),
/// returning its contents. The whole buffer must be consumed.
fn sequence_body(data: &[u8]) -> Result<&[u8], OcspError> {
    let (element, end) = read_tlv(data)?;
    if element.tag != 0x30 || end != data.len() {
        return Err(OcspError::Malformed);
    }
    Ok(element.contents)
}

/// Decode the contents of an explicitly-tagged constructed element (tag
/// `0xA0 | index` — e.g. `[0]`).
const fn explicit_index(tag: u8) -> u8 {
    tag & 0x1F
}

// ── Minting ──────────────────────────────────────────────────────────────────

/// Everything the CA issuer needs to mint one member's staple.
pub(crate) struct OcspMintInput<'a> {
    /// The member certificate's serial, big-endian (rcgen's u64 serial).
    pub serial: [u8; 8],
    /// The issuing CA's Subject DN, full DER encoding.
    pub issuer_subject_der: &'a [u8],
    /// The issuing CA's raw public-key bytes (SPKI bit-string contents).
    pub issuer_public_key: &'a [u8],
    /// The issuing CA's PKCS#8 private-key DER (rcgen's P-256 key pair).
    pub issuer_pkcs8_der: &'a [u8],
    /// `producedAt`/`thisUpdate` — Unix seconds.
    pub now: u64,
    /// Staple freshness window in seconds (`nextUpdate - now`).
    pub validity_secs: u64,
}

/// Mint an OCSP `BasicResponse` attesting **good** for the certificate serial,
/// signed with the issuer's P-256 key over the DER-encoded `ResponseData`.
pub(crate) fn mint_good_response(input: &OcspMintInput<'_>) -> Result<Vec<u8>, OcspError> {
    let next_update = input.now + input.validity_secs;

    // certID: hashAlgorithm, issuerNameHash, issuerKeyHash, serialNumber.
    let mut cert_id = Vec::new();
    cert_id.extend(oid(OID_SHA256));
    cert_id.extend(octet_string(&sha256_bytes(input.issuer_subject_der)));
    cert_id.extend(octet_string(&sha256_bytes(input.issuer_public_key)));
    cert_id.extend(integer(&input.serial));

    // SingleResponse: certID, certStatus good ([0] IMPLICIT NULL),
    // thisUpdate, nextUpdate ([0] EXPLICIT).
    let mut single = Vec::new();
    single.extend(sequence(&cert_id));
    single.extend([0x80, 0x00]);
    single.extend(generalized_time(input.now));
    single.extend(write_explicit(0, &generalized_time(next_update)));

    // ResponseData: responderID byName ([0] EXPLICIT the issuer's Subject
    // DN), producedAt, responses (SEQUENCE OF SingleResponse).
    let mut response_data = Vec::new();
    response_data.extend(write_explicit(0, input.issuer_subject_der));
    response_data.extend(generalized_time(input.now));
    response_data.extend(sequence(&sequence(&single)));
    let tbs = sequence(&response_data);

    // Sign the DER-encoded ResponseData body with the issuer's key. The
    // verifier sees exactly this body (`ResponseData` contents), so the
    // signature covers the response without its own TLV header.
    let signature = sign_p256_sha256(input.issuer_pkcs8_der, &response_data)
        .map_err(|_| OcspError::SignatureInvalid)?;

    // BasicOCSPResponse: tbsResponseData, signatureAlgorithm, signature.
    let mut basic = Vec::new();
    basic.extend(&tbs);
    let mut sig_alg = Vec::new();
    sig_alg.extend(oid(OID_ECDSA_WITH_SHA256));
    sig_alg.extend([0x05, 0x00]);
    basic.extend(sequence(&sig_alg));
    basic.extend(bit_string(&signature));
    let basic = sequence(&basic);

    // OCSPResponse: responseStatus successful(0), responseBytes [0]
    // EXPLICIT { responseType id-pkix-ocsp-basic, response }.
    let mut response_bytes = Vec::new();
    response_bytes.extend(oid(OID_BASIC));
    response_bytes.extend(octet_string(&basic));
    let mut outer = Vec::new();
    outer.extend(enumerated(0));
    outer.extend(write_explicit(0, &sequence(&response_bytes)));
    Ok(sequence(&outer))
}

/// Encode an explicitly-tagged constructed element (`[n] EXPLICIT ...`).
fn write_explicit(index: u8, contents_der: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    write_tlv(&mut out, 0xA0 | (index & 0x1F), contents_der);
    out
}

/// Sign `message` with a PKCS#8 P-256 key, returning the fixed 64-byte
/// `r || s` signature (ECDSA-P256-SHA256, what OCSP's BIT STRING carries).
fn sign_p256_sha256(pkcs8_der: &[u8], message: &[u8]) -> Result<Vec<u8>, OcspError> {
    use ring::rand::SystemRandom;
    use ring::signature::EcdsaKeyPair;
    let key = EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8_der,
        &SystemRandom::new(),
    )
    .map_err(|_| OcspError::SignatureInvalid)?;
    let signature = key
        .sign(&SystemRandom::new(), message)
        .map_err(|_| OcspError::SignatureInvalid)?;
    Ok(signature.as_ref().to_vec())
}

// ── Verification ─────────────────────────────────────────────────────────────

/// The parts of a parsed staple verification needs.
pub(crate) struct ParsedStaple<'a> {
    /// The DER-encoded `ResponseData` the signature covers.
    pub(crate) tbs: &'a [u8],
    /// The raw `r || s` signature bytes.
    pub(crate) signature: Vec<u8>,
    /// The certID serial (big-endian bytes).
    pub(crate) serial: Vec<u8>,
    /// `thisUpdate` (Unix seconds).
    pub(crate) this_update: u64,
    /// `nextUpdate` (Unix seconds).
    pub(crate) next_update: u64,
    /// `certStatus`: `Some(true)` good, `Some(false)` revoked.
    pub(crate) status: bool,
}

/// Parse the minted subset of OCSP from `der_bytes`, checking the outer
/// envelope (status successful, basic response type) before the inner pieces.
pub(crate) fn parse_staple(der_bytes: &[u8]) -> Result<ParsedStaple<'_>, OcspError> {
    let outer = read_sequence(sequence_body(der_bytes)?)?;
    if outer.len() != 2 {
        return Err(OcspError::Malformed);
    }
    // responseStatus ENUMERATED(0).
    if outer[0].tag != 0x0A || outer[0].contents != [0u8] {
        return Err(OcspError::Malformed);
    }
    // responseBytes [0] EXPLICIT SEQUENCE { OID basic, OCTET STRING }.
    if outer[1].tag != 0xA0 || explicit_index(outer[1].tag) != 0 {
        return Err(OcspError::Malformed);
    }
    let bytes_seq = read_sequence(sequence_body(outer[1].contents)?)?;
    if bytes_seq.len() != 2
        || bytes_seq[0].tag != 0x06
        || bytes_seq[0].contents != OID_BASIC
        || bytes_seq[1].tag != 0x04
    {
        return Err(OcspError::Malformed);
    }
    let basic = read_sequence(sequence_body(bytes_seq[1].contents)?)?;
    if basic.len() < 3 || basic[0].tag != 0x30 || basic[1].tag != 0x30 || basic[2].tag != 0x03 {
        return Err(OcspError::Malformed);
    }
    let tbs = &basic[0].contents;
    // signatureAlgorithm: SEQUENCE { OID ecdsa-with-SHA256, NULL } — the
    // first inner element is the OID.
    let alg = read_tlv(basic[1].contents)?;
    if alg.0.tag != 0x06 || alg.0.contents != OID_ECDSA_WITH_SHA256 {
        return Err(OcspError::Malformed);
    }
    // BIT STRING contents: unused-bits octet + r||s.
    let sig_contents = basic[2].contents;
    if sig_contents.first() != Some(&0) || sig_contents.len() != 65 {
        return Err(OcspError::Malformed);
    }
    let signature = sig_contents[1..].to_vec();

    // ResponseData: responderID [0] EXPLICIT, producedAt, responses.
    let response_data = read_sequence(tbs)?;
    if response_data.len() != 3
        || response_data[0].tag != 0xA0
        || response_data[1].tag != 0x18
        || response_data[2].tag != 0x30
    {
        return Err(OcspError::Malformed);
    }
    let produced_at = read_generalized(response_data[1].contents)?;
    let _ = produced_at;

    // SingleResponse: certID SEQUENCE, certStatus, thisUpdate,
    // nextUpdate [0] EXPLICIT.
    let singles = read_sequence(response_data[2].contents)?;
    if singles.len() != 1 {
        return Err(OcspError::Malformed);
    }
    let single = read_sequence(singles[0].contents)?;
    if single.len() < 3 {
        return Err(OcspError::Malformed);
    }
    let cert_id = read_sequence(single[0].contents)?;
    if cert_id.len() != 4 || cert_id[3].tag != 0x02 {
        return Err(OcspError::Malformed);
    }
    let serial = cert_id[3].contents.to_vec();
    let status = match single[1].tag {
        0x80 => true,  // [0] IMPLICIT NULL — good
        0xA1 => false, // [1] IMPLICIT RevokedInfo
        _ => return Err(OcspError::Malformed),
    };
    if single[2].tag != 0x18 {
        return Err(OcspError::Malformed);
    }
    let this_update = read_generalized(single[2].contents)?;
    let next_update = if single.len() >= 4 && single[3].tag == 0xA0 {
        let inner = read_tlv(single[3].contents)?;
        if inner.0.tag != 0x18 {
            return Err(OcspError::Malformed);
        }
        read_generalized(inner.0.contents)?
    } else {
        this_update
    };

    Ok(ParsedStaple {
        tbs,
        signature,
        serial,
        this_update,
        next_update,
        status,
    })
}

/// Parse a `YYYYMMDDHHMMSSZ` `GeneralizedTime` body as Unix seconds.
fn read_generalized(body: &[u8]) -> Result<u64, OcspError> {
    let text = std::str::from_utf8(body).map_err(|_| OcspError::Malformed)?;
    let bytes = text.as_bytes();
    if bytes.len() != 15 || !bytes[..14].iter().all(u8::is_ascii_digit) || bytes[14] != b'Z' {
        return Err(OcspError::Malformed);
    }
    let digits: Vec<u32> = bytes[..14].iter().map(|b| u32::from(b - b'0')).collect();
    let (y, mo, d) = (
        digits[0] * 1000 + digits[1] * 100 + digits[2] * 10 + digits[3],
        digits[4] * 10 + digits[5],
        digits[6] * 10 + digits[7],
    );
    let (h, mi, s) = (
        digits[8] * 10 + digits[9],
        digits[10] * 10 + digits[11],
        digits[12] * 10 + digits[13],
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return Err(OcspError::Malformed);
    }
    // The ranges above prove every value fits its target type.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let month = time::Month::try_from(mo as u8).map_err(|_| OcspError::Malformed)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let date = time::Date::from_calendar_date(y as i32, month, d as u8)
        .map_err(|_| OcspError::Malformed)?;
    #[allow(clippy::cast_possible_truncation)]
    let daytime = date
        .with_hms(h as u8, mi as u8, s as u8)
        .map_err(|_| OcspError::Malformed)?;
    Ok(daytime.assume_utc().unix_timestamp().cast_unsigned())
}

/// Compare serials as minimal unsigned big-endian integers: leading zero
/// bytes stripped, all-zero → empty.
fn minimal_be(bytes: &[u8]) -> &[u8] {
    let stripped = &bytes[bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len())..];
    stripped
}

impl ParsedStaple<'_> {
    /// Verify the staple against one issuer's public key and the peer
    /// certificate's serial, then check freshness.
    pub(crate) fn verify_against(
        &self,
        issuer_public_key: &[u8],
        peer_serial: &[u8],
        now: u64,
    ) -> Result<OcspStatus, OcspError> {
        use ring::signature::UnparsedPublicKey;
        UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, issuer_public_key)
            .verify(self.tbs, &self.signature)
            .map_err(|_| OcspError::SignatureInvalid)?;
        if now < self.this_update || now > self.next_update {
            return Err(OcspError::Expired);
        }
        if minimal_be(&self.serial) != minimal_be(peer_serial) {
            return Err(OcspError::CertMismatch);
        }
        Ok(if self.status {
            OcspStatus::Good
        } else {
            OcspStatus::Revoked
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert_verifier::CertChainVerifier;
    use crate::{msp::test_support::admin_org_with_admin, Organization};

    /// Mint a fresh staple for a member through the org API and verify it
    /// round-trips as `Good` under a verifier built from the same org.
    #[test]
    fn minted_staple_verifies_good() {
        let mut org = Organization::new("PharmaCorp").expect("org");
        let identity = org.issue_identity("node-a").expect("identity").clone();
        let staple = org.ocsp_response_der("node-a").expect("mint");
        let verifier = CertChainVerifier::from_org(&org).expect("verifier");
        let status = verifier
            .verify_ocsp_staple(identity.certificate_pem.as_ref().expect("cert"), &staple)
            .expect("verify");
        assert_eq!(status, OcspStatus::Good);
    }

    /// A staple for a different certificate must not verify for this one.
    #[test]
    fn staple_for_other_certificate_is_a_mismatch() {
        let mut org = Organization::new("PharmaCorp").expect("org");
        let identity = org.issue_identity("node-a").expect("identity").clone();
        org.issue_identity("node-b").expect("identity");
        let other = org.ocsp_response_der("node-b").expect("mint");
        let verifier = CertChainVerifier::from_org(&org).expect("verifier");
        let error = verifier
            .verify_ocsp_staple(identity.certificate_pem.as_ref().expect("cert"), &other)
            .expect_err("mismatch");
        assert_eq!(error, OcspError::CertMismatch);
    }

    /// Flipping one signature byte invalidates the staple.
    #[test]
    fn tampered_staple_fails_signature() {
        let mut org = Organization::new("PharmaCorp").expect("org");
        let identity = org.issue_identity("node-a").expect("identity").clone();
        let mut staple = org.ocsp_response_der("node-a").expect("mint");
        let last = staple.len() - 1;
        staple[last] ^= 0xFF;
        let verifier = CertChainVerifier::from_org(&org).expect("verifier");
        let error = verifier
            .verify_ocsp_staple(identity.certificate_pem.as_ref().expect("cert"), &staple)
            .expect_err("tampered");
        assert_eq!(error, OcspError::SignatureInvalid);
    }

    /// A staple minted with a backdated `producedAt` (short validity) is
    /// rejected as expired.
    #[test]
    fn stale_staple_is_expired() {
        use rustls_pki_types::{pem::PemObject, CertificateDer};
        use x509_cert::{der::Decode, Certificate};

        let mut org = Organization::new("PharmaCorp").expect("org");
        let identity = org.issue_identity("node-a").expect("identity").clone();
        let cert_der = CertificateDer::from_pem_slice(
            identity.certificate_pem.as_ref().expect("cert").as_bytes(),
        )
        .expect("der");
        let cert = Certificate::from_der(cert_der.as_ref()).expect("cert");
        let serial = cert.tbs_certificate().serial_number();
        let mut serial_be = [0u8; 8];
        let raw = serial.as_bytes();
        serial_be[8 - raw.len()..].copy_from_slice(raw);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let stale = mint_good_response(&OcspMintInput {
            serial: serial_be,
            issuer_subject_der: org.ca_subject_der(),
            issuer_public_key: org.ca_public_key(),
            issuer_pkcs8_der: org.ca_key_pkcs8_der(),
            now: now - 3600,
            validity_secs: 1800,
        })
        .expect("mint");
        let verifier = CertChainVerifier::from_org(&org).expect("verifier");
        let error = verifier
            .verify_ocsp_staple(identity.certificate_pem.as_ref().expect("cert"), &stale)
            .expect_err("stale");
        assert_eq!(error, OcspError::Expired);
    }

    /// An unknown (never issued or revoked) node cannot get a staple, and an
    /// admin-issued certificate still verifies.
    #[test]
    fn minting_requires_a_live_member_and_admin_certs_verify() {
        let mut org = Organization::new("PharmaCorp").expect("org");
        assert!(org.ocsp_response_der("never-issued").is_err());

        let _identity = org.issue_identity("node-a").expect("identity").clone();
        org.revoke_identity("node-a").expect("revoke");
        assert!(
            org.ocsp_response_der("node-a").is_err(),
            "revoked members cannot be attested good"
        );

        let admin_identity = admin_org_with_admin(&mut org, "admin-node");
        let staple = org.ocsp_response_der("admin-node").expect("mint");
        let verifier = CertChainVerifier::from_org(&org).expect("verifier");
        let status = verifier
            .verify_ocsp_staple(
                admin_identity.certificate_pem.as_ref().expect("cert"),
                &staple,
            )
            .expect("admin staple verifies");
        assert_eq!(status, OcspStatus::Good);
    }

    /// Malformed bytes never verify.
    #[test]
    fn garbage_is_malformed() {
        let org = Organization::new("PharmaCorp").expect("org");
        let verifier = CertChainVerifier::from_org(&org).expect("verifier");
        let error = verifier
            .verify_ocsp_staple("not a pem", b"garbage")
            .expect_err("malformed");
        assert_eq!(error, OcspError::Malformed);
    }
}
