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

## 5. Tail at scale

Assessed 2026-09-02 against the shipped code, in the sense of Dean & Barroso's
"The Tail at Scale": in a system where one operation touches many components,
the p99 of each component becomes the p50 of the whole.

**Verdict: the built paths are structurally sound — the dangerous one is
unbuilt, one path is genuinely exposed, and we are blind.**

### 5.1 Broadcast is tail-tolerant, and that is the important one

`Node::broadcast` (`crates/glasschain-network/src/node.rs:1919`) snapshots the
sender list, then `try_send`s into per-peer bounded channels (256 slots,
`handle_peer`, same file), each drained by its own writer task. **The broadcaster
never awaits a peer.** One slow validator cannot stall fan-out to the other 299;
it fills its own channel and nothing more. That is precisely the
decouple-the-caller-from-the-straggler pattern, and it is the single most
important thing to have got right.

The cost is that the tail is converted from latency into **silent message loss**:
a full channel logs `"Dropping outbound message: peer channel full"` and moves
on. A chronically slow peer silently misses blocks and resyncs later. That is a
defensible trade — but it is a `log::warn!` and nothing else. No counter, no
per-peer metric. **The straggler is invisible.**

### 5.2 The dangerous path does not exist yet

The classic BFT tail exposure is waiting for the 201st of 300 attestations.
**That code is not written.** `BftConsensusProvider::attest` produces one local
attestation, and `bft.rs`'s own module doc records that gathering attestations
from remote validators is an ADR-010 adoption gate.

So we are not vulnerable *because the vulnerable component is unbuilt* — the
decision is ahead of us, not behind us. Two things to carry into it:

- A ⅔ quorum is **inherently a partial hedge**: you take the 201st fastest and
  discard the slowest 99. That is far better than any fan-out that waits for all
  `n`. But 201/300 is the 67th percentile of the response distribution, which
  under Pareto tails is still deep — this is the `n^(1/α)` order statistic from
  Step 7, made concrete.
- **Do not invent hedging.** Tendermint's round timeout already *is* the hedge.
  The risk when Step 5 or the Malachite path lands is re-deriving it badly, not
  omitting it.

### 5.3 One path is genuinely exposed

`Node::reconcile_private_payloads` (`node.rs:1055`) picks **exactly one** target
peer, then fires every missing-payload request at it with **no timeout, no retry,
no failover, and no hedge** — and returns the count of requests *sent*, not
received. One slow member and reconciliation silently does nothing.

Blast radius is small (an infrequent operator action, not a hot path) and the fix
is cheap: fan the requests across all member peers instead of choosing one.

Relatedly, **there are no timeouts anywhere on the peer path**. `peer.rs`
`send`/`receive` have none; the only `Duration` in `node.rs` is a 5-second
reconnect sleep. Today that is masked by the `try_send` decoupling. It will not
be once any request/response path matters.

### 5.4 The measurement instrument is broken — act on this first

From the recorded gate (`docs/benchmarks/consensus-capacity.md`, 200 validators):

| Round | fan-out 50% | fan-out 100% |
|---|---|---|
| 1 | 496 ms | 0 ms |
| 5 | 7 941 ms | 98 ms |
| 9 | **16 354 ms** | 34 ms |

That is **incoherent as a network measurement** — 100% cannot be reached in 34 ms
immediately after 50% took 16 seconds. The cause is in `propagation_ms`
(`crates/glasschain-network/tests/consensus_capacity.rs:193`): each poll sweeps
*every* connected validator taking `.lock().await` on its ledger, then sleeps
20 ms, and the three thresholds run as sequential polls with independent start
times. The 50% poll absorbs all the lock contention with ongoing commit work; by
the time the later polls begin, everything has already arrived.

**It measures the harness, not the network** — and the sweep is itself
O(connected) lock acquisitions per 20 ms tick, so the instrument degrades as the
thing it measures grows. `consensus-capacity.md` calls this family
"supplementary"; that is too generous.

### 5.5 A write-ahead log is not the answer — but the question found one

Assessed 2026-09-02. **No: we should not build a WAL. We already have two, and a
third would make the tail worse.** The investigation did surface a real exposure
at the same location, so the question earned its keep.

Why not:

1. **Sled is already a log-structured store with its own write-ahead log.**
   Hand-rolling one on top of `sled 0.34.7` reimplements the dependency we
   already pay for.
2. **The block log is semantically already a WAL.** `apply_block`
   (`crates/glasschain-storage/src/sled_backend.rs:85`) writes the block and its
   derived write set in one multi-tree transaction; on failure the chain stays
   authoritative and rebuild-from-chain heals the derived state (ADR-007
   decision 2). Write-ahead, then replay on recovery — that *is* the pattern, and
   `failure_after_block_durable_is_healed_by_rebuild`
   (`node.rs:3718`) is the test that proves it.
3. **A WAL is a durability mechanism, and `fsync` is a *cause* of tail latency,
   not a cure.** Dean & Barroso name background daemons and queueing among the
   sources of tail; a synchronous log flush on the commit path adds one.

#### What the question actually found: persistence sits in front of relay

`after_block_commit` calls `storage.apply_block(block)` — synchronous, blocking
disk and CPU work — directly on a Tokio worker thread with **no
`spawn_blocking`**. On the mine path it sits **between ledger append and
broadcast**:

| | Ledger append | Blocking persist | Broadcast |
|---|---|---|---|
| `mine_async` | `node.rs:1518` | `node.rs:1962` | `node.rs:1546` |
| `process_message` | `node.rs:2675` | `node.rs:1962` | — (see below) |

**Scope this honestly — it does not compound per hop.** `Message::Block` is
broadcast from `mine_async` and *nowhere else*; a peer receiving a block never
re-relays it on the TCP path, and gossipsub mesh forwarding happens inside libp2p
before our handler runs. So there is no multi-hop amplification. Two narrower
effects remain, and both are real:

- **Mine path:** the leader's storage latency is inserted in front of fan-out to
  *every* peer. One node's p99 disk becomes everyone's propagation p50 — the
  tail-at-scale shape, at one hop rather than many.
- **Peer path:** the blocking call runs on the per-peer read task, so subsequent
  messages from that peer queue behind it (head-of-line blocking on that
  connection), and it occupies a runtime worker thread while it runs.

Magnitude is unmeasured (§2 applies). The fix is scheduling, not durability, and
it is a smaller diff than a WAL: broadcast first, persist after. The in-memory
ledger is *already* appended before `after_block_commit`, and ADR-007 decision 2
already makes the chain authoritative with rebuild healing any storage
divergence — so moving persistence behind the broadcast changes no invariant.
`spawn_blocking` is the fallback if ordering turns out to matter somewhere not
yet identified; it fixes the reactor-blocking but keeps the latency in the path.

#### A real durability gap, recorded but not fixed here

`SledStorageProvider::flush` is **never called outside tests**, and sled 0.34's
`flush_every_ms` defaults to `Some(500)`. Up to half a second of committed blocks
can therefore be lost to power loss. Replication masks this — a recovered node
resyncs from peers — *except under correlated failure*, which §6 Step 7 already
names as the bound that actually bites.

Do not reflexively "fix" this by adding a flush: it trades exactly the tail
latency this section is about, on a path that is already in front of relay. It
needs Step 0's measurement first, and then it is a policy decision with a knob,
not a default.

#### The one WAL technique worth keeping on the shelf

**Group commit.** If durability is ever made explicit, batch the flush across
concurrent commits rather than paying it per block — that is the WAL family's own
answer to the tail it would otherwise create. Trigger is the durability decision
above, not this one. Nothing to batch today, because nothing flushes today.

### 5.6 Actions, ranked

1. ~~**Delete or fix the 50% fan-out column.**~~ *(done 2026-09-03)* All three
   thresholds now measured from one start in one poll loop
   (`consensus_capacity.rs::propagation_ms`); output is monotone within every
   round. Residual recorded in `consensus-capacity.md`: the sweep is still
   O(connected) locks/tick and the cross-round fan-out growth (47 → 1 227 ms at
   200) is unattributed — carry into Step 0 attribution.
2. ~~**Count the `try_send` drops, per peer.**~~ *(done 2026-09-03)*
   `NodeState.dropped_outbound: HashMap<addr, u64>`; the warn now names the
   peer and the cumulative count; `Node::dropped_outbound(addr)` exposes it.
   A full metrics registry remains §8.1/Stage 1 work — not built here.
3. ~~**Move persistence behind broadcast** on the mine path, and off the
   reactor on the peer path (§5.5).~~ *(done 2026-09-03)* `mine_async`
   broadcasts the block before `after_block_commit`; on the peer path
   `apply_block` runs in `spawn_blocking` (awaited, preserving per-peer block
   order).
4. ~~**Fan reconcile across all member peers**, and put a timeout on it
   (§5.3).~~ *(done 2026-09-03)* `reconcile_private_payloads` sends every
   missing-payload request to every member peer (the single arbitrary target
   was a silent coin flip between holder and non-holder). A timeout was *not*
   added: requests are fire-and-forget `try_send` into per-peer channels and
   the answer arrives asynchronously — there is no in-function await to time
   out. The validation test (`pdc_distribution::reconcile_fans_out_across_all_member_peers`)
   proves completion with a payload-less member present.
5. **Carry §5.2 into the quorum-gathering work** when Step 5 or Malachite lands.
6. **Record the flush gap** (§5.5) as a durability *decision* with a knob, gated
   on Step 0 — not as a default change.

Items 1 and 2 are prerequisites for Step 0 meaning anything: the plan's first
step is "measure," and this section says the measuring tools need fixing before
they can.

### Step 0 status (2026-09-03)

The instrument fixes (items 1–2) are in, and the fixed harness has been run at
200/300 (see `docs/benchmarks/consensus-capacity.md`). **BFT finality latency
remains unmeasurable**: no cross-validator attestation/vote gathering exists
(`core/bft.rs` produces a one-attestation certificate; the wire protocol has no
Attestation/Vote variants; peers reject BFT blocks at `has_valid_pow`) — all of
it is the ADR-010 adoption-gate work. Until that lands, Step 0's honest output
is the propagation/recovery/PDC numbers above plus the competitive-bar
research; p50/p95/p99 *finality* at 100/200/300 stays blocked.

---

## 6. Ordered path

Each step is independently shippable. The ordering is deliberate: the free
structural wins come before the sophisticated ones, because a speculative fast
path saving one network hop over a JSON wire protocol is optimizing the wrong
layer by an order of magnitude.

### Step 0 — Measure. Blocking.

Everything below is guesswork until a harness drives **real attestation rounds
across real validators** and reports p50/p95/p99 finality at 100/200/300. Extend
`crates/glasschain-network/tests/consensus_capacity.rs` rather than starting
over; keep it `#[ignore]`-gated.

**Fix the instrument in the same pass** — the existing fan-out thresholds measure
the harness's own lock contention rather than propagation (§5.4), and a
straggler currently leaves no trace but a log line (§5.1). Percentiles are
worthless if the tool that produces them is the bottleneck.

Also establish the competitive bar from primary sources (Mysticeti, Aptos,
Malachite, SmartBFT/Fabric) so the target in §1 is citable.

**Until this exists, no performance claim goes in marketing copy or the README.**

### Step 1 — The wire protocol is JSON. This is the biggest structural tax.

> **Status (2026-09-03): base64 encoding + algorithm discriminant shipped.**
> Signature-adjacent byte fields (`Attestation`, `EndorserIdentity`,
> `RecordSignature`) are base64 on the wire via `core::wire::base64_bytes`;
> each carrier names its algorithm (`core::wire::SignatureAlgorithm`, omitted
> when Ed25519, unknown discriminants rejected — post-quantum action 2);
> `ValidatorInfo.public_key` widened to `Vec<u8>`; `PROTOCOL_VERSION` bumped
> to `glasschain/5`. Validated by the 201-attestation size-budget test
> (< 40 KB, was ~79 KB) and the unknown-discriminant round-trip test. The
> full binary-codec swap remains the separate, ADR-sized decision below.

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

**Fold the algorithm discriminant in here.** Nothing on the wire currently says
which algorithm produced a signature — `Attestation`, `RecordSignature` and
`EndorserIdentity` carry bare byte vectors, and `ValidatorInfo.public_key` is a
`[u8; 32]` that cannot physically hold a post-quantum key. Since this step is
already taking a wire-version bump, adding a discriminant costs almost nothing
now and a second break later. See
[`post-quantum.md`](post-quantum.md) §3 — this is that plan's action 2, and it
has no separate schedule.

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
- **It is not future-proofing.** BLS is pairing-based, so Shor breaks it exactly
  as it breaks ed25519, and there is no post-quantum aggregate signature with
  comparable properties today. This does not cancel Step 4 — NIST IR 8547 (ipd)
  puts the horizon at 2035, a full useful life — but the ADR must not sell it as
  durable, and the algorithm discriminant in
  [`post-quantum.md`](post-quantum.md) §3 becomes *more* important, because BLS
  is a second signature scheme on the wire before any post-quantum one.

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
  caveats `consensus-capacity.md` already carries. It is not trustworthy until
  §5.6 items 1 and 2 land — a broken instrument produces confident wrong numbers,
  which is worse than no numbers.
- §5.6 item 3: a propagation measurement taken with persistence behind broadcast,
  compared against the current ordering — the number that says whether moving it
  is worth the change.
- §5.6 item 4: a test proving reconcile still completes when one member peer
  never answers.
- Steps 1–2: unit test asserting serialized certificate size at a synthetic
  201-attestation quorum stays under budget; a batch-verify test proving the
  sequential fallback still names the bad signer.
- Every step: the four workspace gates green in **both** feature configurations
  (default and `--all-features` — `bft` gates real code).

---

## Out of scope

- **Raising the validator ceiling by protocol optimization.** Settled in §4; send
  new proposals back here. Note this is a *scoping* ruling, not a performance
  ceiling: §6 is where the performance work lives.
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
   sell-the-scalability framing it is the enabler of the ladder claim (§4, §6.4).

A tail-at-scale assessment was added as §5 on the same day, in response to a
direct question. It did not change any conclusion above, but it moved two items
ahead of Step 0: the propagation instrument is measuring itself, and dropped
messages to a slow peer leave no trace but a log line.

§5.5 was added on the same day, in response to a direct question about whether a
write-ahead log could mitigate the tail. The answer is no — sled is already
log-structured, the block log is already write-ahead-then-replay, and `fsync` is
a tail *source*. But reading the commit path to answer it found that blocking
persistence sits in front of broadcast on the mine path and on the reactor on the
peer path, and that nothing flushes outside tests, which is a genuine durability
gap. Both are now ranked in §5.6; neither changes the ordered path.

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
