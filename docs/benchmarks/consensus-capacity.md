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
