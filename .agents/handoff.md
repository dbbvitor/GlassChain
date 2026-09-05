# Handoff — GlassChain

**Reviewed:** 2026-09-05
**Source baseline:** `main` / `origin/main` at `f7b434e` after fetching origin.
**Change branch:** `docs/webapp-roadmap-reconciliation` — documentation and plans
only; no runtime code, dependencies or security controls changed.

## Start here

1. Read [AGENTS.md](../AGENTS.md) for repository rules, then the
   [plan index](plans/README.md) for concluded versus pending work.
2. Read [zero-trust §8](plans/zero-trust.md) before consensus optimization or
   claims about authenticated finality/history.
3. Read [source-comment debt](plans/deferred-code-debt.md) before completing
   authorization, private retention or unattended workflow deployment.
4. For the visual product, use the [browser demo plan](plans/gui-demo-benchmark.md).
   **The owner replaced desktop gpui with a web app.** WebGPU is an optional,
   measured renderer, not a framework, validator runtime or shipped feature.

## Current state — code, decisions and evidence are different

| Area | Concluded / available | Still pending |
|---|---|---|
| Workspace | 12 Rust crates; wire `glasschain/6`; 14 accepted ADRs | No browser package or demo bridge exists |
| Ledger/execution | Schema v1, capability/policy history, explicit WASM write sets and replay | Production durability acknowledgement and historical security gates |
| Consensus | PoW dev/test default; BLS proposal/prevote/precommit driver staged behind `bft` and capability | Context-authenticated votes/QCs, reliable evidence receipt state, full historical verification; no audited production BFT or speculative API |
| Identity/privacy | TLS/TOFU, optional federation verifier, CRLs/intermediates, endorsement provider, PDC delivery/reconciliation | Fail-closed unconfigured paths, key/certificate lifecycle, role/scope decisions and restart-safe physical deletion |
| Workflows/read path | Checkpointed flow engine, purchase/recall flows, triage API, provenance/flattener/event bus and RPC queries | Unattended triage discovery, durable external integration, bounded projection costs and complete operational metrics |
| Measurements | Prior local BFT p50 2,021 ms at 100 / 5,284 ms at 200; structural encoding/fan-out fixes | 300 not passing, WAN/fault/long-run memory evidence, independent comparisons; no best-in-class or deployment claim |
| PQ readiness | Discriminants shipped, provider/archival research recorded | Negotiated hybrid TLS test/selection, migration and optional archive profile; no guaranteed quantum-safe lifetime |
| Demonstration | Web-app direction and browser/bridge/renderer acceptance gates specified | Entire implementation; Canvas2D baseline, optional WebGPU, no production data or public hosting |

Earlier tickets for verifier wiring, CRLs, endorsement startup, governance defaults
and TCP fault tests closed. That does not close the deployment gaps above.
The seven source markers remain: six ponytail comments plus one madsim TODO.
D7's real-TCP fault-testing outcome is partly met by the proxy; its simulator
migration is still optional, not silently completed.

## Pending frontiers — what to do next and how to finish

### A. First engineering slice: staged consensus safety

**Ready to specify and test; no backend decision blocks it.** Start with the
four code observations in zero-trust §8, not a faster signature implementation:

1. Add failing tests for changed vote height/round/phase/chain context. Specify
   signed-byte and historical compatibility before changing votes and QCs.
2. Send conflicting votes through the real network handler: retain bounded
   ordinary receipts between messages and prove detection without false blame.
3. Test structurally valid but cryptographically invalid historical QCs through
   chain sync and restart, under the correct historical validator set.
4. Test duplicate/stale traffic against an absolute phase deadline and bounded
   queues. Count distinct eligible voters; do not allow received-message volume
   to extend a round indefinitely.

**Completion:** each regression passes through the actual entry path, default
and all-feature gates pass, compatibility decisions are documented. File narrowly
scoped implementation tickets from these acceptance cases before code; existing
closed BFT tickets do not cover their completion. Do not activate production BFT
or governance penalties merely because local benchmarks pass.

### B. Deployment trust, privacy and recovery

Independent decisions/tests can proceed alongside A:

- [Org-gated fail-open default](https://github.com/dbbvitor/GlassChain/issues/86):
  fail closed on private paths and establish credential possession/session binding.
  The old suggested insecure flag is not approved; real test credentials come first.
- [Certificate-bound MSP principals](https://github.com/dbbvitor/GlassChain/issues/87):
  D4, with deterministic historical authorization and go-forward lifecycle rules.
- [Durable TOFU pins](https://github.com/dbbvitor/GlassChain/issues/88): rotation and
  recovery policy before persistence; distinct from storing node private keys.
- D1 governance bootstrap and D2 recall authority require owner/domain decisions.
  Keep safe defaults; specify unilateral regulator versus independent-party policy.
- D5 deletion-after-restart and D6 triage discovery require acceptance tests before
  a persistent/unattended pilot; consider one narrow storage scan, two outcomes.
- [On-chain revocation registry](https://github.com/dbbvitor/GlassChain/issues/74)
  remains deferred; not a prerequisite to fixing current off-chain lifecycle gaps.

**Completion:** the D1–D6 tests in the debt plan pass and deployment access,
retention/backup and authority policies are explicit. Code alone does not certify
LGPD, ANVISA or ICP-Brasil compliance.

### C. Transport and performance: independent measured improvements

Start a two-node **negotiated KX group** test on each TLS construction path;
aws-lc already exists in the dependency graph, but runtime selection determines
use. Preserve certificate/TOFU checks. The
[backend review](https://github.com/dbbvitor/GlassChain/issues/85) evaluates each
provider's audit, compatibility and CI cost; it is not a universal no-C blocker.

After/alongside safety regressions, follow performance Step 0: WAN fault profiles,
D3 history/pool admission costs, projection-memory/lag measurements, then the BLS
verification/backend experiment. **Completion:** comparable before/after evidence
under unchanged security/quorum assumptions; no assumed 10× gain or 300 pass.
Fast paths and DAG dissemination stay behind measured triggers. Beyond-300 tests
are experimental after 300 succeeds, not ruled out by a theorem.

### D. Browser demonstration: safe parallel product slice

[Browser demo](https://github.com/dbbvitor/GlassChain/issues/61) begins with plan
step 0: one same-origin page + session-protected HTTP/SSE bridge + bounded sample
snapshot and Canvas2D/WebGPU comparison. Then extract shared workload fixtures,
build the real-node headless runner and add traceability/metrics views.

**Completion:** synthetic transactions traverse real available checks; no keys or
unauthorized payloads reach the browser; no-GPU/device-loss/reconnect cases remain
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

Local validation completed on this unchanged source baseline, 2026-09-05:

- `cargo fmt --all --check`: passed.
- `cargo check`, `cargo test`, `cargo clippy -- -D warnings` with
  `--workspace --all-targets --all-features --locked`: passed in full.
- The same check/test/clippy commands with default features (`--all-features`
  omitted): passed. This supersedes the prior 180-second partial-test timeout.
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
