# ADR-009 — Validator eligibility and churn

**Status:** Accepted
**Date:** 2026-09-03
**Decision owner:** project owner
**Relates to:** §1.1–§1.4 · [ADR-002](adr-002-consensus-finality.md) (consensus
family, open questions 4–5 — **closed by this ADR**) ·
[ADR-004](adr-004-scale-topology.md) (membership ladder, 70M horizon) ·
[ADR-010](adr-010-capability-versioning-policy.md) (read-only downgrade,
capability gating) · [ADR-011](adr-011-federation-trust-store.md) (org trust) ·
[#77](https://github.com/dbbvitor/GlassChain/issues/77) (equivocation proofs) ·
[participation model](../../.agents/memories/participation-model.md) (roles are
overlapping, never tiers)

## Context

ADR-002 fixed the consensus family (Tendermint-class BFT, zero-trust validator
set, deterministic ⅔+1 finality, full participation through the ~300 practical
ceiling) but left two questions open: **voting power** (open question 4) and
**churn mechanics** (open question 5) — with the explicit warning that churn,
not set size, is "where the legitimacy risk actually lives." The dichotomy it
recorded: objective published eligibility with self-selection and opt-out makes
the set a *duty roster*; discretionary admission makes it a *cartel*.

The trigger scenario: member organizations hosting nodes grow past the cap.
The membership ladder absorbs members (endorse, verify, read — no ceiling);
only validator slots are scarce. This ADR decides how scarce slots are
allocated and vacated.

Rejected on sight (ground already covered): VRF/RANDAO sortition (ADR-002's
decision; no stake weighting exists to make committee capture improbable),
stake-weighted voting (no economics model exists), node *reputation scores*
(collide with `MetadataTrustScore`, which measures lot provenance completeness
— a node-behavior score would invert its purpose), hardware-attestation
admission (swaps the root of trust to silicon vendors; excludes member
infrastructure that cannot run it), ephemeral validator identities (break the
ADR-008 key→principal directory and ADR-011 TOFU pinning).

## Decision

### 1. Voting power: one org, one vote, equal weight (ADR-002 Q4 — closed)

Every validator carries equal voting power. No stake, no reputation weighting,
no delegable power. Any future weighting proposal must satisfy the standing
constraint — governance standing attaches to membership, not validation — and
currently has no economics model to draw on (Stage 4).

### 2. Eligibility: an objective, published bar (ADR-002 Q5 — closed)

An organization is **eligible** for a validator slot when it meets every item
of a published, independently checkable bar. Each criterion must be verifiable
by any member from the record alone — no discretionary judgment:

- **Operational:** meets the published SLA/uptime attainment measured while
  previously in the set (first-time orgs provisionally eligible);
- **Security posture:** ADR-011 verification configured (trust store + CRLs
  current per ADR-013), ADR-013 fail-closed revocation enforced;
- **Conduct:** no upheld equivocation proof ([#77]) against the org within the
  lookback window;
- **Membership:** a member organization in good standing.

**Reputation is an eligibility criterion only as objective historical fact**
(the items above). A continuous, computed reputation score is rejected: it
would be subjective, centrally owned, and would reintroduce tiers through the
back door. The bar's items are pass/fail facts, not a score.

Eligibility is self-selection with opt-out: an eligible org that does not wish
to validate simply does not enter the roster.

### 3. Allocation: cap, epochs, and no-repeat rotation

- **Cap:** 300 validators — the technical ceiling, not a political one. No
  governance cap below it.
- **Epoch:** the active set is fixed for an epoch (days-scale, knob not
  constant). Per-height set churn is technically supported but rejected as a
  default: incoming validators must sync before voting, so per-height churn
  makes liveness depend on permanent catch-up.
- **Proposer rotation is per height** and native to the consensus family
  (round-robin, equal power) — no validator proposes twice in a row.
- **Set rotation:** each epoch, the active set is the next 300 from a
  round-robin walk over the eligible roster, **re-seeded per epoch from the
  chain** (deterministic, verifiable by any member, not predictable far in
  advance). This is a fair shuffle of an already-eligible roster — it borrows
  none of VRF's legitimacy role, which belongs to the objective bar.
- **No-repeat constraint:** when eligible orgs ≥ cap, an org does not serve
  two consecutive epochs. When eligible < cap, minimal-repeat applies.
- **Cap on consecutive service** for the eligible < cap regime, so even then
  the roster rotates.

### 4. Removal: two causes, two mechanisms

- **Misbehavior:** an upheld equivocation proof ([#77] — two valid attestations
  from one key over different hashes at the same height) triggers automatic
  **suspension** for the next epoch. Permanent exclusion is a governance act
  recorded on-chain, informed by the proof — never an automatic slash. There is
  no stake to slash and no balance to burn.
- **Non-performance (offline, slow):** no jailing — evicting the absent is the
  split jail exists to avoid. Non-performers lose rotation priority and miss
  their published SLA commitment commercially. ⅔ liveness tolerates absent
  validators by design.

### 5. Genesis

The founding orgs that build the network form the initial set, enumerated in
the genesis configuration, having met the same published bar. They are subject
to the same rotation cap from the first epoch — no permanent founder seats.

### 6. Governance standing is unchanged

One member organization, one governance vote, validator or not. Validation is
an operational duty roster slot, not a status (participation model: roles are
overlapping, never tiers).

## Consequences

- ADR-002 open questions 4 and 5 are closed; the consensus-swap execution plan
  can consume a settled legitimacy model.
- The rotation machinery itself lands with the BFT adoption gate (ADR-010) —
  until cross-validator vote rounds exist, the set is whatever the operator
  configures, and the epoch/re-seed logic is dormant behind the same gate.
- The published bar becomes operational documentation that member orgs can
  read and prepare against before ever requesting a slot.
- Targeted-DoS exposure is bounded: rotation order is only predictable about
  one epoch ahead (seeded from the chain), and validators are replaceable
  roster entries rather than permanent high-value targets.

## Validation

- Legitimacy property: every exclusion and every seat is explainable from
  published, member-verifiable facts — no discretionary step anywhere in the
  path.
- ADR-002 open questions 4/5 marked closed; participation model's "proposed
  home: adr-009" resolved.
- [#77] provides the misbehavior evidence this ADR consumes.
