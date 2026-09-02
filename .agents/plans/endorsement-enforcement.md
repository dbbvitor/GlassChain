# Plan — Endorsement enforcement at the commit path (ticket #45)

**Ticket:** [#45 Endorsement enforcement at the commit path](https://github.com/dbbvitor/GlassChain/issues/45)
**Spec:** spec decision 3 (Endorsement) · ADR-008 §4 · blockers #36/#41/#37 closed

## Decisions (settled with the owner)

- **Per-transaction carrier:** `Transaction.endorsements: Vec<EndorserIdentity>`
  (`#[serde(default)]`, so existing constructors are untouched). Signers sign
  the canonical transaction bytes; the node builds the `EndorsementRequest` at
  admission from the tx + its **committed write set** and evaluates every
  applicable policy layer.
- **Capability-gated enforcement:** all enforcement activates only when the
  `endorsement` capability is active at the effective height (ADR-010), so
  every existing non-endorsement test path is untouched.

## Scope

1. **`glasschain-core/src/endorsement.rs`**
   - `PolicyUpdate` control-plane record: target scope, new `ScopedPolicies`,
     `signatures: Vec<RecordSignature>`; validated as v1 policy metadata.
   - `PolicyHistory` mirroring `CapabilityHistory`: replay from committed
     blocks (`build_from_blocks`), `validate_block` folds updates in block
     order, `policies_for(channel, contract)` returns the effective
     `ScopedPolicies` (fallback: one-signature channel default), height-
     effective lookup, append-only.
   - **Same-block rule (AC4):** reject a block whose earlier tx updates a key's
     policy and whose later tx (write set) touches that same key.
   - `operation_defaults` (AC2): custody handoff → 2-of-2 (sender + receiving
     custodian); recall/quarantine/dispute → configured multi-party;
     certification/audit → issuer. Evaluated in addition to `applicable()`.
   - `validate_block_endorsements(block, history, provider)`: per tx, build
     `EndorsementRequest { target (tx carrier ∪ committed write set), payload
     = canonical tx bytes, signers = tx.endorsements }`, evaluate all
     applicable layers + operation defaults; any unsatisfied layer rejects the
     whole block — no partial state (AC1).
2. **`glasschain-core/src/transaction.rs`** — `endorsements` carrier on
   `Transaction`; new `TransactionKind::PolicyUpdate(PolicyUpdate)`.
3. **`glasschain-core/src/ledger.rs`** — admission gate for `PolicyUpdate`
   (validate metadata + authorization signatures under the current effective
   policy) and the commit gate via `validate_block_endorsements` in
   `commit_mined_block`; peer-block path calls the same validator from the node.
4. **Exhaustive match sites** — indexer `kind_name`/`event_bus`,
   rpc `build_transaction_protos`, node REPL.
5. **`glasschain-network/src/node.rs`** — `NodeState.endorsement:
   Option<Arc<dyn EndorsementProvider>>` + `set_endorsement_provider`;
   `PolicyHistory` rebuilt from the chain on start/sync (like world_state);
   enforcement in `mine_async`/commit and the peer `Message::Block` path.
6. **`glasschain-rpc/src/server.rs`** — `verify_endorsement` deserializes the
   proposal (`EndorsementRequest` JSON), resolves `ScopedPolicies` from
   `PolicyHistory` at the tip, returns a real evaluation. No proto change.
7. **Tests** — core: PolicyHistory replay/same-block/authorization,
   operation defaults, admission gate, carrier serde back-compat. Node
   integration (`tests/endorsement.rs`, endorsement capability activated):
   failed authorization w/ no partial state, multi-key all-layers,
   distinct-signer counting, PDC membership vs endorsement separation, custody
   2-of-2 default, policy update activates next block, same-block conflict.
8. **Docs** — PLUGIN_KIT.md enforcement wiring; plan file; handoff update.

## Out of scope

- Certificate-bound MSP directory plumbing (msp_policy stays key-directory).
- PDC membership registry (a #46/#47 concern; the separation test uses the
  collection policy layer only).
- BFT (#42), PDCs (#46/#47), RPC surfacing of policy metadata.
