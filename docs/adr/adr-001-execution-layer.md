# ADR-001 — Execution layer: WASM stands, EVM is optional

**Status:** Accepted
**Date:** 2026-08-18
**Decision owner:** project owner
**Relates to:** §2.1, §3.4 · [`requirements-alignment.md`](../../.agents/plans/requirements-alignment.md) D1

## Context

The requirements list stated "the core execution layer MUST support EVM-compatible
smart contracts (authored in Solidity/standard tooling)". Taken literally this
conflicted with the entire `glasschain-vm` crate (Wasmtime, fuel-based gas metering,
~1,130 lines) and with `glasschain-contracts`, including the autonomous
replenishment path (§5.1) — the one requirement that is fully delivered and
stress-tested today.

The requirement was clarified as three separate items of differing strength:

| Item | Strength |
|---|---|
| Smart contract support | **MUST** |
| EVM compatibility | **SHOULD** |
| Solidity | **not a requirement** |

## Decision

1. **WASM/Wasmtime remains the execution layer.** The MUST — smart contract
   support — is satisfied by `WasmExecutionProvider` with independent instruction
   fuel and host-operation gas budgets, and a documented contract module
   interface. `GasCounter` charges the base invocation and state operations.
   The call-depth guard remains deferred until recursive contract calls exist.
2. **No EVM runtime is planned in GlassChain.** EVM compatibility remains a
   decoupled, optional adapter seam behind the existing `ExecutionProvider` trait
   if a concrete requirement later justifies it. An adapter must not become a
   dependency of `glasschain-core` or replace the WASM engine.
3. **Solidity is out of scope.** No tooling, ABI, or compiler work is planned.

## Consequences

- §2.1 moves from **CONFLICT** to **met** for the MUST portion. The largest single
  cost item in the programme is removed.
- **D3 (privacy model) is unblocked.** EVM assumes a globally consistent state trie,
  which is what made "EVM-compatible" and "no global ledger replication" (§3.2)
  mutually exclusive. With EVM optional, both Fabric-style and Corda-style privacy
  models are now open — see ADR-003.
- Stage 4 of the roadmap shrinks to the account/balance and fee-delegation work
  needed for §2.4, which has no foundation in the codebase today. Fee sponsorship
  is deferred until a concrete onboarding-friction case exists.
- EVM compatibility, if ever promoted from SHOULD, is a separate adapter project
  behind `ExecutionProvider`; it is not an execution feature of the core engine.
- `ExecutionProvider` keeps returning state mutations; `GasReport` remains a
  standalone accounting type rather than expanding that result interface.
- The call-depth guard is intentionally deferred and must not be described as an
  active runtime protection until recursive contract calls exist.
- The developer-ecosystem rationale behind the original requirement is served by
  WASM, which accepts Rust, C/C++, Go, AssemblyScript, and others.

## Alternatives rejected

- **Replace Wasmtime with an EVM (`revm`).** Discards tested work to satisfy a
  SHOULD, and re-introduces the §3.2 conflict.
- **Run both VMs now.** Doubles the execution surface, the gas model, and the
  contract-packaging story for no current requirement.
