# Participation model — validators, verifiers, and why "no tiers" keeps failing

**Learned:** 2026-08-24
**Scope:** clarifies the membership ladder already settled by
[ADR-002](../../docs/adr/adr-002-consensus-finality.md) decision 3 and
[ADR-004](../../docs/adr/adr-004-scale-topology.md) decision 3. Does **not** reopen
either. Records the reasoning that three separate design passes rediscovered —
and the four wrong answers they each converged on first.

## Finding

### 1. "Validate" means four different things. Only one of them is bounded.

| Sense | What it is | Cost shape | Who |
|---|---|---|---|
| **Propose** | Choose contents and order of a block | 1 node per height | rotating proposer |
| **Vote** | Cast prevote/precommit counted toward the ⅔+ quorum that makes a block final | **O(n²)** — all-to-all gossip | the validator set |
| **Verify** | Independently recheck a block: signatures, hashes, endorsement policies, schema, state transition | **O(block)** per node, purely local | anyone holding the data |
| **Endorse** | Sign that a specific business transaction is authorized (ADR-008) | per-transaction | **every member org** |

These are **overlapping roles, not tiers**. A validating cooperative is
simultaneously proposer, voter, verifier, and endorser; a smallholder is an
endorser and optionally a verifier. Every member sits in the same *Endorse*
column — the one that authorizes business.

Verification is a unilateral local act. Voting is a global agreement act.
Agreement is what costs quadratically, because agreement requires everyone to
hear from everyone. This is the whole reason the validator set is bounded.

### 2. Liveness is why voting can't be *universal* — but it is not what bounds an institutional set at ~300

**Scope of this argument (narrowed 2026-09-02):** it settles *universal* voting.
It does **not** set the ceiling among institutional validators. Both halves
matter; conflating them has already produced one wrong inference.

**Universal voting fails on liveness.** Tendermint-class BFT needs **⅔+ of the
validator set online and responsive to commit anything**. If the validator set is
every member, then a third of members being offline means no block ever commits.
At ADR-004's 70M-entity horizon — including Tier-3 smallholders transacting from
phones — per-node offline probability is far above ⅓, so that is the normal state
of the world, not an edge case.

**Universal voting is self-defeating**: the standard fix is to evict absent
validators (jailing), and evicting the absent *is* the split it was meant to
avoid. Bounding the set is a liveness requirement, not an exclusion mechanism.
For *this* case the argument is stronger than the O(n²) cost argument, because it
is a correctness argument rather than a budget argument. Lead with it.

**But it does not carry to `n = 300` among institutions.** There, per-node
offline probability is well under ⅓, and Chernoff makes a ⅔ quorum *easier* to
reach as `n` grows, not harder. Independent uptime is the wrong model at that
scale. The real bounds are:

- **Correlated failure** — one BGP event, one cloud region, one DDoS takes many
  orgs at once. Quorum availability is governed by the min-cut of correlated
  sets, not by counting organizations. A federation of Brazilian pharma companies
  sharing three cloud providers gains little independence from more members.
- **Heavy-tailed latency** — the round timeout tracks the (⅔n)-th order statistic
  of validator response time. With Pareto-shaped tails that grows roughly as
  `n^(1/α)`; a few chronically slow members percolate into every quorum.
- **Governance** — as `n` grows, P(at least one quorum member is having a bad
  operations week) approaches 1. That conversation gets worse with more orgs.
- **Bandwidth genuinely does bite for our family.** "Not bandwidth" holds for
  leader-based protocols (O(n) per round). CometBFT's all-to-all vote gossip is
  **O(n²) bytes per round** — at n=1000, ~2×10⁸ B/round. For ADR-002's chosen
  family bandwidth is a real wall, just further out than 300.

The conclusion — bound the set — is unchanged. Reaching 300 *reliably* is
liveness engineering (provider and geographic diversity requirements, per-org SLA
expectations, monitored participation), not protocol work. See
[`../plans/performance.md`](../plans/performance.md) and
[#62](https://github.com/dbbvitor/GlassChain/issues/62).

### 3. Signature aggregation does not escape the bound — but it is the highest-value optimization below it

BLS aggregation kills the linear commit-size term (ADR-002's quorum certificate
is embedded in every block forever). It does not get you out, for four reasons:

- **Aggregators are a tier.** Every practical scheme — aggregation trees,
  Handel, Ethereum's attestation subnets — designates nodes to collect and
  combine. The distinguished role moves and gets a new name; it does not vanish.
- **It does not deliver single-slot finality.** Ethereum reaches finality in
  ~2 epochs and gets there by sampling validators into per-slot committees —
  the sortition ADR-002 rejected. §8.2's "immediate" is literal, so this is
  disqualifying.
- **It is a primitive swap.** AGENTS.md pins ed25519; Tendermint and Malachite
  are ed25519 throughout.
- **It does nothing for liveness.** You still need ⅔ of participants awake.

**General principle:** any scheme that compresses many votes into few messages
creates a distinguished role. The split is not an arbitrary design preference —
it falls out of needing agreement among many parties in bounded time.

**Reframed 2026-09-02:** "does not escape the bound" is right, and it was
misread as "not worth doing." With latency and scalability treated as sell
factors, BLS is **the enabler of the scalability claim**, not a footnote: the
sellable number is not `n`, it is how many participants can *verify*, and
ADR-004's light-client ladder is what reaches them. A ⅔ certificate lands in
every block forever and in every proof a light client checks; BLS collapses
~79 KB to ~0.15 KB and turns 201 verifications into one. Two corrections to the
four reasons above: plain n-of-n aggregation needs only a proof of possession,
**not** a DKG (O(1) at a ⅔ *threshold* is what needs threshold BLS and resharing
on membership change) — and permissioned membership makes that a rare auditable
ceremony rather than an ongoing burden. Verification saves almost no round CPU
(~1–3 ms aggregate vs. tens of ms to batch-verify 200 ed25519); the win is bytes,
storage, and third-party proof size. Sequence it *with or after* the Malachite
decision, since it is a primitive swap. And fix the JSON encoding first — §3a.

### 3a. Before any of that: the wire protocol is JSON, and it costs ~5× for free

Every peer message and gossipsub payload is `serde_json`, which renders `Vec<u8>`
as decimal arrays (`[12,34,255,…]`) — so `Attestation`'s 96 bytes of key and
signature become ~393. Measured: 508 B for a one-attestation certificate against
a 115 B empty baseline; ~79 KB projected for a 201-of-300 quorum against a
measured 11 567 B block. `serde_bytes` takes that to ~34 KB for one serde
attribute and a wire-version bump; a binary codec to ~20 KB.

Certificates are **not persisted with blocks yet** (open ADR-010 gate), so this is
the cheap moment. Reaching for BLS before fixing a decimal-array encoding solves
a 3× problem with a cryptographic migration.

**And note what is not measured:** the committed capacity gate reports *PoW mine
latency* on the dev engine with a degenerate certificate and zero vote rounds.
There is no BFT finality number anywhere. Any latency argument — including one
that defers an optimization as unnecessary — is unfounded until there is.

### 4. Light client ≠ verifying member. This was conflated in three separate passes.

A **light client** verifies headers against the validator set's signatures — it
takes validity *on trust from the quorum*. It cannot detect that validators
signed an invalid state transition, because it does not hold the state.

A **verifying member** (full non-voting node) re-executes and rechecks
everything, locally, with no third party and no RPC dependency.

This matters for two claims that keep getting made and are wrong:

- *"Light clients can publish fraud proofs."* They cannot. Only full nodes can.
  The strength of any dissent/audit backstop is the number of **verifying
  members**, which is not the same as the number of members.
- *"Validators get faster settlement than non-voters."* Under single-slot
  finality a block is final at commit for everyone; a verifying member confirms
  finality locally from the commit certificate. The only real asymmetry is that
  a validator sees the proposal one round early — which, given ADR-004 already
  declines to claim full fair ordering, is a front-running hazard, not a feature.
  Do not formalize it as an incentive.

### 5. Consortium networks invert the incentive question

Public chains need validator rewards because participation is voluntary and
pseudonymous. GlassChain is permissioned, MSP-identified, and **the members are
the beneficiaries**. Two of the three reference architectures ship **zero**
validator rewards and no token (Fabric, Corda); only Thor has one, and Thor is
a public chain.

In a consortium the problem is usually not recruiting validators — large actors
*want* to validate, for resilience, influence, and early visibility. The design
problem is **rationing the burden and stopping that appetite from becoming
privilege**. If an explicit allocation model is ever needed, the fitting shapes
are dues-and-duty or a clearing-house model, not a fee market.

### 6. Share infrastructure, never share authority

A cooperative can run one validator while its 5,000 smallholder members each
hold their own MSP identity and endorse their own transactions with their own
keys. The co-op **cannot** forge those endorsements: ADR-008 counts distinct
principals, and multiple identities from one organization cannot satisfy two
distinct organization principals.

So "the co-op validates for the region" disenfranchises nobody, because
validation confers no authority to disenfranchise anyone with. ADR-002 already
accepted this exact shape one layer down — cooperatives as local identity
providers, framed as "identity-layer delegation, not consensus-layer open
membership." Extending it to node operation is the same move.

### 7. Scaling lives off the ordering path

ADR-004 decision 5 already settled that on-chain load is **commitments, not
events** (~2k events/s → tens of commitments/s), and that "the core chain's
ceiling is not the binding constraint; the off-chain event layer's is."

| Axis | Bottleneck | Tiering pressure |
|---|---|---|
| Write / ordering | O(n²) vote gossip | the only axis that creates a split |
| Dissemination | PDC payload distribution | none |
| Read / query | indexer + gRPC fan-out to 70M members | none |

Two of three axes carry zero tiering pressure and hold most of the real load.
Any proposal that reaches for sharding or sortition is optimizing the one
component that is not the bottleneck.

## Evidence

- Four-sense decomposition: derived from ADR-002 (vote/propose), ADR-008
  (endorse, and its explicit separation from BFT finality and its quorum
  certificate), ADR-003/ADR-004 (verify).
- Liveness threshold and jailing: `bft-at-scale.md` Q1; ADR-002's FBA rejection
  ("a halted ledger during a recall is unacceptable, §5.2").
- O(n²) vote gossip: Tendermint paper arXiv:1807.04938, via `bft-at-scale.md`.
- Commit size linear in n and embedded in every block: `bft-at-scale.md` Q2
  bottleneck 3.
- Sortition rejection: ADR-002 Q4 comparison table (Algorand VRF).
- Reference-architecture reward models: `reference-architectures.md`.
- §2 narrowing, §3 reframing, and §3a certificate measurements: recorded
  2026-09-02 in [`../plans/performance.md`](../plans/performance.md)
  ([#62](https://github.com/dbbvitor/GlassChain/issues/62)), against the primary
  protocol papers and live chain data listed there.

### Two fact-checks worth keeping

Both of these were asserted confidently by a design pass and are false:

- **Gas is execution metering, not a fee.** Per the shipped gas/status reconciliation (plan since deleted):
  `ExecutionLimits { fuel_limit, operation_gas_limit }`, trap on exhaustion,
  and constraint 6 is *"Keep `GasReport` out of the execution result."* There is
  no gas price, no payer, and no balance to debit. `requirements-alignment.md`
  §2.4 confirms **no account, balance, or fee model exists anywhere**.
- **`MetadataTrustScore` has nothing to do with node reputation.**
  `glasschain-core/src/asset.rs` computes it from **completeness of SNCM/Anvisa
  traceability metadata on a `TraceableAsset`** (GTIN, batch, expiry, serial).
  It has no concept of nodes, uptime, or block signatures. Tying it to validator
  participation would also invert its purpose — a smallholder's lot would score
  as less trustworthy because the farmer does not run a server.

## Implication

State the model as **four overlapping roles over one uniform member status**,
never as a stack of tiers. The moment it is drawn as a vertical stack, the next
pass starts attaching privileges to the upper boxes.

Two rules that make "no tiers" true rather than rhetorical:

1. **Governance standing attaches to membership, not to validation.** One member
   organization, one governance vote, whether or not it runs a node. This is the
   single line that prevents the validator set becoming a cartel.
2. **Validating confers no read access, no write authorization, and no fee
   advantage.** Reading private payloads is PDC membership (ADR-003); authorizing
   business is endorsement (ADR-008). Keep all three axes separate.

The real open risk is not the existence of a bounded set but **how members enter
it** — ADR-002 open questions 4 (voting power) and 5 (churn mechanics), both
unresolved and unowned. Objective published eligibility, self-selection with
opt-out, and capped consecutive epochs make it a duty roster; discretionary
admission makes it a cartel. Proposed home for that decision: `adr-009`.
