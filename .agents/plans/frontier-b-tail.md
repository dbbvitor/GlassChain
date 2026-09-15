# Plan — Frontier B code tail (ADR-017 implementation)

**Status:** shipped (2026-09-14) — all four ADR-017 code slices merged into the
working tree; delete this plan once the PR carries it.
**Decisions:** [ADR-017](../../docs/adr/adr-017-deployment-trust-and-retention.md) —
OCSP stapling only (no responder egress), certificate-bound MSP admin RBAC,
backup scrubbing, #74 deferred.

## What shipped

1. **OCSP staple (identity + network).**
   - `glasschain-identity/src/ocsp.rs`: minimal DER encoder/decoder over the
     OCSP subset the project emits; `mint_good_response` (responderID byName,
     SHA-256 certID hashes, `ecdsa-with-SHA256` signature over the
     `ResponseData` body by the CA's P-256 key); `parse_staple` +
     `ParsedStaple::verify_against` (signature, freshness, serial).
     `CertChainVerifier::verify_ocsp_staple` resolves the issuing CA (own
     root, federation anchor, intermediate) by subject DN and verifies
     locally — no egress.
   - `Organization::ocsp_response_der` / `IntermediateCa::ocsp_response_der`
     mint per live member; revoked/unknown nodes are refused.
   - `glasschain-network/src/protocol.rs`: additive `Hello.ocsp_response_der`
     (base64; stays `glasschain/6`); the node mints at startup under `--org`
     and staples every Hello. Receive side: a `revoked` staple fails the
     session's org verification closed; malformed/mismatched/expired staples
     fall back to the CRL result.
   - Tests: identity `ocsp::tests` (round-trip, mismatch, tamper, staleness,
     mint gate), network `hello_carries_ocsp_staple_on_the_wire`,
     `tofu_known_hello_reauthorizes_org_verification`, and
     `protocol_security::{ocsp_staple_travels_and_keeps_verified_org_on_private_path,
     node_hello_carries_its_minted_ocsp_staple}`.

2. **Admin RBAC (identity + rpc + node).** `issue_identity_with_role` stamps
   `OU=admin` (`ADMIN_ROLE`); `certificate_admin_role[_der]` reads it from a
   verified cert. `glasschain-rpc::auth::AdminGate` authorizes the three
   `NodeService` channel RPCs from `x-glasschain-*` + `x-glasschain-cert`
   (base64 DER): fail-closed chain/CRL verify, CN == node id, possession via
   the header signature under the certificate key, admin OU. `glasschain-node`
   installs the gate with the cert verifier; without it the RPCs fail closed.
   Tests in `glasschain-rpc/src/auth.rs` and `server_integration.rs`.

3. **backup-scrub (cli).** `glasschain backup-scrub --storage <PATH>` opens a
   copied sled store, runs `TransientStore::purge_expired` (the D5 sweep),
   reports the count. Test: expired payloads purged, live ones survive, a
   second run is a no-op.

4. **Established-session reauthorization.** The TOFU `Known` path now assigns
   the fresh Hello's `org_verified` (upgrade or downgrade) — identity and
   fingerprint pins stay stable.

## Deferred (unchanged)

On-chain revocation registry (#74), delegated OCSP responders, CRL file
hot-reload, admin CLI client, on-chain channel registry.

## Gates (run 2026-09-14)

`cargo fmt --all --check` ✅ · `cargo check --workspace --all-targets
--all-features --locked` ✅ · `cargo test --workspace --lib --bins --tests
--all-features --locked` ✅ (33 harnesses, 0 failures) ·
`cargo clippy --workspace --all-targets --all-features --locked -- -D
warnings` ✅ (0 diagnostics).
