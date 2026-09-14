# Consensus capacity gate — recorded evidence (ticket #48)

**Status:** gate executed 2026-09-01 at 200 and 300 validators, in-process.
**Reproduce:** `cargo test -p glasschain-network --test consensus_capacity -- --ignored --nocapture`
(harness: `crates/glasschain-network/tests/consensus_capacity.rs`; madsim mode:
`RUSTFLAGS="--cfg madsim" ...`, seeded/deterministic scheduling).

## Methodology

- **Topology:** star — every validator dials the mining leader (the block
  broadcast topology). 200 validators: 133 connected + 67 partitioned;
  300 validators: 200 connected + 100 partitioned.
- **Workload (ADR-010 §7 compact set):** per round, 20 canonical records —
  anchored lots, `state_commitment` batch anchors, and certification anchors —
  submitted to the leader, then mined and broadcast. 10 rounds.
- **Measurements:** leader submit time, leader mine/commit latency, serialized
  block size, quorum-certificate size (from the commit notification's
  `BlockMined` event), pending-pool depth at mine time (backpressure), and a
  single-start propagation poll (50%/95%/100% of the connected validators
  reach the new height, all thresholds measured concurrently — see caveat 2).
- **Certificate honesty:** the mesh validators run dev/test PoW admission, so
  the leader stays on the PoW path and the measured certificate is the
  **degenerate PoW attestation (115 B — empty attestation set)**. The staged
  BFT engine's real one-attestation certificate measures **508 B** (leader-side
  commit in a variant run); BFT blocks are rejected at the peers'
  `has_valid_pow` admission check, so a BFT-attested mesh measurement requires
  the ADR-010 adoption-gate peer work and is out of scope here.
- **Partition/recovery:** the unconnected third joins after the workload;
  convergence = time until every validator holds the leader's tip.
- **Private data (measured separately):** one collection, a member every 10th
  validator (20 of 200 / 30 of 300), one payload disseminated by the leader;
  time until every member's transient store holds it.

## Results

> **2026-09-03 re-run — instrument fixed.** The fan-out instrument was fixed
> (single start, single poll loop for all three thresholds — see caveat 2) and
> persistence was moved behind block broadcast on the mine path (#62 §5.6-3).
> The tables below are the current evidence.

### 200 validators (2026-09-03 re-run, release build)

| Metric | Value |
|---|---|
| Leader mine/commit latency | p50 **10 ms**, p95 **17 ms** |
| Block size (20 compact records) | **11 567 B** avg |
| Quorum certificate size | degenerate PoW attestation (empty set on this path; staged BFT one-attestation cert: 508 B) |
| Pending-pool depth at mine | 20 (one round's submissions; drained every block) |
| Fan-out to 100% of connected | median **557 ms** (47–1 227 ms across rounds; monotone within every round) |
| Partition recovery (67 join) | **573 ms** to full convergence |
| PDC dissemination (20 members) | **53.2 ms**, 20/20 delivered |

### 300 validators (2026-09-03 re-run, release build)

| Metric | Value |
|---|---|
| Leader mine/commit latency | p50 **12 ms**, p95 **25 ms** |
| Block size (20 compact records) | **11 567 B** avg |
| Quorum certificate size | degenerate PoW attestation |
| Pending-pool depth at mine | 20 |
| Fan-out to 100% of connected | median **834 ms** |
| Partition recovery (100 join) | **912 ms** to full convergence |
| PDC dissemination (30 members) | **53.3 ms**, 30/30 delivered |

## BFT finality (2026-09-04, post-adoption-gate #82)

The vote-round driver makes REAL deterministic finality measurable. Three
`#[ignore]` gates (`bft_finality_gate_{100,200,300}`, run explicitly with
`ulimit -n 65535`–`200000`), loopback, release, 8 runtime threads:

| Validators | finality p50 | p95 | p99 | replication | quorum/round | first vote |
|---|---|---|---|---|---|---|
| 100 | **2 021 ms** | 2 162 | 2 162 | ~850 ms | exact 67 every round | 2.0 s |
| 200 | **5 284 ms** | 5 744 | 5 744 | ~3.1 s | exact 134 every round | 5.4 s |
| 300 | **not passing on the pure-Rust backend** — first vote 34.8 s, then the precommit re-verification herd (299 × 202-pairing multi-miller loops) exceeds the scaled phase budget | | | | | |

## BFT finality on the `blst` backend (2026-09-13, ADR-015 / #85 swap)

`bls-signatures` moved from the pure-Rust `pairing` backend to the audited `blst`
C backend (ADR-015), with the same-message verify rewritten over `blstrs` as the
**sum-of-keys PopScheme check**: per bilinearity the signer keys collapse into
one G1 sum, so a certificate verification is **two pairing terms regardless of
quorum size** — not `quorum + 1` Miller loops. The `_300` gate now passes
end to end, release, loopback, 4-core/15 GB shared host (not 8 threads; mesh
setup dominated by 44 950 dials):

| Validators | finality p50 | p95 | p99 | replication | quorum/round | first vote |
|---|---|---|---|---|---|---|
| 100 | **1 096 ms** | 1 143 | 1 143 | ~210 ms | exact 67 every round | 1.16 s |
| 200 | **2 397 ms** | 2 508 | 2 508 | ~650 ms | exact 134 every round | 2.32 s |
| 300 | **3 996 ms** | 4 072 | 4 072 | ~1.31 s | exact 201 every round | 3.94 s |

Before/after: unchanged quorum assumptions, same harness, same machine class —
p50 −46 % at 100 and −55 % at 200, and the previously unpassable 300 gate
commits every round at exact quorum 201. Certificate verification is no longer
the wall: the remaining round cost is dominated by mesh replication (the
leader's ~4 s budget at 300 now tracks wire fan-out, not pairings).

Micro-benchmark (`cargo bench -p glasschain-core --bench bft_verify`, release,
same machine; criterion medians):

| Verify path (quorum 201) | time |
|---|---|
| pure-Rust `pairing`, 202-term multi-Miller loop | **80.0 ms** |
| `blst`, 202-term multi-Miller loop | 44.0 ms |
| `blst`, sum-of-keys (shipped) | **1.78 ms** |
| pure-Rust `pairing`, 102-term loop / 101 quorum | 42.0 ms |
| `blst`, sum-of-keys @ 101 | 1.54 ms |

No production capacity claim: loopback, in-process, synthetic workload. The
backend selection per se does not assert sub-second finality at scale — the
measured p50 at 300 is ~4.0 s on loopback.

### Phase decomposition (2026-09-14, Step 0 instrumentation)

`run_vote_round` now records per-phase wall clock
(`Node::last_round_phase_timings`, printed per round and as p50s in the gate
summary). First recorded run, release, same shared 4-core host as the table
above, 10 rounds:

| Validators | finality p50 | proposal | prevote | prevote-agg | precommit | precommit-agg | replication |
|---|---|---|---|---|---|---|---|
| 100 | 1 145 ms | 3 ms | 237 ms | 196 ms | 420 ms | 196 ms | ~230 ms |
| 300 | 4 612 ms | 16 ms | 916 ms | 619 ms | 2 215 ms | 620 ms | ~1.6 s |

Notes: `precommit` includes the Precommit broadcast and the validators'
prevote-certificate re-verification; the aggregation columns are the leader's
aggregate + certificate assembly only (blst sum-of-keys — flat ~0.2/0.6 s
reflected in the round's collection intervals, not in verify). Phase sums
(~4 385 ms at 300) sit just under the 4 612 ms finality; the separately
polled replication lags the in-memory commit and overlaps the post-commit
window, so finality is not the sum. Run-to-run variance on this shared host is
±15 % (the 2026-09-13 run recorded 3 996 ms; same gate, same day). The
dominant measured cost remains the two vote-collection phases against the
full mesh — mesh fan-out (Step 6) is the lever, verification is not
(~1.8 ms/cert — unchanged).

Operationally noted: the gate needs a raised fd limit above 65 535 at 300
validators (the full mesh holds ~180 K sockets across the harness process —
`ulimit -n 524288` passed; 65535 aborts with `Too many open files`).

### Wire codec profile (2026-09-14, Step 1, ADR-015 shape)

`cargo bench -p glasschain-network --features bft --bench wire_codec`
(release). Message variants that make up one round, current BLS wire (`/6`,
base64 signatures + bitmap certificate):

| Variant | wire size | encode | decode |
|---|---|---|---|
| Block (200-signer certificate attached) | 628 B | 0.97 µs | 1.31 µs |
| Vote | 569 B | 1.01 µs | 1.23 µs |
| Proposal (pre-certificate block) | 651 B | 1.02 µs | 1.44 µs |
| Precommit (block + prevote certificate) | 660 B | 1.03 µs | 1.47 µs |

Attribution: a round at 300 moves ~1 proposal + 1 precommit (leader) and
~300 votes + ~300 precommit echoes (validators) — even 300 × 1.5 µs ≈
**0.5 ms** across both phases, against measured collection phases of 916 ms
and 2 215 ms. Codec cost is **<1 % of the round**. Decision: no binary
encoding; JSON stays the wire format.

### D3 admission after the incremental index (2026-09-14, Step 3)

`Ledger::add_transaction` no longer rebuilds capability history or scans
committed IDs per admission: an incremental capability history plus a
committed-ID set fold chain-wise (rebuilt as one full scan when first
needed, reset on chain replacement; `Ledger` carries untracked marker
fields that a deserialized ledger rebuilds lazily). Before/after on the
same `ledger_admission` bench (release, 64-admission bursts):

| History size | before (rebuild per admission) | after | change |
|---|---|---|---|
| 1 000 | ~2.3 ms | ~0.19 ms | −92 % |
| 10 000 | ~21.3 ms | ~0.19 ms | −99.1 % |

Admission cost is **flat in history size** (176–190 µs across 100/1 000/10
000, dominated by the burst's mining setup), and duplicate IDs no longer pay
the rebuild (188 µs, was ~21 ms at 10k). Canonical validation semantics
unchanged — the folded index agrees with a from-genesis rebuild and resets on
chain replacement (regression: `test_d3_index_semantics_match_full_rebuild…`).

### Latency-plan opportunities (2026-09-14, Step 5 review → `.agents/plans/latency-opportunities.md`)

After-evidence run at 300 (release, same shared host, 10 rounds, all items
live — bounded-concurrency vote verification, priority lanes, height-bounded
catch-up, reconnect backoff):

| Phase | before (2026-09-14 a.m.) | after | change |
|---|---|---|---|
| finality p50 | 4 612 ms | **4 117 ms** | −11 % |
| prevote collect | 916 ms | 783 ms | −15 % |
| precommit collect | 2 215 ms | 1 904 ms | −14 % |

Quorum stays exact-201 every round; the gate's convergence polls unchanged.
Per-item evidence:

- **Bounded-concurrency vote verification** (`collect_phase_votes` verifies
  bursts on `spawn_blocking`, batch ≤ cores; distinct-voter/dedup/deadline
  semantics preserved — `concurrent_verification_keeps_distinct_voter_quorum_under_burst`).
  Drives the prevote/precommit reductions above.
- **Consensus/background priority lanes**: two bounded queues per peer,
  consensus-class (Proposal/Precommit/Block/Vote/Chain) drained first, with
  per-class drop counters (`dropped_outbound` / `dropped_background`).
  Flood/liveness behavior unchanged in the §8.4 in-process tests; zero-trust
  unchanged (scheduling only).
- **Height-bounded catch-up**: `RequestChainFrom { from_index }` +
  `Chain { from_index, blocks }` (wire `/7`); the too-far-ahead path now
  pulls only the missing suffix and the receiver folds every block through
  the standard single-block admission path (regression:
  `chain_suffix_folds_through_block_admission`). Reconnect sync picks the
  shape: fresh nodes bootstrap with the full chain + one rebuild, nodes
  holding history pull the suffix only.
  `chain_catch_up_recovery_at_1k_blocks_opt_in` recorded a 1 004-block
  bootstrap in **272 ms** (release, loopback mesh).
- **Reconnect backoff**: 1 s → 2 s → capped 5 s instead of flat 5 s
  (`reconnect_backoff_ladder`); handshake reset clears the ladder.
- **Step 7 churn exercise** (`validator_set_churn_reconfigures_the_round`):
  after the four-validator set certifies block 3, a governance delete
  removes one validator; the height-4 round commits under the reconfigured
  set — quorum exactly 3-of-3, proposer rotated, registry cache invalidated
  by content hash, identical tips on every node (including the removed one).
  The in-repo half of the epoch-change exercise; failure-domain placement
  and fleet participation metrics remain deployer evidence.
- **Step 5 fault profile** (`bft_vote_rounds.rs` leader-quorum-loss): the
  round **fails closed at 3.01 s** under the phase deadline — no node
  finalizes without quorum reach — and after link repair the round commits
  in **173 ms**. Safety boundary measured, not assumed.
- **Step 6 offered-load saturation** (`bft_offered_load_saturation_100_validators`,
  release, 100 validators, 5 unloaded + 8 loaded rounds, burst size
  env-overridable): unloaded finality p50 1 158–1 170 ms; the measured curve:

  | burst txs/round | finality p50 (p99) | pool depth / bytes | drain | rejections |
  |---|---|---|---|---|
  | 400 | 1 229 ms (+6 %) | 400 / 73 KB | 8/8 | 0 |
  | 2 000 | 1 774 ms (+52 %, p99 1 958) | 2 000 / 368 KB | 8/8 | 0 |
  | 5 000 | 3 330 ms (+187 %, p99 3 543) | 5 000 / 925 KB | 8/8 | 0 |
  | 9 000 | **8 294 ms round 1** | 8 000 / 1.48 MB | round 1 | **1 000** (bound engaged) |

  At 9 000-tx bursts on the pre-batching code the gate **failed**: after the
  1 000 explicit rejections (the operator-visible bound), the ~1.4 MB block
  never converged across the 100-validator mesh within the 120 s poll.
  **The failing budget was found: replication of ~1.5 MB blocks** — the
  measured trigger for batching.
- **Step 6 batching (shipped, same study re-run)**: `MAX_BLOCK_TRANSACTIONS
  = 4_000` slice per round; excess stays pending and flows into the next
  rounds; stale-tip restores bypass the bound (`test_restore_bypasses_the_bound`,
  `test_slice_quota_spreads_a_burst_across_rounds`). The 9 000-tx probe now
  **sustains**: 4 000-tx (≈740 KB) blocks converge 8/8, 4 000 committed per
  round with a 4 000-tx backlog and 3 000–4 800 explicit rejections per
  round, finality p50 **5 275 ms** under sustained 9 000-tx offered load
  (was 8.3 s + convergence failure). Default 2 000-tx bursts: p50 1 728 ms,
  full drain 8/8 (fits one slice). The failing budget is not reachable at
  this scale anymore — the bound is the designed operator signal, not an
  emergency valve.
- **Scale table (2026-09-14, all optimizations live, release, same host):**
  10 validators → finality p50 **194 ms** (quorum 7 exact); 100 → 1 145 ms;
  200 → 2 468 ms (quorum 134 exact); 300 → 4 117 ms (quorum 201 exact).
- **§5 read-path gaps closed** (`read_path_memory.rs`, release):
  *lagging subscriber* — publishing 5 000 events into a 4 096-slot bus
  reports exactly `published − capacity` = **904** drops to the lagged
  receiver (`Lagged(n)`), observed within 2 µs; the drop count is the
  operator-visible signal. *Bursts vs steady* (1 250 blocks, 10 000
  registrations, concurrent O(rows) query load): ingestion sub-ms/block in
  both patterns; concurrent-query p50 **105 µs** steady vs **73 µs** bursty
  — the analytics consumer does not explode under burst input. Node-level
  peak-RSS remains open (needs a node-level harness).
- **Step 6 straggler scenario** (`vote_round_commits_without_a_bandwidth_starved_validator`):
  one validator paced at 4 KiB/s + 100 ms one-way; the height-3 round
  commits in **148 ms** on the 3-of-4 quorum with identical tips. CPU
  throttling is not proxy-simulable — this covers the network side of a
  slow validator.
- **Gossip relay of blocks — measured, REVERTED.** k-fanout relay with a
  seen-ring dedup was implemented and measured on a 100-node mesh: with
  deterministic per-relayer windows the union of waves leaves **permanent
  coverage gaps** (nodes stuck behind the tip with no pull trigger), and
  even with per-relay entropy rotation waves stalled. The existing
  leader-serial block fan-out is already pipelined by the concurrent
  per-peer writer tasks, so replication at 300 is bound by receiver-side
  post-commit work (per-node validate + index + watcher on shared cores),
  not by leader sends — relay cannot move that number. Reverted before
  merge; the negative result redirects Step 6's remaining work to per-node
  post-commit cost, not dissemination shape. Full mesh-wide relay needs
  pull-based anti-entropy (a design project, not a patch).

### Mempool bound and handshake re-audit (2026-09-14, Step 6)

- Bounded pending-pool admission: `MAX_PENDING_TRANSACTIONS = 8 000`;
  overflow rejects with an operator-visible error (`pending pool is full`),
  duplicates ride free, a mine drains the bound (regression:
  `test_pending_pool_bound_rejects_and_drains`). Wire-frame consequence: an
  8 000-tx small-workload pool stays in one round well under the 16 MiB
  frame limit.
- Handshake-budget audit re-run: shaped-from-byte-one relays formed 300-ms-jitter
  meshes at 60 ms (604 ms), 100 ms (812 ms), 140 ms (1 207 ms), and a round
  commits at every profile — the ≥120 ms formation stall recorded on
  2026-09-09 is not reproduced on current main (`tcp_partition.rs`
  `real_tcp_wan_handshake_budget_audit_opt_in`, re-runnable).

---

**Observed, honestly — ATTRIBUTED and FIXED (2026-09-03, #62 Step 0):**

---

**Observed, honestly — ATTRIBUTED and FIXED (2026-09-03, #62 Step 0):**
fan-out grew monotonically across rounds (47 → 1 227 ms at 200). The attribution
instrument (per-tick sweep cost + first-reached, then a stride sample) split the
causes:

1. **Instrument (minor):** the full O(connected) sweep grew per round and
   contaminated the thresholds (669 ms of 1751 ms at round 10). Fixed by
   stride-sampling (~connected/40 nodes).
2. **Real cause (dominant):** every received block ran
   `CapabilityHistory::build_from_blocks` — a full replay of the chain — while
   holding the peer's ledger lock, so each peer did O(height) work per block
   and the 200-peer commit herd's cost grew linearly with the round count.
   Fixed with an incremental capability cache on `NodeState`: advanced
   block-by-block at the commit choke point, rebuilt from the chain on
   start/sync/replacement, validated on a clone at admission. The same fix
   removes the per-submission replays.

Post-fix at 200 validators: round 1 fan-out 52 ms → round 10 **459 ms**
(was 70 → 1 751 ms); first-reached stays 25–99 ms throughout (was growing to
839 ms). At 300: median **344 ms**, recovery 911 ms, PDC 53 ms. A mild residual
growth remains (attributed to other O(height) admissions such as `chains_to`
timestamp checks and task-scheduling contention on the shared runtime) —
bounded and small; revisit only if the harness grows much longer.

### 200 validators (2026-09-01 original run — instrument broken, superseded)

| Metric | Value |
|---|---|
| Setup (create + connect 133) | 1.58 s |
| Leader mine/commit latency | p50 **36 ms**, p95 **113 ms** |
| Block size (20 compact records) | **11 567 B** avg |
| Quorum certificate size | **115 B** (degenerate PoW attestation; the staged BFT one-attestation cert is 508 B — see certificate honesty above) |
| Pending-pool depth at mine | 20 (one round's submissions; drained every block) |
| Fan-out to 100% of connected | median **0 ms** (0–34 ms across rounds) |
| Partition recovery (67 join) | **1 257 ms** to full convergence |
| PDC dissemination (20 members) | **73.5 ms**, 20/20 delivered |

### 300 validators (2026-09-01 original run — superseded)

| Metric | Value |
|---|---|
| Setup (create + connect 200) | 2.51 s |
| Leader mine/commit latency | p50 **33 ms**, p95 **148 ms** |
| Block size (20 compact records) | **11 567 B** avg |
| Quorum certificate size | **115 B** (degenerate PoW attestation; staged BFT: 508 B) |
| Pending-pool depth at mine | 20 |
| Fan-out to 100% of connected | median **11 ms** |
| Partition recovery (100 join) | **1 717 ms** to full convergence |
| PDC dissemination (30 members) | **102.6 ms**, 30/30 delivered |

## Honest scope of these numbers

1. **Consensus engine:** the mesh runs dev/test Proof-of-Work admission, so
   the measured certificate is the degenerate PoW attestation (115 B). The
   staged BFT engine's one-attestation certificate measures 508 B leader-side
   (verified in a variant run); **no cross-validator vote rounds exist to
   measure** — the "vote traffic" row is per-block certificate size, not
   gossip bandwidth. Real Tendermint-class vote gossip (O(n²)) and BFT peer
   admission are the ADR-010 testnet/adoption gates and are not substitutable
   by this in-process gate.
2. **Fan-out thresholds — FIXED 2026-09-03 (#62 §5.4/§5.6-1).** The original
   run's three thresholds were sequential polls, each with its own start time,
   and each poll swept every connected validator taking a lock on its ledger
   before sleeping 20 ms; the 50% poll absorbed the lock contention while the
   later polls found the block already delivered (50% grew 496 → 16 354 ms
   while 100% stayed at 0–98 ms — incoherent). All three thresholds are now
   measured from one start in one poll loop, and the recorded output is
   monotone within every round. Residual: the sweep is still O(connected)
   locks per tick, and the cross-round fan-out growth is unattributed (see
   the 2026-09-03 table).
3. **Recovery** models an application-layer partition (validators that never
   dialed join late); it does not sever established TCP sessions. WAN delay is
   not injected; madsim's deterministic scheduling covers ordering, not
   latency.
4. **No production capacity claim** is made or implied: this gate evidences
   that the compact workload executes and converges at 200/300 in-process
   validators with the stated engine. ADOPTION of production BFT still
   requires the ADR-010 §7 gates (testnet at target count, API/stability
   evidence, licensing/stewardship review, security audit).

## Per-round raw output (200 validators)

```text
round   1: submit    43 ms | mine   30 ms | block  11561 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 496 95%  81 100%   0 ms
round   2: submit    64 ms | mine   36 ms | block  11561 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 2596 95% 112 100%   0 ms
round   3: submit   178 ms | mine  113 ms | block  11562 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 4276 95% 156 100%   0 ms
round   4: submit   121 ms | mine   41 ms | block  11562 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 6484 95% 219 100%   0 ms
round   5: submit   254 ms | mine   34 ms | block  11562 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 7941 95% 636 100%  98 ms
round   6: submit   244 ms | mine   37 ms | block  11562 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 10443 95% 287 100%  21 ms
round   7: submit   229 ms | mine   16 ms | block  11561 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 12942 95% 310 100%  21 ms
round   8: submit   343 ms | mine   48 ms | block  11561 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 14513 95% 322 100%   0 ms
round   9: submit   347 ms | mine   66 ms | block  11563 B / 20 txs | cert  115 B | pool-before-mine  20 | fan-out 50% 16354 95% 421 100%  34 ms
```

(300-validator raw output follows the same shape; see the SUMMARY above and
re-run the harness for the full table.)
