# Plan — VM typed execution result and persistence host op (ticket #35)

**Ticket:** [#35 VM typed execution result and persistence host op](https://github.com/dbbvitor/GlassChain/issues/35)
**Spec:** ADR-007 (implementation handoff steps 1–2) · spec-close-debt-gap.md decision 2

## Scope

The smallest typed change that separates ephemeral from persistent execution
output, plus the WASM ABI that produces it:

1. **`glasschain-core/src/write_set.rs`** — `ExecutionResult` (ephemeral output
   + `Vec<PersistentWrite>`), `PersistentWrite { channel, contract, key, op,
   visibility }`, `WriteOp::{Set,Delete}`, `WriteVisibility::{Public,Pdc}`.
   `ExecutionResult::canonicalize()` validates scope non-emptiness and rejects
   duplicate scoped keys, returning a deterministically sorted copy; serde
   derives everywhere (typed, serializable).
2. **`ExecutionProvider`** — `execute`/`execute_with_state` return
   `ExecutionResult`. `From<Vec<(String,Vec<u8>)>>` keeps legacy providers one
   line. `PLUGIN_KIT.md` updated (trait change).
3. **`glasschain-vm`** — new `env::persist_state` host op carrying channel,
   contract, key, value, set/delete op, and public/PDC visibility; `set_state`
   stays ephemeral. `WasmExecutionProvider` returns the split result.
4. **`glasschain-contracts`** — `ApprovalGate` consumes `result.ephemeral` only
   (behavior unchanged); StubExecutor updated.
5. **Tests** — core canonicalization rules; VM ABI tests (set_state lands in
   ephemeral / persist lands in writes / duplicate scoped key rejected by
   canonicalize / approval evaluation persists nothing); existing suite must
   not regress.

## Out of scope (ticket #41)

Block inclusion/commit of the write set, atomic block-plus-state apply,
replay-from-chain, and PDC materialization.
