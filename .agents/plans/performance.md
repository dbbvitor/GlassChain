# Plan — Best-in-class performance within zero-trust, ICP, and LGPD

**Status:** evaluation complete and filed as [#62](https://github.com/dbbvitor/GlassChain/issues/62); implementation not started
**Date:** 2026-09-02 (revised the same day — see [Revision note](#revision-note))
**Issue:** [#62](https://github.com/dbbvitor/GlassChain/issues/62)
**Relates to:** [ADR-002](../../docs/adr/adr-002-consensus-finality.md) (family),
[ADR-004](../../docs/adr/adr-004-scale-topology.md) (ladder),
[ADR-010](../../docs/adr/adr-010-capability-versioning-policy.md) (adoption gates)

---

## Goal

**Latency and scalability are sell factors.** GlassChain should be best-in-class
on both, subject to three non-negotiable constraints: zero trust between
validators, compliance with Brazilian law, and interoperability with ICP-Brasil
credentials. This plan says what "best in class" means for the class we are
actually in, what it would take, and in what order.

The constraints are not a handicap to apologise for. **They are the category we
win in** — see §1.

---

## 1. What class are we in

The comparison set is permissioned, zero-trust, deterministically-final,
regulated-supply-chain ledgers. Two axes, two different peer groups:

**Scalability — our peers are Fabric and Corda, and the constraint is the
advantage.** Hyperledger Fabric's default ordering service is Raft, which is
**crash-fault-tolerant only** — it assumes orderers do not lie, which is exactly
the assumption a consortium of commercial rivals cannot make. Fabric's BFT
orderer (SmartBFT) is a recent addition, and ordering sets in both Fabric and
Corda are typically a handful of nodes.

So the honest positioning is not "we are limited to 300 validators." It is:

> **300 mutually-distrusting validators with deterministic finality, plus an
> authenticated light-client ladder to the 70M-participant horizon.**

That is a stronger scalability claim than any permissioned supply-chain ledger
in production, and it is a claim our zero-trust constraint *creates*.

**Latency — the bar is set by public-permissionless BFT**, and it has moved to
sub-second. The relevant datum for us is in-family and already recorded:
**Malachite finalises in ~780 ms at 100 validators with 1 MB blocks**
(`bft-at-scale.md` Q1, from the project's own experiments). Our blocks are
11.5 KB, not 1 MB.

**Sub-second deterministic finality at 100–300 validators is reachable inside
ADR-002's family.** We do not need a family swap to be best in class. That is
the single most important finding in this document.

> Competitor latency figures for Sui/Mysticeti and Aptos are **not verified in
> this session** and are deliberately not quoted. Establishing the competitive
> bar with primary sources is step 0 below — a performance claim we cannot cite
> is a performance claim we cannot sell.

---

## 2. The honest baseline: we do not have one

**There is currently no latency evidence for the real consensus path.**

`docs/benchmarks/consensus-capacity.md` measures the **dev/test Proof-of-Work
engine**: the reported p50 33–36 ms is *mine latency*, the measured certificate
is the **degenerate 115 B PoW attestation**, and — in that document's own words —
"**no cross-validator vote rounds exist to measure**." It is a capacity and
convergence gate, and an honest one. It is not a BFT latency benchmark and never
claimed to be.

This is a correction to the previous revision of this plan, which cited those
numbers as evidence that latency is already comfortable. It is not evidence of
that. Any best-in-class claim is unmeasured until a harness drives real
attestation rounds across real validators.

---

## 3. What the constraints permit and forbid

Worth stating precisely, because "zero trust" and "ICP compliance" are usually
invoked to forbid things they do not actually forbid.

| Constraint | Forbids | Permits |
|---|---|---|
| **Zero trust** | Raft/CFT ordering. Accepting a leader's aggregate result unverified. Skipping signature verification on any received commit. Trusting a peer's self-asserted org. | Batch signature verification *with a sequential fallback for attribution*. Signature aggregation (BLS). Separating dissemination from ordering. Speculative execution, provided commit still verifies. |
| **ICP-Brasil / MP 2.200-2** | Nothing on the consensus hot path. | ICP-Brasil as an **onboarding and attestation credential**. MP 2.200-2 Art. 10 §2º expressly preserves other means of proving authorship "desde que admitido pelas partes" — see `external-review-verdicts.md`. |
| **LGPD** | PII on-chain. | Everything we already do — and it *helps* latency, because commitment-only blocks are small. |

**The load-bearing design rule:** ICP-Brasil certificates are RSA X.509 with
revocation semantics, and revocation checking (OCSP/CRL) is a **network round
trip**. Putting either on the block path would cost more than every optimization
in this document combined saves. Keep ICP-Brasil at the identity and attestation
boundary; the consensus hot path stays ed25519. Compliance and speed are
compatible *only if that boundary holds* — this is the rule to defend in review.

---

## 4. The ceiling, repositioned

The evidence has not changed: **no production system runs deterministic
per-round ⅔ finality with all `n` participating beyond roughly 209.** Every
larger network changed the participation model, decoupled finality, or weakened
it — all three rejected by ADR-002.

| Network | n | How it gets there |
|---|---|---|
| Cosmos Hub | 180 (live, 2026-09-02) | It doesn't — hard `max_validators` cap |
| SBFT (published evaluation) | 209, f=64 | Leader + collectors + threshold sigs + fast path |
| Aptos | ~130 | Jolteon/Jellyfish (HotStuff-class) |
| Sui | ~100–160 | Narwhal → Mysticeti DAG |
| Polkadot / Kusama | ~297 / ~1000 | **GRANDPA is a separate finality gadget with coarse rounds** |
| Ethereum | ~1M | **Committee sampling** + per-committee BLS |
| Algorand | ~10k | **VRF-sampled committee** per step |
| Avalanche / Solana | ~1.6k / ~3.4k | Subsampling; **non-deterministic finality** |

What changes is the conclusion drawn from it. Previously: "the ceiling is a
governance question, stop optimizing." Now: **`n` is the wrong axis to sell on.**
Nobody buys a supply-chain ledger because it has 1,000 validators; they buy
throughput, finality latency, and the number of participants who can verify.
Validator count is an input to trust, not a performance metric — and at 300
mutually-distrusting validators the trust argument is already won (§1).

So compete on **latency, throughput, and ladder reach**, and treat 300 as the
designed operating point rather than a ceiling to apologise for.

---

## 5. Ordered path

Each step is independently shippable. The ordering is deliberate: the free
structural wins come before the sophisticated ones, because a speculative fast
path saving one network hop over a JSON wire protocol is optimizing the wrong
layer by an order of magnitude.

### Step 0 — Measure. Blocking.

Everything below is guesswork until a harness drives **real attestation rounds
across real validators** and reports p50/p95/p99 finality at 100/200/300. Extend
`crates/glasschain-network/tests/consensus_capacity.rs` rather than starting
over; keep it `#[ignore]`-gated.

In the same pass, establish the competitive bar from primary sources (Mysticeti,
Aptos, Malachite, SmartBFT/Fabric) so the target in §1 is citable.

**Until this exists, no performance claim goes in marketing copy or the README.**

### Step 1 — The wire protocol is JSON. This is the biggest structural tax.

Every peer message and every gossipsub payload is `serde_json`
(`crates/glasschain-network/src/peer.rs` `send`/`receive`;
`libp2p_swarm.rs` transaction and block topics). Two costs: bytes on the wire,
and encode/decode CPU on every hop.

The pathological case is byte arrays. `serde_json` renders `Vec<u8>` as an array
of decimal numbers, so `Attestation`'s 32-byte key plus 64-byte signature — 96
bytes — becomes roughly 393 bytes of `[12,34,255,…]`. Measured: a
one-attestation certificate is 508 B against a 115 B empty baseline. Projected to
a ⅔+ quorum (201 of 300) against a measured 11 567 B block:

| Encoding | Certificate at n=300 | vs. block | Change required |
|---|---|---|---|
| Today (JSON decimal arrays) | **~79 KB** | ~7× the block | — |
| `serde_bytes` / base64 | **~34 KB** | ~3× | A serde attribute and a wire-version bump |
| Binary codec (bincode/postcard) | ~20 KB | ~1.7× | Codec swap behind `peer.rs`; needs an ADR |
| BLS threshold signature | **~0.15 KB** | negligible | Primitive swap + DKG (step 4) |

Two reasons this is first among the real work:

- **Certificates are not persisted with blocks yet** (an open ADR-010 gate), so
  the format is free to change *now* and expensive to change once it is in
  committed history and in light-client proofs.
- **`serde_bytes` is a serde attribute.** It fixes the worst of it without
  touching the codec. Take that rung first; the full binary-codec swap is a
  separate, ADR-sized decision that trades away human-debuggable wire dumps.

Note the re-siting that fell out of this: certificate size is a **storage,
replay, and light-client-proof cost**, not per-round bandwidth. In a
leader-based protocol per-round bytes are ~`n·(payload + signature)` regardless.
That makes it matter more for ADR-004's ladder than for round latency.

### Step 2 — Batch signature verification

`BftConsensusProvider::verify_certificate` verifies sequentially: ~50 µs per
signature, so **~10 ms at a 201-attestation quorum** — the dominant CPU cost on
that path, paid by every node on every block. `ed25519-dalek`'s `verify_batch`
is roughly 2× faster.

**The zero-trust catch:** batch verification reports "some signature in this set
failed," not *which*. A consortium ledger must name the misbehaving validator.
The correct pattern is therefore batch-verify optimistically and fall back to
sequential verification on failure — fast in the common case, fully attributable
in the adversarial one. Costs a dependency feature flag and a fallback branch.

### Step 3 — ~~Quadratic validator lookup~~ *(done 2026-09-02)*

`verify_certificate` scanned the validator `Vec` linearly *inside* the
per-attestation loop — O(n·m), ~60k comparisons at n=300 with a 201-attestation
quorum, and the only quadratic term on the verification path. Now indexed into a
`HashSet` once per call. Minor next to steps 1 and 2 (~0.3 ms), but it was five
lines and it was the term that grew with the ceiling.

### Step 4 — BLS aggregation — promoted, because it serves the scalability story

Collapses the certificate to O(1) and turns 201 light-client verifications into
one. Under the previous framing this was "does not raise the ceiling, therefore
defer." Under a sell-the-scalability framing it is **the enabler of the claim in
§1**: the ladder is how we reach 70M participants, and BLS is what makes a
finality proof cheap enough for a phone to verify.

Honest costs, unchanged:

- **Verification saves almost no round CPU** (~1–3 ms for an aggregate versus
  tens of ms to batch-verify 200 ed25519). The win is bytes, storage, and
  third-party proof size.
- **Plain n-of-n aggregation needs no DKG**, only a proof of possession against
  rogue-key attacks. **O(1) certificates at a ⅔ threshold need threshold BLS**,
  which needs a DKG ceremony and resharing on every membership change.
- **Permissioned membership makes that cheap** — governance-driven membership
  turns DKG into a rare auditable ceremony rather than an ongoing burden. One of
  the few places being permissioned is a straight advantage.
- **It is a primitive swap.** `AGENTS.md` pins ed25519; Malachite is ed25519
  throughout. Sequence it *with or after* the Malachite decision, not before.
  Needs an ADR.

### Step 5 — Speculative fast paths (HotStuff-1, SBFT) — candidate, no longer deferred

The previous revision deferred these on the grounds that "ADR-002 accepted
seconds-level blocks and no business requirement demands sub-second." **A
requirement now exists**, so that rationale is retired. HotStuff-1 frames its win
as two network hops; SBFT's fast path reached ~2× PBFT throughput at 209
replicas.

They stay behind steps 0–2 for a reason that is about sequencing, not merit: a
fast path removes one round trip, while the JSON encoding inflates *every*
message on *every* round trip. Fix the constant factor before shaving hops, then
re-rank against measured p99.

### Step 6 — Narwhal-style DAG mempool — gated on a measured trigger

Separating dissemination from ordering keeps the critical path carrying hashes
rather than payloads, and the papers' claim is **throughput and latency stability
under load** (130k tx/s vs 1.8k for HotStuff at the same `n`). Worth being precise
about the limit: **no Narwhal-family paper claims improved validator-count
scaling** — each block still needs 2f+1 availability votes, so the quadratic
authenticator pattern persists, just in *small* messages.

Architecturally it fits behind `ConsensusProvider` as a mempool layer, so it is
ADR-002-preserving. But it is a large surface (DAG storage, garbage collection,
equivocation handling), and measured pending-pool depth is 20 transactions per
round, drained every block.

**Trigger to record rather than a design to write:** build it when step 0's
harness shows pending-pool depth failing to drain within one round, or block
propagation dominating p99 finality, under a realistic workload.

### Step 7 — Liveness engineering, not protocol work

The bounds that actually bite at 300 among institutional validators are
**correlated failure** (shared clouds and ISPs — quorum availability tracks the
min-cut of correlated sets, not the organization count), **heavy-tailed latency**
(the round timeout tracks the ⅔n-th order statistic, growing roughly as
`n^(1/α)` under Pareto tails), and **governance**. Concretely: geographic and
provider diversity requirements for the validator set, per-org SLA expectations,
monitored participation rates.

This is governance documentation, and for reaching 300 *reliably* it is worth
more than any protocol item above. Belongs with the federation trust model
([#57](https://github.com/dbbvitor/GlassChain/issues/57)).

---

## Validation

- **Step 0** is itself the validation instrument; nothing after it is claimable
  without it. `#[ignore]`-gated, reproducible, with the same honest-scope
  caveats `consensus-capacity.md` already carries.
- Steps 1–2: unit test asserting serialized certificate size at a synthetic
  201-attestation quorum stays under budget; a batch-verify test proving the
  sequential fallback still names the bad signer.
- Every step: the four workspace gates green in **both** feature configurations
  (default and `--all-features` — `bft` gates real code).

---

## Out of scope

- **Raising the validator ceiling by protocol optimization.** Settled in §4; send
  new proposals back here. Note this is a *scoping* ruling, not a performance
  ceiling: §5 is where the performance work lives.
- Reopening ADR-002's consensus family or ADR-004's ladder. Best-in-class latency
  is reachable inside both (§1).
- Committee sampling, VRF sortition, separate finality gadgets — rejected with
  reasons in ADR-002.
- ICP-Brasil certificates or revocation checks anywhere near the block path (§3).
- BLS before the Malachite/BFT adoption decision — sequencing a primitive swap
  ahead of the engine decision risks doing it twice.

---

## Revision note

First revision (2026-09-02) answered a narrower question — "can fast paths or a
DAG mempool surpass 300 validators?" — with "no, so defer them." The ceiling
finding survives unchanged. Three things did not:

1. It cited the PoW mine-latency benchmark as evidence that latency is already
   comfortable. That benchmark measures neither BFT nor vote rounds (§2).
2. It deferred fast paths for want of a sub-second requirement. That requirement
   now exists.
3. It treated BLS as merely "not an escape route." Under a
   sell-the-scalability framing it is the enabler of the ladder claim (§4, §5.4).

---

## Sources

Research recorded 2026-09-02 against primary papers and live chain data:
HotStuff (arXiv:1803.05069), HotStuff-1 (arXiv:2408.04728), Narwhal+Tusk
(arXiv:2105.11827), SBFT (arXiv:1804.01626), Tendermint (arXiv:1807.04938),
GRANDPA (arXiv:2007.01560), ethereum.org proof-of-stake documentation, Algorand
consensus documentation, Cosmos Hub live staking parameters. Validator counts for
Aptos, Sui, Polkadot/Kusama, Solana, and Avalanche are secondary and marked
unverified in the research record; **competitor finality latencies are not yet
verified and are step 0 work.** See also
[`../memories/bft-at-scale.md`](../memories/bft-at-scale.md) and
[`../memories/participation-model.md`](../memories/participation-model.md).
