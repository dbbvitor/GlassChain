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

## test code is skipped

`visit::attrs_excluded` skips `#[cfg(test)]`, `#[test]`, `#[tokio::test]`, and
`#[mutants::skip]` nodes, so test-only edits produce no mutants and new tests
are not themselves mutated.
