# ADR-019 — Analysis stack: blocking PR gates, scheduled deep checks, deferred tools

**Status:** Accepted
**Date:** 2026-09-21
**Revised:** 2026-09-24 — Kani and Verus moved to PR/push `ci.yml` jobs; Verus-first zero-trust assignment with the BFT quorum/bitmap, certificate-admission, TOFU pin, trust-score and MSP height-window slices proved; endorsement algebra deferred on both tools with evidence; autoharness deferral
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
| Mutation (diff) | `cargo mutants --in-diff pr.diff --baseline=skip --in-place --timeout 240` |

Guardrail: a job whose cold-cache runtime exceeds 20 minutes moves to the
scheduled workflow (this is the `analysis.yml` demotion gate; the owner's
separate 30-minute rule below governs the opposite direction, scheduled jobs
moving onto PR/push). The first measurement (PR #175) kept every job in place;
the slowest was the cache-line layout at ~6.5 minutes.

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
  after merge.
- **Whole-workspace by nature**: `cargo fmt` (module-aware and ~13s), the
  coverage job (the Codecov project gate needs the complete report; patch
  coverage is Codecov's own diff status), snarf (whole-program analysis) and
  deny/machete (lockfile-wide).

### Scheduled deep checks — `deep-checks.yml`

Nightly: the full mutants run over all 12 crates in 16 shards, and ASan/LSan
over the workspace. Everything whose measured run fits the owner's 30-minute
promotion rule runs in `ci.yml` on every PR and push instead: Kani (6m33s cold),
Verus (2m13s cold), the six-crate Miri matrix (long pole `glasschain-core`
~19 minutes), turmoil (sub-second) and the `#[ignore]`d capacity/measurement
gates (~5 minutes of test time, serial under the runner's 65535-fd hard
limit). The 200- and 300-validator BFT finality gates are manual-only: their
~80k/~180k-socket meshes exceed that limit, and the 300-validator mesh does
not diffuse on a 4-core runner either (measured >15 minutes).

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
  `--timeout` on the command line: `.cargo/mutants.toml` rejects unknown
  fields, so the cap cannot live in the config file. The diff job uses 240
  (the first mutant's test phase compiles the cold test binaries), the nightly
  shards 180, and `make mutants` 60 locally. The full run shards
  because `--in-place` and `--jobs` are mutually exclusive, making each shard
  serial internally.
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
  → scoring). Six modules are already proved in production form: the vm gas
  arithmetic; the BFT quorum/bitmap kernels in `glasschain-core`
  (`quorum_threshold`, `bitmap_len`/`bitmap_byte`/`bitmap_mask`,
  `meets_quorum`, and the `bitmap_contains` bounds lemma) — the certificate
  gate and every signer bitmap access now route through them; the TOFU
  pin decision (`pin::decide`/`spec_decide`, with `poisoned_always_rejects`,
  `known_keeps_the_fingerprint` and `rotation_requires_a_valid_proof`) — the
  network's `PeerRegistry` delegates every Hello's accept/reject/rotate
  decision to it, and the ed25519 check is reduced to `RotationProof` before
  the call; the trust-score arithmetic (`asset::trust_proofs`: the exact
  20/10-point formula, the `<= 100` bound and the ≥ 80 standard gate); and
  the certificate-admission gate (`consensus::cert_proofs`: acceptance iff
  the certificate names the block and is degenerate-or-complete — no
  bitmap-only or mislabelled certificate passes); and the MSP height-window
  authorization (`identity/msp_policy.rs::authz_proofs`: registered-before-use
  and go-forward revocation, the committed-height rules a replay enforces).
  Both proof jobs run in `ci.yml` on every PR and push.
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

The remaining blockers — the consensus round loop, the bitmap-expansion
quorum predicate, the HashMap-keyed membership/gating surfaces and the
CBMC-blocked codec harnesses — are tracked as technical debt in
[#176](https://github.com/dbbvitor/GlassChain/issues/176).

## Consequences

- **Verus-first is deliberate here**: the zero-trust surfaces are stable and
  high-value, so paying the specification cost up front is affordable. For a
  new or churning module, write the cheap Kani harness first — it finds
  panics and boundary violations in minutes — and only then invest in specs.
  Proof code is code: it decays when the implementation moves, so every
  proved module keeps its tests and mutation coverage.
- Kani, Verus, Miri, turmoil and the ignored gates run on PRs because they
  measured inside the owner's 30-minute promotion rule (cold: Verus 2m13s, Kani 6m33s;
  Miri's long pole ~19 minutes; turmoil sub-second; gates ~7 minutes test
  time). If the Kani harness set approaches the cap, Kani moves back to
  `deep-checks.yml` first. The full mutants run stays nightly (16 runners
  would duplicate the diff job on every PR) and ASan/LSan stays nightly (its
  cold `-Zbuild-std` build exceeds 50 minutes).
- Contributors run `make ci` before a PR; the raw equivalents stay in
  `AGENTS.md`/`CONTRIBUTING.md`. Deep tooling is opt-in locally and pinned in
  the scheduled workflow.
- The PR gate adds roughly 15 minutes of runner time per code PR (jobs run in
  parallel); the scheduled workflows add nightly and weekly runner time.
- Security controls are unchanged: the analysis stack observes, it does not
  weaken the peer transport, TOFU pinning or certificate validation.
