# Plan — Best-in-class performance within zero-trust, ICP and LGPD constraints

**Status:** active. Concluded 2026-09-14, in step order: **Step 0** (WAN/mem
baselines + per-phase round timing), **Frontier C's scaling lever / Step 4**
(blst, ADR-015, #121 merged), **Step 1** (codec profiled at the BLS shape:
569–660 B, 1.0–1.5 µs — JSON stays the wire), **Step 3** (D3 incremental
index in `Ledger`: admission ~0.19 ms flat, was ~21 ms at 10k), **Step 6
first installment** (bounded 8 000-tx pool + stats + handshake re-audit
clean through 140 ms). Durability decided (ADR-016). The latency-opportunity
plan shipped its implementable candidates with gates executed: concurrent
vote verification, priority lanes, height-bounded catch-up (wire `/7`),
reconnect backoff — 300-gate p50 4 612 → 4 117 ms; **Step 6 batching shipped**
(4 000-tx slice; the previously failing 9 000-tx probe now sustains:
740 KB blocks converge 8/8, finality p50 5 275 ms under 9 000-tx offered
load); scale table complete (10/100/200/300 → 194 ms/1.1 s/2.5 s/4.1 s);
§5 read-path gaps closed (lagging-subscriber drop counts, burst-vs-steady);
Step 5 fault profile recorded; #6 declined for now (ADR-002 amendment note);
block-relay gossip measured and reverted. Open: node-level peak-RSS
harness; 400/500 sweeps under hardware budgets; **Step 7 remains deployer
evidence, not code work**.
**Reviewed:** 2026-09-14 against main
**History:** [Performance programme](https://github.com/dbbvitor/GlassChain/issues/62) is closed, not proof that every step or production gate passed.
**Related:** [ADR-002](../../docs/adr/adr-002-consensus-finality.md), [ADR-004](../../docs/adr/adr-004-scale-topology.md), [ADR-010](../../docs/adr/adr-010-capability-versioning-policy.md), [ADR-014](../../docs/adr/adr-014-bls-aggregated-certificates.md), [ADR-016](../../docs/adr/adr-016-quorum-replication-durability.md), [zero-trust](zero-trust.md), [source-comment debt](deferred-code-debt.md).

## Goal

Compete on finality latency, sustained committed throughput, resource cost and
traceability—not validator count alone. **Best-in-class is an objective, not a
measured achievement.** Seek sub-second finality within the accepted quorum-BFT
architecture without weakening authentication, endorsement, confidentiality,
finality or deployment-specific legal obligations. Compare with Fabric's BFT
ordering as well as Raft, and distinguish competitor lab results from production.

## 1. Current implementation and evidence

`rounds.rs` implements a staged, default-off Tendermint-shaped proposal → prevote
→ precommit driver. It is **not HotStuff-1**, and there is no client-visible
speculative execution/rollback API. BLS aggregation does not establish a HotStuff
protocol or a production safety proof. The accepted family remains ADR-002.

[Recorded benchmark evidence](../../docs/benchmarks/consensus-capacity.md), not
re-run by this documentation review:

| Harness / scale | Recorded result | Scope |
|---|---|---|
| `bft_finality_gate_100` | p50 ~1.1 s (2026-09-14: 1 145/1 096 ms across runs) | Release, loopback, shared runtime, synthetic workload, blst backend |
| `bft_finality_gate_200` | p50 2 397 ms | Same; exact-quorum certificates |
| `bft_finality_gate_300` | **passes** on the blst backend: p50 3 996–4 612 ms (2026-09-13/14 runs); 4 117 ms with the latency-plan optimizations | Same; exact quorum 201 every round |
| `consensus_capacity` 200/300 | PoW production/propagation, recovery and PDC measurements | Not BFT finality or WAN/testnet evidence |

Short runs do not establish a reliable p99; the table preserves the reported
figures, not a tail-latency guarantee. The blst swap (ADR-015) is measured —
see the benchmark record's before/after table. There is no demonstrated 10×
speedup or automatic pass beyond 300.

### Shipped and superseded work

- Broadcast fan-out uses independent bounded peer channels with `try_send`;
  full broadcast channels increment per-peer `dropped_outbound` counters;
  consensus/background priority lanes split each peer's queue
  (`dropped_outbound` vs `dropped_background`).
- Mine-path block broadcast precedes persistence; `after_block_commit` offloads
  `storage.apply_block` through awaited `spawn_blocking`.
- PDC reconciliation now fans requests to all member peers. Its return value is
  requests queued, **not** payloads delivered; completion/retry remains separate.
- Propagation thresholds share a start/poll loop; stride sampling reduces the
  observer's lock contention. `NodeState` caches capability history incrementally.
- Wire versions `/5`–`/7`: base64/signature discriminants and BLS QCs, then
  height-bounded chain catch-up (`RequestChainFrom` + `Chain { from_index,
  blocks }`, wire `/7`). Current peer codec remains JSON. `Attestation` was
  removed by ADR-014.
- Ed25519 batch verification and the old attestation lookup optimization were
  superseded by BLS. Do not reimplement obsolete steps.
- Latency-plan candidates (`.agents/plans/latency-opportunities.md`):
  bounded-concurrency vote verification, consensus priority lanes,
  height-bounded catch-up and reconnect backoff shipped 2026-09-14; block-relay
  gossip measured and **reverted** (negative result, benchmark record).
- Liveness placement/SLA guidance exists in `docs/liveness.md`; guidance is not
  proof of runtime placement enforcement or measured fleet availability.

**Residual D3 — resolved 2026-09-14 (Step 3).** `Ledger::add_transaction`
now maintains an incremental capability history + committed-ID set folded
block-wise (reset on chain replacement); admission is flat in history size
(~0.19 ms at a 10 000-record history — was ~21 ms; see the benchmark
record).

## 2. Can optimizations help beyond 300 validators?

**Potentially; 300 is a design/test operating point, not a mathematical BFT limit.**
Earlier revisions inferred impossibility from a small set of published network
sizes. That inference is withdrawn. Faster verification, less allocation,
smaller messages and improved dissemination can move an implementation's feasible
operating point. They cannot eliminate quorum communication, correlated outages
or resource limits, and there is no evidence yet that this implementation meets
its target at 300, much less beyond it.

First pass 100/200/300 with correct authentication and safety gates. Then, if
that evidence and hardware budgets justify it, run an explicitly experimental
400/500-validator sweep with the **same quorum/participation/fault assumptions**.
Report which latency, bandwidth, memory or recovery budget fails first. Do not
change to sampled committees or probabilistic finality to improve the headline.
The 70M-participant ladder remains an architectural horizon, not measured reach.

## 3. Security and compliance boundaries

- Never skip vote/QC verification, proof of possession, scope checks or
  endorsement to make a benchmark pass. Correctness gates precede optimization.
- Keep live external certificate/OCSP retrieval out of consensus decisions.
  **CRL verification can be local** once evidence is loaded. Historical
  authorization needs deterministic, height-bound evidence, not current wall time.
- Ed25519 remains on application identity/endorsement paths; staged consensus
  uses classical BLS. Neither is post-quantum. See [post-quantum plan](post-quantum.md).
- Off-chain payloads and on-chain hashes reduce disclosure; they do not by
  themselves establish LGPD compliance or anonymization. Model retention,
  linkage, access controls and lawful evidence preservation explicitly.
- A dependency containing C/assembly is not forbidden by the workspace Rust
  lint. Review each provider's security, interoperability and CI footprint;
  transport and consensus migrations need not have identical decisions.

## 4. Tail at scale and durability

Quorum collection now exists and **is exposed to tails**. Taking `floor(2n/3)+1`
responses avoids waiting for every validator, but shared CPU, leader-side
verification, slow disks, saturated queues and correlated network loss can still
stall the quorum or cause repeated rounds. Count full-channel drops by message
class and peer; a fast enqueue is not reliable delivery.

Do not use the old `n^(1/alpha)` Pareto formula for a fixed-fraction quorum:
that describes extremes, not a universal 2/3-quorum scaling law. Under independent
identically distributed delays, the order statistic tends toward the relevant
quantile; at scale dependencies and changing load invalidate that simple model.
Measure faults and correlation rather than extrapolating from a formula.

**Durability decided (ADR-016).** "Committed" is quorum replication; local
stable-storage acknowledgement is a separate metric, not merged into the
commit path. Sled already logs writes; block+write-set replay rebuilds
derived state — no third WAL. `SledStorageProvider::flush` exists for the
explicit durable-ack mode only; the default periodic flush is not an
application durability acknowledgement or a hard 500 ms loss bound.

Before a persistent pilot, decide what “committed” promises across power loss
**per measurement** (the ADR answers for the architecture), implement the
explicit durable-ack mode if the pilot wants it, and measure throughput/p99
plus crash recovery. Do not postpone required durability
merely to protect a latency number. Keep logical finality, replication and local
stable-storage acknowledgement as separate metrics.

## 5. Benchmark additions from the external report

### WAN and round-change scenarios — proxy shipped, scenarios started

`tests/common/proxy.rs` is the shared real-TCP proxy (extended from
`tests/tcp_partition.rs`, source debt **D7**): bounded-by-socket-buffer
per-direction relay shaping (seeded one-way latency + jitter, bandwidth
pacing), mid-scenario `set_profile`, and global `partition`/`repair`.
Shipped scenarios (`real_tcp_wan_*`, 4 validators, wall-clock):

- no-fault baseline through the overlay;
- asymmetric WAN delay (200 ms ± 80 ms on one link, applied to established links) — convergence, tips agree;
- partition-while-mining then repair — no conflicting finalization;
  time-without-quorum measured separately and printed;
- BFT vote round with the leader's link shaped 200 ms ± 80 ms — quorum
  commits, no conflicting tips;
- BFT leader-quorum loss: two of the leader's three peers separated at
  height 3 — no node finalizes, the height stalls, and after repair the
  certificate commits with identical tips (`bft_vote_rounds.rs`);
- bandwidth budget: one node's relay paced at ~8 KiB/s — convergence without
  conflicting tips (`tcp_partition.rs`).

**Measured finding (Step 0 follow-up) — re-audited 2026-09-14 (Step 6):** the
first recording (2026-09-09) found mesh formation through a shaped relay
stalls at ≥~120 ms per chunk (~4 × 5 s reconnect cycles). The Step-6
handshake-budget audit (`real_tcp_wan_handshake_budget_audit_opt_in`) does
**not** reproduce the stall on current main: formation through shaped-from-
the-first-byte relays came in at 604 ms (60 ms profile), 812 ms (100 ms) and
1 207 ms (140 ms), with a commit round converging at every profile. The
audit is opt-in and re-recordable; WAN profiles no longer carry the
formation stall as a hard gate, but a re-run at scaled profiles (>200 ms)
must re-pass the audit before those rows are used.

Still open: slow CPU/disk scenario (leader-loss and saturation landed 2026-09-14
— `bft_round…leader_quorum_loss…heals` in `bft_vote_rounds.rs`, and
`real_tcp_wan_low_bandwidth_budget_still_converges` in `tcp_partition.rs`).
Stale/duplicate-vote floods stay covered by the in-process node tests; a
real-TCP variant exists only if a measured divergence shows one. The §5
handshake-budget audit (mesh formation stalls at ≥~120 ms shaped delay)
gates scaling the profiles further. 10/100/200/300 sweeps happen under
explicit hardware budgets. Deterministic madsim execution stays optional
behind compatibility evidence, not a prerequisite to these WAN measurements.

### High-frequency flattening and read-path memory — baseline measured

`AnalyticalFlattener` retains a growing `Vec<FlatAssetRecord>` and currently
ingests **AssetRegistration**, not every canonical event. The event bus's
bounded broadcast/ring buffer does not bound the flattener, provenance index or
caller-side query allocations. Ingestion also runs inside
`Node::after_block_commit`; “off-chain” does not mean zero cost to the node.

**Baseline (2026-09-09, `cargo test -p glasschain-indexer --release --test
read_path_memory -- --ignored --nocapture`, release, Linux RSS):** ~3 KiB per
registration retained across the three projections (indexer payload JSON
dominates; the flat-record struct is 424 B and the serialized payload ~485 B).
Linear growth: 1 000 rows ≈ 3 MiB, 10 000 ≈ 30 MiB, 100 000 ≈ 299 MiB — under
the 512 MiB scenario budget. Ingestion is sub-ms per block at these scales
(712 ms total for 100 k); a full rebuild of a fresh flattener over the same
chain took 368 ms at 100 k. Linear-scan queries (`records_by_gtin`, …) are
O(rows): 5.2 ms at 100 k — fine today, a measured trigger for indexes later,
not before. **Remaining measured gaps:** lagging-subscriber lag/drop counts,
bursts vs steady input with concurrent finality load, and peak-RSS at the
node level (this harness measures the projections in isolation).

Remaining fix path unchanged: if budgets fail at deployment scale, bounded
batch export/pagination and a rebuildable projection come before a new
database or service. A slow analytics consumer must not block consensus, and
loss recovery must replay committed history rather than silently omit events.

## Frontier C residual map (2026-09-14, post-latency-plan)

With the backend swap and the sum-of-keys verify landed, certificate
verification is flat at ~1.8 ms (quorum 201) and is **no longer a scaling
lever** — the two changes that were, are done. The measured round budget at
300 (p50 4 117 ms with the latency-plan optimizations) decomposes roughly as:
prevote collection 783 ms, precommit collection 1 904 ms, aggregation ~1.2 s
across both phases, and post-commit mesh replication ~1.3–1.6 s (separately
polled). The remaining improvement opportunities, ranked by measured headroom:

1. **Receiver-side post-commit cost at scale (Step 6, biggest headroom).** The
   relay experiment (reverted) showed replication is bound by per-node
   validate + index + watcher work on shared cores, not by the leader's
   sends. Bounded admission (8 000-tx pool), pool stats, priority lanes and
   the height-bounded catch-up shipped 2026-09-14; the offered-load
   saturation study (100 validators) holds at 2 000-tx bursts (+52 %
   finality, full drain every round). Fair batching and explicit backpressure
   metrics remain open, gated on a failing budget being measured first.
2. **Vote-collection phases at 300.** 783 ms + 1 904 ms remain the two
   dominant phase costs; concurrent verification already landed. Batching
   (per-round proposal slices) is only justified when the saturation study
   names it the failing budget.
3. **Durability promise (§4) — decided (ADR-016, 2026-09-14).** "Committed"
   means quorum-replicated; local disk flush is not the durability promise and
   stays a separate pilot-gated metric; no WAL. A single node heals missing
   local tiers from peers (restart replay + repair-from-peers); a power-loss
   pilot needs the explicit durable-ack mode before how it appears in latency
   numbers is claimed.
4. **Read-path bounds (§5).** Flattener/provenance projections are linear
   (~3 KiB/record, 299 MiB @ 100 k) and ingest inside `after_block_commit`;
   bounded export + rebuildable projection remain the fix path if deployment
   scale budgets fail.
5. **Steps 5 and 7 — research only.** The Step 5 fault profile recorded
   2026-09-14: leader-quorum loss fails closed at the 3 s phase deadline
   (no commit), and the round commits 173 ms after repair. Next research
   step: a rollback/prefix-fork design doc before any HotStuff-1-style
   candidate; liveness placement enforcement is operational, not a code
   change.

Not opportunities: sampled committees or probabilistic finality (§2), a new
WAL (§4), or unsafe/C beyond ADR-015's conditions (ADR-015 §1).

## 6. Ordered path (stable Step 0–7 names)

- [x] **Step 0 — trustworthy measurement and safety baseline.** Both existing
  harnesses retained; §5 WAN/round-changes shipped (proxy #108), read-path
  memory baseline (#107), per-phase round timing recorded via
  `Node::last_round_phase_timings` and the gate summaries (2026-09-14:
  proposal/prevote/aggregate/precommit/aggregate p50s in
  `docs/benchmarks/consensus-capacity.md`); D7 scenarios cover no-fault,
  asymmetric delay, partition+repair, leader-quorum loss + heal and a
  bandwidth-budget link; stale/duplicate-vote floods stay covered in-process.
  Recorded runs must still state commit, toolchain/features, hardware,
  workload, topology and repetitions (they do in the benchmark record; new
  runs update it). Re-run variants on changed hosts and keep timeouts counted.
  Longer runs and primary-source, like-for-like competitor measurements are
  required before claiming sub-second or best-in-class performance.
- [x] **Step 1 — remaining codec costs, measured first.** Profiled
  (`cargo bench -p glasschain-network --features bft --bench wire_codec`,
  2026-09-14): round messages are 569–660 B and 1.0–1.5 µs to encode or
  decode; even 300 validators move codec cost ≈ 0.5 ms per two-phase round
  against 916/2 215 ms collection phases — **<1 %** of the round. Decision
  recorded: JSON stays the wire format; no binary encoding
  (`docs/benchmarks/consensus-capacity.md` §wire codec). Wire/history
  compatibility note: `/6` already base64 + discriminants.
- [x] **Step 2 — Ed25519 batch verification: superseded.** BLS removed the
  per-attestation certificate loop. Invalid aggregate rejection does not identify
  an individual signer; attribution relies on authenticated votes/evidence, not
  a sequential fallback that no longer exists.
- [x] **Step 3 — history-dependent admission (D3): done 2026-09-14.** One
  rebuildable index at the owning layer, as specified: `Ledger` keeps an
  incremental `CapabilityHistory` (folded block-wise, reset on chain
  replacement, lazily rebuilt after deserialization) plus a block-wise
  committed-ID set. `add_transaction` and `commit_mined_block` no longer
  rebuild from genesis and the per-caller fixes (node caches) sit on top of
  it. Measured on the `ledger_admission` bench: ~2.3 → 0.19 ms at 1 000,
  ~21.3 → 0.19 ms at 10 000 (−99 %), duplicate IDs no longer pay the
  rebuild; cost is flat in history size (`docs/benchmarks/consensus-capacity.md`).
  Regression: `ledger::tests::test_d3_index_semantics_match_full_rebuild…`.
- [x] **Step 4 — BLS backend: done (ADR-015, 2026-09-13).** The audited `blst`
  C backend is selected and the pure-Rust `pairing` path is retired, decided
  once as policy ADR-015 ([#85](https://github.com/dbbvitor/GlassChain/issues/85)
  — accepted, feature-gated, byte-identical signatures; `aws-lc-rs` was already
  behind `pq-tls`). The same-message verify moved from a 202-term
  multi-Miller loop to the **sum-of-keys PopScheme check over `blstrs`** —
  two pairing terms at any quorum size (`crates/glasschain-core/src/bft.rs`,
  PoP at registration is the rogue-key defense). Measured
  (`cargo bench -p glasschain-core --bench bft_verify`): pure-Rust 80.0 ms →
  blst sum-of-keys 1.78 ms at quorum 201. **The 300-validator gate now passes**
  (p50 3 996 ms, exact quorum 201 every round; 100 → 1 096 ms, 200 → 2 397 ms
  p50 — before/after evidence in `docs/benchmarks/consensus-capacity.md`).
  Remaining round cost is mesh replication, not verification.
- [ ] **Step 5 — in-family latency candidates, research only.** Research
  brief completed 2026-09-14 (`.agents/memories/latency-candidates.md`);
  the driver's fault profile recorded 2026-09-14 (`bft_vote_rounds.rs`):
  leader-quorum loss fails closed at the 3 s phase deadline — no node
  finalizes, no conflicting tip — and after repair the round commits in
  173 ms. Verified against the HotStuff-1 v3 / SBFT report: the prefix
  speculation dilemma and slotting are real HotStuff-1 mechanics (the
  report's "No-Gap Rule" name is unverified); GlassChain's measured round
  attributes to collection phases and receiver-side post-commit work, so
  "thread starvation / allocator remediation" does not apply here.
  Remaining research work, in order: a rollback/prefix-fork design doc,
  then prototype + rollback/prefix-fork benches. Their safe early-reply
  conditions are protocol-specific, not a one-phase toggle. Any proposal
  must specify fallback, locking/view synchronization and fault
  assumptions; client-visible speculative results may not be called final
  or authorize inventory, payment, endorsement or private-data side
  effects. Adoption that changes ADR-002 semantics requires an explicit
  decision (an ADR), not this report. **The decision was made 2026-09-14:
  declined for now** — ADR-002 carries the amendment note; the rollback
  design doc remains the evidence pack if the revisit condition is met.
- [ ] **Step 6 — mempool/dissemination, simplest changes first.**
  *Shipped 2026-09-14 (installments 1–2):* bounded pending-pool admission
  (`MAX_PENDING_TRANSACTIONS = 8 000`, explicit rejection; duplicates ride
  free — `ledger::tests::test_pending_pool_bound_rejects_and_drains`),
  pool stats (`Node::pending_pool_stats`: depth + serialized bytes),
  peer-path flood failures warn-and-drop, consensus/background priority
  lanes with per-class drop counters, height-bounded chain catch-up
  (wire `/7`: `RequestChainFrom` for gaps, full bootstrap for fresh nodes —
  `chain_catch_up_recovery_at_1k_blocks_opt_in` recorded a 1 004-block
  bootstrap in 272 ms), and reconnect backoff (1 s → 2 s → cap 5 s).
  *Shipped 2026-09-14 (installment 3 — batching):* `MAX_BLOCK_TRANSACTIONS
  = 4_000` slice per round (≈740 KB, >2× headroom under the measured-good
  925 KB ceiling); excess stays pending and flows into following rounds;
  stale-tip/endorsement restores bypass the bound so already-admitted
  transactions are never lost. *Saturation re-run:* the 9 000-tx probe that
  previously failed (8.3 s round + convergence failure) now **sustains** —
  740 KB blocks converge 8/8, 4 000 committed per round, a 4 000-tx backlog
  persists, 3 000–4 800 explicit rejections per round, finality p50 5 275 ms
  under that sustained load. The pool bound is the designed backpressure
  signal. *Scale table complete (10/100/200/300):* 194 ms / 1 145 ms /
  2 468 ms / 4 117 ms finality p50, exact quorums. *§5 read-path gaps
  closed:* lagging-subscriber drop counts are receiver-observable exactly
  (`published − capacity`), and burst-vs-steady ingestion with concurrent
  query load stays sub-ms/block with 73–105 µs query p50.
  *Still open:* node-level peak-RSS harness; 400/500 sweeps under
  explicit hardware budgets. Consider Narwhal-style
  availability/dissemination only if propagation or persistent backlog
  remains dominant after batching (the relay experiment already showed
  receiver-side cost is the bound, not dissemination shape). It does not
  cure hot-key contention. Availability certificates, missing payload
  recovery, GC and PDC authorization need a design; DAG ordering/Snow
  consensus are not implied.
- [ ] **Step 7 — operational liveness.** The in-repo half of the epoch-
  change exercise shipped 2026-09-14
  (`validator_set_churn_reconfigures_the_round`): a governance delete
  reconfigures the validator set across heights and the next round commits
  under the new set — quorum recomputed, proposer rotated, no forks.
  Research confirmed there is no other code gap to close: guidance shipped,
  enforcement and operational evidence remain the gates. Exercise real
  failure-domain placement (multi-rack / multi-AZN / multi-ASN evidence
  from deployment manifests, not from code) and participation monitoring
  without reputation weighting or weakened quorums — an authenticated
  participation read (from the certificates' signer bitmaps) is the one
  optional future piece, deferred until an operator consumer exists. Use authenticated signer data for metrics — signed
  votes/QC/evidence only, never penalties inferred from unverified or
  missing traffic (the D7 flood/duplicate coverage already pins the
  in-protocol side of this). Unweighted quorums and rotation are ADR-009
  policy: every active validator holds equal power, maintained through the
  on-chain registry.

## Validation and claim gates

Each implementation leaves a regression test for its failure mode. Run the four
workspace gates, default and all-features for consensus changes; keep large WAN
and scale tests opt-in with bounded duration and memory. A timed-out benchmark
is an unsuccessful run, not a missing data point to discard.

No production claim until ADR-010's testnet, API/stability, licensing and security
audit gates pass. The [browser demo plan](gui-demo-benchmark.md) renders the same
backend metrics through a proposed HTTP/SSE bridge. Browser/WebGPU frame time is
presentation cost, not finality; compare headless, Canvas2D and accelerated runs.
The web app remains unimplemented and cannot replace adoption evidence.

## Out of scope

No new WAL, HotStuff/Snow engine, speculative client API, ZK identity stack,
FL training, mandatory IPFS, or binary-codec rewrite based solely on this review.
Do not lower safety/durability requirements to move a graph. Research/design is
not authorization to ship a different protocol.

## Sources and revision note

2026-09-05 reconciliation replaces conflicting “not implemented” and “done”
paragraphs with this single current plan. Historical runs stay in
[the benchmark record](../../docs/benchmarks/consensus-capacity.md); prior
architecture research stays in [bft-at-scale](../memories/bft-at-scale.md) and
[participation-model](../memories/participation-model.md).

Re-read primary abstracts on 2026-09-05:
[HotStuff-1 v3](https://arxiv.org/abs/2408.04728v3) distinguishes early speculative
confirmations from the rest of consensus;
[Narwhal and Tusk v4](https://arxiv.org/abs/2105.11827v4) separates dissemination
and ordering, and reports both throughput gains and fault-related latency costs.
Their figures are not GlassChain measurements or proof of a 300-validator cap.
