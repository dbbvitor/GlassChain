# Deepen automation seams

**Status:** complete — validated 2026-08-19
**Date:** 2026-08-19

## Goal

Use TDD to consolidate the duplicated WASM approval-gate protocol and improve
locality around automation without changing the public execution result or
reopening deferred ADR decisions.

## Decisions

- Add one private concrete approval-gate module in `glasschain-contracts`.
- It receives an existing `ExecutionProvider` and caller-provided state.
- Active gate failures deny; no configured executor preserves the current
  inactive-gate fast path.
- Preserve exact `approve = b"1"` semantics and per-item isolation.
- Centralize named policies while preserving 50,000/100,000 current limits.
- Keep `Node::set_execution_provider` production code unchanged; add a public
  node-level regression test for both automation paths.
- Keep `ContractEngine::evaluate_supply_offer` public interface unchanged.
  Extract only private matching/planning and emission helpers.
- Preserve transaction ordering, deterministic IDs, quantity caps, status
  transitions, and empty results for non-matches.
- Defer execution usage results and state-aware provider changes.

## TDD slices

1. Red: approval behavior at ContractEngine and WatcherService seams; green with
   the shared gate module.
2. Red: Node registration behavior; green with a node-level integration test.
3. Red: offer-evaluation characterization behavior; green with private helper
   decomposition.
4. Run targeted and workspace validation; inspect the final diff.

## Validation

- Approval-gate contract/watcher tests pass.
- Node integration tests pass, including both automation paths.
- `cargo fmt --all --check` passes.
- `cargo check --workspace --all-targets --all-features --locked` passes.
- `cargo test --workspace --all-targets --all-features --locked` passes.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` passes.
