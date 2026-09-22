# ADR-019 — Analysis stack: blocking PR gates, scheduled deep checks, deferred tools

**Status:** Accepted
**Date:** 2026-09-21
**Decision owner:** project owner
**Relates to:**
[ADR-015](adr-015-audited-c-crypto-backends.md) (audited backends / dependency evidence) ·
[wayfinder #139](https://github.com/dbbvitor/GlassChain/issues/139) (the decision map) ·
[`.agents/plans/analysis-stack-tails.md`](../../.agents/plans/analysis-stack-tails.md)

## Context

`ci.yml` covers fmt, clippy, the three-OS test matrix, coverage and the security
audit; `fuzz.yml` and `audit.yml` cover their own concerns. On top of that the
project adopted a four-tier analysis stack: PR gates, scheduled deep dynamic
checks, and formal verification. #165 shipped the PR gate, #166 the scheduled
tier, #167 the release hardening. Without a record, the gating policy, the
deliberate strictness exceptions and the tools that were rejected or deferred
would read as drift — so this ADR pins them.

## Decision

### Blocking PR gate — `analysis.yml`

Six ubuntu jobs run on every PR that touches code or config, each with its own
timeout:

| Job | Check |
|---|---|
| Dependency hygiene | `cargo machete --with-metadata crates`, `cargo deny --all-features check` |
| Spelling | `typos` |
| Cache-line layout | `cargo +nightly snarf --format github` (findings to stdout, collision warnings to a file) |
| Feature matrix | `cargo hack check --each-feature --workspace --all-targets --locked` |
| cargo-careful | `cargo +nightly careful nextest run --profile ci --workspace --lib --bins --tests --all-features --locked` |
| Mutation (diff) | `cargo mutants --in-diff pr.diff --baseline=skip --in-place --timeout 60` |

Guardrail: a job whose cold-cache runtime exceeds 20 minutes moves to the
scheduled workflow. The first measurement (PR #175) kept every job in place;
the slowest was the cache-line layout at ~6.5 minutes.

### Scheduled deep checks — `deep-checks.yml`

Nightly: Miri over the six-crate allowlist (per-crate skips, strict flags);
the full mutants run over all 12 crates in 16 shards; ASan/LSan over the
workspace. Weekly Monday: Kani and Verus proof jobs, the 13 `#[ignore]`d
measurement and capacity gates under `ulimit -n 65535`, and the turmoil
determinism scenario.

Failures surface as **one rolling issue per workflow** (`ci-failure` label):
`ci-failure-issues.yml` opens or updates it on a scheduled failure and closes
it on the next green run. PR failures do not open issues.

### Strictness, and the exceptions that are deliberate

- **Miri** runs with `-Zmiri-disable-isolation -Zmiri-strict-provenance
  -Zmiri-symbolic-alignment-check` and **no** `-Zmiri-ignore-leaks`; the one
  known leak (the schema registry's `String::leak`) was fixed rather than
  exempted (`Cow<'static, str>`). Default features only: `bft` (blst), the
  WASM provider (wasmtime) and TLS (ring/aws-lc-rs) are FFI and cannot run
  under Miri.
- **snarf** gates in its default contention mode, not `--strict`: the strict
  mode has no ignore mechanism and flags plain-data fields with no false-
  sharing meaning. The one true finding (a test-only `AtomicBool` sharing a
  cache line) was fixed.
- **Sanitizers** run with zero suppressions; `ASAN_OPTIONS=detect_leaks=1`.
  The two leak sites found on the way (`with_schema`'s `Cow` fix, three
  `mem::forget(handle)` in the RPC tests) were fixed, not suppressed.
- **Mutants** always runs with `--all-features` (without it, cfg-gated `bft`
  mutants compile out and report as false misses). The per-mutant cap is
  `--timeout 60` on the command line: `.cargo/mutants.toml` rejects unknown
  fields, so the cap cannot live in the config file. The full run shards
  because `--in-place` and `--jobs` are mutually exclusive, making each shard
  serial internally.
- **Kani** gates the predicate surface: the ISO-8601 structural check, the
  allocation-free proof-of-work prefix predicate, and the expiry-date
  contribution to the trust score. kani-verifier 0.68.0 / CBMC 6.11.0 cannot
  process the rest of `glasschain-core`: the capability SHA-256 path pulls in
  unsupported x86 intrinsics, unbounded symbolic heap aborts CBMC, and
  `validate_asset`'s `format!` messages never finish. The evidence lives in
  `.agents/memories/kani-deferral.md`.

### Coverage engine

The blocking coverage gate runs on `cargo-llvm-cov` with the same nextest
runner and flags as the test matrix. The switch was measured before flipping:
94.18% line coverage (19,523 / 20,729 lines) locally against the pinned 90%
target, so the gate kept its margin.

### Determinism: madsim out, turmoil in, behind a runtime seam

The madsim harness only manipulated time and never intercepted sockets. It is
removed. Network-level partitions now run under **turmoil** with a transport
seam: `glasschain-network/src/net.rs` selects real sockets or simulated
sockets at runtime (`turmoil::in_simulation()`), so the `turmoil-sim` Cargo
feature stays additive and every non-simulation build keeps tokio's sockets.
No compiler-cfg runtime swap.

### Formal verification split

- **Verus is the primary tool** (unbounded proofs over the critical roadmap:
  gas → quorum safety → determinism → chain rules → scoring). The first
  module is gas: `state_cost` and `apply_charge` are proved in production form
  with saturation specs, and the shipped methods delegate to them. Verus is
  what covers the modules Kani cannot process.
- **Kani is the breadth layer** for heap-free predicates, with the scope
  limitation and evidence above.

### Deferred, with triggers

| Tool | Trigger to adopt |
|---|---|
| loom | first-party concurrent data structures land |
| Flux | prebuilt installs appear, or gas/asset invariants become safety-critical |
| cargo-semver-checks | first `v*` tag (`--baseline-rev <tag>`, distinguishing exit 100 from 101) |
| cross-platform release binaries | first external release |
| cargo-dist | upstream keyless Sigstore support |
| Kani expansion | CBMC supports the allocation/intrinsic paths |
| nightly live-mutation triage | the mutants shards' `outcomes.json` show a stable survivor set worth a score gate |

## Consequences

- Contributors run `make ci` before a PR; the raw equivalents stay in
  `AGENTS.md`/`CONTRIBUTING.md`. Deep tooling is opt-in locally and pinned in
  the scheduled workflow.
- The PR gate adds roughly 15 minutes of runner time per code PR (jobs run in
  parallel); the scheduled workflows add nightly and weekly runner time.
- Security controls are unchanged: the analysis stack observes, it does not
  weaken the peer transport, TOFU pinning or certificate validation.
