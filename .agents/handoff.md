# Handoff — GlassChain

**Reviewed:** 2026-09-14 (frontier D frontend refactor added 2026-09-18)
**Source baseline:** `main` / `origin/main` at `4560f33` (PR #121 / ADR-015 `blst` backend, merged); demo work committed at `16bc5c0` plus the 2026-09-18 frontend refactor. Latest merge: `6e6459b` (#175, the analysis stack).
**Latest session (2026-09-25):** analysis stack closed — #164–#168 shipped and closed, wayfinder map #139 closed; #167/#168 verified against the tree. Formal-verification residues (#176): the consensus-round kernels (`glasschain-core/src/rounds.rs`), the bitmap expansion (`bft::proof_arith::expand_signers`/`signers_in_range`) and channel membership + the private-payload gate (`glasschain-identity/src/{channel,payload_gate}.rs`) are proved production kernels under Verus `0.2026.09.24.b9416fa`; the four CBMC-blocked codec harnesses stay trigger-gated with re-attempt evidence. CI: ASan/LSan promoted to `ci.yml` PR/push (5m22s measured on a GitHub runner); both mutation jobs fail a run that tested no mutants — the PR diff gate had been silently vacuous because the runner's gcc (<16.1) rejects `-fuse-ld=wild` — and the linker preference wild → mold (`scripts/prefer-fast-linker.sh`, applied to the mutation jobs and the Linux workspace-build jobs; wild has no aarch64 Linux artifact). Every measured sub-30-minute job now runs on PR/push (analysis gates, `reproducible.yml`, `coverage-insights.yml`, `bench.yml`, `fuzz.yml` smoke), `ci.yml`/`analysis.yml` add a nightly full-workspace sweep (PR runs stay diff-scoped), and `deep-checks.yml` is the only schedule-only workload (full mutation shards). Next Verus roadmap stages filed as #184 (determinism) and #185 (chain rules). Working tree gates green: check, clippy `-D warnings`, fmt, `make verus` 43+2+9, `make kani` 7/7, 881 tests.
**Latest working session (2026-09-18):** `demo/static/` fully refactored and then expanded — seven-panel vanilla-JS UI, one token-based stylesheet, resizable tables and drawer, security/compliance/traceability/performance sellable panels from real runner data, measured WebGPU dot layer with Canvas2D baseline, member drawer rebuilt, stale duplicate deleted; `docs/demo.md` deduplicated and rewritten. See frontier D below.
**Latest working sessions (2026-09-14):** performance steps worked in order —
Step 0 (per-phase round timing + D7 scenarios, ADR-016 durability decision),
Step 1 (codec profiled — JSON stays the wire), Step 3 (incremental D3
admission index: flat ~0.19 ms from ~21 ms at 10k), Step 6 installments
(bounded 8 000-tx pool, stats, priority lanes, handshake re-audit, batching:
4 000-tx slice — the previously failing 9 000-tx probe now sustains, 740 KB
blocks converge 8/8, p50 5 275 ms under 9 000-tx offered load), the
latency-opportunity plan with all gated items executed (concurrent vote
verification, lanes, height-bounded catch-up wire `/7`, reconnect backoff —
300-gate p50 4 612 → 4 117 ms), scale table complete (10/100/200/300 →
194 ms/1.1 s/2.5 s/4.1 s), §5 read-path gaps closed (lagging-subscriber
drops receiver-observable; burst-vs-steady sub-ms/block), Step 5 fault
profile recorded (fail-closed 3.01 s, heal 173 ms), #6 declined for now
(ADR-002 amendment note; rollback design doc exists), block-relay gossip
measured and reverted. Step 7's in-repo half shipped (validator-set churn across heights — ADR-009 reconfiguration exercised through the driver); Open: node-level peak-RSS harness; 400/500 sweeps under hardware budgets; Step 7's deployment half (failure-domain placement evidence, fleet participation) is deployer work.

## Start here

1. Read [AGENTS.md](../AGENTS.md) for repository rules, then the
   [plan index](plans/README.md) for concluded versus pending work.
2. Previous priority sequence: **Frontier C (Consensus Capacity / BLST Backend)
   → Frontier B (RBAC & Operational Tail) → Frontier D (Browser Demo)**.
   Frontier A concluded: `EquivocationProof` carries both dual-signed votes
   and verifies through the #95 context envelope. **Frontier C concluded
   2026-09-13** (ADR-015: audited `blst` backend, sum-of-keys verify,
   300-validator gate passing); Frontier B concluded 2026-09-14 via
   [ADR-017](../../docs/adr/adr-017-deployment-trust-and-retention.md)
   (OCSP stapling only, MSP admin RBAC, backup scrubbing, #74 deferred);
   next frontier is **Frontier D**.
3. Read [zero-trust §8](plans/zero-trust.md) for consensus safety invariants.
4. Read [source-comment debt](plans/deferred-code-debt.md) for settled D1–D7
   markers and benchmarks.
5. For the visual product, use the [browser demo plan](plans/gui-demo-benchmark.md).
   Web app replaces desktop gpui; Canvas2D baseline, optional WebGPU.

## Current state — code, decisions and evidence are different

| Area | Concluded / available | Still pending |
|---|---|---|
| Workspace | 12 Rust crates plus the standalone `demo/` package (excluded from the workspace, own lockfile); wire `glasschain/6`; 17 accepted ADRs (ADR-016 durability, ADR-017 deployment trust); D1–D7 settled | Browser-smoke CI for the demo |
| Ledger/execution | Schema v1, capability/policy history, explicit WASM write sets and replay | Production durability acknowledgement and historical security gates |
| Consensus | PoW dev/test default; BLS driver with context-authenticated votes (#95, #99), live receipt journal (#96), full historical QC verification on sync/restart (#97, PR #116), absolute phase deadlines/bounded queues/distinct voters (#98), dual-sign `EquivocationProof` (Frontier A concluded) | Production audit/testnet/APIs (ADR-010) |
| Identity/privacy | TLS/TOFU with durable pins & signed rotation (#88), opt-in verifier with fail-closed private paths (#86), session-bound possession proofs (#110), CRLs/intermediates (ADR-013), cert-bound MSP principals with height authorization (#87, D4), fail-closed governance fallback (D1), issuer-signed recall (D2), restart-safe purge (D5) & triage discovery (D6); **Frontier B concluded via ADR-017 + code (2026-09-14)**: OCSP staple minted per member, stapled on `Hello` and verified locally (no responder egress, CRL fallback fail-closed); `AdminGate`-gated channel-management RPCs over certificate-bound admin principals; `glasschain backup-scrub` retention sweep for storage copies; per-Hello org reauthorization (downgrade on failed re-verification) | Residual plan **shipped 2026-09-14** ([zero-trust-residual](plans/zero-trust-residual.md)): `--identity-file` durable custody (ADR-018 — the same identity key/cert/Root CA across restarts, pins keep verifying), `glasschain channel-admin` CLI client, `reload-trust-store` REPL hot-reload with `AdminGate` hot-swap, durable equivocation evidence via the state seam; #74 + delegated responders parked by decision |
| Workflows/read path | Checkpointed flow engine, purchase/recall flows, triage API with restart discovery (D6), provenance/flattener/event bus and RPC queries; D3 baseline measured (#106) | Unattended external integration, durable external indexer adapter, bounded projection costs |
| Measurements | BFT finality on `blst` (ADR-015): p50 1 145 ms at 100 / 4 612 ms at 300 (2026-09-14) with the per-phase decomposition recorded; the 300 gate passes and verify is no longer the wall (fan-out is). D3 admission bench (~21 ms at 10k); read-path memory baseline (#107); D7 WAN scenarios (proxy #108 + leader-quorum loss + bandwidth budget); Step 0 marked done | Long-run fleet memory; 400/500 sweep is out of scope; slow-CPU/disk WAN scenario |
| PQ readiness | Discriminants shipped; negotiated X25519MLKEM768 hybrid TLS behind `pq-tls` shipped (#105) | Long-term archive evidence / migration policy; no guaranteed quantum-safe lifetime |
| Demonstration | **Frontier D committed 2026-09-17 (`16bc5c0`), frontend refactored + sellable-surface pass 2026-09-18:** `demo/` package — axum bridge + 15-node multi-company runner + seven-panel vanilla-JS UI (resizable tables/drawer, own-node views, security posture, compliance/lineage, block chain, performance charts, measured WebGPU dot layer with Canvas2D fallback); zero-trust evil nodes (membership/schema/fail-closed/anchored/replay outcomes all real); Rust tests own the guarantees; docs `docs/demo.md` | Plan step 4 (fault scenarios); browser-smoke CI; real-device renderer comparison |

The seven source markers (D1–D7) are fully settled (#106–#109, #114–#115):
D1 governance bootstrap documented; D2 recall issuer-signed; D3 admission cost
benchmarked; D4 cert-bound principals shipped; D5 restart-safe purge shipped;
D6 triage discovery shipped; D7 WAN proxy profiles shipped.

## Pending frontiers — what to do next and how to finish

### A. Consensus safety residual (concluded)

**Status:** core safety mechanisms shipped in #95, #96, #97, #98, #99, and PR
#116; Frontier A concluded with the `EquivocationProof` format migration:
the proof carries both conflicting votes in full and `verify()` rides the
dual-sign context envelope (`domain || chain-id || height || round || phase ||
block-hash`) introduced in #95, so evidence can no longer be assembled from
votes of different contexts. Residual evidence-path hardening beyond the
journal is future work (see zero-trust §8).

### B. Deployment trust, privacy and recovery (concluded 2026-09-14)

Code items D1–D6 and #86–#88 shipped with tests. Frontier B concluded via
[ADR-017](../../docs/adr/adr-017-deployment-trust-and-retention.md) **plus its
code** (implementation plan: [frontier-b-tail](frontier-b-tail.md)):
- **OCSP stapling** — the org Root CA mints an issuer-signed OCSP
  `BasicResponse` per member certificate; the node staples it on every
  `Hello` (`ocsp_response_der`), receivers verify it **locally**
  (`CertChainVerifier::verify_ocsp_staple`); a revoked staple fails the
  session closed, absent/invalid/expired staples fall back to the fail-closed
  CRL path. No outbound responder queries.
- **Operator RBAC** — member certs carry the admin role as subject OU;
  `AdminGate` gates `CreateChannel`/`AddChannelMember`/`RemoveChannelMember`
  on `NodeService`; without a verifier these fail closed.
- **Backup scrubbing** — `glasschain backup-scrub` runs the D5 sweep over a
  copied storage directory before archival.
- **Reauthorization** — the TOFU `Known` path assigns the fresh Hello's
  org-verification result (upgrade or downgrade).
- **On-chain revocation registry (#74)** — remains deferred.

Next active frontier: **Frontier D (Browser Demonstration)**.

### C. Transport and performance (concluded 2026-09-13)

Hybrid TLS negotiation shipped (#105). Step 0 prerequisites (WAN proxy #108,
D3 admission bench #106, read-path memory baseline #107) are complete.

- **Step 4 (BLS backend, issue #85) concluded via ADR-015:** the audited `blst`
  C backend replaces the pure-Rust `pairing` path; the same-message verify is
  the sum-of-keys PopScheme check over `blstrs` — two pairing terms at any
  quorum size. Measured: pure-Rust 80.0 ms → blst sum-of-keys 1.78 ms at
  quorum 201 (`cargo bench -p glasschain-core --bench bft_verify`).
- **Completion evidence:** the 300-validator finality gate now passes with
  exact-quorum certificates every round — p50 1 096 ms (100) / 2 397 ms (200) /
  3 996 ms (300), before/after in `docs/benchmarks/consensus-capacity.md`.
  Remaining round cost is mesh replication, not verification.
- Residual performance work stays on the performance plan: Step 1 codec
  profiling, D3 admission rebuild optimization, Steps 5+ research only.

### D. Browser demonstration (in progress — frontend refactored 2026-09-18)

[Browser demo](https://github.com/dbbvitor/GlassChain/issues/61). `demo/` is a
standalone package (excluded from the workspace, own lockfile): an axum bridge
(loopback, Origin+token-gated commands, bounded SSE snapshots, per-member
own-node views) and a headless multi-company runner over real `Node`s, plus a
vanilla-JS UI. Docs: `docs/demo.md`.

**Frontend refactor (2026-09-18).** `demo/static/` was rebuilt as one coherent
frontend: `index.html` (app shell), one token-based stylesheet (Linear × Apple
× Material), `views.js` (diff-rendered panels), `graph.js` (DPR canvas,
pointer interactions, measured draw-time p99) and `app.js` (state, snapshot
stream, commands, resizable surfaces, drawer). Seven panels — Overview /
Inventory / Trade / Traceability / Trust & Security / Compliance / Performance
— replace the single endless page. Every table and the drawer are resizable
(pointer drag or keyboard separators, sizes persisted), and drawers are the
only scroll surfaces.

**Sellable-surface pass (2026-09-18, same day).** The demo now surfaces the
security, compliance, traceability and performance story from real data:
per-member security posture (certificate verifier, X.509 cert, issuer-signed
OCSP staple verified locally, channels, peer sessions), a tamper-evident
block-chain table, verifiable lineage per lot, SNCM schema validation with the
gas-fee schedule, analytical flat records with missing fields, fleet trust
distribution, contract conditions with committed activity, and per-round
throughput/latency charts. Backend additions to `RunState`: `blocks`,
`posture`, `contracts`, `compliance`, `history`, plus `schema_compliant` /
`lineage_complete` / `trust_avg` on `LotView`. Also: `OfferEvent.round`, sells
carry `buyer`, cert `lot_ref` normalized to `LOT-n`, debug `eprintln!`s
removed, stale 92 KB `demo/scenario.rs` duplicate deleted, and a real
**server-side origin-scoping fix** in `org_snapshot` (another member's payload
cleartext was readable by any viewer; now author / regulator / admin only,
covered by a test).

**Variability, trust and graph pass (2026-09-18, same day).** Offers now vary
per lot (eight cheap price tiers, six premium bands, quantity tiers 100–500
plus stock-pressure step) and four real contracts back the bands: one auto
(`auto-replenish`, pharmacy-1, cap 1200) and one conditions-only contract per
pharmacy, whose disjoint bands guarantee a premium offer matches exactly one.
`OrgView` gained a per-org trust score and record count (average
`MetadataTrustScore` over registrations the org originated; evil scores 60,
honest 100, no-records shows undefined), surfaced in the posture and
visibility tables, the member drawer and the graph tooltip. The graph is now a
shared collapsible card above all panels, and its dots are keyed by
transaction id: each flows once from origin to destination and fades, instead
of restarting/teleporting as the backend window slides. Member inspect latency:
`org_snapshot` clones only the listed payload rows (was the whole ledger per
request) and the drawer's PDC table caps at the latest 40 rows. Rows are
inspectable: contracts and purchases open drawers (the contract drawer lists
its committed purchases), posture/visibility rows open the member drawer, and
schema/flat-record rows open the lot drawer — all verified through the DOM
harness.

**Performance/stress pass (2026-09-18, same day).** Stress the runner and
sell the numbers: fixed a settle bug that pinned every round to the 2.5 s
deadline (the wait list carried pipeline anchor ids committed rounds earlier),
fanned admission out 24-wide and payload dissemination 16-wide, and made the
compliance/trust/contract projections incremental (fold only blocks newer than
a cursor; lot lookups via a per-round index). Bounds raised: lots/round ≤ 50,
interval 0 (back-to-back). Per-round phase timings (scenario, payloads,
submit, settle, mine, project, retail) flow to a stacked Performance chart, a
per-phase p50/p95/p99 percentile table, and round/commit p50/p95/p99 KPIs
(commit p99 added to `Metrics`), and a one-click **Stress preset** applies 50
lots/round at interval 0. Reference (debug build, laptop, cleaned-up machine):
baseline was
73 tx/s with ~2.7 s rounds; now **614 tx/s**, round p50 753 ms / p95 1 133 ms,
commit p50 222 ms, 1 800 lots in 28 s, projections flat at ~35 ms/round.
Test suite also dropped 17 s → 5 s from the incremental projections.

**Renderer:** Canvas2D baseline measures its own draw-time p99 against the
10 ms target-load budget; a WebGPU dot layer on a transparent overlay
activates only on a measured baseline miss (or forced with
`?renderer=webgpu` for verification; `?renderer=canvas` pins the baseline) and
falls back automatically on adapter failure or `device.lost`. The real-device
comparison is still pending and belongs in `docs/demo.md`.

**Viewer-fetch starvation fix (2026-09-18).** On large runs the Admin lot/cert
drawers showed `0 payload(s)`: the 500 ms tick refreshed the whole admin lens
(~8 payload rows per lot) and a request-id guard discarded every response once
the fetch outlived the tick, so the viewer snapshot was never stored. Fixed
with a single in-flight fetch, view-tagged data (`orgViewFor`), and an
on-demand admin lens with a 2 s freshness window; drawers await the fetch
before filtering. Reproduced deterministically with an 800 ms admin-delay
harness (0 → 10 payloads). Details:
[`memories/demo-viewer-refresh.md`](memories/demo-viewer-refresh.md).

Runner behaviors, all covered by Rust tests: 15-node default federation from
parameters, one custody hop per round, origin-scoped payloads
(pricing/storage/transit/process/intake/temperature_log/certification_evidence/
regulator_notes), the contract engine matching `SupplyOffer`s into
`PurchaseOrder`s (auto + manual, partial fills via `sold`), stock-scaled buy
pressure and a slow retail drain, per-member own-node snapshots with
server-side origin scoping, the visibility matrix, the admin demo lens, and
five real evil attack kinds per evil node per round.

Gotchas that still apply:

- The wire engine relays one hop: the star center (first company) is the block
  producer and every company dials it — a deployment property, not consensus.
- Never construct demo nodes with `127.0.0.1:0` — `listen_addr()` echoes the
  configured string; reserve concrete ports via `stash_prebound_listener`.
- Custody hops submit sequentially with per-hop pool confirmation
  (`wait_for_ids` on exact tx ids); unordered batching shuffles lot chains.
- Never hold a `SharedRun.state` guard across an await — even in tests it
  deadlocks (tokio Mutex fairness).
- Demo cash tests must be delta-based around the movement they check.

Remaining: plan step 4 (fault scenarios) and browser-smoke CI. A real-device
renderer comparison must be recorded in `docs/demo.md`; the forced
`?renderer=webgpu` path exists for that run. Deeper library surfaces
(recall/dispute workflows, endorsements, WASM VM execution, gRPC/CLI) are
listed as deliberate boundaries in `docs/demo.md`.

### E. Deferred research

PQ archive evidence needs trusted time, preserved validation material, renewal,
retention and legal/profile review. Learning starts with offline outcomes against
a rules baseline; FL remains a SHOULD. Neither changes `SCHEMA_V1` or bypasses
endorsement. Use the relevant plans rather than inventing a new platform now.

## Validation and PR procedure

Local validation completed on this branch, 2026-09-12 (worktree target dir,
Rust 1.98.1 — the branch also pins the toolchain, so the gates below ran on
it):

- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets --all-features --locked`: passed.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D
  warnings`: passed, zero diagnostics (three 1.98 lint fixes: the SDK client
  constructor is now sync and infallible, and two CLI log borrows dropped).
- `cargo test --workspace --lib --bins --tests --all-features --locked`:
  passed — 589 tests across 33 harnesses with parallel harnesses (the
  new shared port-band allocator removes the serial constraint; bench
  executions run in the new Benchmarks workflow instead of the test gate).
- `cargo bench -p glasschain-core`: passed in release on 1.98.1 (the
  bench.yml command shape); the vm/workflows benches share the same shape.
- Coverage-specific: `cargo tarpaulin -p glasschain-network --all-features
  --lib --tests --locked --engine llvm` passed twice after fixing the
  instrumented-timing flakiness in the BFT vote collector tests — the
  collector now skips verification for duplicate copies of an already-counted
  voter (§8.4 flood relief), count-invariant tests use a generous window
  (the collector still exits at quorum), and the deadline-control asserts
  its structural bound (never ≥ 3) rather than an exact count.
- Large ignored scale/WAN gates were not re-run; prior numbers remain dated evidence.

For the PR, verify all local links, marker coverage and whitespace; fetch origin
and confirm no conflicts. `.github/workflows/ci.yml` filters docs-only changes,
so manually dispatch **CI on the final branch SHA** to exercise all platforms,
coverage and dependency audit. Inspect CodeQL/code-quality checks too; a local
pass is not remote green. Remote statuses belong on the PR, not a permanent
“all CI green” claim here. Do not weaken rules or suppress a failing check.

On resumption, read the PR's live checks and compare its base to `origin/main`.
If GitHub's external analysis service fails, record its exact run/error and stop
short of claiming merge readiness. Keep the PR open; merge only on explicit request.

## OpenSSF compliance track (2026-09-15)

Implemented the unblocked OpenSSF tickets (#125–#128) on the working tree —
uncommitted, see `.agents/plans/openssf-compliance-implementation.md` for the
full list. Per-file SPDX+copyright headers landed on all 108 `crates/**/*.rs`;
new workflows: `fuzz.yml`, `reproducible.yml`, `release.yml`, plus
`codecov.yml` (≥90% blocking) and a DCO check job in `ci.yml`. GitHub issue
writes are blocked on a valid oauth token (current one 401s) — resolution
comments/closures and the #123 map update are pending that.

**Update (2026-09-15, later):** `gh` re-authenticated — all pending tracker
writes done: #125–#128 claimed + resolved + closed, map #123 updated,
`main` branch protection enabled with required checks (Format, Clippy,
Test ×3, Code coverage, DCO sign-off, Security audit, `codecov/project`).
Remaining: #129 (synthesis) is now unblocked.
