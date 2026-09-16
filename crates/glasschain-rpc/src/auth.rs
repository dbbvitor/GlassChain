// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
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

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use glasschain_identity::CertChainVerifier;
use rustls_pki_types::pem::PemObject as _;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Metadata header carrying the caller's base64 DER organization certificate
/// (ADR-017 admin authorization).
///
/// The certificate must verify against this node's trust store, name the
/// calling node in its subject CN, and carry the admin Organizational Unit.
pub const CERT_HEADER: &str = "x-glasschain-cert";

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
    fn verify_request(&self, metadata: &tonic::metadata::MetadataMap) -> Result<(), tonic::Status> {
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
        let node_id = node_id_mv.and_then(|mv| mv.to_str().ok()).ok_or_else(|| {
            tonic::Status::unauthenticated("missing or invalid x-glasschain-node-id")
        })?;

        let ts_str = ts_mv.and_then(|mv| mv.to_str().ok()).ok_or_else(|| {
            tonic::Status::unauthenticated("missing or invalid x-glasschain-auth-ts")
        })?;

        let sig_hex = sig_mv.and_then(|mv| mv.to_str().ok()).ok_or_else(|| {
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
    fn call(&mut self, request: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        self.verify_request(request.metadata())?;
        Ok(request)
    }
}

// ── AdminGate (ADR-017 operator RBAC) ────────────────────────────────────────

/// Certificate-bound operator authorization (ADR-017): a caller is an
/// **operator administrator** only when every one of these holds:
///
/// 1. the three `x-glasschain-*` headers are present and well-formed,
/// 2. a base64 DER certificate rides `x-glasschain-cert`,
/// 3. the certificate verifies against this node's trust store — chain,
///    validity, CRL, all fail-closed (ADR-011/ADR-013),
/// 4. the certificate's subject CN equals `x-glasschain-node-id`,
/// 5. the header signature verifies under the **certificate's own key** over
///    `{node_id}:{timestamp}` — possession, bound to the replay window,
/// 6. the verified subject carries the admin Organizational Unit
///    (`glasschain_identity::ADMIN_ROLE`).
///
/// Without a configured verifier the gate refuses everything: there is no
/// trust basis to admit an administrator.
///
/// The verifier sits behind a shared lock so an operator can hot-reload the
/// trust store (`reload-trust-store` REPL command, ADR-017 residual plan
/// ZT-R3): every clone of the gate sees the swap.
#[derive(Clone)]
pub struct AdminGate {
    verifier: Arc<std::sync::RwLock<Arc<CertChainVerifier>>>,
    /// The ±seconds replay window around now for the header timestamp.
    skew_secs: u64,
}

impl std::fmt::Debug for AdminGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminGate").finish_non_exhaustive()
    }
}

impl AdminGate {
    /// Build a gate that admits only certs issued under `verifier`'s trust
    /// store carrying the admin role.
    #[must_use]
    pub fn new(verifier: Arc<CertChainVerifier>) -> Self {
        Self {
            verifier: Arc::new(std::sync::RwLock::new(verifier)),
            skew_secs: 60,
        }
    }

    /// Hot-swap the trust store this gate verifies against (ZT-R3): the next
    /// authorization sees the new chain/CRLs.
    ///
    /// # Panics
    ///
    /// Panics if the internal verifier lock was poisoned (a panic while
    /// another thread held the write guard).
    pub fn update_verifier(&self, verifier: Arc<CertChainVerifier>) {
        *self
            .verifier
            .write()
            .expect("AdminGate verifier lock poisoned") = verifier;
    }

    /// Authorize `metadata` as an admin principal. Returns the verified
    /// subject CN (the certificate-bound node id).
    ///
    /// # Errors
    ///
    /// [`tonic::Status::permission_denied`] for every failure: missing or
    /// malformed headers, an unverifiable certificate, a CN/node-id mismatch,
    /// a possession failure, a non-admin role, or an expired timestamp.
    ///
    /// # Panics
    ///
    /// Panics if the internal verifier lock was poisoned.
    pub fn authorize(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<String, tonic::Status> {
        let node_id = metadata
            .get("x-glasschain-node-id")
            .and_then(|mv| mv.to_str().ok())
            .ok_or_else(|| tonic::Status::permission_denied("missing x-glasschain-node-id"))?;
        let ts_str = metadata
            .get("x-glasschain-auth-ts")
            .and_then(|mv| mv.to_str().ok())
            .ok_or_else(|| tonic::Status::permission_denied("missing x-glasschain-auth-ts"))?;
        let sig_hex = metadata
            .get("x-glasschain-auth-sig")
            .and_then(|mv| mv.to_str().ok())
            .ok_or_else(|| tonic::Status::permission_denied("missing x-glasschain-auth-sig"))?;
        let cert_b64 = metadata
            .get(CERT_HEADER)
            .and_then(|mv| mv.to_str().ok())
            .ok_or_else(|| {
                tonic::Status::permission_denied(format!(
                    "missing {CERT_HEADER}: an admin operation requires an organization certificate"
                ))
            })?;

        let ts: u64 = ts_str
            .parse()
            .map_err(|_| tonic::Status::permission_denied("invalid timestamp format"))?;
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now_secs.abs_diff(ts) > self.skew_secs {
            return Err(tonic::Status::permission_denied("token expired"));
        }

        let cert_der = use_base64::decode(cert_b64)
            .map_err(|_| tonic::Status::permission_denied("certificate is not valid base64"))?;
        // The shared verifier snapshot: hot-reload (ZT-R3) swaps behind this
        // lock and every clone of the gate sees the new chain/CRLs.
        let verifier = self
            .verifier
            .read()
            .expect("AdminGate verifier lock poisoned")
            .clone();
        // Chain, validity, CRL — fail closed (ADR-011/ADR-013).
        verifier.verify_cert_der(&cert_der).map_err(|e| {
            tonic::Status::permission_denied(format!("certificate verification failed: {e}"))
        })?;
        // Subject CN must name the calling node.
        let cn = verifier.verified_subject_cn(&cert_der).map_err(|e| {
            tonic::Status::permission_denied(format!("certificate subject CN missing: {e}"))
        })?;
        if cn != node_id {
            return Err(tonic::Status::permission_denied(format!(
                "certificate CN '{cn}' does not match node id '{node_id}'"
            )));
        }
        // Possession: the header signature verifies under the certificate's
        // own key over `{node_id}:{timestamp}`.
        let public_key = glasschain_identity::certificate_ed25519_public_key_der(&cert_der)
            .ok_or_else(|| {
                tonic::Status::permission_denied("certificate carries no ed25519 public key")
            })?;
        let sig_bytes: [u8; 64] = hex::decode(sig_hex)
            .ok()
            .and_then(|decoded| decoded.try_into().ok())
            .ok_or_else(|| tonic::Status::permission_denied("invalid signature encoding"))?;
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| tonic::Status::permission_denied("invalid certificate public key"))?;
        let challenge = format!("{node_id}:{ts_str}");
        verifying_key
            .verify(challenge.as_bytes(), &Signature::from_bytes(&sig_bytes))
            .map_err(|_| tonic::Status::permission_denied("signature verification failed"))?;
        // The certificate-bound admin role (ADR-017).
        if glasschain_identity::certificate_admin_role_der(&cert_der).as_deref()
            != Some(glasschain_identity::ADMIN_ROLE)
        {
            return Err(tonic::Status::permission_denied(format!(
                "certificate does not carry the admin role (OU={} required)",
                glasschain_identity::ADMIN_ROLE
            )));
        }
        Ok(cn)
    }
}

/// Local alias so the gate's base64 dependency is explicit.
mod use_base64 {
    pub use base64::engine::general_purpose::STANDARD as Engine;
    pub use base64::Engine as _;

    /// Decode a base64 string to bytes.
    pub fn decode(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
        Engine.decode(input)
    }
}

/// Build the four ADR-017 admin headers from a certificate PEM and its
/// 32-byte ed25519 seed — the client side of [`AdminGate`].
///
/// The caller's node id is the certificate's verified subject CN, so a
/// stolen certificate cannot name a different node.
///
/// # Errors
///
/// Returns `Err` for an unparseable certificate, a certificate without a
/// subject CN, or an invalid seed.
pub fn admin_headers_from_cert(
    cert_pem: &str,
    seed: &[u8; 32],
) -> Result<[(&'static str, String); 4], String> {
    let node_id = glasschain_identity::certificate_subject_cn(cert_pem)
        .ok_or_else(|| "certificate carries no subject CN".to_owned())?;
    let signing_key = SigningKey::from_bytes(seed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let challenge = format!("{node_id}:{ts}");
    let sig_hex = hex::encode(signing_key.sign(challenge.as_bytes()).to_bytes());
    let cert_der: rustls_pki_types::CertificateDer<'static> =
        rustls_pki_types::CertificateDer::from_pem_slice(cert_pem.as_bytes())
            .map_err(|e| format!("certificate is not PEM: {e}"))?;
    Ok([
        ("x-glasschain-node-id", node_id),
        ("x-glasschain-auth-ts", ts.to_string()),
        ("x-glasschain-auth-sig", sig_hex),
        (
            CERT_HEADER,
            base64::engine::general_purpose::STANDARD.encode(cert_der.as_ref()),
        ),
    ])
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
    use glasschain_identity::{CertChainVerifier, Identity, Organization};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_verifier(org: &Organization) -> CertChainVerifier {
        // Fail-closed ADR-013 posture: the CRL rides with the root.
        let mut verifier = CertChainVerifier::from_org(org).expect("verifier");
        verifier.add_crl_pem(&org.crl_pem().unwrap()).expect("crl");
        verifier
    }

    /// Build metadata carrying all four admin headers: the MSP headers, the
    /// timestamp, the signature over `{node_id}:{ts}` under the certificate
    /// key, and the base64 DER certificate itself (ADR-017).
    fn admin_metadata(
        identity: &Identity,
        node_id: &str,
        verifier: &CertChainVerifier,
    ) -> tonic::metadata::MetadataMap {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_secs();
        let sig = identity.sign_bytes(format!("{node_id}:{ts}").as_bytes());
        let cert_der = rustls_pki_types::CertificateDer::from_pem_slice(
            identity.certificate_pem.as_ref().expect("cert").as_bytes(),
        )
        .expect("cert der");
        // The self-test path first: the cert must check out.
        verifier
            .verify_cert_der(cert_der.as_ref())
            .expect("test cert verifies");
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert(
            "x-glasschain-node-id",
            node_id.parse().expect("valid header"),
        );
        map.insert(
            "x-glasschain-auth-ts",
            ts.to_string().parse().expect("valid header"),
        );
        map.insert(
            "x-glasschain-auth-sig",
            hex::encode(sig).parse().expect("valid header"),
        );
        map.insert(
            CERT_HEADER,
            base64::engine::general_purpose::STANDARD
                .encode(cert_der.as_ref())
                .parse()
                .expect("valid header"),
        );
        map
    }

    /// An admin-role member certificate signed by the live key is admitted,
    /// and the verified CN is the caller.
    #[test]
    fn admin_gate_accepts_a_cert_bound_admin() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let admin = org
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let gate = AdminGate::new(Arc::new(test_verifier(&org)));
        let metadata = admin_metadata(&admin, "admin-node", &test_verifier(&org));
        assert_eq!(gate.authorize(&metadata).expect("authorized"), "admin-node");
    }

    /// `admin_headers_from_cert` produces headers that pass `authorize` —
    /// the client/server header contract end to end.
    #[test]
    fn admin_headers_from_cert_round_trip() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let admin = org
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let verifier = test_verifier(&org);
        // The seed from the custody snapshot (ADR-018) drives the headers.
        let snapshot: serde_json::Value =
            serde_json::from_str(&org.export_json().unwrap()).unwrap();
        let seed_hex = snapshot["members"][0]["seed_hex"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&hex::decode(&seed_hex).unwrap());
        let gate = AdminGate::new(Arc::new(verifier));
        let headers = admin_headers_from_cert(admin.certificate_pem.as_ref().unwrap(), &seed)
            .expect("headers");
        let mut map = tonic::metadata::MetadataMap::new();
        for (name, value) in &headers {
            map.insert(*name, value.parse().expect("valid header value"));
        }
        assert_eq!(gate.authorize(&map).expect("authorized"), "admin-node");
    }

    /// A member certificate without the admin role is refused — membership
    /// alone is not operator authority.
    #[test]
    fn admin_gate_rejects_a_non_admin_certificate() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let member = org.issue_identity("member-node").unwrap().clone();
        let gate = AdminGate::new(Arc::new(test_verifier(&org)));
        let metadata = admin_metadata(&member, "member-node", &test_verifier(&org));
        let error = gate.authorize(&metadata).expect_err("must refuse");
        assert!(error.to_string().contains("admin role"), "{error}");
    }

    /// A certificate from a foreign organization fails the chain check
    /// before any role decision.
    #[test]
    fn admin_gate_rejects_a_foreign_certificate() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let _admin = org
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let mut foreign = Organization::new("MedCorp").unwrap();
        let outsider = foreign
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let gate = AdminGate::new(Arc::new(test_verifier(&org)));
        let metadata = admin_metadata(&outsider, "admin-node", &test_verifier(&foreign));
        let error = gate.authorize(&metadata).expect_err("must refuse");
        assert!(error.to_string().contains("verification failed"), "{error}");
    }

    /// A signature from a different key than the certificate's own is
    /// rejected — possession is proven per request.
    #[test]
    fn admin_gate_rejects_a_stolen_signature() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let admin = org
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let impostor = org.issue_identity("impostor").unwrap().clone();
        let verifier = test_verifier(&org);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let cert_der = rustls_pki_types::CertificateDer::from_pem_slice(
            admin.certificate_pem.as_ref().unwrap().as_bytes(),
        )
        .unwrap();
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "admin-node".parse().unwrap());
        map.insert("x-glasschain-auth-ts", ts.to_string().parse().unwrap());
        map.insert(
            "x-glasschain-auth-sig",
            // Signed by the impostor's key, presented with the admin's cert.
            hex::encode(impostor.sign_bytes(format!("admin-node:{ts}").as_bytes()))
                .parse()
                .unwrap(),
        );
        map.insert(
            CERT_HEADER,
            base64::engine::general_purpose::STANDARD
                .encode(cert_der.as_ref())
                .parse()
                .unwrap(),
        );
        let gate = AdminGate::new(Arc::new(verifier));
        let error = gate.authorize(&map).expect_err("must refuse");
        assert!(error.to_string().contains("signature"), "{error}");
    }

    /// A timestamp outside the replay window is refused even with a
    /// otherwise-valid admin certificate.
    #[test]
    fn admin_gate_rejects_an_expired_timestamp() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let admin = org
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 3600;
        let sig = admin.sign_bytes(format!("admin-node:{ts}").as_bytes());
        let cert_der = rustls_pki_types::CertificateDer::from_pem_slice(
            admin.certificate_pem.as_ref().unwrap().as_bytes(),
        )
        .unwrap();
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "admin-node".parse().unwrap());
        map.insert("x-glasschain-auth-ts", ts.to_string().parse().unwrap());
        map.insert("x-glasschain-auth-sig", hex::encode(sig).parse().unwrap());
        map.insert(
            CERT_HEADER,
            base64::engine::general_purpose::STANDARD
                .encode(cert_der.as_ref())
                .parse()
                .unwrap(),
        );
        let gate = AdminGate::new(Arc::new(test_verifier(&org)));
        let error = gate.authorize(&map).expect_err("must refuse");
        assert!(error.to_string().contains("expired"), "{error}");
    }

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

    // ── AuthTokenBuilder ───────────────────────────────────────────────────────

    /// `build_headers` produces the three recognised headers that, once
    /// re-inserted into metadata, pass a full `verify_request` round-trip.
    #[test]
    fn test_build_headers_end_to_end() {
        let seed = [0x12u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let pub_key_bytes = signing_key.verifying_key().to_bytes();

        let headers =
            AuthTokenBuilder::build_headers(&seed, &pub_key_bytes, "node-builder").unwrap();
        assert_eq!(headers[0].0, "x-glasschain-node-id");
        assert_eq!(headers[0].1, "node-builder");
        assert_eq!(headers[1].0, "x-glasschain-auth-ts");
        assert_eq!(headers[2].0, "x-glasschain-auth-sig");
        assert!(!headers[2].1.is_empty());

        let registry = TrustedKeyRegistry::new();
        registry.register("node-builder", pub_key_bytes);
        let interceptor = MspAuthInterceptor::new_strict(registry);

        // Re-insert the three headers into a MetadataMap and verify.
        let mut map = tonic::metadata::MetadataMap::new();
        for (name, value) in &headers {
            map.insert(*name, value.parse().expect("valid header value"));
        }
        assert!(interceptor.verify_request(&map).is_ok());
    }

    /// `build_headers` must reject a verifying key that is not a valid
    /// ed25519 compressed point.
    #[test]
    fn test_build_headers_invalid_verifying_key() {
        let seed = [0x12u8; 32];
        // 0x42 is not a valid ed25519 compressed point in curve25519-dalek,
        // so it must be rejected before any headers are built.
        let bad_pub_key = [0x42u8; 32];

        let result = AuthTokenBuilder::build_headers(&seed, &bad_pub_key, "node-1");
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("invalid verifying key bytes"),
            "error should mention the verifying-key validation",
        );
    }

    // ── verify_request malformed-input branches ───────────────────────────────

    /// A non-numeric `x-glasschain-auth-ts` must be rejected as an invalid
    /// timestamp format.
    #[test]
    fn test_verify_invalid_timestamp_rejected() {
        let registry = TrustedKeyRegistry::new();
        let interceptor = MspAuthInterceptor::new_strict(registry);

        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "node-1".parse().unwrap());
        map.insert("x-glasschain-auth-ts", "not-a-number".parse().unwrap());
        map.insert("x-glasschain-auth-sig", "00".parse().unwrap());

        let result = interceptor.verify_request(&map);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    /// A partial header set (e.g. only `node-id` present) must be rejected:
    /// once any auth header is present, all three are required.
    #[test]
    fn test_verify_partial_headers_rejected() {
        let registry = TrustedKeyRegistry::new();
        let interceptor = MspAuthInterceptor::new_strict(registry);

        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "node-1".parse().unwrap());
        // auth-ts and auth-sig are intentionally absent.

        let result = interceptor.verify_request(&map);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    /// A signature that hex-decodes to the wrong length (not 64 bytes) must be
    /// rejected as an invalid signature encoding.
    #[test]
    fn test_verify_bad_sig_encoding_rejected() {
        let registry = TrustedKeyRegistry::new();
        let interceptor = MspAuthInterceptor::new_strict(registry);

        let ts = now_secs();
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "node-1".parse().unwrap());
        map.insert("x-glasschain-auth-ts", ts.to_string().parse().unwrap());
        // Valid hex but only 4 bytes — not a 64-byte ed25519 signature.
        map.insert("x-glasschain-auth-sig", "deadbeef".parse().unwrap());

        let result = interceptor.verify_request(&map);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    // ── register_from_org ──────────────────────────────────────────────────────

    /// `register_from_org` should populate the registry with each member's
    /// verifying key, and that key must drive a subsequent `verify_request`.
    #[test]
    fn test_register_from_org_and_verify_request() {
        let mut org = Organization::new("acme-corp").unwrap();
        org.issue_identity("org-node-1").unwrap();

        let registry = TrustedKeyRegistry::new();
        registry.register_from_org(&org);

        // The org member's key is registered under its node ID.
        let member = org.get_member("org-node-1").unwrap();
        assert!(registry.contains("org-node-1"));
        assert_eq!(registry.get("org-node-1"), Some(member.public_key_bytes()));

        // The org-registered key is the one enforced by verify_request: a
        // request signed with any other key fails at signature verification.
        let interceptor = MspAuthInterceptor::new_strict(registry);
        let ts = now_secs();
        let wrong_seed = [0xEEu8; 32];
        let sig_hex = sign_challenge(&wrong_seed, "org-node-1", ts);
        let metadata = make_metadata("org-node-1", ts, &sig_hex);

        let result = interceptor.verify_request(&metadata);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    /// Only some auth headers present: each missing one is named precisely.
    #[test]
    fn test_partial_headers_name_each_missing_one() {
        let interceptor = MspAuthInterceptor::new_strict(TrustedKeyRegistry::new());

        // Only the signature header: node-id and then ts are reported missing.
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-auth-sig", "00".parse().unwrap());
        let err = interceptor.verify_request(&map).unwrap_err();
        assert!(err.to_string().contains("x-glasschain-node-id"), "{err}");

        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "n1".parse().unwrap());
        let err = interceptor.verify_request(&map).unwrap_err();
        assert!(err.to_string().contains("x-glasschain-auth-ts"), "{err}");

        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "n1".parse().unwrap());
        map.insert(
            "x-glasschain-auth-ts",
            now_secs().to_string().parse().unwrap(),
        );
        let err = interceptor.verify_request(&map).unwrap_err();
        assert!(err.to_string().contains("x-glasschain-auth-sig"), "{err}");
    }

    /// `Interceptor::call` delegates to `verify_request` (the tonic wiring).
    #[test]
    fn test_interceptor_call_delegates_to_verify_request() {
        let mut interceptor = MspAuthInterceptor::new_strict(TrustedKeyRegistry::new());
        let err = <MspAuthInterceptor as tonic::service::Interceptor>::call(
            &mut interceptor,
            tonic::Request::new(()),
        )
        .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    /// The `AdminGate` debug view names the type without leaking the verifier.
    #[test]
    fn test_admin_gate_debug() {
        let gate = AdminGate::new(Arc::new(test_verifier(
            &Organization::new("PharmaCorp").unwrap(),
        )));
        assert!(format!("{gate:?}").contains("AdminGate"));
    }

    /// `update_verifier` hot-swaps the trust store (ZT-R3): after the swap the
    /// gate admits only the new organization's principals.
    #[test]
    fn test_admin_gate_update_verifier_swaps_the_trust_store() {
        let mut old = Organization::new("PharmaCorp").unwrap();
        let admin_old = old
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let mut new_org = Organization::new("MedCorp").unwrap();
        let admin_new = new_org
            .issue_identity_with_role("med-admin", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();

        let gate = AdminGate::new(Arc::new(test_verifier(&old)));
        gate.authorize(&admin_metadata(
            &admin_old,
            "admin-node",
            &test_verifier(&old),
        ))
        .expect("old org admitted");

        gate.update_verifier(Arc::new(test_verifier(&new_org)));
        assert!(gate
            .authorize(&admin_metadata(
                &admin_old,
                "admin-node",
                &test_verifier(&old)
            ))
            .is_err());
        gate.authorize(&admin_metadata(
            &admin_new,
            "med-admin",
            &test_verifier(&new_org),
        ))
        .expect("new org admitted after the swap");
    }

    /// Missing admin headers each produce a precise denial.
    #[test]
    fn test_admin_gate_missing_headers_fail_precisely() {
        let org = Organization::new("PharmaCorp").unwrap();
        let gate = AdminGate::new(Arc::new(test_verifier(&org)));

        // node-id present only → ts, sig, cert reported missing in order.
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "n1".parse().unwrap());
        assert!(gate
            .authorize(&map)
            .unwrap_err()
            .to_string()
            .contains("x-glasschain-auth-ts"));

        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "n1".parse().unwrap());
        map.insert(
            "x-glasschain-auth-ts",
            now_secs().to_string().parse().unwrap(),
        );
        assert!(gate
            .authorize(&map)
            .unwrap_err()
            .to_string()
            .contains("x-glasschain-auth-sig"));

        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("x-glasschain-node-id", "n1".parse().unwrap());
        map.insert(
            "x-glasschain-auth-ts",
            now_secs().to_string().parse().unwrap(),
        );
        map.insert("x-glasschain-auth-sig", "0".repeat(128).parse().unwrap());
        let err = gate.authorize(&map).unwrap_err();
        assert!(
            err.to_string()
                .contains("requires an organization certificate"),
            "{err}"
        );
    }
}
