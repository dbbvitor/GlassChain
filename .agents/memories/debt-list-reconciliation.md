# Debt-list reconciliation (external review vs. working tree)

**Learned:** 2026-08-20

## Finding

An externally supplied debt list (7 component rows, 4 resolution phases, repo
pinned at `51bbcca`) re-states the requirements-alignment programme at lower
fidelity. Verified against the working tree at `516f4f9` (main): roughly half
its items are already planned in
[`requirements-alignment.md`](../plans/requirements-alignment.md), two
contradict settled ADRs, two are stale (the capability already exists), and two
are genuine unplanned deltas.

## Evidence

| Debt-list claim | Verified reality at `516f4f9` | Verdict |
|---|---|---|
| core: no strict schema validation | `schema.rs` has `validate_asset` + `SNCM_SCHEMA` (6-field const) for assets only; no runtime registry, no extension namespaces, 8 entity types undefined | Real; already Stage 1 (§4.1–4.3) |
| vm: lacks EVM parity | No EVM runtime is planned; compatibility remains a decoupled optional adapter behind `ExecutionProvider`, never a `core` dependency ([ADR-001](../plans/adr-001-execution-layer.md)) | Runtime out of scope; adapter backlog only |
| vm: state outputs not hooked to world state | `ExecutionProvider::execute_with_state` returns `(key, value)` mutations; only `ApprovalGate` consumes them (reads `approve == b"1"`, discards rest). `StorageProvider::put_state`/`get_state` keyspace carries only `watcher:state` and `signed_tx:*` | Real, **unplanned delta** |
| identity: lacks state-based endorsement | `EndorsementEngine` complete + tested; enforcement stub at `server.rs` ("endorsement engine not yet wired"); `EndorsementProvider` seam already planned (integration plan Phase 2). The *state-based policy model* itself is undesigned | Real, partially planned; policy-model decision open |
| network: incomplete RPC streaming hooks | `StreamBlocks` + `SubscribeToEvents` server-streaming RPCs **implemented** in `server.rs` | Stale claim |
| network: partition handling | App-layer madsim tests exist; real TCP-level fault injection is the known `madsim_chaos.rs` TODO | Real; already Phase 6.1 |
| rpc: stubbed, lacks bidirectional streaming | 12 RPCs defined, 2 streaming implemented, `VerifyEndorsement` exists. No requirement needs bidi streaming | False / YAGNI |
| indexer: skeleton, missing off-chain DB | `flattener.rs` is the 3rd-largest file in the repo; provenance + event bus exist; not wired into RPC (`ServerState` holds only `node`). In-tree PostgreSQL/ClickHouse **rejected** — separate reference-adapter crate is the settled position | Wiring real + planned (Phase 4 / Stage 6); DB part settled out |
| contracts: needs PO→Receipt→Settlement workflow engine | True — ECA watcher + offer→PO engine only, no state machine | Real; already Stage 3 (largest build; after Stage 1 schema) |
| Fee sponsorship in `vm::gas` | No account, balance, or fee type exists anywhere in the workspace | Real but mis-sited — needs an account/balance model first (Stage 4), not a gas-meter feature |
| PDC hashing in `identity::channel` | **Already exists**: `submit_private_data` / `get_private_data` / `committed_hashes`, member-gated, SHA-256 commitments (`channel.rs`) | Already built. Missing parts are dissemination + reconciliation (network), transient store (storage) — blocked on D3 |
| *(omitted by the list)* consensus / deterministic finality | D2 still open (Raft vs BFT on validator-set ownership) | The actual critical path — see [ADR-002](../plans/adr-002-consensus-finality.md) |

## Scope decision (2026-08-20)

The later VM-state decision does not reopen any of these scope boundaries.

The requirement owner confirmed:

- No EVM execution runtime in GlassChain; retain only a decoupled compatibility
  seam behind `ExecutionProvider` for a future, explicitly promoted SHOULD.
- No PostgreSQL/ClickHouse writer in the node; keep a separate reference adapter
  behind `IndexerProvider`.
- Bidirectional gRPC streaming is backlog, not current scope; existing server
  streams are sufficient.
- Account/balance/fee sponsorship work is deferred until a concrete onboarding-
  friction case exists. Do not add a sponsor field or gas-credit abstraction
  before fee semantics exist.

## Implication

Plan against `requirements-alignment.md`, not the debt list. The two deltas the
list added that the programme did not already track were: (1) where VM state
mutations land, and (2) the state-based endorsement policy model. VM state
semantics are now resolved by wayfinder [#21](https://github.com/dbbvitor/GlassChain/issues/21)
and [ADR-007](../plans/adr-007-vm-state-semantics.md): explicit persistent
writes use a committed write set, while approval output remains ephemeral and
public/PDC visibility is explicit. The endorsement policy model is now resolved by wayfinder [#22](https://github.com/dbbvitor/GlassChain/issues/22)
and [ADR-008](../plans/adr-008-endorsement-policy-model.md): deterministic
Fabric-style signature policies over verified MSP principals, scoped defaults and
key constraints, distinct signers, and explicit custody/regulatory protections.
Treat the scope decisions above as boundaries, not missing implementation work.

## Follow-up BFT/scale review (2026-08-25)

The subsequent BFT-at-scale review did not overturn the scope decision. It
confirmed that raw 70M-entity event ingress would couple user volume to the
O(n²) validator vote path, and that private commercial/evidence payloads must not
be globally replicated. The accepted response is the existing one-chain,
off-chain-commitment topology: approved public canonical records and
`StateCommitment` envelopes enter the BFT path; private payloads, raw evidence,
and high-frequency telemetry remain in PDC/off-chain storage.

The review's proposed ZK validium, SCITT, DAC, KERI/DID, IPFS, execution-shard,
and separate aBFT-core alternatives remain outside v1. Malachite stays a
default-off staged candidate behind `ConsensusProvider`, subject to a real
200/300-validator compact-workload benchmark and security/stewardship gates.
Capability/versioning policy is now resolved in
[ADR-010](../plans/adr-010-capability-versioning-policy.md).
