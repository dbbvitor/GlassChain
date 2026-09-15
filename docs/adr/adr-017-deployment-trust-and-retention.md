# ADR-017 — Deployment Trust, OCSP Stapling, Operator RBAC, and Physical Retention

**Status:** Accepted
**Date:** 2026-09-14
**Decision owner:** project owner
**Relates to:**
[ADR-011](adr-011-federation-trust-store.md) (federation trust store) ·
[ADR-013](adr-013-certificate-revocation.md) (fail-closed certificate revocation) ·
[zero-trust plan](../../.agents/plans/zero-trust.md) (Frontier B)

## Context

With ADR-011 (Federation Trust Store), ADR-013 (fail-closed CRL checking), cert-bound MSP principals (#87), session-bound possession proofs (#110), and restart-safe PDC purge (#86, D5) implemented, Frontier B addresses the operational trust, live revocation, administrative authorization, and physical retention gaps for production deployments.

Four key decisions were required:
1. Live session revocation status check mechanism (OCSP stapling vs. external live queries).
2. Operator deployment RBAC and channel management authorization.
3. Physical replica and backup data retention vs. on-chain immutability.
4. Disposition of on-chain revocation registry (#74).

## Decision

1. **OCSP Stapling at Session Establishment (No Outbound Egress):**
   - An organization's Root CA mints an issuer-signed OCSP `BasicResponse`
     (good status, serial-bound, fresh `nextUpdate`) for each member
     certificate (`Organization::ocsp_response_der`); `glasschain-node`
     mints its own at startup and staples it on every `Hello`.
   - Receiving nodes verify the staple **locally** against the trust store
     (`CertChainVerifier::verify_ocsp_staple`): ECDSA-P256-SHA256 signature
     under the issuing CA's key, certID serial match, producedAt/nextUpdate
     freshness. A `revoked` staple fails the session's org verification
     closed; a malformed, mismatched or expired staple is not a positive
     signal — it falls back to the fail-closed CRL path (ADR-013) and never
     upgrades or blocks it.
   - **Architectural note:** in `GlassChain` the organization certificate
     rides the `Hello` message — the TLS transport certificate is a
     self-signed, transport-only certificate (TOFU-pinned) — so the staple
     rides the `Hello` (the session-establishment message) rather than a TLS
     extension. Functionally this is peer-pushed, locally verified status at
     connection establishment, which is what the stapling decision protects;
     an `ocsp_response_der` wire field was added to `Hello` (still
     `glasschain/6`).
   - Outbound live OCSP responder network queries are rejected to protect
     consensus isolation and eliminate egress latency/failure modes.

2. **Operator RBAC via Certificate-Bound MSP Admin Principals + Height-Anchored Multi-Org Channels:**
   - Member certificates can carry an operational role as an Organizational
     Unit in their verified subject (`issue_identity_with_role`, `OU=admin`
     for `ADMIN_ROLE`); the role is read from the verified certificate,
     never a caller-supplied label.
   - Channel-management RPCs on `NodeService` (`CreateChannel`,
     `AddChannelMember`, `RemoveChannelMember`) require an admin principal:
     the `x-glasschain-*` headers plus a base64 DER certificate that
     verifies against the node's trust store (chain, validity, CRL —
     fail-closed), whose subject CN equals the calling node id, whose key
     signs the header challenge, and which carries the admin Organizational
     Unit. `AdminGate` enforces this; without a configured verifier the RPCs
     fail closed (`PermissionDenied`).
   - The authoritative source for collection-scoped endorsement policy
     stays the committed `PolicyUpdate` records (ADR-008/ADR-012); the admin
     RPCs mutate this node's runtime collection declaration.

3. **Physical Backup & Storage Scrubbing:**
   - Local active storage-scanning purge (D5) guarantees runtime eviction of expired private payloads.
   - For physical backups, snapshots, and disk-level replication, `glasschain backup-scrub --storage <PATH>` runs the same D5 retention sweep against a *copied* storage directory, pruning expired private payloads before archival (the live node's sweep never covers a backup image).
   - Operators relying on raw block-device/volume snapshots must enforce disk-level encryption key destruction upon retention expiration to guarantee physical unrecoverability under LGPD/GDPR requirements.

4. **On-Chain Revocation Registry (#74) Stays Deferred:**
   - D4 height-stamped authorization (`valid_from` / `revoked_at`) handles deterministic historical replay, and ADR-013 plus OCSP stapling handles transport revocation.
   - On-chain certificate revocation transactions remain deferred until multi-org production testnet operational feedback warrants a dedicated capability activation.
