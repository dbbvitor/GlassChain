# ADR-013 — Certificate revocation via fail-closed CRLs in the trust store

**Status:** Accepted
**Date:** 2026-09-03
**Decision owner:** project owner
**Relates to:** §1.1 (MSP/X.509 identity) · Lei 14.063/2020 Art. 4º §2º
(revocation expectation) · [ADR-011](adr-011-federation-trust-store.md)
(federation trust store — the distribution channel) ·
[ADR-010](adr-010-capability-versioning-policy.md) (height-pinned historical
validation) · [#58](https://github.com/dbbvitor/GlassChain/issues/58)

## Context

`CertChainVerifier` verified a peer certificate's signature against a trusted
Root CA but never checked revocation: a compromised or decommissioned member
certificate stayed valid until expiry, on the layer that gates endorsement
principals and private-payload delivery. Lei 14.063/2020 Art. 4º §2º expects
signature schemes to provide revocation regardless of who issues the
certificate.

The option space, and why each was set:

- **CRL files distributed with the trust store** — the same out-of-band,
  operator-configured channel ADR-011 established; `rustls-webpki` verifies
  CRL signatures against the trusted CA natively. **Chosen.**
- **On-chain revocation registry** — self-bootstrapping, but couples identity
  revocation to chain bootstrap and ADR-010 gating; deferred to the backlog
  ([#74](https://github.com/dbbvitor/GlassChain/issues/74)).
- **OCSP** — a per-check network round trip; the performance plan's ICP rule
  keeps revocation network calls off every verification path.
- **Short-lived certificates** — needs automated re-issuance infrastructure
  that does not exist.

## Decision

1. **CRLs are part of the trust store and are mandatory.** Verification is
   fail-closed at every point:
   - no CRL loaded at all → `CrlMissing`, the certificate is rejected;
   - the issuing CA's CRL missing from the store → webpki
     `UnknownStatusPolicy::Deny` rejects (`RevocationStatusUnknown`);
   - the issuing CA's CRL expired (`next_update` passed) → webpki
     `ExpirationPolicy::Enforce` rejects (`CrlExpired`) — the org promised a
     freshness cadence and broke it;
   - the certificate's serial is on the CRL → `Revoked`.
   `glasschain-node` loads `*.crl` files from `--trust-store` (or `X509 CRL`
   blocks from `.pem` bundles) and warns loudly when anchors exist without
   CRLs, since every peer verification will then reject.

2. **Organizations mint and publish their own CRLs.** `Organization` tracks
   issued certificate serials; `revoke_identity(node_id)` marks a serial
   revoked; `crl_pem()` mints a CA-signed CRL with a 30-day `next_update`.
   Operators publish the refreshed file through the same ceremony as the Root
   CA certificate. Intermediate CAs (below) mint their own CRLs for the
   members they issued.

3. **Intermediate-CA support.** Trust-store entries are classified at load:
   self-signed certificates are anchors; anything else is an intermediate
   that webpki may build paths through (leaf → intermediate → anchor). A
   chain of any depth webpki accepts now verifies, including two-hop
   organizations that issue members through a subordinate CA. Intermediates
   are revocation-checked too (webpki depth `Chain`).

4. **Revocation is a go-forward control.** Blocks and transactions signed by a
   since-revoked certificate stay valid — the same height-pinned historical
   validation ADR-010 established. Revoked certificates are rejected on new
   connections from the moment the fresh CRL is loaded.

5. **Structural mode keeps its semantics.** `VerificationLevel::Structural`
   performs no path building and no CRL checking; it remains a diagnostic
   posture, not a deployment one.

## Consequences

- Revocation now works end to end: org revokes → mints CRL → publishes to
  peers' trust stores → peers reject the certificate on next verification.
- The trust-store contract is stricter: an operator who supplies anchors but
  no CRLs gets a node that verifies nobody. The startup warning and the
  per-verification `CrlMissing`/`RevocationStatusUnknown` errors make the
  misconfiguration impossible to miss.
- CRL freshness is an operational obligation per organization (≤ 30 days).
  There is no automatic revocation distribution yet — that is the deferred
  on-chain registry (#74).
- The revocation check runs at certificate verification (Hello handshake,
  PDC org gate) — never on the block path, preserving the performance plan's
  ICP/liveness boundary.

## Validation

- `glasschain-identity` unit tests: revoked member rejected (`Revoked`), valid
  member passes, missing CRL fails closed (`CrlMissing`), expired CRL fails
  closed (`CrlExpired`), intermediate-CA chain verifies and revokes through
  the subordinate CA, trust-store bundle classification (anchor vs
  intermediate).
- Network integration tests carry CRLs for every verifier they install; all
  pre-existing verification tests updated to the fail-closed contract.
