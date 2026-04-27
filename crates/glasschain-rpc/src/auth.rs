#![allow(clippy::module_name_repetitions)]
//! MSP (Membership Service Provider) authentication interceptor for `GlassChain` gRPC services.
//!
//! This module provides three cooperating types:
//!
//! - [`TrustedKeyRegistry`] — a thread-safe map of node IDs to their ed25519 verifying keys,
//!   populated at start-up from an [`Organization`](glasschain_identity::Organization) or
//!   manually.
//!
//! - [`MspAuthInterceptor`] — a [`tonic::service::Interceptor`] that validates the three
//!   `x-glasschain-*` metadata headers on every inbound RPC, enforcing that the caller is
//!   a known, non-replayed member of the trust domain.
//!
//! - [`AuthTokenBuilder`] — a client-side helper that builds the three headers from raw
//!   ed25519 key material, ready to be inserted into a tonic [`MetadataMap`](tonic::metadata::MetadataMap).
//!
//! ## Auth protocol
//!
//! Every authenticated RPC must carry three ASCII metadata headers:
//!
//! | Header | Value |
//! |--------|-------|
//! | `x-glasschain-node-id`  | The caller's node ID string |
//! | `x-glasschain-auth-ts`  | Current Unix timestamp as a decimal `u64` (seconds) |
//! | `x-glasschain-auth-sig` | Lowercase hex of the 64-byte ed25519 signature over `"{node_id}:{timestamp}"` |
//!
//! The timestamp window is **±60 seconds** to prevent replay attacks while tolerating
//! reasonable clock skew between nodes.
//!
//! ## Modes
//!
//! | Constructor | Behaviour when headers are absent |
//! |-------------|-----------------------------------|
//! | [`MspAuthInterceptor::new`]        | Passes the request through (backward-compatible) |
//! | [`MspAuthInterceptor::new_strict`] | Rejects the request with `UNAUTHENTICATED`        |

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// ── TrustedKeyRegistry ────────────────────────────────────────────────────────

/// Thread-safe registry of trusted node public keys.
///
/// The `node_id` → 32-byte ed25519 verifying-key mapping is populated at
/// startup from an [`Organization`](glasschain_identity::Organization)'s
/// member list (via [`register_from_org`](Self::register_from_org)) or
/// manually via [`register`](Self::register).
///
/// All methods are safe to call from multiple threads concurrently; internal
/// access is protected by an [`RwLock`](std::sync::RwLock).
#[derive(Debug, Clone, Default)]
pub struct TrustedKeyRegistry {
    keys: Arc<std::sync::RwLock<HashMap<String, [u8; 32]>>>,
}

impl TrustedKeyRegistry {
    /// Create a new, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a node's 32-byte ed25519 verifying key.
    ///
    /// If the node ID is already present the stored key is overwritten with
    /// `public_key_bytes`.
    ///
    /// # Panics
    ///
    /// Panics if the internal [`RwLock`](std::sync::RwLock) is poisoned
    /// (another thread panicked while holding a write guard).
    pub fn register(&self, node_id: impl Into<String>, public_key_bytes: [u8; 32]) {
        self.keys
            .write()
            .expect("TrustedKeyRegistry RwLock poisoned")
            .insert(node_id.into(), public_key_bytes);
    }

    /// Populate the registry from all members of an
    /// [`Organization`](glasschain_identity::Organization).
    ///
    /// Iterates over every member returned by
    /// [`Organization::member_ids`](glasschain_identity::Organization::member_ids)
    /// and registers each member's
    /// [`public_key_bytes`](glasschain_identity::Identity::public_key_bytes).
    /// Members whose identity cannot be retrieved are silently skipped.
    ///
    /// # Panics
    ///
    /// Panics if the internal [`RwLock`](std::sync::RwLock) is poisoned.
    pub fn register_from_org(&self, org: &glasschain_identity::Organization) {
        for node_id in org.member_ids() {
            if let Some(identity) = org.get_member(node_id) {
                self.register(node_id, identity.public_key_bytes());
            }
        }
    }

    /// Look up a node's 32-byte ed25519 verifying key.
    ///
    /// Returns `None` if the node is not registered.
    ///
    /// # Panics
    ///
    /// Panics if the internal [`RwLock`](std::sync::RwLock) is poisoned.
    #[must_use]
    pub fn get(&self, node_id: &str) -> Option<[u8; 32]> {
        self.keys
            .read()
            .expect("TrustedKeyRegistry RwLock poisoned")
            .get(node_id)
            .copied()
    }

    /// Return `true` if the node ID has a registered verifying key.
    ///
    /// # Panics
    ///
    /// Panics if the internal [`RwLock`](std::sync::RwLock) is poisoned.
    #[must_use]
    pub fn contains(&self, node_id: &str) -> bool {
        self.keys
            .read()
            .expect("TrustedKeyRegistry RwLock poisoned")
            .contains_key(node_id)
    }

    /// Return the number of registered keys.
    ///
    /// # Panics
    ///
    /// Panics if the internal [`RwLock`](std::sync::RwLock) is poisoned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys
            .read()
            .expect("TrustedKeyRegistry RwLock poisoned")
            .len()
    }

    /// Return `true` if no keys are registered.
    ///
    /// # Panics
    ///
    /// Panics if the internal [`RwLock`](std::sync::RwLock) is poisoned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys
            .read()
            .expect("TrustedKeyRegistry RwLock poisoned")
            .is_empty()
    }
}

// ── MspAuthInterceptor ────────────────────────────────────────────────────────

/// Tonic interceptor that enforces MSP (Membership Service Provider) authentication.
///
/// When `require_auth = true`, every inbound RPC must include the three
/// `x-glasschain-*` metadata headers.  When `require_auth = false` (the default),
/// headers are validated **if present** but their absence is permitted (backward
/// compatible mode).
///
/// # Construction
///
/// ```rust,ignore
/// use glasschain_rpc::auth::{MspAuthInterceptor, TrustedKeyRegistry};
///
/// let registry = TrustedKeyRegistry::new();
/// // populate registry …
///
/// // Permissive (absent headers pass through):
/// let interceptor = MspAuthInterceptor::new(registry.clone());
///
/// // Strict (absent headers are rejected):
/// let strict = MspAuthInterceptor::new_strict(registry);
/// ```
#[derive(Debug, Clone)]
pub struct MspAuthInterceptor {
    /// Registry of trusted node verifying keys used for signature verification.
    pub registry: TrustedKeyRegistry,
    /// When `true`, every inbound RPC must carry the three `x-glasschain-*` auth headers.
    pub require_auth: bool,
}

impl MspAuthInterceptor {
    /// Create a **permissive** interceptor (`require_auth = false`).
    ///
    /// Auth headers are validated when present; their absence is allowed.
    /// Use this for backward-compatible roll-outs where some callers have not
    /// yet been updated to attach credentials.
    #[must_use]
    pub const fn new(registry: TrustedKeyRegistry) -> Self {
        Self {
            registry,
            require_auth: false,
        }
    }

    /// Create a **strict** interceptor (`require_auth = true`).
    ///
    /// Every inbound RPC must carry all three `x-glasschain-*` headers and
    /// pass signature verification.  Requests that are missing headers or
    /// carry an invalid signature are rejected with
    /// [`Status::unauthenticated`](tonic::Status::unauthenticated).
    #[must_use]
    pub const fn new_strict(registry: TrustedKeyRegistry) -> Self {
        Self {
            registry,
            require_auth: true,
        }
    }

    /// Validate the MSP authentication headers carried in `metadata`.
    ///
    /// Verification steps:
    ///
    /// 1. If all three headers are absent and `require_auth = false` → `Ok(())`.
    /// 2. If any header is absent and `require_auth = true` → `Err(Unauthenticated)`.
    /// 3. Parse `x-glasschain-auth-ts` as a decimal `u64`; reject if malformed.
    /// 4. Reject requests whose timestamp falls outside the ±60 s replay-prevention window.
    /// 5. Hex-decode `x-glasschain-auth-sig` to exactly 64 bytes; reject if malformed.
    /// 6. Look up `x-glasschain-node-id` in the registry; reject if unknown.
    /// 7. Verify the ed25519 signature over `"{node_id}:{timestamp}"` bytes.
    fn verify_request(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<(), tonic::Status> {
        let node_id_mv = metadata.get("x-glasschain-node-id");
        let ts_mv = metadata.get("x-glasschain-auth-ts");
        let sig_mv = metadata.get("x-glasschain-auth-sig");

        let headers_present = node_id_mv.is_some() || ts_mv.is_some() || sig_mv.is_some();

        if !headers_present {
            return if self.require_auth {
                Err(tonic::Status::unauthenticated(
                    "missing x-glasschain auth headers",
                ))
            } else {
                Ok(())
            };
        }

        // All three headers must be present and valid ASCII.
        let node_id = node_id_mv
            .and_then(|mv| mv.to_str().ok())
            .ok_or_else(|| {
                tonic::Status::unauthenticated("missing or invalid x-glasschain-node-id")
            })?;

        let ts_str = ts_mv
            .and_then(|mv| mv.to_str().ok())
            .ok_or_else(|| {
                tonic::Status::unauthenticated("missing or invalid x-glasschain-auth-ts")
            })?;

        let sig_hex = sig_mv
            .and_then(|mv| mv.to_str().ok())
            .ok_or_else(|| {
                tonic::Status::unauthenticated("missing or invalid x-glasschain-auth-sig")
            })?;

        // Parse the timestamp as decimal Unix seconds.
        let ts: u64 = ts_str
            .parse()
            .map_err(|_| tonic::Status::unauthenticated("invalid timestamp format"))?;

        // Reject requests outside the ±60 s replay-prevention window.
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let skew = now_secs.abs_diff(ts);
        if skew > 60 {
            return Err(tonic::Status::unauthenticated("token expired"));
        }

        // Hex-decode the signature to exactly 64 bytes.
        let sig_bytes: [u8; 64] = hex::decode(sig_hex)
            .ok()
            .and_then(|decoded| decoded.try_into().ok())
            .ok_or_else(|| tonic::Status::unauthenticated("invalid signature encoding"))?;

        // Look up the node's verifying key.
        let pub_key_bytes = self
            .registry
            .get(node_id)
            .ok_or_else(|| tonic::Status::unauthenticated("unknown node"))?;

        // Reconstruct the verifying key and check the signature.
        let verifying_key = VerifyingKey::from_bytes(&pub_key_bytes)
            .map_err(|_| tonic::Status::unauthenticated("invalid public key in registry"))?;

        let signature = Signature::from_bytes(&sig_bytes);
        let challenge = format!("{node_id}:{ts_str}");

        verifying_key
            .verify(challenge.as_bytes(), &signature)
            .map_err(|_| tonic::Status::unauthenticated("signature verification failed"))?;

        Ok(())
    }
}

impl tonic::service::Interceptor for MspAuthInterceptor {
    fn call(
        &mut self,
        request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        self.verify_request(request.metadata())?;
        Ok(request)
    }
}

// ── AuthTokenBuilder ──────────────────────────────────────────────────────────

/// Builds the three MSP auth metadata headers for outbound gRPC calls.
///
/// Used on the **client** side to attach authentication credentials before
/// sending a request.  The caller provides the raw ed25519 key material
/// directly, avoiding any dependency on the full
/// [`Identity`](glasschain_identity::Identity) type.
///
/// # Example
///
/// ```rust,ignore
/// use glasschain_rpc::auth::AuthTokenBuilder;
/// use tonic::metadata::MetadataValue;
///
/// let headers = AuthTokenBuilder::build_headers(
///     &signing_key_seed,    // [u8; 32] — ed25519 signing key seed
///     &verifying_key_bytes, // [u8; 32] — corresponding public key
///     "node-1",
/// )?;
///
/// let mut req = tonic::Request::new(payload);
/// for (name, value) in &headers {
///     req.metadata_mut()
///        .insert(*name, value.parse().unwrap());
/// }
/// ```
pub struct AuthTokenBuilder;

impl AuthTokenBuilder {
    /// Produce the three `x-glasschain-*` headers for a single outbound RPC.
    ///
    /// Steps performed:
    ///
    /// 1. Validates `verifying_key_bytes` as a well-formed ed25519 public key.
    /// 2. Captures the current Unix timestamp (seconds).
    /// 3. Constructs the challenge string `"{node_id}:{timestamp}"`.
    /// 4. Signs the UTF-8 challenge bytes with `signing_key_bytes`.
    /// 5. Returns the three `(static header name, value)` pairs ready to be
    ///    inserted into a tonic [`MetadataMap`](tonic::metadata::MetadataMap).
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` if `verifying_key_bytes` do not represent a valid
    /// 32-byte ed25519 compressed point (i.e., they are not a valid public key).
    pub fn build_headers(
        signing_key_bytes: &[u8; 32],
        verifying_key_bytes: &[u8; 32],
        node_id: &str,
    ) -> Result<[(&'static str, String); 3], String> {
        // Eagerly validate the verifying key — catches mismatched key pairs early
        // before any network I/O is attempted.
        VerifyingKey::from_bytes(verifying_key_bytes)
            .map_err(|err| format!("invalid verifying key bytes: {err}"))?;

        let signing_key = SigningKey::from_bytes(signing_key_bytes);

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let challenge = format!("{node_id}:{ts}");
        let sig_bytes: [u8; 64] = signing_key.sign(challenge.as_bytes()).to_bytes();
        let sig_hex = hex::encode(sig_bytes);

        Ok([
            ("x-glasschain-node-id", node_id.to_string()),
            ("x-glasschain-auth-ts", ts.to_string()),
            ("x-glasschain-auth-sig", sig_hex),
        ])
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_secs()
    }

    /// Build a `MetadataMap` carrying all three auth headers.
    fn make_metadata(node_id: &str, ts: u64, sig_hex: &str) -> tonic::metadata::MetadataMap {
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert(
            "x-glasschain-node-id",
            node_id.parse().expect("valid header value"),
        );
        map.insert(
            "x-glasschain-auth-ts",
            ts.to_string().parse().expect("valid header value"),
        );
        map.insert(
            "x-glasschain-auth-sig",
            sig_hex.parse().expect("valid header value"),
        );
        map
    }

    /// Sign `"{node_id}:{ts}"` with the given 32-byte signing-key seed.
    fn sign_challenge(seed: &[u8; 32], node_id: &str, ts: u64) -> String {
        let signing_key = SigningKey::from_bytes(seed);
        let challenge = format!("{node_id}:{ts}");
        let sig_bytes: [u8; 64] = signing_key.sign(challenge.as_bytes()).to_bytes();
        hex::encode(sig_bytes)
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// Empty registry + permissive interceptor + no headers → request passes.
    #[test]
    fn test_empty_registry_no_auth_required() {
        let registry = TrustedKeyRegistry::new();
        let interceptor = MspAuthInterceptor::new(registry);

        let empty = tonic::metadata::MetadataMap::new();
        assert!(interceptor.verify_request(&empty).is_ok());
    }

    /// Empty registry + strict interceptor + no headers → `UNAUTHENTICATED`.
    #[test]
    fn test_empty_registry_auth_required_fails() {
        let registry = TrustedKeyRegistry::new();
        let interceptor = MspAuthInterceptor::new_strict(registry);

        let empty = tonic::metadata::MetadataMap::new();
        let result = interceptor.verify_request(&empty);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    /// Registering a key and looking it up returns the expected bytes.
    #[test]
    fn test_registry_register_and_lookup() {
        let registry = TrustedKeyRegistry::new();
        let key_bytes = [0x42u8; 32];

        registry.register("node-abc", key_bytes);

        assert!(registry.contains("node-abc"));
        assert!(!registry.contains("node-xyz"));
        assert_eq!(registry.get("node-abc"), Some(key_bytes));
        assert_eq!(registry.get("node-xyz"), None);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    /// A correctly signed, freshly timestamped request is accepted.
    #[test]
    fn test_valid_token_accepted() {
        let seed = [0xABu8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let pub_key_bytes = signing_key.verifying_key().to_bytes();

        let registry = TrustedKeyRegistry::new();
        registry.register("node-valid", pub_key_bytes);

        let interceptor = MspAuthInterceptor::new_strict(registry);

        let ts = now_secs();
        let sig_hex = sign_challenge(&seed, "node-valid", ts);
        let metadata = make_metadata("node-valid", ts, &sig_hex);

        assert!(interceptor.verify_request(&metadata).is_ok());
    }

    /// A signature produced by a different key is rejected, even if the
    /// timestamp and node-id are valid.
    #[test]
    fn test_invalid_signature_rejected() {
        let seed = [0xABu8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let pub_key_bytes = signing_key.verifying_key().to_bytes();

        // Attacker signs with a different key.
        let wrong_seed = [0xCDu8; 32];

        let registry = TrustedKeyRegistry::new();
        registry.register("node-test", pub_key_bytes);

        let interceptor = MspAuthInterceptor::new_strict(registry);

        let ts = now_secs();
        let sig_hex = sign_challenge(&wrong_seed, "node-test", ts);
        let metadata = make_metadata("node-test", ts, &sig_hex);

        let result = interceptor.verify_request(&metadata);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    /// A timestamp 120 seconds in the past lies outside the ±60 s window
    /// and must be rejected even when the signature is valid.
    #[test]
    fn test_expired_timestamp_rejected() {
        let seed = [0xABu8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let pub_key_bytes = signing_key.verifying_key().to_bytes();

        let registry = TrustedKeyRegistry::new();
        registry.register("node-test", pub_key_bytes);

        let interceptor = MspAuthInterceptor::new_strict(registry);

        // 120 seconds in the past — well outside the ±60 s replay window.
        let ts = now_secs().saturating_sub(120);
        let sig_hex = sign_challenge(&seed, "node-test", ts);
        let metadata = make_metadata("node-test", ts, &sig_hex);

        let result = interceptor.verify_request(&metadata);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    /// A node whose ID is not in the registry must be rejected, even if the
    /// signature is cryptographically valid.
    #[test]
    fn test_unknown_node_rejected() {
        let seed = [0xABu8; 32];

        // Registry is empty — no nodes are trusted.
        let registry = TrustedKeyRegistry::new();
        let interceptor = MspAuthInterceptor::new_strict(registry);

        let ts = now_secs();
        let sig_hex = sign_challenge(&seed, "node-unknown", ts);
        let metadata = make_metadata("node-unknown", ts, &sig_hex);

        let result = interceptor.verify_request(&metadata);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }
}
