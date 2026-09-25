# cargo-mutants footguns

Cost real time in wayfinder #165/#175; directly relevant to #166 (deep-checks
shards) and `make mutants`.

## The per-mutant cap is CLI-only

`.cargo/mutants.toml` is deserialized with `deny_unknown_fields`, and `timeout`
is **not** a config key — `--timeout` is CLI-only. The config-file timeout keys
are `minimum_test_timeout` (a floor on the autoset timeout, not a cap) and
`timeout_multiplier` / `build_timeout_multiplier` (relative). A stray
`timeout = 60` in the config makes *every* `cargo mutants` invocation die with
`unknown field 'timeout'` before it does anything; that is how PR #175's
Mutation (diff) job failed 37s in. The cap is passed explicitly by
`make mutants`, `make mutants-diff`, and `analysis.yml`.

## `--in-place` is serial

`--in-place` cannot be combined with `--jobs <N>` (cargo-mutants 27.1.0:
`the argument '--in-place' cannot be used with '--jobs <JOBS>'`). #156's
"16 shards × `--jobs 2`, `--in-place` on ephemeral runners" is therefore not
expressible in one invocation: a shard must either drop `--in-place` (copy the
tree, the default) or run serially. Check this before wiring #166.

## nextest skips doctests

`test_tool = "nextest"` / `--test-tool nextest` means mutations caught only by
doctests report as missed; cargo-mutants does not run doctests separately
(`book/src/nextest.md`). The diff job therefore installs only cargo-mutants and
runs `cargo test`, which does run doctests.

## `--in-diff` expands to whole functions

A changed line selects every mutant in its enclosing function. A comment-only
PR still mutation-tests the functions it comments: PR #175's diff is comments,
a rename, a cfg attribute and a test helper, yet it selected 15 mutants in
`WasmExecutionProvider::build_linker` and `<ShipOrderTransition as
Transition<PurchaseFlowState>>::apply`.

## `wild`: gcc rejects `-fuse-ld=wild` before 16.1, and the failure is silent

`taiki-e/install-action` installs `wild`, and the old jobs created an
`ld.wild` symlink for gcc's `-fuse-ld=wild`. Upstream
(https://github.com/wild-linker/wild) documents that flag for **gcc 16.1+**;
the GitHub runner's gcc rejects it (`cc: error: unrecognized command-line
option '-fuse-ld=wild'; did you mean '-fuse-ld=gold'?`). Worse, cargo-mutants
swallows the build failure: every mutant is marked **unviable**, the run exits
0, and the PR diff gate reported SUCCESS while testing nothing — PR #175's
Mutation (diff) had `total_mutants = 123`, `unviable = 123`,
`caught = missed = timeout = 0`.

Both mutation jobs now select wild through `scripts/prefer-fast-linker.sh`,
which appends `-C linker=clang -C link-arg=--ld-path=<wild>` to `RUSTFLAGS` —
clang's `--ld-path` works on any gcc. Never select it the old way with a bare
`-fuse-ld=wild` under the runner's old gcc. `-C linker=wild` alone fails
("Couldn't find library `gcc_s`") because rustc then skips the gcc driver.

Both gates now read `mutants.out/outcomes.json` and fail when
`total_mutants > 0 && caught + missed + timeout == 0`, so a build-level
failure can never pass as "no mutants". The wild-linked artifacts still need
their own cache key (`-wild` suffix): rust-cache does not hash cargo-config
rustflags, so a shared key would restore non-wild fingerprints.

**Linker preference (project rule, AGENTS.md/ADR-019):** wild first; where
the platform has no wild build (aarch64 Linux has no release artifact), fall
back to `mold` (`clang -fuse-ld=mold`, clang resolves `ld.mold` on `PATH`).
`scripts/prefer-fast-linker.sh` applies it to the mutation jobs and the Linux
workspace-build jobs (`test`'s ubuntu leg, `coverage`, `cargo-careful`,
ASan/LSan); `make mutants` does the same locally. The gain is largest for
repeated relinks — that is the mutation workload — but the policy covers the
Linux build jobs too.

**Where it does not apply:** macOS and Windows (both tools link Linux ELF
targets only — mold's `*-windows.zip` release asset is a Windows *host* build
for linking ELF, and `rui314/setup-mold` is Linux-only; neither has a Mach-O
or PE target), and jobs that never link natively — Miri (interpreter),
Kani/CBMC (GOTO programs), Verus (verification conditions). Linux's default is
already the bundled lld
(`-B<sysroot>/lib/rustlib/<target>/bin/gcc-ld -fuse-ld=lld`), so per-job gains
are small; the selector is cheap and consistent, not a measured necessity.

## timeouts fail too, and are fixed at the source

cargo-mutants returns 3 (timeout) in preference to 2 (missed) when a run has
both, so the CI gates read `mutants.out/missed.txt` as well as the exit code.
Both codes fail. The 15 timeout mutants from the first full run were fixed
rather than skipped:

- `Block::calculate_hash` / `crypto::sha256` whole-body replacements:
  `Block::mine` now `debug_assert`s that the hash is 64 hex chars, so the
  non-terminating PoW loop panics immediately instead of spinning.
- `Ledger::fold_capability` / `fold_committed_ids` `+=`→`*=`: the counters
  use `saturating_add`, which the mutator cannot turn into a stall.
- `glasschain-node` `parse_args`: iterator-based (a flag/value pair per
  iteration), so there is no index `+=` to mis-mutate.
- `glasschain-cli` `channel_admin::execute`: the retry window is now enforced
  by an outer `tokio::time::timeout`, so a broken guard cannot spin.
- `glasschain-network` `PeerWriter::send -> Ok(())` / `>`→`<` and
  `PeerReader::receive` `>`→`==`: the peer tests bound their receives with
  `tokio::time::timeout`, so a writer that never sends fails the test.
- `demo` `build_federation` / `wait_for_ids`: skipped in
  `demo/.cargo/mutants.toml` (the runner internals).

## test code is skipped

`visit::attrs_excluded` skips `#[cfg(test)]`, `#[test]`, `#[tokio::test]`, and
`#[mutants::skip]` nodes, so test-only edits produce no mutants and new tests
are not themselves mutated.
