# Plan — Latency/liveness opportunities from the Step 5 review (zero-trust invariant)

**Status:** implemented 2026-09-14 in the proposed order, minus #1; all gated
items executed and recorded (see "Gate results"). #2 (concurrent
verify), #3 (priority lanes), #4 (height-bounded catch-up, wire `/7`) and #5
(backoff) shipped with gates; #1 (gossip relay) was implemented, measured on a
100-node mesh, and **reverted as a recorded negative** — deterministic windows
leave permanent coverage gaps and replication at 300 is receiver-side bound;
see the benchmark record. #6 stays frozen pending the ADR-002 question.
**Source:** `.agents/memories/latency-candidates.md`, the Step 0 phase
decomposition (docs/benchmarks/consensus-capacity.md), and code review of the
driver (`run_vote_round`, `collect_phase_votes`, `append_peer_block`, chain sync).
**Related:** [performance plan](performance.md) §5–§6, ADR-002 (finality
semantics — unchanged by everything below), ADR-010 (wire versioning).

## Zero-trust invariance contract (every candidate must state it)

No candidate may: skip vote/QC/PoP/endorsement/scope checks, lower the
`floor(2n/3)+1` quorum, weight votes, infer penalties from unauthenticated
traffic, expose client-visible speculative results as final, or move crypto
verification out of the receipt path. Candidates that rely on a property the
content itself already authenticates (a block's hash chain + certificate) are
admissible: verification is end-to-end per recipient, so relay/gossip hops add
no trust assumption.

## Measured reality (what actually costs time at 300 validators)

- Round p50 4 612 ms: prevote collection 916 ms, precommit collection +
  broadcast 2 215 ms, aggregates ~1.24 s, proposal 16 ms, replication after
  commit ~1.6 s (separately polled).
- Certificate verify: 1.78 ms (sum-of-keys) — not the wall. Codec <1 %.
- ~~`collect_phase_votes` verifies arriving votes **serially**~~ — fixed by #2
  (bounded-concurrency verification; prevote 916 → 783 ms, precommit
  2 215 → 1 904 ms at 300).
- Block fan-out is leader-serial point-to-point over the mesh (~1.6 s);
  the *next* round waits for mesh-wide convergence, so replication gates
  sustained throughput.
- ~~Chain sync is a full-chain push on every reconnect/resync~~ — fixed by #4
  (gap pulls carry only the missing suffix; fresh bootstraps still carry the
  history by necessity — 1 004 blocks in 272 ms recorded).
- ~~Per-peer write channels are a single 256-slot queue for **all** message
  classes~~ — fixed by #3 (consensus/background lanes, per-class drop
  counters).

## Opportunities, ranked by measured headroom

### 1. Gossip dissemination for block fan-out — fit: strong, biggest lever
Relay verified blocks k-neighbor outward instead of leader-serial 1→n.
Zero-trust: every recipient already validates hash linkage, consensus
admissibility and endorsements on receipt — a relayed block is as verified as
a leader-delivered one. Shape: prefer the smallest change to the existing TCP
path (k-fanout relay on `Message::Block`, dedup by hash — broadcast already
dedups via pending/committed IDs) over adopting the dormant gossipsub swarm;
adopting gossipsub is the fallback if relaying under-performs. Gate: 300-gate
replication ms + sustained rounds/sec, before/after.

### 2. Bounded-concurrency vote verification at the leader — fit: strong, small
`collect_phase_votes` verifies serially; verify is embarrassingly parallel
across voters. Bound concurrent `spawn_blocking` verification (e.g., CPU
cores, join ordered), keep identical checks, counting and flood rules
(distinct-voter quorum, absolute deadline, bounded channel). Zero-trust:
same signatures verified, just concurrently. Gate: prevote/precommit phase
ms in the 300 gate, before/after; expect ~100–200 ms per phase saved on a
4-core host.

### 3. Consensus-message priority lanes — fit: strong under flood
Split the per-peer write channel into consensus-class
(Proposal/Precommit/Block/Vote/Chain) and background (Transaction,
PrivatePayload*) queues, consensus dequeued first; both bounded with per-class
drop counters. Zero-trust: no message becomes trusted, only scheduled.
Gate: the §8.4 flood scenarios keep quorum under saturated tx input; WAN
asymmetric scenario keeps passing.

### 4. Height-bounded chain catch-up — fit: strong, liveness/recovery
`RequestChainFrom { from_index }` alongside `RequestChain`; receiver folds
only the missing suffix (genesis-anchored, fully validated — zero-trust
identical, just less data). Wire `/7` per ADR-010. Gate: partition-repair
recovery time at a 1 000-block history, before/after.

### 5. Reconnect backoff — fit: honest small liveness
Flat 5 s reconnect becomes bounded backoff (1 s initial, ×2 capped at 5 s);
the WAN audit's formation stalls shrink on flaky links. Same TOFU/TLS, no
verification change. Gate: WAN audit formation times at 60/100/140 ms.

### 6. HotStuff-1 one-phase speculation / slotting — NOT authorized
Requires a rollback/prefix-fork design doc + explicit ADR-002 decision
before any code. SBFT's fast path rejected at research: a single faulty
replica degrades it to a slower path. Recorded in
`.agents/memories/latency-candidates.md`; no implementation plan until the
ADR-002 question is asked.

## Gate results (executed 2026-09-14)

- **#2**: 300-gate run after implementation — finality p50 4 243 ms, prevote
  778 ms, precommit 2 000 ms (later re-run with all items live: 4 117 / 783 /
  1 904 ms). Recorded in the benchmark record.
- **#3**: in-process flood/quorum tests pass with the lanes live
  (`vote_channel_bound_rejects_flood_after_capacity`,
  `phase_deadline_is_absolute_under_continuous_stale_traffic`,
  `quorum_sized_traffic_is_never_lost_to_the_bound`); the WAN asymmetric and
  bandwidth-budget scenarios pass unchanged.
- **#4**: `chain_catch_up_recovery_at_1k_blocks_opt_in` — a fresh joiner
  bootstrapped a 1 004-block history in 272 ms through the height-bounded
  sync flow; gap-filling requests carry only the missing suffix by
  construction (`RequestChainFrom { from_index }`), fresh nodes bootstrap
  with the full chain + one rebuild (the wholesale path is correct for a
  from-genesis join).
- **#5**: handshake audit re-run with backoff live — formation 603 ms
  (60 ms), 808 ms (100 ms), 1 208 ms (140 ms); formation was never
  reconnect-bound, so the audit rows are unchanged, and the backoff's
  benefit is measured in the reconnect paths (leader-loss heal, D7
  scenarios).
- **#5 fault profile (Step 5)**: leader-quorum loss fails closed at 3.01 s
  (phase deadline, no commit); after repair the round commits in 173 ms.
- **Step 6 saturation study**: the measured curve at 100 validators is
  400-tx bursts +6 %, 2 000 → +52 %, 5 000 → +187 % (full drain every
  round), and 9 000-tx bursts **found the failing budget**: 1 000 explicit
  pool-bound rejections, an 8.3 s round, then mesh convergence failure on
  ~1.4 MB blocks. **Batching shipped** (`MAX_BLOCK_TRANSACTIONS = 4 000`):
  the 9 000-tx probe now sustains — 740 KB blocks converge 8/8, finality
  p50 5 275 ms under that load. The straggler scenario (4 KiB/s + 100 ms
  starved validator) commits the round in 148 ms on the 3-of-4 quorum.
  §5 read-path gaps closed: lagging-subscriber drops are receiver-observable
  exactly; burst-vs-steady stays sub-ms/block (73–105 µs query p50).

## Proposed order

1. **#2** (bounded-concurrency verify) — shipped.
2. **#1** (gossip fan-out) — measured and reverted; negative result recorded.
3. **#3** (priority lanes) — shipped.
4. **#4** (height-bounded catch-up) — shipped (wire `/7`).
5. **#5** (backoff) — shipped.
6. **#6** — **answered 2026-09-14: declined for now** (ADR-002 amendment
   note recorded); revisit condition documented; the ack-carries-QC
   constraint stands as the hard precondition if ever reopened.

Every item ships with the phase-timer gate run at 300 (and the matching WAN
scenario), the four workspace gates, and a recorded before/after row in
docs/benchmarks/consensus-capacity.md.
