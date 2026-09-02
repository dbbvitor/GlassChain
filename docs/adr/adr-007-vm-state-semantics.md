# ADR-007 — VM state semantics: explicit persistence and committed write sets

**Status:** Accepted
**Date:** 2026-08-24
**Decision owner:** project owner
**Relates to:** §2.1, §3.1, §3.3, §4.1, §6.1 · [ADR-003](adr-003-privacy-model.md)
(private data) · [ADR-004](adr-004-scale-topology.md) (off-chain commitments) ·
[ADR-006](adr-006-canonical-schema-v1.md) (canonical records) ·
wayfinder [#21](https://github.com/dbbvitor/GlassChain/issues/21)

## Context

`ExecutionProvider::execute_with_state` currently returns `(key, value)` pairs,
and the only production consumer is `ApprovalGate`. That consumer reads
`approve = b"1"` to make an approval decision and discards the other mutations.
The result therefore does not yet distinguish an invocation-local result from a
world-state change.

`StorageProvider` exposes individual block and state operations, but no commit
boundary that couples a block with the state derived from it. The committed
chain is the authoritative input for rebuilding contract and watcher state;
using a storage sidecar as the authority would allow a restart or a peer replay
to disagree with the chain.

ADR-003 and ADR-004 also set two boundaries that this decision must preserve:
commercial payloads and evidence are private/off-chain, and high-frequency
telemetry is represented by off-chain state commitments rather than raw global
state. VM contract state must not become a back door around either boundary.

## Decision

### 1. Execution has explicit ephemeral and persistent results

GlassChain uses a **hybrid** model:

- **Ephemeral execution output** is visible only to the current invocation and
  its caller. The existing `set_state` semantics remain ephemeral. Approval
  gates continue to use this output for `approve` decisions, and an approval
  evaluation does not silently persist arbitrary values.
- **Persistent state writes** require a separate, explicit host operation. A
  contract must opt in to each persistent set or delete; persistence is not the
  default interpretation of a host write.
- The execution environment remains side-effect-free: guest code receives a
  state snapshot and cannot access storage handles, the network, or signing
  material. It returns a result; the node/commit path decides whether that
  result is admissible for a transaction.

The execution result must represent ephemeral output separately from the
persistent write set. The current tuple return type is not permission to treat
every returned pair as a committed world-state mutation; the VM integration
work must introduce the smallest typed representation that preserves this
separation and supports set/delete operations.

### 2. The committed block contains the write set

A persistent write set is part of the committed transaction/execution record
covered by the block hash and consensus certificate. It is not an authoritative
storage sidecar and is not reconstructed by re-running WASM during replay.

The commit path must:

1. read a committed world-state snapshot;
2. execute without external side effects;
3. validate and deterministically canonicalize the returned write set;
4. include the canonical write set in the proposed/validated block; and
5. commit the block and materialize its accepted writes through one atomic
   commit boundary.

A client cannot make an arbitrary world-state change merely by attaching a write
set to a transaction; the normal transaction and contract-validation path must
validate that the set is admissible. A scoped key may have only one canonical
result per execution; ambiguous duplicate operations are rejected rather than
leaving ordering to a provider-specific implementation.

Replay and state rebuild consume the write sets in committed-block order. WASM
re-execution is not the source of truth for rebuilding state. The state database
is a materialized cache of the committed chain.

### 3. Every persistent write has explicit scope and visibility

Each persistent operation carries an explicit:

- channel scope;
- contract scope;
- logical key;
- set or delete operation; and
- visibility: public or a named private data collection.

Public writes include the value in the committed record. A PDC-scoped write
never places its private value in the globally replicated block; the block
contains the collection reference and value/tombstone commitment, while the
private payload follows ADR-003 dissemination and reconciliation rules. An
authorized collection member can materialize the private value and verify its
commitment; non-members can verify the public commitment without reading it.
There is no implicit global or cross-channel keyspace.

This is contract state, not a replacement for `StateCommitment`. High-frequency
telemetry, raw evidence, and other edge data remain off-chain and are anchored
with the canonical commitment records defined by ADR-004 and ADR-006.

### 4. Commit failure is recoverable without changing history

The commit boundary must not acknowledge a block while applying a partial
write set. The storage seam therefore needs one atomic block-plus-state apply
operation (or an equivalent journaled boundary); the commit path must not model
one logical commit as a sequence of independently acknowledged `put_state`
calls.

If a backend failure occurs after the block is durable but before the derived
state cache is complete, the block remains authoritative. The node retries or
rebuilds the materialized state by replaying committed blocks, and never rolls
back or edits the committed block to make the cache fit. A stale-tip race rejects
the whole candidate block and its write set together.

## Consequences

- The VM state-output debt is resolved narrowly: explicit persistent contract
  writes reach the ledger commit path, while existing approval output remains
  invocation-local.
- `ExecutionProvider` and the block/transaction representation need a typed,
  serializable write-set result. `WasmExecutionProvider` needs a separate
  persistence host operation; the exact ABI spelling is implementation detail,
  not a persist-by-default change to `set_state`.
- `StorageProvider` and its backends need an atomic materialization boundary,
  and node replay must apply committed write sets rather than re-execute guest
  code. Existing `watcher:state` and signed-transaction snapshots remain
  derived application state, not an alternative ledger authority.
- Public/private state follows ADR-003: public values can be projected by any
  peer, while PDC values require collection membership and retain only a public
  commitment on the global chain.
- State-based endorsement policies target the fully scoped persistent keys under
  [ADR-008](adr-008-endorsement-policy-model.md), which defines policy language,
  precedence, and signer distinctness.
- The design does not turn VM execution into EVM execution, add a database
  dependency, or put high-frequency telemetry/evidence on the global chain.

## Implementation handoff

The implementation plan should stay behind the existing provider seams and
cover, in order:

1. core types for scoped set/delete operations and the separated execution
   result, including canonical serialization and validation;
2. the WASM host operation and tests proving `set_state` remains ephemeral;
3. transaction/block inclusion and validation of the committed write set;
4. one atomic block-plus-write-set operation in the storage provider/backends;
5. node replay/materialization from committed blocks; and
6. public/PDC visibility tests, stale-tip tests, failed-apply recovery tests, and
   a regression proving approval-gate evaluation does not persist its output.

## Out of scope

- An EVM runtime or Solidity support; ADR-001 remains authoritative.
- A rollup, ZK proof system, execution-level sharding, or an off-chain telemetry
  database; ADR-004 and the scope decision remain authoritative.
- The final spelling and low-level ABI encoding of the persistent host operation,
  provided it is separate from `set_state` and enforces the rules above.
- The implementation of endorsement enforcement, RBAC, and channel wiring;
  the policy model is settled by [ADR-008](adr-008-endorsement-policy-model.md).
