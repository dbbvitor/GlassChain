//! Wire-encoding helpers for signature-adjacent byte fields (#62 Step 1).
//!
//! `serde_json` renders `Vec<u8>` as an array of decimal numbers — a 96-byte
//! key + signature becomes ~393 bytes of `[12,34,255,…]`. The [`base64_bytes`]
//! module encodes those fields as base64 strings instead (~⅓ of that), and
//! [`SignatureAlgorithm`] is the algorithm discriminant the post-quantum plan
//! requires on every signature carrier: buy agility now, buy algorithms later.

use serde::{Deserialize, Serialize};

/// Serialize/deserialize a byte vector as a standard base64 string.
///
/// Apply with `#[serde(with = "wire::base64_bytes")]`. Note: this changes the
/// JSON form of the field — serialized content from before the change no
/// longer deserializes (acceptable pre-launch; recorded in #62 Step 1).
pub mod base64_bytes {
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine as _;
    use serde::{Deserialize, Serializer};

    /// Encode as a standard base64 string.
    ///
    /// # Errors
    ///
    /// Never fails; `Serialize`'s error type is satisfied vacuously.
    pub fn serialize<S: Serializer>(v: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64_STANDARD.encode(v))
    }

    /// Decode from a standard base64 string.
    ///
    /// # Errors
    ///
    /// Returns a deserialization error when the string is not valid base64.
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        BASE64_STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

/// The signature algorithm that produced a key/signature pair.
///
/// Post-quantum plan action 2: every signature carrier names its algorithm;
/// an unknown discriminant is a deserialization error, never silently treated
/// as ed25519.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SignatureAlgorithm {
    /// ed25519 (RFC 8032) — transaction signatures, endorsements, identity.
    #[default]
    Ed25519,
    /// BLS12-381 aggregate signatures (min-pubkey-size: G1 keys, G2 sigs) —
    /// quorum certificates only (ADR-014).
    Bls12381,
}

impl SignatureAlgorithm {
    /// `true` for the default algorithm. Fields carrying it are omitted on
    /// the wire (`skip_serializing_if`) and restored by `#[serde(default)]`
    /// — absent means ed25519, and any *written* discriminant must parse.
    // serde's `skip_serializing_if` calls this through a `&T`, so the copy
    // lint's by-value suggestion cannot apply here.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    #[must_use]
    pub const fn is_ed25519(&self) -> bool {
        matches!(self, Self::Ed25519)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Carrier {
        #[serde(with = "base64_bytes")]
        signature: Vec<u8>,
        #[serde(default)]
        algorithm: SignatureAlgorithm,
    }

    #[test]
    fn test_base64_field_round_trips() {
        let carrier = Carrier {
            signature: vec![0xDE, 0xAD, 0xBE, 0xEF],
            algorithm: SignatureAlgorithm::Ed25519,
        };
        let json = serde_json::to_string(&carrier).unwrap();
        // Base64 string, not a decimal array.
        assert!(json.contains("3q2+7w"), "{json}");
        assert!(!json.contains("[222,"), "{json}");
        let decoded: Carrier = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, carrier);
    }

    #[test]
    fn test_algorithm_field_defaults_for_legacy_json() {
        // Payloads from before the discriminant existed (no `algorithm` field)
        // still deserialize — the field defaults to Ed25519.
        let legacy = r#"{"signature":"3q2+7w=="}"#;
        let decoded: Carrier = serde_json::from_str(legacy).unwrap();
        assert_eq!(decoded.algorithm, SignatureAlgorithm::Ed25519);
    }

    #[test]
    fn test_unknown_algorithm_is_rejected_not_silently_ed25519() {
        // post-quantum.md §3 validation: an unknown discriminant must be a
        // deserialization error, never silently treated as ed25519.
        let future = r#"{"signature":"3q2+7w==","algorithm":"MlDsa"}"#;
        let result = serde_json::from_str::<Carrier>(future);
        assert!(result.is_err(), "unknown discriminant must be rejected");
    }
}
