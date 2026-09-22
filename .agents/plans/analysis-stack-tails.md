# Analysis stack — remaining map children (#166, #167, #168)

Base: `origin/ci/pr-analysis-gate` (PR #175, 19/19 green). Branch `ci/deep-checks`.
Map: [#139](https://github.com/dbbvitor/GlassChain/issues/139). Decisions live in
the map body; this file is execution order and constraints only.

## Constraints discovered

- The research/probe branches (`probe/leak-fix`, `probe/verus-trial`, the
  checker research) do not exist in this clone, so their commits cannot be
  cherry-picked. `Registry::with_schema`'s leak is reimplemented as the record
  describes (`Cow<'static, str>`); the Verus module is rewritten from the
  pinned release.
- `--in-place` cannot be combined with `--jobs` (cargo-mutants 27.1.0). CI
  jobs stage a scratch copy and run `--in-place` there: the checkout is never
  mutated and builds stay incremental. `--jobs` would force copy mode, which
  the tool documents as losing all build reuse — infeasible for the full
  workspace (`.agents/memories/cargo-mutants-footguns.md`).
- Nightly/formal tools are installed by the workflow, not locally: the box has
  4 cores and the repo's `target/` is not shared between worktrees.
- Kani covers three predicates; the rest is deferred with evidence
  (`.agents/memories/kani-deferral.md`). Verus covers the modules Kani cannot
  process.

## #166 — deep-checks + prerequisites

1. Prereqs: `Cow` schema hash; rpc tests `mem::forget` → drop; madsim vs
   turmoil (port holds or a documented reason it stayed).
2. `.github/workflows/deep-checks.yml`
   - nightly: miri matrix (6 crates, per-crate skips, strict flags), full
     mutants 16 shards, ASan/LSan (`-Zbuild-std`, `detect_leaks=1`).
   - weekly Monday: Kani 0.68.0, Verus 0.2026.09.20.aef82ed, the 13 ignored
     gates under `ulimit -n 65535`, turmoil chaos run.
3. Auto-issue: rolling per-workflow issue, `ci-failure` label, auto-close on
   green — deep-checks, fuzz, reproducible, coverage-insights.

## #167 — release/CD repair and hardening

CycloneDX invocation, `reproducible.yml` alignment, SHA-pin actions +
Dependabot `github-actions`, `release.yml` hardening (cache, concurrency,
draft/tag check, SHA256SUMS), `bench.yml` off PRs, cache/nightly pin fixes.

## #168 — ADR + docs

ADR for the analysis stack (gates vs scheduled, strictness exceptions,
deferral triggers) and sync of `AGENTS.md`, `CONTRIBUTING.md`, `README.md`,
`docs/operations.md`, `codecov.yml` once 166/167 are in the tree.
