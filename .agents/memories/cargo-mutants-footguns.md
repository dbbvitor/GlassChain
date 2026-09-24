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

## `wild` needs an `ld.wild` symlink

`taiki-e/install-action` installs the `wild` binary (alias `wild-linker`) but
not the `ld.wild` name gcc's `-fuse-ld=wild` resolves on PATH. The CI jobs
create it (`ln -sf "$CARGO_HOME/bin/wild" "$CARGO_HOME/bin/ld.wild"`) before
setting `RUSTFLAGS=-C link-arg=-fuse-ld=wild`. `-C linker=wild` alone fails
("Couldn't find library `gcc_s`") because rustc then skips the gcc driver.
`wild` changed the cache key: rust-cache does not hash `RUSTFLAGS` by default,
so the jobs suffix their key with `-wild` or the cached artifacts never match
the wild-linked fingerprints.

## the timeout bucket is non-fatal but hides missed

cargo-mutants returns 3 (timeout) in preference to 2 (missed) when a run has
both, so the CI gates check `mutants.out/missed.txt` rather than the exit
code. Known timeout mutants (the suite hangs under them, so they are
detections, not survivors — not skipped):

- `glasschain-core` `Block::calculate_hash` / `crypto::sha256` whole-body
  replacements: `Block::mine`'s nonce loop never terminates.
- `glasschain-core` `Ledger::fold_capability` / `fold_committed_ids`
  `+=`→`*=`: the fold loop never advances.
- `glasschain-cli` `channel_admin::execute` retry-guard → `true` and
  `glasschain-node` `parse_args` first `i += 1`→`-=`: infinite loops.
- `glasschain-network` `PeerWriter::send -> Ok(())` and `>`→`<`: small sends
  never leave, so peers spin to their own deadlines.

Turning any of these into a fast assertion needs a production-side bound or a
test-level timeout around the loop.

## test code is skipped

`visit::attrs_excluded` skips `#[cfg(test)]`, `#[test]`, `#[tokio::test]`, and
`#[mutants::skip]` nodes, so test-only edits produce no mutants and new tests
are not themselves mutated.
