# ADR-002 — Consensus and transaction finality

**Status:** **Resolved 2026-08-20** — BFT, Tendermint/CometBFT-class, full
participation by every member organization (see Decision below). "Immediate"
confirmed literal (Options A and D rejected).
**Date:** 2026-08-18
**Relates to:** §8.2, §1.1, §1.3 · [`requirements-alignment.md`](requirements-alignment.md) D2

## Context

§8.2 requires "immediate, deterministic transaction finality (preventing chain
forks)". The ledger today uses Proof-of-Work with longest-chain resolution:

- `PowConsensusProvider` is the only `ConsensusProvider` implementation
  (`glasschain-core/src/providers.rs:219`).
- Forking is not a defect but a designed behaviour — `chaos_tests.rs` contains a
  passing `test_concurrent_mining_longest_chain_wins`, and `madsim_chaos.rs`
  asserts longest-chain convergence after a partition merge.

PoW gives probabilistic finality. No amount of difficulty tuning turns it into
deterministic finality; this requires a voting-based consensus.

This is the highest-leverage unresolved decision: **§1.3 (endorsement), §1.4
(channels), and the entire workflow engine hook into the commit path**, and
"committed" means something different under each option.

### Resolved: "immediate" is literal

**2026-08-18, requirement owner.** §8.2's "immediate" is a hard requirement, not
prose. A block must be final at the moment it commits — not one quorum-round
later. This eliminates **Option D**, and with it the possibility of keeping any
part of the current mining path. The ledger must move to a consensus in which a
fork cannot form, rather than one in which forks form and are resolved.

The decision is now **B vs. C, on threat model alone.**

## Options

| Option | Finality | Fault model | Cost | Meets §8.2 |
|---|---|---|---|---|
| **A. Keep PoW** | Probabilistic | Byzantine, open membership | zero | ✗ |
| **B. Raft (CFT)** | Deterministic, immediate | Crash-fault only; assumes non-malicious validators | Moderate | ✓ |
| **C. IBFT/PBFT (BFT)** | Deterministic, immediate | Byzantine, tolerates f of 3f+1 malicious | High | ✓ |
| **D. Finality gadget over separate production** | Deterministic, *lagged* | Byzantine at the finality layer | Moderate | ✗ — **rejected**, see above |

### Option D — rejected, retained for the record

Rejected because finality lags production. Kept here because the reasoning is
reusable if the requirement is ever revisited.

VeChain Thor does not obtain deterministic finality from its consensus algorithm.
It keeps `consensus/` (PoA validation) and `packer/`/`scheduler/` (proposal
ordering) as one subsystem, and layers a **separate `bft/` package**
(`engine.go`, `justifier.go`, `casts.go`) that marks a block final once a
validator quorum has voted for it. Block production and finality are decoupled.

Applied literally to GlassChain — keep `PowConsensusProvider`, add a justifier —
this is the cheapest path to a finality guarantee, because it does not touch the
existing mining path or invalidate the fork-resolution chaos tests.

**But the literal application is a trap.** Thor's production layer is *PoA*:
identity-based, near-zero cost, and already permissioned. GlassChain's is *PoW*.
Bolting a finality gadget onto PoW keeps the entire cost of Byzantine-resistant,
open-membership block production in a network where §1.1 has already established
every validator's legal identity — paying twice for a property we get for free
from the MSP. A faithful port of Thor's design is therefore not one change but
two: replace PoW production with PoA, *then* add the finality gadget.

Option D also fails §8.2 outright. The requirement says "immediate, deterministic"
finality "preventing chain forks". Under D, forks still occur below the finality
threshold and are resolved afterwards; the ledger is fork-*tolerant*, not
fork-*preventing*. Under B and C a fork cannot form at all. With "immediate"
confirmed as literal, this is disqualifying.

## Decision

**2026-08-20, requirement owner — wayfinder ticket
[#16](https://github.com/dbbvitor/GlassChain/issues/16), grilling rounds 1–3.
The earlier recommendation below (Raft, conditional on a neutral validator
subset) was overtaken by the ownership answer: the validator set is *not*
neutral.**

1. **Validator-set ownership: every member organization.** Zero trust is the
   stated posture — commercial rivals operate validators, and censorship or
   reordering by a validator must be defeated by the protocol, not by off-chain
   recourse. → **Option C (BFT)**. Raft is eliminated.
2. **Scale: 200+ voting validators**, all MSP-identified.
3. **Participation: full** — every member org votes on every block. No
   committee, election, or rotation in v1. The chosen family supports
   per-height validator-set changes, so bounding the set later is
   configuration, not redesign.
4. **Ledger boundary: one chain.** Edge/local aggregation, if it ever exists,
   is pure transaction ingress; its confirmations are never treated as
   committed state. (Keeps the Option D rejection intact.)
5. **Permissioned membership, no exceptions in v1.** Every participant —
   Tier-3 smallholders included — transacts under an MSP-issued identity.
   Smallholder onboarding is delegated to cooperatives/aggregators acting as
   local identity providers (identity-layer delegation, not consensus-layer
   open membership). Basis: INC 02/2018 and RDC 157/2017 custody-chain
   identity requirements; NF-e fiscal-document alignment.
6. **Commit latency: a few seconds per block is acceptable.** §8.2's
   "immediate" remains final-at-commit; the block interval need not be
   sub-second.

### Consensus family

The stated profile — BFT, 200+ *identified* validators, full participation,
seconds-level commits, permissioned membership — is deliverable today by
**Tendermint/CometBFT-class BFT** (single-slot deterministic finality;
production-proven at ~180 validators).

Rejected during the grilling:

- **FBA (Stellar/SCP):** sacrifices liveness under partition (a halted ledger
  during a recall is unacceptable, §5.2), and its safety depends on
  hand-maintained global quorum-slice intersection across 200+ orgs.
- **Classic aBFT (Hashgraph, HoneyBadger):** requires a fixed, known validator
  set and has no production evidence at 200+ (largest deployments ≤ ~40–100).
- **FBA-edge + aBFT-core tiered hybrid:** its edge solves an open-membership
  problem this network does not have (point 5), and its bounded anchor core
  contradicts full participation (point 3).

The Rust implementation path behind `ConsensusProvider` is verified by
[#23](https://github.com/dbbvitor/GlassChain/issues/23) — candidate: Malachite
(Informal Systems' Tendermint). The swap plan is Stage 2 execution work, not
this ADR.

## Consequences

- Contained by design: `ConsensusProvider` is the correct abstraction and the
  swap does not touch the transaction model.
- **Retain `PowConsensusProvider` as a second implementation rather than deleting
  it.** All three reference systems ship multiple consensus implementations behind
  one seam (Fabric `etcdraft` + `smartbft`; Corda JPA/Raft/BFT-SMaRt notaries;
  Thor PoA v1/v2/PoS). A seam with a single implementation is unproven, and PoW is
  the cheapest way to keep this one honest — as a dev/test consensus, not a
  production option.
- **Not contained in meaning.** Under Raft there is one canonical chain and no
  reorganisation. Code that assumes forks can be discarded — but the chaos tests
  that *assert* fork resolution (`test_concurrent_mining_longest_chain_wins`,
  `test_madsim_application_layer_partition_and_merge`) become invalid and must be
  rewritten to assert liveness/quorum behaviour instead.
- Validator set management becomes a governance concern, tying into §1.2 RBAC.
- **Design the seam so C is a later swap, not a rewrite.** The cheapest insurance
  against getting the B/C call wrong is to make `ConsensusProvider` carry a
  *quorum certificate* — the set of validator signatures attesting a block — from
  day one, even though Raft does not need one to be safe. If the interface only
  exposes "the leader said so", swapping in BFT later means changing every
  consumer of commit notifications. If it exposes an attestation set, Raft supplies
  a degenerate one and BFT supplies a real one.
- **The chosen family is partially synchronous, not asynchronous.** Safety holds
  regardless of network timing; liveness requires messages to arrive within
  timeout bounds. Timeouts must be tuned for a global WAN. This is the accepted
  price of deterministic single-slot finality at 200+ validators. (Fully
  asynchronous BFT was evaluated and rejected — see Decision.)
- **Scaling ladder (ADR-004).** Full participation holds while the validator count
  is within the practical BFT ceiling (~300). At national scale the validator set
  is a bounded institutional set and other members participate as authenticated
  light clients. The family choice is unchanged at every rung; bounding the set
  is configuration, not redesign.
- **Bounding the set is a liveness requirement, not an exclusion mechanism, and
  the cost argument is the weaker one.** A quorum needs ⅔+ of the validator set
  online and responsive to commit anything. If every member votes, a third of
  members being offline halts the chain — and at ADR-004's 70M-entity horizon,
  with Tier-3 smallholders transacting from phones, a third offline is the normal
  state of the world. Universal voting is also self-defeating: the standard fix is
  to evict absent validators (jailing), and evicting the absent *is* the split it
  was meant to avoid. State this before the O(n²) bandwidth argument; it is a
  correctness argument rather than a budget argument.
- **Validating confers no authority.** The validator set orders blocks and nothing
  more. It cannot read a private payload (that is PDC membership, ADR-003), cannot
  authorize a business change (that is endorsement, ADR-008 — a colluding validator
  set still cannot manufacture a custody transfer), and must not carry governance
  standing, fee advantage, or settlement privilege. Keep the three axes separate;
  every design pass so far has collapsed them. See
  [`participation-model.md`](../memories/participation-model.md).
- **Consensus input is bounded by design.** The BFT core receives approved public
  canonical records and commitment envelopes, not private commercial payloads,
  raw evidence, or unbatched high-frequency telemetry. This avoids treating 70M
  identities as 70M validators or raw event submissions as the consensus workload;
  the exact compact-workload capacity remains a GlassChain benchmark question
  ([ADR-010](adr-010-capability-versioning-policy.md)).
- **Engine risk remains explicit.** Malachite is a staged, default-off candidate
  behind `ConsensusProvider`; its pre-1.0 status, stewardship, licensing, audit,
  and 200/300-validator testnet results remain adoption gates. `tendermint-rs` is
  a type/light-client toolkit, not an engine.
- The `mine` / `mine-async` REPL commands and `MineBlock` RPC lose their meaning
  and need replacing (README and proto both change).

## Open questions

1. ~~Does §8.2's "immediate" admit lagged finality?~~ **Answered — no, it is
   literal.** Option D rejected.
2. ~~Who operates the validator set?~~ **Answered 2026-08-20 — every member
   organization, zero trust, 200+ validators, full participation.** See Decision.
3. ~~Target throughput and validator count?~~ **Validator count answered (200+).**
   Throughput remains unquantified ("high throughput", §8.2). Production evidence
   at 200 validators establishes feasibility, not a ceiling; the compact GlassChain
   workload at 200/300 validators must be measured before a number is claimed.
4. **Voting power:** one-org-one-vote is assumed for v1. Weighting (stake or
   reputation) would require governance input and an economics model that does
   not exist (Stage 4). Recorded as an explicit assumption — revisit before the
   consensus swap ships; do not let it stay silent. **Constraint on any answer:**
   governance standing attaches to membership, not to validation (CONTEXT.md).
   A proposal that grants network-rule votes only to validators is rejected on
   sight — it disenfranchises the members least able to run infrastructure and
   hands the rules of a compliance ledger to its largest commercial actors.
5. **Validator-set churn mechanics** (join/leave cadence, per-height updates):
   part of the consensus-swap execution plan, to be designed when #23's
   implementation-path findings land. **This, not the size of the set, is where
   the legitimacy risk actually lives.** Objective published eligibility,
   self-selection with opt-out, and a cap on consecutive epochs make it a duty
   roster; discretionary admission makes it a cartel. Random sampling (VRF/RANDAO
   sortition) is **not** an answer here — it was rejected in the Decision above,
   and GlassChain has no stake weighting to make committee capture improbable.

Open questions 4 and 5 together determine whether the membership ladder is
legitimate, and both are currently unowned. Proposed home for the decision:
`adr-009-validator-eligibility.md`.
