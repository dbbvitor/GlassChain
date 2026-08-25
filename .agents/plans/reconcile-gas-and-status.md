# Reconcile gas accounting and document transport/security status

## Goal

Make the confirmed execution-layer design real without broad architectural refactoring:

- enforce independent Wasmtime fuel and operation-gas budgets;
- keep the existing mutation result shape;
- retain `GasCounter` and distinguish budget exhaustion;
- defer the unused call-depth guard honestly;
- mark libp2p experimental and TOFU-only trust deliberate;
- leave `Node` lifecycle, constructor, and WASM-gate refactors deferred.

## Code changes

1. Add `ExecutionLimits { fuel_limit, operation_gas_limit }` to the core execution seam.
2. Replace the single execution `gas_limit` parameter at current callers.
3. Add a meter discriminator to `CoreError::GasExhausted`.
4. Reuse `GasCounter` in `WasmExecutionProvider` for base, read, and write charges.
5. Trap host calls when operation gas is exhausted; retain Wasmtime fuel traps.
6. Keep `GasReport` out of the execution result.
7. Add tests for operation-budget success/exhaustion and meter-specific errors.

## Documentation changes

- Correct gas/guard status in `gas.rs`, `PLUGIN_KIT.md`, and `plans/adr-001-execution-layer.md`.
- Mark libp2p experimental and unwired in `libp2p_swarm.rs`, `PLUGIN_KIT.md`, and `README.md`.
- State that TOFU-only peer trust is deliberate in `AGENTS.md` and `README.md`.

## Constraints

- No broad `Node` decomposition or constructor abstraction.
- No recursive-call depth enforcement until recursive contract calls exist.
- No new gas report in the provider result.
- Preserve security checks and deterministic execution.
- Do not commit or create a branch.

## Validation

- Format touched Rust files only.
- Run targeted VM/contracts tests.
- Run `cargo check --workspace --all-targets --all-features --locked`.
- Run `cargo test --workspace --all-targets --all-features --locked`.
- Run clippy as far as the repository baseline permits; do not weaken its gate.
