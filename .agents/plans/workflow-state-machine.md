# Plan — Workflow state-machine framework (ticket #40)

**Ticket:** [#40 Workflow state-machine framework](https://github.com/dbbvitor/GlassChain/issues/40)
**Spec:** spec decision 8 · reference-architectures §4 (Corda statemachine decomposition)

## Scope

1. **New crate `crates/glasschain-workflows`** — the Corda-shaped foundation:
   `Action` / `Event` / `TransitionResult` algebra, one type per transition,
   deterministic `apply` functions that advance a flow state and emit actions
   without performing I/O (the runner performs the I/O). Depends on
   `glasschain-core` (records, `StorageProvider` seam) and
   `glasschain-contracts` (automation exercised by tests). No workspace
   dependency points at it yet (#43/#44 will consume it; #49 splits packaging).
2. **Deterministic transitions** — a transition is a pure function
   `(FlowState, Event) -> TransitionResult { new_state, actions }`; replaying
   the same event sequence from the same state yields the same state and the
   same actions, so a checkpoint re-run cannot diverge.
3. **Checkpoint persistence** — `CheckpointStore` over the existing
   `StorageProvider` `put_state`/`get_state`/`delete_state` seam (key
   `workflow:checkpoint:<flow_id>`), no new storage crate. Delivery/
   acknowledgement split: `handle` returns the transition's actions without
   executing them; the caller executes each durably and then calls `ack` — the
   only place the checkpoint advances. A crash before submission re-delivers
   the action (no loss); a crash after submission but before `ack` re-executes
   it (at-least-once), and deterministic emission ids make the ledger effect
   exactly-once.
4. **Triage view** — `FlowTriage::stuck_flows()` lists flows whose checkpoint
   is older than a configurable staleness threshold, with their last state;
   flows re-surface with their stored timestamp when driven after a triage
   restart (full cross-restart enumeration needs a storage `list` capability —
   deferred to #43/#44, which need it first).
5. **Canonical-record flows** — the framework carries canonical records as
   events/actions: flows *consume* committed `CanonicalRecord`s as inputs and
   *emit* new `CanonicalRecord`s as outputs; a `lot_commitment` reference field
   on flow state links emitted records to immutable lot commitments without
   ever mutating the source record (emit-only, append-only).
6. **Automation exercise (AC4)** — workflow tests drive the existing seams
   unchanged: `ContractEngine` offer→PO, `WatcherService` inventory triggers
   (with the WASM approval gate through `ApprovalGate` + `WasmExecutionProvider`),
   asserting the engine/watcher/approval behaviour is identical when steered by
   a flow. No changes to `glasschain-contracts` source.

## Out of scope

- RFQ→Quote→PO→Acceptance→Shipment→Receipt→Dispute→Settlement flows (#43),
  recall/quarantine/dispute flows (#44) — they build on this framework.
- Packaging split into deployable contract/workflow units (#49).
- Node wiring: the node does not host flows yet; flows run over the seams
  (storage, ledger events) and are exercised in-crate.
