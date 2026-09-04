# Liveness engineering — keeping a 300-validator quorum reachable

**Status:** Governance documentation (performance plan Step 7, #62)
**Date:** 2026-09-03
**Owner:** federation governance, enforced through the ADR-009 eligibility bar
**Relates to:** [ADR-002](adr/adr-002-consensus-finality.md) (⅔+ quorum,
deterministic finality) · [ADR-009](adr/adr-009-validator-eligibility.md)
(duty roster, eligibility bar) · [ADR-014](adr/adr-014-bls-aggregated-certificates.md)
(constant-size certificates) · [participation model](../.agents/memories/participation-model.md)

## Why this document exists

At 300 mutually-distrusting institutional validators, the bounds that actually
bite are **not protocol bounds**. Chernoff makes a ⅔ quorum *easier* to reach
as `n` grows with independent failures; what breaks liveness is correlated
failure, heavy-tailed latency, and human operations. No protocol change fixes
any of them — the fixes are placement rules, published expectations, and
measurement. This document is those rules.

The three bounds (from the performance plan §7):

1. **Correlated failure** — one cloud region, one BGP event, one DDoS takes
   many orgs at once. Quorum availability tracks the **min-cut of correlated
   sets**, not the count of organizations.
2. **Heavy-tailed latency** — the round timeout tracks the ⅔n-th order
   statistic of validator response times; under Pareto-shaped tails it grows
   roughly as `n^(1/α)`. A few chronically slow members percolate into every
   quorum.
3. **Governance** — as `n` grows, P(at least one quorum member is having a bad
   operations week) approaches 1. The answer is published expectations, not
   punishment (ADR-009 §4: no jailing).

## 1. Provider and geographic diversity (placement constraint)

**Rule: no single failure domain may hold more than 25% of the active
validator set.**

- The ⅔ liveness threshold means the set tolerates **⅓ of validators absent**.
  A provider hosting more than 25% of the set puts a single-region outage
  uncomfortably close to that line once any other stragglers exist.
- A "failure domain" is a (cloud provider, region) pair — AWS sa-east-1 and
  AWS us-east-1 are distinct regions but the same provider; corporate ISP,
  on-prem, and colocation count as their own domains.
- **Enforcement point:** the ADR-009 epoch rotation. When the seeded
  round-robin walk would seat a candidate who breaches the 25% bound, the walk
  skips to the next eligible candidate. The skip is deterministic, published
  with the rotation order, and recorded — it is a placement rule, not a
  discretionary judgment (ADR-009's legitimacy property is preserved).
- **Geographic floor:** no single country/jurisdiction may host more than 40%
  of the active set, for the same min-cut reason at regulatory scale
  (a jurisdictional action is a correlated failure).

## 2. Per-org SLA expectations (published, measured, non-punitive)

Each validator org publishes an **uptime commitment** for its term — the
default bar is **99.0% monthly**, measured as heights attested / heights while
in the active set.

- Measurement is from the chain itself: a validator is "present at height H"
  when its attestation (or, post-adoption-gate, its vote) is in the
  certificate. ADR-014's bitmap makes this a per-height fact, already
  committed.
- SLA attainment feeds the **ADR-009 eligibility bar** (rotation priority), it
  never triggers removal: absent validators are tolerated by ⅔ liveness, and
  evicting the absent is the split jailing exists to avoid. An org that
  persistently misses its commitment loses rotation priority and answers
  commercially under its own published SLA.
- The commitment is per-org and public — a member that cannot commit to 99.0%
  should not enter the roster, and that is a legitimate self-selection
  outcome, not an exclusion.

## 3. Participation monitoring (what to watch)

The chain makes liveness observable. The metrics below are derivable from
committed data; the observability work itself is §8.1 (Stage 1) — until then,
the capacity-gate harness and the node's straggler counters are the
instruments:

| Metric | Source | Alarm shape |
|---|---|---|
| Per-validator attestation rate | certificate bitmaps (ADR-014) | org < 95% of heights during its term |
| Failure-domain concentration | active-set placement | any domain > 25% (or jurisdiction > 40%) |
| Outbound drop counters per peer | `Node::dropped_outbound(addr)` (#62 §5.6-2) | chronically rising on the same peer — a straggler silently missing blocks |
| Round-timeout frequency | post-adoption-gate: vote rounds | timeouts clustering on the same minority |
| Recovery convergence | capacity-gate harness / operations | convergence time growing round over round |
| Chain-sync lag distribution | peers' chain lengths | a widening gap between p50 and p99 lag |

## 4. Round-timeout guidance (post-adoption-gate)

The round timeout must track the **⅔n-th order statistic** of validator
response times, not the mean and not the p99 of the fastest. Setting it from
the mean makes a Pareto tail dominate every round; setting it from the p99 of
all validators lets the slowest tail set the block time. The capacity-gate
harness's attestation/vote-round latency measurement (Step 0 instrument) is
the tool that produces this number once vote rounds exist; until then no
timeout value is claimable.

## 5. Non-goals

- **No jailing or automatic ejection for absence** (ADR-009 §4).
- **No reputation weighting** — participation facts feed the binary
  eligibility bar only (ADR-009 §2).
- **No protocol work** — nothing here touches the consensus family; the
  ceiling and its bounds are settled (ADR-002, #62 §4).
- **No availability claim** — until the ADR-010 adoption gates land and the
  testnet runs at target scale, these are the rules for *reaching* 300
  reliably, not evidence that we have.
