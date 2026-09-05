# Plan — Best-in-class performance within zero-trust, ICP and LGPD constraints

**Status:** active; structural work shipped, adoption/performance gates incomplete
**Reviewed:** 2026-09-05 against `f7b434e`
**History:** [Performance programme](https://github.com/dbbvitor/GlassChain/issues/62) is closed, not proof that every step or production gate passed.
**Related:** [ADR-002](../../docs/adr/adr-002-consensus-finality.md), [ADR-004](../../docs/adr/adr-004-scale-topology.md), [ADR-010](../../docs/adr/adr-010-capability-versioning-policy.md), [ADR-014](../../docs/adr/adr-014-bls-aggregated-certificates.md), [zero-trust](zero-trust.md), [source-comment debt](deferred-code-debt.md).

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
| `bft_finality_gate_100` | p50 2,021 ms; p95/p99 2,162 ms | Release, loopback, shared runtime, synthetic workload |
| `bft_finality_gate_200` | p50 5,284 ms; p95/p99 5,744 ms | Same; exact-quorum certificates, no view changes in recorded run |
| `bft_finality_gate_300` | Not passing on the pure-Rust pairing path | First vote 34.8 s; subsequent verification exceeds the budget |
| `consensus_capacity` 200/300 | PoW production/propagation, recovery and PDC measurements | Not BFT finality or WAN/testnet evidence |

Short runs do not establish a reliable p99; the table preserves the reported
figures, not a tail-latency guarantee. A backend swap has not been measured here.
There is no demonstrated 10× speedup or automatic pass at 300.

### Shipped and superseded work

- Broadcast fan-out uses independent bounded peer channels with `try_send`;
  full broadcast channels now increment per-peer `dropped_outbound` counters.
- Mine-path block broadcast precedes persistence; `after_block_commit` offloads
  `storage.apply_block` through awaited `spawn_blocking`.
- PDC reconciliation now fans requests to all member peers. Its return value is
  requests queued, **not** payloads delivered; completion/retry remains separate.
- Propagation thresholds share a start/poll loop; stride sampling reduces the
  observer's lock contention. `NodeState` caches capability history incrementally.
- Wire versions `/5` and `/6` shipped base64/signature discriminants and BLS QCs.
  Current peer codec remains JSON. `Attestation` was removed by ADR-014.
- Ed25519 batch verification and the old attestation lookup optimization were
  superseded by BLS. Do not reimplement obsolete steps.
- Liveness placement/SLA guidance exists in `docs/liveness.md`; guidance is not
  proof of runtime placement enforcement or measured fleet availability.

**Residual D3:** `Ledger::add_transaction` still rebuilds capability history for
canonical records/activations and scans historical/pending IDs, including when
called by `Node::submit_transaction`. The node cache did not remove this cost.

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

**No extra WAL.** Sled already logs writes; block+write-set replay already
rebuilds derived state. A third log is not a cure for scheduling or queueing.
`SledStorageProvider::flush` is only called in a test; the default periodic flush
is not an application durability acknowledgement or a hard 500 ms loss bound.

Before a persistent pilot, decide what “committed” promises across process crash,
power loss and quorum-wide failure. Compare current periodic flushing with
explicit durable acknowledgement and, only if useful, bounded group commit.
Measure throughput/p99 plus crash recovery. Do not postpone required durability
merely to protect a latency number. Keep logical finality, replication and local
stable-storage acknowledgement as separate metrics.

## 5. Benchmark additions from the external report

### WAN and round-change scenarios — adopt for the actual driver

Extend `tests/tcp_partition.rs`'s established-session proxy rather than replacing
Tokio first (source debt **D7**). It currently uses `copy_bidirectional` and
partition/repair, **not** a WAN latency matrix. Add bounded per-direction relay
queues, a seedable latency/jitter/bandwidth profile and correlated link failures;
verify advertised addresses/reconnects cannot bypass the overlay. TCP byte-stream
faults are not a simulation of independent packet reordering/loss.

Test 4/10 validators first, then 100/200/300 under explicit hardware budgets:

- no-fault baseline; asymmetric WAN delays; slow leader/validator CPU and disk;
- leader loss before/after prevote or precommit; delayed/duplicated/stale votes;
- equivocation and invalid signatures; partition below/above quorum then repair;
- saturation and recovery with the same workload, retention and security settings.

Assert no conflicting finalization and no premature side effects; measure time
without quorum separately from recovery once a quorum is reachable. Distinguish
seeded fault profiles over real wall-clock TCP from full deterministic madsim
execution. The optional madsim-tokio migration remains behind compatibility and
additional-coverage evidence, not a prerequisite to these WAN measurements.

### High-frequency flattening and read-path memory — adopt

`AnalyticalFlattener` retains a growing `Vec<FlatAssetRecord>` and currently
ingests **AssetRegistration**, not every canonical event. The event bus's
bounded broadcast/ring buffer does not bound the flattener, provenance index or
caller-side query allocations. Ingestion also runs inside
`Node::after_block_commit`; “off-chain” does not mean zero cost to the node.

Use existing asset fixtures with a fixed seed and increasing histories (for
example 1k/10k/100k registrations, stopping at an explicit memory budget).
Include canonical-record-only traffic as a control so a no-op flattening path
cannot masquerade as high throughput. Run bursts, long steady input, lagging
subscribers, repeated lineage/CSV queries and rebuild/replay.

Record retained rows, bytes/row, peak and steady RSS, allocation/CPU cost where
available, ingestion p50/p95/p99, query latency, consumer lag/drop counts, replay
time, and node finality with/without the load. Separate expected retained history
from a leak; count replay duplicates and verify output correctness. Sampling and
GUI rendering overhead get an on/off comparison. If budgets fail, try bounded
batch export/pagination and a rebuildable projection before a new database or
service. A slow analytics consumer must not block consensus, and loss recovery
must replay committed history rather than silently omit events.

## 6. Ordered path (stable Step 0–7 names)

- [ ] **Step 0 — trustworthy measurement and safety baseline.** Retain both
  existing harnesses, add §5 WAN/round-change and memory scenarios, instrument
  proposal/vote/verification/commit/replication/flush separately. Record commit,
  toolchain/features, security configuration, workload, seed, offered vs committed
  rate, topology, CPU/RAM, bytes and repetitions. Count timeouts/rejects instead
  of excluding them; use offered-load timing to avoid hiding queueing delay.
  Longer runs and primary-source, like-for-like competitor measurements are
  required before claiming sub-second or best-in-class performance.
- [ ] **Step 1 — remaining codec costs, measured first.** Base64/discriminants
  shipped. Profile JSON encode/decode and bytes with the *current* BLS shape;
  consider binary encoding only when it matters. A wire/history compatibility
  decision and decode-boundary tests precede a swap.
- [x] **Step 2 — Ed25519 batch verification: superseded.** BLS removed the
  per-attestation certificate loop. Invalid aggregate rejection does not identify
  an individual signer; attribution relies on authenticated votes/evidence, not
  a sequential fallback that no longer exists.
- [ ] **Step 3 — history-dependent admission (D3).** Old quadratic attestation
  lookup is obsolete; benchmark the still-live `Ledger::add_transaction` rebuild
  and ID scans. Optimize shared ownership/invalidation, not just one caller.
- [ ] **Step 4 — BLS follow-up, not a new adoption.** Aggregation shipped. QC
  signature is 96 bytes **plus `ceil(n/8)` bitmap and metadata**; the whole
  certificate is not O(1). `verify_same_message_multisig` currently performs
  O(quorum) pairing terms. Compare PoP-validated aggregate-public-key verification
  and an audited `blst` path before selecting a backend. Check subgroup/identity
  rejection, duplicate keys/signers, signed message domains and byte-for-byte
  compatibility against existing fixtures. [Backend review](https://github.com/dbbvitor/GlassChain/issues/85)
  records the trade; no speed multiplier or passing 300 gate is assumed.
- [ ] **Step 5 — in-family latency candidates, research only.** Profile and fix
  the existing driver before borrowing HotStuff-1/SBFT ideas. Their safe early
  reply conditions are protocol-specific, not a one-phase toggle. Any proposal
  must specify fallback, locking/view synchronization and fault assumptions;
  client-visible speculative results may not be called final or authorize
  inventory, payment, endorsement or private-data side effects. Benchmark
  rollback/prefix-fork cases only after a design/prototype exists. Adoption that
  changes ADR-002 semantics requires an explicit decision, not this report.
- [ ] **Step 6 — mempool/dissemination, simplest changes first.** Measure D3,
  pending count/**bytes**/age, bursts, duplicates, slow proposers and backlog
  drain under offered load. Specify bounded admission, fair batching, explicit
  backpressure, retry/idempotency and abandoned-proposal restoration first.
  A 20-tx pool drained each round is not a saturation study. Consider
  Narwhal-style availability/dissemination only if propagation or persistent
  backlog remains dominant after these changes. It does not cure hot-key
  contention. Availability certificates, missing payload recovery, GC and PDC
  authorization need a design; DAG ordering/Snow consensus are not implied.
- [ ] **Step 7 — operational liveness.** Guidance shipped, enforcement and
  operational evidence remain gates. Exercise real failure-domain placement,
  epoch changes and participation monitoring without reputation weighting or
  weakened quorum. Use authenticated signer data for metrics, not penalties
  inferred from unverified or missing traffic.

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
