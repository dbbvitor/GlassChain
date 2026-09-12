# Handoff — GlassChain

**Reviewed:** 2026-09-12
**Source baseline:** `main` / `origin/main` at `523cd42` (PR #116 / `fix/sync-certificate-events-and-doc-drift`).
**Change branch:** `feat/frontier-a-dual-sign-proof-ci` — dual-sign `EquivocationProof` (Frontier A
conclusion), parallel-safe test ports and CI speed, workflow badges, Rust
1.98.1 pin and a dedicated parallel bench workflow.

## Start here

1. Read [AGENTS.md](../AGENTS.md) for repository rules, then the
   [plan index](plans/README.md) for concluded versus pending work.
2. Active priority sequence: **Frontier C (Consensus Capacity / BLST Backend) →
   Frontier B (RBAC & Operational Tail) → Frontier D (Browser Demo)**.
   Frontier A concluded: `EquivocationProof` carries both dual-signed votes
   and verifies through the #95 context envelope.
3. Read [zero-trust §8](plans/zero-trust.md) for consensus safety invariants.
4. Read [source-comment debt](plans/deferred-code-debt.md) for settled D1–D7
   markers and benchmarks.
5. For the visual product, use the [browser demo plan](plans/gui-demo-benchmark.md).
   Web app replaces desktop gpui; Canvas2D baseline, optional WebGPU.

## Current state — code, decisions and evidence are different

| Area | Concluded / available | Still pending |
|---|---|---|
| Workspace | 12 Rust crates; wire `glasschain/6`; 14 accepted ADRs; D1–D7 settled | No browser package or demo bridge exists |
| Ledger/execution | Schema v1, capability/policy history, explicit WASM write sets and replay | Production durability acknowledgement and historical security gates |
| Consensus | PoW dev/test default; BLS driver with context-authenticated votes (#95, #99), live receipt journal (#96), full historical QC verification on sync/restart (#97, PR #116), absolute phase deadlines/bounded queues/distinct voters (#98), dual-sign `EquivocationProof` (Frontier A concluded) | Production audit/testnet/APIs (ADR-010) |
| Identity/privacy | TLS/TOFU with durable pins & signed rotation (#88), opt-in verifier with fail-closed private paths (#86), session-bound possession proofs (#110), CRLs/intermediates (ADR-013), cert-bound MSP principals with height authorization (#87, D4), fail-closed governance fallback (D1), issuer-signed recall (D2), restart-safe purge (D5) & triage discovery (D6) | OCSP verification & stapling, deployment access (operator RBAC/channel-management operations), explicit replica/backup retention policy, deferred on-chain revocation (#74) |
| Workflows/read path | Checkpointed flow engine, purchase/recall flows, triage API with restart discovery (D6), provenance/flattener/event bus and RPC queries; D3 baseline measured (#106) | Unattended external integration, durable external indexer adapter, bounded projection costs |
| Measurements | Prior local BFT p50 2,021 ms at 100 / 5,284 ms at 200; D3 admission bench (~21 ms at 10k); read-path memory baseline (#107); D7 WAN proxy profiles (#108) | 300-validator pass (pure-Rust pairing bottleneck; active Frontier C focus); long-run fleet memory |
| PQ readiness | Discriminants shipped; negotiated X25519MLKEM768 hybrid TLS behind `pq-tls` shipped (#105) | Long-term archive evidence / migration policy; no guaranteed quantum-safe lifetime |
| Demonstration | Web-app direction and browser/bridge/renderer acceptance gates specified | Entire implementation; Canvas2D baseline, optional WebGPU (Frontier D, queued after C and B) |

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

### B. Deployment trust, privacy and recovery (queued after C)

Code items D1–D6 and #86–#88 shipped with tests. Frontier B remains open on:

- OCSP (Online Certificate Status Protocol) verification and stapling for live peer authentication.
- Deployment access (operator RBAC and channel-management operations specification).
- Explicit replica/backup physical retention and recovery policy.
- On-chain revocation registry (#74) remains deferred.

### C. Active Priority: Transport and performance (unblock 300 validators)

Hybrid TLS negotiation shipped (#105). Step 0 prerequisites (WAN proxy #108,
D3 admission bench #106, read-path memory baseline #107) are complete.
Active focus:

- **Step 3 (BLS verification / pairing backend experiment, issue #85):** evaluate
  `blst` vs pure-Rust `pairing` to unblock the 300-validator finality gate timeout.
  Validate API portability, Windows/macOS/Linux CI, and aggregate-public-key verification.
- **Completion:** comparable before/after evidence under unchanged quorum assumptions;
  300-validator gate passing within budget.

### D. Browser demonstration (queued after C and B)

[Browser demo](https://github.com/dbbvitor/GlassChain/issues/61) begins with plan
step 0: one same-origin page + session-protected HTTP/SSE bridge + bounded sample
snapshot and Canvas2D/WebGPU comparison. Queued after Frontiers C and B.
usable; headless and UI totals agree; run resources are bounded and cleaned up.
Core validation never depends on a browser/GPU. This can be built without waiting
for speculative consensus, FL, an archive TSA or a production REST gateway, but
must label the staged engine and unresolved privacy/recovery guarantees honestly.

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
