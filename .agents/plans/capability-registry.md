# Plan — Capability registry and future-height activation (ticket #36)

**Ticket:** [#36 Capability registry and future-height activation](https://github.com/dbbvitor/GlassChain/issues/36)
**Spec:** ADR-010 (implementation handoff 1–3, 5–6; #7 is the BFT benchmark, later)

## Scope

1. **`glasschain-core/src/capability.rs`** — identity-neutral types:
   - static v1 registry (5 capabilities: `canonical_schema_v1`, `state_commitment`,
     `pdc`, `endorsement`, `bft_consensus`) with immutable version + deterministic
     hash; genesis-active set = the two behaviors the ledger already validates
     (`canonical_schema_v1`, `state_commitment`);
   - `CapabilityActivation` — signed, append-only control-plane record naming
     capability id, version, hash, and a **future** activation height;
   - `CapabilitySet` / `CapabilityHistory` — height-based fold with a
     deterministic `effective_set(height)` lookup; `build_from_blocks` derives
     the same history from committed blocks (replay) and validates every
     canonical record under the set effective at its block height.
2. **One real gate** — `validate_record_under`: `state_commitment` records
   require the `state_commitment` capability in the effective set (genesis-
   active → zero behavior change; makes height selection observable).
3. **Ledger wiring** — admission (`add_transaction`) validates canonical
   records and activations under the set at the next height; commit paths
   (`commit_mined_block`, `validate_chain`, `try_replace_chain`, peer-block
   handler) fold activations per block. Same-block transitions and duplicate
   capability ids are rejected.
4. **Handshake** — `Hello` advertises `capabilities` (serde-default for old
   peers); `PROTOCOL_VERSION` mismatch disconnects; a peer lacking an active
   capability becomes a read-only observer: its transactions/blocks are
   ignored and transaction relays skip it (history sync still flows).
5. **Tests** — core matrix (registry identity, height selection, same-block
   rejection, duplicates, replay derivation, state-commitment gate); ledger
   admission/commit gates; node-level scenarios (future-height activation
   commits and the set flips at the declared height, old blocks unchanged,
   version-mismatch disconnect, unsupported peer read-only, replay via
   two-node sync).

## Out of scope

Endorsement/PDC *integration* gating (tickets #37/#45/#46 wire their own
capability checks), the 200/300-validator benchmark (#48), governance vote
transport after validator bounding (future decision), and the private-payload
PROTOCOL_VERSION bump (#46).
