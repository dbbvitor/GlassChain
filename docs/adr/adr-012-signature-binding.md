# ADR-012 — Signature binding: endorsement carriers authorize, record signatures are advisory

**Status:** Accepted
**Date:** 2026-09-03
**Decision owner:** project owner
**Relates to:** §8.2 (governance-signed capability activations) ·
[ADR-006](adr-006-canonical-schema-v1.md) (schema identity) ·
[ADR-008](adr-008-endorsement-policy-model.md) (endorsement policy model) ·
[ADR-010](adr-010-capability-versioning-policy.md) (consensus input boundary) ·
[#60](https://github.com/dbbvitor/GlassChain/issues/60)

## Context

Two signature sets in `glasschain-core` were validated **by count only**, and
nothing in the codebase verified their bytes:

1. `CanonicalRecord.signatures` — schema validation requires non-empty, and
   for `state_commitment` at least one entry per named counterparty.
2. `CapabilityActivation.signatures` — presence-only, documented as "the
   required governance signature set".

The sharp one is the capability activation: ADR-010 makes the committed
capability set the network-wide switch for every consensus-visible and
validation-affecting behaviour, and says activations are governance-signed —
yet the field was decorative, and `operation_default` (endorsement.rs) covered
only canonical record schemas. Under an active `endorsement` capability, a
capability activation required zero endorsement carriers.

Considered options:

- **Bind the fields** — verify `RecordSignature.signature_bytes` against
  MSP-resolved principals via a new provider trait. Requires building a
  signer-name→public-key directory that exists nowhere, a second verification
  path parallel to the endorsement engine, and a payload convention for
  activations (none exists — `canonical_form` covers only records).
- **Route authorization through the existing engine** — the endorsement layer
  already does cryptographically what these fields pretend to do: it verifies
  ed25519 signatures over the exact transaction payload, binds keys to
  principals through a registered directory, counts distinct principals, and
  is wired at node startup (ADR-011-era #59 work).
- **Accept and document** — declare the fields decorative and rely on
  endorsement carriers for everything.

## Decision

1. **Authorization rides the endorsement layer.** When the `endorsement`
   capability is active at the candidate height, `operation_default`
   additionally requires:
   - every `CapabilityActivation` transaction: `SignedBy("network-governance")`;
   - every `state_commitment` record: `NOutOf{all}` over the record's issuer
     plus every named counterparty — making enforcement match what the
     count-only schema check already pretended to require.

   Both defaults resolve through the same verified-carrier machinery as all
   other operation defaults (ADR-008 §4): ed25519 verification over
   `TransactionEndorsement::payload(tx)`, key→principal binding via the
   registered MSP directory, distinct-principal counting, hard reject on
   unknown keys.

2. **The fixed `network-governance` principal is the v1 genesis fallback.**
   Before any deployment commits a `PolicyUpdate` naming its real governance
   principals, the fallback stands — fail-closed, documented as exactly that.
   This mirrors the policy-history default already shipped in ADR-008.

3. **The `signatures` fields stay in the record shape, as advisory metadata.**
   They are not removed or renamed: `RecordSignature` bytes participate in
   transaction serialization and block hashing, and ADR-006 makes the schema
   identity immutable — removal is a schema break bought for zero security
   gain. Their doc comments and this ADR state their advisory status. The
   count-only schema checks remain as structural validation, not security
   controls.

4. **No new capability.** The change is validation-affecting, but the existing
   `endorsement` capability is precisely the gate for "endorsement enforcement
   is active at this height" (ADR-010/ADR-008). Nodes without the capability —
   or without a provider attached — see zero behavior change; nodes with both
   get the stricter defaults. A separate capability would force a second
   activation ceremony for a switch that already exists.

## Consequences

- Capability activations are actually governance-authorized when enforcement
  is on: the consensus-boundary switch rests on a performed check, closing the
  gap between ADR-010's prose and the code.
- Deployments that attach an endorsement provider must now endorse capability
  activations under the `network-governance` principal (or re-key the default
  via a `PolicyUpdate` before activating further capabilities). The startup
  log from the #59 work names whether a provider is attached.
- A second signature-verification path is deliberately **not** built: one
  engine, one directory, one payload convention.
- #58 (revocation) and future governance work inherit a clear answer to "what
  do governance signatures mean": they are endorsement carriers evaluated by
  the MSP-backed engine, not free-floating bytes.

## Validation

- Core unit tests: unendorsed capability activation rejects with the operation
  default error; governance carrier satisfies it; `state_commitment` accepts
  issuer + all counterparties and rejects a partial set.
- Network integration tests (`endorsement.rs`): unendorsed `bft_consensus`
  activation rejected at admission while the capability is active, endorsed
  one commits; advisory `signatures` fields do not satisfy the default; an
  endorsed `state_commitment` commits.
- All pre-existing suites pass unchanged — every other capability-activation
  test runs with enforcement dormant (no provider / inactive capability),
  confirming decision 4's no-behavior-change-without-the-gate property.
