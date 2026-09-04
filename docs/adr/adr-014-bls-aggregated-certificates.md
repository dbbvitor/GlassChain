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
into one pairing check, and the certificate becomes constant-size.

The plan's own sequencing note warned: "a primitive swap ahead of the engine
decision risks doing it twice." **The owner explicitly overrode that ordering
for this decision** (grilling round, 2026-09-03): the swap is recorded here
with the acceptance that the ADR-010 adoption-gate work (vote rounds, the
Malachite decision) may force re-work. The certificate is not on the wire, not
persisted with blocks, and no chain is live — the swap is cheapest now.

Considered and rejected along the way: `blst` (audit-grade C backend — carries
unsafe into the build; revisit only with a measured need), threshold BLS
(needs a DKG ceremony plus resharing on **every epoch** — ADR-009's duty-roster
rotation would make that constant), and a binary-codec-first ordering
(orthogonal; remains Step 1's tail).

## Decision

1. **Plain n-of-n BLS12-381 aggregation** — each validator holds an individual
   BLS key and signs the block hash; the certificate carries **one aggregate
   signature plus a signer bitmap** over the validator set's canonical order.
   No DKG, no ceremony, no resharing. Constant-size: bitmap + 96-byte
   signature regardless of validator count.

2. **Crate: `bls-signatures` (Filecoin, pure-Rust `pairing` backend)** over
   `bls12_381` — no C toolchain, no unsafe in the build. The backend ships
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
   feature and the `bft_consensus` capability, dormant until vote rounds exist
   (ADR-010 adoption gates). Until then the proposer's own BLS signature is
   the single-signer aggregate, and dev/test PoW still drives the default
   build.

## Consequences

- Light-client verification of finality is one pairing check against a
  bitmap — the ADR-004 ladder's cheap-proof requirement is satisfied by
  construction.
- Certificate size no longer grows with validator count: the ~300 ceiling
  costs bytes, not bandwidth, on the commit path.
- The aggregate is deterministic (BLS is), so equivocation evidence (#77)
  will compare two distinct aggregates over the same height.
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
