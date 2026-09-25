# ADR-019 — Analysis stack: blocking PR gates, scheduled deep checks, deferred tools

**Status:** Accepted
**Date:** 2026-09-21
**Revised:** 2026-09-24 — Kani and Verus moved to PR/push `ci.yml` jobs; Verus-first zero-trust assignment with the BFT quorum/bitmap, certificate-admission, TOFU pin, trust-score and MSP height-window slices proved; endorsement algebra deferred on both tools with evidence; autoharness deferral
**Revised:** 2026-09-25 — #166 re-evaluation: ASan/LSan promoted to PR/push `ci.yml` (5m22s measured on a GitHub runner); the mutation gates select wild through `scripts/prefer-fast-linker.sh` and fail when a run tests no mutants; the wild→mold linker policy extended to the Linux workspace-build jobs; Verus pin bumped to `0.2026.09.24.b9416fa`; every measured sub-30-minute job now runs on PR/push, and `ci.yml` + `analysis.yml` add a nightly full-workspace sweep
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

Six ubuntu jobs run on every PR that touches code or config, and again
(full-workspace) on pushes to main and the nightly schedule, each with its own
timeout:

| Job | Check |
|---|---|
| Dependency hygiene | `cargo machete --with-metadata crates`, `cargo deny --all-features check` |
| Spelling | `typos` |
| Cache-line layout | `cargo +nightly snarf --format github` (findings to stdout, collision warnings to a file) |
| Feature matrix | `cargo hack check --each-feature --workspace --all-targets --locked` |
| cargo-careful | `cargo +nightly careful nextest run --profile ci --workspace --lib --bins --tests --all-features --locked` |
| Mutation (diff) | `cargo mutants --in-diff pr.diff --baseline=skip --in-place --timeout 240` |

Guardrail: a job whose cold-cache runtime exceeds 20 minutes moves to the
scheduled workflow (this is the `analysis.yml` demotion gate; the owner's
separate 30-minute rule below governs the opposite direction, scheduled jobs
moving onto PR/push). The first measurement (PR #175) kept every job in place;
the slowest was the cache-line layout at ~6.5 minutes. The 2026-09-25 sweep
measured every gate again (0-5 minutes) and added the push trigger plus the
nightly full-workspace sweep; the mutation-diff job stays PR-only because the
full-repo mutation run is the nightly `deep-checks.yml` shards.

### Diff-scoped PR checks

Every PR check runs over the diff where its tool allows, so a leaf-crate PR
does not pay for the whole workspace:

- **File-oriented** gates run on the changed files: `typos` and the mutation
  `--in-diff` job.
- **Package-oriented** gates run on the changed crates plus their
  reverse-dependency closure (`scripts/affected-crates.sh`): clippy, the test
  matrix, cargo-careful, the feature matrix, Miri, Kani, Verus, the ignored
  gates and turmoil.
- **Workspace-level files** (manifests, lockfile, toolchain/lint config,
  `.cargo/`, `.config/`, `.github/`) switch every gate back to the full
  workspace, and pushes to main always run full — main is verified end to end
  after merge. Both `ci.yml` and `analysis.yml` additionally re-run the full
  workspace nightly (`10 3` / `37 3` UTC), so the whole repo is swept even
  without a merge.
- **Whole-workspace by nature**: `cargo fmt` (module-aware and ~13s), the
  coverage job (the Codecov project gate needs the complete report; patch
  coverage is Codecov's own diff status), snarf (whole-program analysis) and
  deny/machete (lockfile-wide).

### Scheduled sweeps — `deep-checks.yml` and the nightly full runs

Nightly:

- `deep-checks.yml`: the full mutants run over all 12 crates in 16 shards —
  the one workload that stays schedule-only: whole-workspace by nature, and
  each shard relinks per mutant (the local baseline measured `glasschain-core`
  alone at 495 mutants / 33 minutes with `--jobs 8`; the next nightly records
  per-shard timings, which will confirm or revisit the verdict).
- `ci.yml` (`10 3` UTC): the full-workspace variants of every diff-scoped PR
  job — clippy, the three-OS test matrix, coverage, Kani, Verus, the
  six-crate Miri matrix, turmoil, ASan/LSan and the capacity gates. All
  measured under 30 minutes (2026-09-25 sweep: Kani 5m, Verus 2m, Miri long
  pole 12m, Test long pole 9m, coverage 4m, gates 6m, clippy 2m).
- `analysis.yml` (`37 3` UTC): the full-workspace analysis gates.

Weekly (Monday) — each also runs on code PRs/pushes per the measured
promotion rule, with the weekly run kept as the whole-repo baseline:

- `fuzz.yml` (`04:00`): 60-second smoke per PR/push, 300-second deep runs
  weekly (~14 minutes).
- `reproducible.yml` (`05:00`): build-twice hash verification, ~10 minutes.
- `coverage-insights.yml` (`05:30`): advisory feature/fuzz coverage views,
  5-11 minutes per job, every upload `joined: false`.
- `bench.yml` (`06:00`): criterion benches, ~7 minutes, never a gate.

The 200- and 300-validator BFT finality gates are manual-only: their
~80k/~180k-socket meshes exceed the runner's 65535-fd hard limit, and the
300-validator mesh does not diffuse on a 4-core runner either (measured >15
minutes).

Failures surface as **one rolling issue per workflow** (`ci-failure` label):
`ci-failure-issues.yml` opens or updates it on a scheduled or dispatched
failure and closes it on the next green run. PR failures do not open issues.

PR results are additionally compiled into **one rolling conversation comment**
per PR (`pr-summary.yml` → `scripts/pr-summary.sh`): every Actions job that
has no native PR reporting, updated in place as each workflow completes.
Native reporters (Codecov statuses, Code Scanning annotations) keep their own
surfaces and are not duplicated. The summary never gates and skips fork PRs,
whose `workflow_run` token cannot comment.

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
  `--timeout` on the command line: `.cargo/mutants.toml` rejects unknown
  fields, so the cap cannot live in the config file. The diff job uses 240
  (the first mutant's test phase compiles the cold test binaries), the nightly
  shards 180, and `make mutants` 60 locally. The full run shards
  because `--in-place` and `--jobs` are mutually exclusive, making each shard
  serial internally.
  Both jobs select the `wild` linker through
  `scripts/prefer-fast-linker.sh` (clang `--ld-path`): gcc only accepts
  `-fuse-ld=wild` from 16.1, and the earlier `ld.wild` symlink approach left
  every mutant unviable while cargo-mutants still exited 0 — the PR diff gate
  was green while testing nothing. Both gates now fail a run with
  `total_mutants > 0` and zero tested mutants.
  **Linker policy: wild first; where a platform has no wild build (aarch64
  Linux has no release artifact), fall back to `mold` — both through clang
  (`--ld-path` / `-fuse-ld=mold`). The selector applies to the mutation jobs
  and the other Linux workspace-build jobs (`test`'s ubuntu leg, `coverage`,
  `cargo-careful`, ASan/LSan); `make mutants` does the same locally. Both
  cover Linux ELF, including mold's aarch64/arm/riscv64 releases; neither
  links Mach-O or PE, so macOS/Windows keep their default linker.**
- **Kani** is the fallback for zero-trust logic Verus cannot express, and the
  breadth layer for heap-free predicates. The curated harnesses run in
  `ci.yml`'s `kani` job on every PR and push — the `glasschain-core`
  predicate surface plus the `glasschain-identity` zero-trust byte surfaces
  (`ocsp::minimal_be` serial comparison, `ocsp::read_tlv` DER framing).
  Measured warm at 2m40s, inside the owner's 30-minute promotion rule, which is why the
  weekly `deep-checks.yml` Kani job was retired. What CBMC still cannot
  reach — the capability SHA-256 path (unsupported x86 intrinsics),
  unbounded symbolic heap, `validate_asset`'s `format!` messages — stays
  test + mutation covered. `cargo kani autoharness -Z autoharness` is **not**
  a gate: kani-verifier 0.68.0 kills `goto-instrument` on this workspace even
  with `--harness-timeout` and `-j 1`, and whole-crate runs hit an
  `intrinsics.rs` ICE on crates whose dependency monomorphizations reach
  `catch_unwind` (wasmtime). It remains a local, advisory tool. The evidence
  lives in `.agents/memories/kani-deferral.md`.

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

### Formal verification split: Verus first on zero-trust, Kani for the rest

Every zero-trust surface is assigned to the strongest tool that can express
it — Verus whenever possible, Kani where Verus has no leverage, tests +
mutation for the crypto primitives underneath:

- **Verus is the primary tool** for zero-trust decision logic that is pure
  Rust: trust-score arithmetic, BFT quorum/bitmap rules, the TOFU pin
  transition, channel membership and the private-payload gate. Verus also
  owns the critical roadmap (gas → quorum safety → determinism → chain rules
  → scoring). Nine slices are already proved in production form: the vm gas
  arithmetic; the BFT quorum/bitmap kernels in `glasschain-core`
  (`quorum_threshold`, `bitmap_len`/`bitmap_byte`/`bitmap_mask`,
  `meets_quorum`, and the `bitmap_contains` bounds lemma) — the certificate
  gate and every signer bitmap access now route through them; the exact
  bitmap expansion (`bft::proof_arith::expand_signers`/`signers_in_range`:
  the set bits, ascending, no phantom index, no dropped signer); the TOFU
  pin decision (`pin::decide`/`spec_decide`, with `poisoned_always_rejects`,
  `known_keeps_the_fingerprint` and `rotation_requires_a_valid_proof`) — the
  network's `PeerRegistry` delegates every Hello's accept/reject/rotate
  decision to it, and the ed25519 check is reduced to `RotationProof` before
  the call; the trust-score arithmetic (`asset::trust_proofs`: the exact
  20/10-point formula, the `<= 100` bound and the ≥ 80 standard gate); the
  certificate-admission gate (`consensus::cert_proofs`: acceptance iff
  the certificate names the block and is degenerate-or-complete — no
  bitmap-only or mislabelled certificate passes); the MSP height-window
  authorization (`identity/msp_policy.rs::authz_proofs`: registered-before-use
  and go-forward revocation, the committed-height rules a replay enforces);
  the consensus-round kernels (`rounds::{proposer_slot, receipt_action,
  should_retain}`: overflow-safe rotation, the equivocation decision table,
  the retirement bound — the network's `VoteReceipts` delegates each); and
  channel membership plus the private-payload gate
  (`identity/channel.rs::channel_proofs`: slice membership on a `Vec` member
  store, and the fail-closed `verifier ∧ verified ∧ member` conjunction the
  node delegates to). Both proof jobs run in `ci.yml` on every PR and push.
- **The endorsement policy algebra is deferred on both tools** (Verus cannot
  pattern-match the serde-derived external enum without an upstream fix;
  CBMC times out on its recursive heap tree) and stays tests + mutation
  until a trigger in the table below. BFT context-message framing is
  Kani/mutation territory by the same panic-on-length-cast reasoning as
  before.
- **Kani is the fallback** wherever Verus cannot model the code but CBMC can:
  parsers, byte-framing, and hash-adjacent structural properties. The
  curated set is a hand-written safety battery over untrusted-input codecs
  (the ISO-8601 check, the PoW prefix, the expiry contribution, the
  `is_hex64` width gate, and the identity `minimal_be`/`read_tlv`/
  `read_generalized` parsers); the per-surface assignment, the
  attempted-but-blocked targets and the toolchain evidence live in
  `.agents/memories/kani-deferral.md`.
- **Neither tool reaches the primitives** (`ed25519-dalek`, BLS12-381,
  `ring`, `webpki`) or the DER/X.509 parsers they sit behind. Those stay
  tested and mutation-covered; a proof may only assume them.

### Deferred, with triggers

| Tool | Trigger to adopt |
|---|---|
| loom | first-party concurrent data structures land |
| Flux | prebuilt installs appear, or gas/asset invariants become safety-critical |
| cargo-semver-checks | first `v*` tag (`--baseline-rev <tag>`, distinguishing exit 100 from 101) |
| cross-platform release binaries | first external release |
| cargo-dist | upstream keyless Sigstore support |
| Kani expansion | CBMC supports the allocation/intrinsic paths |
| Kani autoharness sweep | Kani stops killing `goto-instrument` and stops hitting the `catch_unwind` ICE on whole-crate runs |
| Kani hash-path proofs | a harness needs a hash-adjacent property; `crypto::sha256` is the `#[kani::stub]` seam |
| Endorsement policy algebra proofs | a tool models the serde-derived recursive enum (Verus upstream fix or hand-written serde in a `verus!` type) **or** the tree is flattened into an arena/index encoding CBMC can model |
| Singular (`integer_ring`) | a ring-equality proof appears **and** Singular 4.3.2 is the installed version (4.4.x is incompatible) |
| verusdoc | specs must render in rustdoc; verusdoc currently needs Verus built from source |
| nightly live-mutation triage | the mutants shards' `outcomes.json` show a stable survivor set worth a score gate |

The consensus round loop's pure kernels, the bitmap-expansion quorum
predicate and the HashMap-keyed membership/gating surfaces landed as proved
production kernels in
[#176](https://github.com/dbbvitor/GlassChain/issues/176) (the maps stay
behind the seam; the four CBMC-blocked codec harnesses were re-attempted on
kani-verifier 0.68.0 and are still blocked with dated evidence in
`.agents/memories/kani-deferral.md`). What remains in #176 is the blocked
codec set and the deferred-tool triggers below.

## Consequences

- **Verus-first is deliberate here**: the zero-trust surfaces are stable and
  high-value, so paying the specification cost up front is affordable. For a
  new or churning module, write the cheap Kani harness first — it finds
  panics and boundary violations in minutes — and only then invest in specs.
  Proof code is code: it decays when the implementation moves, so every
  proved module keeps its tests and mutation coverage.
- Every measured sub-30-minute job runs on PR and push: Kani, Verus, Miri,
  turmoil, ASan/LSan and the ignored gates in `ci.yml` (cold: Verus 2m13s,
  Kani 6m33s; Miri's long pole ~19 minutes; turmoil sub-second; ASan/LSan
  5m22s including the `-Zbuild-std` build; gates ~7 minutes test time), the
  analysis gates, and the formerly schedule-only `reproducible.yml` (~10m),
  `coverage-insights.yml` (5-11m/job, still advisory) and `bench.yml` (~7m)
  — the latter three keep their weekly runs as the stable whole-repo
  baseline. If the Kani harness set approaches the cap, Kani moves back to
  `deep-checks.yml` first. The full mutants run stays nightly: 16 serial
  shards cover the whole workspace and cannot be diff-scoped, and the PR's
  `--in-diff` job already covers the diff.
- The diff-scoped jobs' whole-workspace variants run nightly (`ci.yml` and
  `analysis.yml` schedules, plus the `deep-checks.yml` mutants) so the full
  repo is verified even when no merge lands.
- Contributors run `make ci` before a PR; the raw equivalents stay in
  `AGENTS.md`/`CONTRIBUTING.md`. Deep tooling is opt-in locally and pinned in
  the scheduled workflow.
- The PR gate adds roughly 15 minutes of runner time per code PR (jobs run in
  parallel); the scheduled workflows add nightly and weekly runner time.
- Security controls are unchanged: the analysis stack observes, it does not
  weaken the peer transport, TOFU pinning or certificate validation.
