# ADR-014 — BLS-aggregated quorum certificates

**Status:** Accepted
**Date:** 2026-09-03
**Decision owner:** project owner
**Relates to:** §8.2 (deterministic finality) ·
[ADR-002](adr-002-consensus-finality.md) (consensus family) ·
[ADR-009](adr-009-validator-eligibility.md) (duty-roster rotation) ·
[ADR-010](adr-010-capability-versioning-policy.md) (capability gating) ·
[#62](https://github.com/dbbvitor/GlassChain/issues/62) Step 4 ·
post-quantum plan action 2 (algorithm discriminants)

## Context

ADR-002's quorum certificate is embedded in every block forever and in every
proof a light client verifies — ADR-004's ladder reaches 70M participants only
if a finality proof is cheap enough for a phone. The performance plan measured
the per-validator pattern at ~79 KB for a 201-of-300 certificate before Step 1,
~34 KB after the base64 encoding, and identified BLS aggregation as the
**enabler of the scalability claim**: 201 light-client verifications collapse
into aggregate verification, and the signature becomes constant-size (the
signer bitmap still grows with the validator set).

The plan's own sequencing note warned: "a primitive swap ahead of the engine
decision risks doing it twice." **The owner explicitly overrode that ordering
for this decision** (grilling round, 2026-09-03): the swap is recorded here
with the acceptance that the ADR-010 adoption-gate work (vote rounds, the
Malachite decision) may force re-work. At decision time the certificate was not
yet transported/persisted with blocks. Subsequent round-driver work attaches it
to `Block.certificate`; production adoption gates remain.

Considered and rejected along the way: `blst` (audit-grade C backend — carries
unsafe into the build; revisit only with a measured need), threshold BLS
(needs a DKG ceremony plus resharing on **every epoch** — ADR-009's duty-roster
rotation would make that constant), and a binary-codec-first ordering
(orthogonal; remains Step 1's tail).

> **The `blst` revisit condition was met on 2026-09-04.** The 300-validator
> finality gate does not pass on the pure-Rust `pairing` backend (first vote
> 34.8 s; the precommit re-verification herd of 299 × 202-pairing multi-Miller
> loops exceeds the scaled phase budget — see `docs/benchmarks/consensus-capacity.md`).
> 300 is the designed operating point, so this is a measured need, not a
> preference. The reconsideration is scoped in
> [`.agents/plans/zero-trust.md`](../../.agents/plans/zero-trust.md) §6, alongside
> the transport-provider review. Native dependencies already exist; each backend
> needs its own security, compatibility and measured-cost evaluation. Neither a
> 10× gain nor passing 300 follows from selecting `blst`. This note does **not**
> reverse decision 2.

## Decision

1. **Plain BLS12-381 multisignature aggregation** — each participating validator
   holds an individual BLS key and signs the block hash; the certificate carries
   **one aggregate signature plus a signer bitmap** over the set's canonical
   order. Quorum counting is separate; this does not require all n signatures.
   No DKG or resharing. The signature is 96 bytes; the bitmap is `ceil(n/8)` bytes,
   so the whole certificate is compact but not constant-size.

2. **Crate: `bls-signatures` (Filecoin, pure-Rust `pairing` backend)** over
   `bls12_381` — no additional C backend for this implementation (not a claim
   that the workspace's dependency graph contains no native/unsafe code). It ships
   only the *distinct-message* aggregate verify, so the same-message multisig
   check (a quorum signs one hash) is implemented directly over `bls12_381`
   pairings: `e(−G1, agg_sig) · Π e(pk_i, hash) = 1`.

3. **Rogue-key defense: proof of possession at registration.** Plain
   same-message aggregation is rogue-key-vulnerable; each `ValidatorInfo`
   carries an individual BLS signature over `"glasschain-bls-pop:<pk>"`,
   verified at provider construction. One invalid key would corrupt every
   aggregate it joins — registration fails closed.

4. **Scope boundary: quorum certificates only.** ed25519 stays for
   transaction signatures, endorsements, MSP identities, and TLS. The
   aggregate carries the `Bls12381` algorithm discriminant (post-quantum plan
   action 2) — a certificate naming another algorithm is rejected at
   validation.

5. **Wire shape (`glasschain/6`):** `QuorumCertificate = { block_index,
   block_hash, signers_bitmap, aggregate_signature, algorithm: Bls12381 }`.
   The per-validator `Attestation` struct is gone. Certificate size at 300
   signers: **< 1 KB** (validated by test), against ~79 KB before Step 1.

6. **Capability gating unchanged:** the aggregate path lives behind the `bft`
   feature and `bft_consensus` capability. Vote rounds have since shipped as a
   staged dev/test driver; PoW still drives the default build. A local single-
   signer aggregate is not a multi-validator quorum or production adoption.

## Consequences

- The pure-Rust implementation uses O(quorum) pairing terms in one
  multi-Miller-loop check. Cheap mobile/light-client verification needs a
  measurement, not an inference from signature size.
- A failed aggregate requires rejection but cannot implicate individual claimed
  signers; a sender could have fabricated it. Attribution requires independently
  authenticated conflicting votes, not two aggregate certificates alone.
- **Implementation follow-up (2026-09-05):** vote signatures currently bind the
  hash but omit routing height/round/phase, and `handle_vote` recreates its
  receipt tracker per call. Context binding and end-to-end evidence detection
  need the regression tests in [zero-trust §8](../../.agents/plans/zero-trust.md).
  Sync/restart QC validation also remains a gate. No complete attribution or
  production-safety claim follows from this ADR.
- Ed25519 batch verification (performance Step 2) was superseded by aggregation.
  Do not restore per-signer signatures inside the QC; a bounded live receipt
  journal for conflict detection is separate and may still be required.
- The post-quantum horizon note stands: BLS12-381 is Shor-broken like ed25519;
  the discriminant field is what makes the eventual migration a version bump,
  not a reinterpretation problem.
- The implementation may require re-work if the ADR-010 adoption gate selects
  Malachite (which has its own ed25519-based flow). Accepted by the owner.

## Validation

- `glasschain-core` (feature `bft`): registration rejects an invalid PoP;
  an aggregated 10-of-10 certificate verifies in one pairing check; 2-of-10
  is rejected below quorum; bitmap bits beyond the set are rejected; a
  decode-valid aggregate over the wrong message fails the pairing; a
  300-signer certificate serializes under 1 KB and round-trips.
- `glasschain-network`: `bft_finality` — capability-gated engine swap still
  produces final-at-commit certificates that verify end to end.
- `PROTOCOL_VERSION` → `glasschain/6`: a `/5` peer cannot parse the new
  certificate.
