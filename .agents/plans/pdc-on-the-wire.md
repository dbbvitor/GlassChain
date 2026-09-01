# Plan — Private data collections on the wire (ticket #46)

**Ticket:** [#46 PDCs on the committed path](https://github.com/dbbvitor/GlassChain/issues/46)
**Builds on:** #34 (schema), #36 (capability `pdc` registered, inactive at
genesis), #41 (PDC write-set commitment redaction). Dissemination engine,
retention/purge, reconciliation, and cert-verified delivery are **#47**.

## The boundary model (ADR-003 decision 7)

A private value may exist in exactly four places: inside the writer's
execution, in the point-to-point `Message::PrivatePayload` between members, in
the members' transient store, and nowhere else. Blocks, pending pools, world
state, and events carry only `sha256(value)` commitments + the collection
name. Everything below enforces one direction: cleartext flows member→member;
commitments flow everywhere.

## Changes

### 1. `glasschain-identity/src/channel.rs` — collection config (AC1)
- `ChannelConfig.endorsement_policy: Option<PolicyExpression>` (`#[serde(default)]`)
  — membership (who may read/write/receive) stays separate from the optional
  collection endorsement policy (consumed by the #45 engine, dormant unless
  configured). `Channel::endorsement_policy()` accessor.
- `DEFAULT_REGULATOR_ORGS = ["anvisa", "mapa"]` — `Channel::new` inserts them
  into the member set of every collection (ADR-003 decision 2: policy-level
  members of every collection by default). `member_orgs()` accessor.
- Structural "membership ≠ endorsement": `is_member` never implies policy
  satisfaction — documented + unit test.

### 2. `glasschain-storage/src/transient.rs` — transient pre-commit store
`TransientStore` over `Arc<dyn StorageProvider>`: keys
`transient:<collection>:<commitment>`; `put`/`get`/`delete` (delete is the
#47 purge hook). No retention windows here (#47). The store is dumb storage —
membership gating lives at the node boundary.

### 3. `glasschain-network/src/protocol.rs` — wire (AC2)
- `Message::PrivatePayload { collection, commitment, payload }` — the
  point-to-point message; NEVER broadcast.
- `PROTOCOL_VERSION` → `"glasschain/3"` (private-payload wire change is the
  ADR-010-sanctioned bump reason).

### 4. `glasschain-network/src/node.rs` — the four boundaries
- **Admission:** `NodeState.collections: Vec<Channel>` (`set_collections`).
  `submit_private_payload(collection, payload)`: membership first, then `pdc`
  capability active at the **next height** (where the write lands — ADR-010
  §4) + transient store + point-to-point send to member peers. Non-member
  local org → `Err` (leakage rejected at admission).
- **Transport:** `Hello` gains `org: String` (`#[serde(default)]`), recorded
  in `VerifiedPeer`. `PrivatePayload` is sent only to peers whose org is a
  collection member; on receipt the node requires a completed handshake, then
  verifies `pdc` capability at the next height + local membership + sender
  membership + `commitment == sha256(payload)`, then stores transiently and
  emits `NodeEvent::PrivatePayloadReceived`. A non-member recipient rejects
  (transport leakage rejection); a commitment mismatch rejects. The org is
  self-asserted until #47 attaches certificate verification.
- **Storage/commit:** already redacted by #41 (`block_form`); `mine_async`
  disseminates the RAW Pdc writes from `per_tx_writes` to member peers after
  commit. The world-state cache holds only commitments (mirrors blocks).
- **Replay:** `rebuild_world_state` mirrors committed (redacted) write sets —
  assert cleartext never appears after replay (scenario).

### 5. Scenarios: `crates/glasschain-network/tests/pdc_boundary.rs` (AC5/AC6)
- `pdc` capability activated at a future height on all nodes.
- Three nodes: writer (miner, member), member-peer, non-member — all share the
  collection config; only writer+member-peer are members.
- Member write + commit: writer deploys a `persist_state` PDC-scoped contract
  (WAT from the vm-crate pattern), executes; block carries collection +
  commitment; member-peer's transient store holds the payload; non-member's
  does not.
- Non-member verification: the non-member's chain carries the commitment equal
  to `sha256(payload)`; no payload bytes anywhere in its chain/state.
- Leakage rejection: direct `PrivatePayload` to the non-member is rejected;
  `submit_private_payload` on a non-member errs; commitment mismatch rejects.
- `PROTOCOL_VERSION` enforcement: a `/2` Hello is now rejected by the
  handshake.

### 6. README protocol section rewrite (AC2)
Private-payload message, `/3` bump rationale, collection membership vs
endorsement, regulator default membership, the four-boundary model, and the
#47 remainder (dissemination engine, retention/purge, reconciliation,
cert-verified delivery).

## Out of scope (#47)
gossipsub/Kademlia dissemination engine, pull reconciliation, retention/purge
windows, certificate verification attached to the payload path.
