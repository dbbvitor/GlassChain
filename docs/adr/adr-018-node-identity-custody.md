# ADR-018 — Node identity material is an operator-owned file, not storage state

**Status:** Accepted
**Date:** 2026-09-14
**Decision owner:** project owner
**Relates to:**
[ADR-011](adr-011-federation-trust-store.md) (federation trust store) ·
[ADR-013](adr-013-certificate-revocation.md) (fail-closed revocation) ·
[ADR-017](adr-017-deployment-trust-and-retention.md) (deployment trust) ·
[zero-trust residual plan](../../.agents/plans/zero-trust-residual.md) (ZT-R1)

## Context

`glasschain-node` regenerated its organization and member identity on every
start: a fresh ed25519 key, a fresh certificate (new serial), a fresh Root
CA, and a new TLS fingerprint. Any peer holding a persisted TOFU pin
(`tofu:peer:<addr>`, ADR-011/#88) then saw an unknown fingerprint *and* a pin
whose rotation key no longer exists — reconnect failed until an operator
removed the pin by hand, and a peer's trust-store copy of the "old" Root CA
kept rejecting the new one.

## Decision

Identity material is **durable and operator-owned**: with
`--identity-file <PATH>`, the node creates a JSON file on first start
(permissioned `0600` on Unix) holding the Root CA key pair, serial
bookkeeping, revocation history and member identity seeds
(`Organization::export_json` / `import_json`), and loads it on every
restart — the same identity key, certificate and Root CA are re-presented,
so pins, possession proofs, OCSP staples and trust-store anchors stay
coherent across restarts.

The file lives **outside** the storage seam deliberately: putting private
keys inside the replicated/archived ledger storage would leak them into
every physical backup copy (`glasschain backup-scrub` scrubs private
payloads, not keys). Operators own the file, its permissions, and its
backup policy. Without the flag the node keeps the ephemeral per-start
behaviour and logs a warning — that stays the dev default.

### Considered alternatives

- **Storage seam** (`identity:seed:<node-id>` state key): one less flag,
  but every storage copy/archive would carry the private key, and
  `backup-scrub` would have to grow key-scrubbing semantics. Rejected.
- **Signed-rotation-only recovery** (keep re-keying, peers accept rotations):
  the rotation proof must be signed by the *pinned* key, which no longer
  exists after a re-key — the mechanism #88 built cannot recover from it.
