# Spec: Close the verified debt gap — canonical schema v1, VM write sets, endorsement, capabilities, PDCs, BFT, analytics, workflows

**Status:** ready-for-agent
**Source:** Wayfinder map [Close the verified debt gap](https://github.com/dbbvitor/GlassChain/issues/15) (all tickets resolved), ADRs 001–010, and the verified reconciliation in `.agents/memories/debt-list-reconciliation.md`.

## Problem Statement

GlassChain's crates are individually tested, but the architecture the resolved ADRs describe is not implemented end-to-end. A member organization cannot yet do any of the following on a running node:

- submit or commit a canonical record that is strictly validated against a network-wide schema (today only a 6-field asset constant is checked);
- distinguish an ephemeral contract result from a persistent state write, or rebuild contract state from the chain rather than by re-running guest code;
- have a business authorization (endorsement policy) enforced at the commit path — the engine exists but is not wired, and policies cannot target a scoped key;
- rely on rules that cannot change shape retroactively — there is no capability registry or activation mechanism, only a bare wire-version constant;
- keep commercial payloads private while proving they happened — private-data commitment exists only as a crate-local test type, with no dissemination, reconciliation, or transient store;
- enjoy immediate deterministic finality — the only consensus implementation is Proof-of-Work;
- query provenance or live events through the RPC surface — the indexer and flattener are not wired into the server;
- run a purchase-to-settlement process as a stateful multi-party flow — only offer→PO automation exists; recall is a simulation test.

Each of these is a verified, in-scope gap; each has a settled architectural answer in an ADR. The implementation must close the gaps against those settled answers without re-litigating them.

## Solution

Build the v1 consensus-boundary architecture, one work stream per settled decision, all testable through one node-level integration seam:

1. **Canonical schema v1** — an immutable, network-wide schema registry validating 13 canonical record families strictly at admission and commit, with registered extension namespaces, certification/audit records referencing immutable lot commitments, and `StateCommitment` batch anchors (ADR-005, ADR-006).
2. **VM state semantics** — typed execution results separating ephemeral output from explicit persistent set/delete operations with public/PDC visibility; committed write sets carried in blocks; replay from the chain; one atomic block-plus-state apply (ADR-007).
3. **Endorsement enforcement** — a provider seam in `glasschain-core`, a policy-tree evaluation in `glasschain-identity` over verified MSP principals, and enforcement at the network commit path with policy metadata in committed history (ADR-008).
4. **Capability/versioning** — a committed, network-wide capability set with future-height activation, height-based historical validation, wire-version and handshake enforcement, and read-only observers (ADR-010).
5. **Private data collections** — point-to-point dissemination and reconciliation over the libp2p swarm, a transient pre-commit store, per-collection retention/purge, and separation of membership from endorsement (ADR-003).
6. **BFT finality** — a Tendermint/CometBFT-class consensus behind `ConsensusProvider` carrying a quorum certificate on the seam, PoW retained as dev/test consensus, chaos tests rewritten for the no-fork model, and a 200/300-validator compact-workload benchmark gate (ADR-002, ADR-004, ADR-010).
7. **Analytics read path** — the provenance index and analytical flattener wired into the RPC server, bounded event-bus channels with an explicit backpressure policy, and provenance-backed lineage queries (ADR-004 consequences, integration-completion Phase 4).
8. **Workflow engine** — a Corda-style state-machine framework with RFQ→Quote→PO→Acceptance→Shipment→Receipt→Dispute→Settlement, recall/quarantine/dispute, and certification/audit flows; contract and workflow code split into separate deployable modules (requirements-alignment Stage 3).

## User Stories

1. As a member organization, I want to register my legal identity (`PartyIdentity`) as a canonical record, so my custody edges are attributable to a verified organization.
2. As a seller, I want to register products/SKUs with their GTIN/batch/lot identifiers, so buyers and regulators can identify exactly what moved.
3. As a producer, I want to anchor lot/batch records, so downstream custody and certification reference an immutable lot commitment.
4. As a buyer, I want inventory thresholds as canonical policy records, so trigger and threshold operations are governed by committed state.
5. As a buyer, I want purchase orders as canonical records referencing offer, lot, and quantity, so PO commitments are globally ordered and auditable.
6. As a shipper, I want shipment records, so custody edges are public and traversable.
7. As a transporter, I want transit events, so movement is provable without publishing commercial terms.
8. As a receiving organization, I want delivery receipts, so custody handoffs are signed by both sides and public.
9. As an inventory operator, I want inventory transformation records, so stocks reconcile against receipts and shipments.
10. As a manufacturer, I want recall notices as full public records, so recall traversal works for parties who were never counterparties.
11. As a certifier, I want `QualityCertification` records referencing immutable lot commitments, so certificates are signed first-class records and never mutate the lot.
12. As an auditor, I want `AuditAttestation` records with scope, validity, and status, so corrections, renewals, suspensions, and revocations are append-only signed events.
13. As a member organization, I want to submit a `StateCommitment` (Merkle root plus counterparty signatures) for a batch of off-chain events, so high-frequency telemetry is tamper-evident and globally ordered without entering the chain.
14. As a member organization, I want my canonical records strictly validated at admission and commit, so a record missing a required field is rejected rather than silently accepted.
15. As a member organization, I want partner-specific fields carried under a registered namespace, so extensions are validated against an immutable schema descriptor and unknown namespaces are rejected.
16. As a verifying member, I want historical records validated under the schema version effective at their height, so old blocks keep their meaning.
17. As a partner, I want private extension values kept in a PDC with only their commitment public, so extension data need not be globally replicated.
18. As a contract author, I want ephemeral execution output (for example approval decisions) to stay invocation-local, so no execution implicitly mutates committed state.
19. As a contract author, I want an explicit persistent set/delete host operation with channel, contract, key, and public/PDC visibility, so writes persist only when requested and scoped.
20. As a member organization, I want the committed block to carry the canonical write set of accepted persistent writes, so state is rebuildable from the chain.
21. As a node operator, I want replay and state rebuild to consume committed write sets, so a restart or peer replay cannot disagree with the chain and never re-executes guest code.
22. As a member organization, I want PDC-scoped VM writes to expose only their commitment on the global chain, so contract state cannot smuggle private values into global replication.
23. As a node operator, I want one atomic block-plus-state apply, so a crash cannot acknowledge a block with a partial write set.
24. As a member organization, I want a persistent state write authorized only when the applicable endorsement policy (channel default, contract default, collection policy, key policy) is satisfied, so business authorization stays application-level, separate from consensus finality.
25. As a custodian, I want custody handoffs to require sender and receiving custodian signatures (normally 2-of-2), so custody edges are cross-organization by default.
26. As a member organization, I want `NOutOf` policy trees over verified MSP principals with distinct-signer counting, so duplicate or replayed signatures cannot inflate approval.
27. As a member organization, I want a caller-supplied organization label rejected when it conflicts with the verified certificate identity, so policies cannot be spoofed at admission.
28. As a member organization, I want policy changes to be signed transactions satisfying the current effective policy and effective only after their block commits, so policy metadata lives in committed history.
29. As a member organization, I want a block that changes a key's policy and writes the same key in the same block rejected, so within-block ordering never becomes provider-specific.
30. As a member organization, I want endorsement failure to reject the whole transaction with no partial write set materialized.
31. As a governance participant, I want capability activations as signed records with a future activation height finalized by consensus, so rule changes are part of committed history.
32. As a node operator, I want validation to select the capability and schema version effective at each height, so enabling a new capability never reinterprets old blocks.
33. As a peer, I want the handshake to advertise supported capabilities and enforce the wire protocol version, so incompatible peers negotiate admission without activating ledger semantics.
34. As an operator of an outdated node, I want to remain a read-only observer when I lack an active capability, so I can parse validated history without proposing, voting, or relaying writes.
35. As a member organization, I want private payloads (quantities, pricing, terms, evidence) disseminated point-to-point only to collection members, so commercial data never enters global replication.
36. As a non-member, I want to verify that a private-data transaction occurred and its commitment is unaltered, without reading the payload.
37. As a regulator, I want policy-level membership in every collection by default, so per-collection audit grants cannot create recall blind spots.
38. As a member organization, I want a peer that was offline at dissemination time to pull and reconcile private payloads within the collection's retention window, so collection peers can catch up.
39. As a member organization, I want private payloads purged per collection retention with hash commitments persisting forever, so legal record-keeping shelf life is respected.
40. As a member organization, I want a PDC write authorized by collection membership AND the collection's endorsement policy, so membership alone is never an endorsement.
41. As a member organization, I want a block final at commit, so no fork can form and "committed" has one meaning.
42. As a verifying member, I want to validate a block's quorum certificate (the validator signature attestation set), so finality is verifiable without trusting any single operator.
43. As a validator, I want one-organization-one-vote full participation at 200+ MSP-identified validators, so no committee or election exists in v1.
44. As a developer, I want the Proof-of-Work consensus retained as a dev/test implementation behind the same seam, so tests run without a BFT network.
45. As a benchmark operator, I want the compact workload measured at 200 and 300 validators (records, commitments, anchors; block latency/size, vote traffic, pending-pool backpressure, private-data dissemination separately, WAN delay/partition recovery), so capacity claims are evidence-based.
46. As an analytics operator, I want the block stream and committed records projected through the indexer, so dashboards read a queryable projection.
47. As an analyst, I want provenance lineage keyed by GTIN/lot/batch through the RPC surface, so traceability questions are answered without scanning the chain.
48. As a member organization, I want live block and contract events via the existing server streams, so dashboards stay current without new bidirectional streaming.
49. As a node operator, I want the event bus bounded with an explicit backpressure policy, so a slow consumer cannot exhaust memory.
50. As an RFQ issuer, I want a stateful RFQ→Quote→PO→Acceptance→Shipment→Receipt→Dispute→Settlement flow, so a multi-step bilateral transaction is tracked as one process.
51. As a member organization, I want recall, quarantine, and dispute as first-class flows, so recalls are executable processes, not a simulation.
52. As a member organization, I want checkpoint persistence and a triage view for stuck flows, so interrupted multi-party processes can be resumed.
53. As a contract packager, I want contract code (verification-only, deterministic) separated from workflow code (I/O-driven), so the two ship and version independently.
54. As a member organization, I want certification and audit processes executable as flows that emit signed canonical records referencing lot commitments.

## Implementation Decisions

### 1. Canonical schema v1 and registry (`glasschain-core`)

- Replace the compile-time schema constant with an immutable runtime registry of canonical record definitions keyed by `(schema_id, schema_version, schema_hash)`. A record's identity and schema version are part of its signed canonical form; records are append-only once anchored.
- The 13 v1 record families are: `PartyIdentity`, Product/SKU, Lot/batch, Inventory threshold/trigger policy, Purchase order, Shipment, Transit event, Delivery receipt, Inventory transformation, Recall, `QualityCertification`, `AuditAttestation`, `StateCommitment`. `EvidenceManifest` is an embedded manifest structure referenced by certification/audit records, not a standalone entity. `InventoryThreshold` is a policy record, not a custody event.
- Common record envelope: `record_id`, `schema_id`, `schema_version`, `occurred_at`, originating/issuing MSP identity, required signature set, optional channel/PDC reference, registered namespaced extensions, and the canonical record hash where an anchor is required.
- Strict, deterministic validation at canonical ingress and commit: every required field present and valid or the record is rejected; optional fields may be absent; validation never depends on partner extension semantics; the existing metadata trust score remains a quality signal and cannot make an invalid record valid; certification, audit, recall, custody, and state-commitment records fail without their required issuer/scope/reference/signature/validity/commitment fields.
- Registered extension namespaces: each namespace carries an immutable schema descriptor and version; validation uses a JSON-Schema-compatible representation; unknown namespaces are rejected for v1 records; extensions cannot override or shadow core fields; canonical serialized extension values are included in the record commitment; a private extension value may live in a PDC with only its commitment public.
- Certification and audit records follow ADR-005: they reference the immutable lot commitment; the public anchor carries evidence-manifest commitment, issuer identity and signatures, scope, validity interval, and status; status changes are themselves signed, anchored events; nothing edits the source transaction.
- NF-e semantics are reused for shipment, transit-event, delivery-receipt, and recall records; a SEFAZ adapter (Stage 5, out of scope here) translates NF-e events into these canonical records without inventing a second vocabulary.
- Schema activation and deprecation go through the ADR-010 capability mechanism; historical blocks are validated under the version effective at their height; no schema change retroactively changes an existing block's meaning.
- Legacy inputs (`TraceableAsset`-shaped) need an explicit migration or compatibility boundary; they must never be silently accepted as valid v1 records.

### 2. VM state semantics (`glasschain-core`, `glasschain-vm`, `glasschain-storage`, node commit path)

- The execution result becomes a typed value separating ephemeral execution output from the persistent write set. Existing `set_state` semantics stay ephemeral; approval gates continue to consume ephemeral output, and an approval evaluation never silently persists arbitrary values.
- A persistent write is a separate, explicit host operation carrying channel scope, contract scope, logical key, set or delete, and visibility (public or a named PDC). Guest code remains side-effect-free: snapshot state in, no storage handles, no network, no signing material.
- The write set is validated and deterministically canonicalized before inclusion; a scoped key has at most one canonical result per execution; ambiguous duplicate operations are rejected rather than left to provider-specific ordering.
- The committed block contains the canonical write set, covered by the block hash and consensus certificate. The commit path is: read committed snapshot → execute without side effects → validate and canonicalize → include the write set in the proposed/validated block → apply block and accepted writes through one atomic commit boundary.
- Replay and state rebuild consume committed write sets in block order; WASM is never re-executed to rebuild state; the state database is a materialized cache of the chain.
- Failure semantics: if a backend fails after the block is durable but before the cache is complete, the block stays authoritative and the node rebuilds from committed blocks; a stale-tip race rejects the whole candidate block and its write set together; no partial write set is ever acknowledged.
- PDC-scoped writes put the collection reference and value/tombstone commitment in the block; the private payload follows the ADR-003 dissemination/reconciliation path.
- Contract state stays distinct from `StateCommitment`: high-frequency telemetry, raw evidence, and edge data remain off-chain and are anchored with canonical commitment records, never smuggled in as contract state.

### 3. Endorsement (`glasschain-core` seam, `glasschain-identity` engine, `glasschain-network` enforcement)

- Add identity-neutral policy-expression, principal, target, and request/result types behind the `glasschain-core` provider seam (an `EndorsementProvider` trait). `glasschain-core` must not depend on `glasschain-identity`; certificate and MSP verification stay in the identity crate.
- v1 policy expression is a deterministic signature-policy tree: `SignedBy(principal)`, `NOutOf(required, rules)`, and local AND/OR builders that serialize to `NOutOf`. The wire form is data, never executable policy code. Implicit ANY/ALL/MAJORITY languages are not added.
- A v1 principal is a verified MSP organization member derived from the authenticated certificate/credential — never from a caller-supplied organization label. At most one signature counts per distinct principal; duplicate identities, multiple nodes of one organization, and replayed signatures cannot increase the count.
- Policy scope and precedence: channel default → optional stricter contract default → optional PDC collection policy → optional key-level policy. A transaction satisfies every applicable policy; a more specific policy may add constraints but never weaken a base policy; a transaction touching multiple persistent keys satisfies all of their effective policies; a channel without an explicit default is not allow-all — v1 policies name at least one principal and require at least one signature.
- Operation defaults (ADR-008 §3): custody handoffs normally 2-of-2 across organizations; recall/quarantine/dispute transitions use an explicit configured multi-party policy; certification/audit require the issuer signature by default; a PDC write requires collection membership plus the collection's endorsement policy; threshold inventory operations use explicit contract/key policy; settlement finalization and endorsement-policy changes follow the committed-policy and governance rules.
- Policy metadata is committed in-band, versioned, and append-only. A policy update is a signed transaction satisfying the current effective policy and any governance requirement, activates after its containing block commits, and leaves historical blocks governed by the policy version effective at their height. A key-level policy may be cleared only through the same authorization, falling back to the applicable base policy after commit.
- A block that changes a key's policy and writes the same key later in the same block is rejected; the new policy applies deterministically from the next block.
- Enforcement runs at transaction/block admission against the exact transaction and its committed write set, before any materialization; an unsatisfied policy rejects the transaction with no partial state.

### 4. Capability and versioning (`glasschain-core`, `glasschain-network`)

- Capabilities gate every consensus-visible or validation-affecting behavior: private-payload/PDC support, canonical schema v1 and namespace activation/deprecation, `StateCommitment` semantics, endorsement enforcement, and the future BFT activation.
- An activation is a signed, append-only control-plane record naming the capability identifier, an immutable version/hash, and a future activation height; it is finalized by the normal consensus process with the >2/3 threshold and validated under the capability set active before the transition; the new set starts at the declared height and never takes effect midway through its own block.
- Validation selects the capability, schema, and validation-logic versions effective at each block height; versioned validators are preserved, never mutated in place; replay derives the same capability history from committed blocks before rebuilding state.
- The handshake advertises supported capabilities and enforces `PROTOCOL_VERSION` as the wire-encoding gate; connection negotiation is admission-related and cannot activate ledger semantics outside committed history. Incompatible peers are rejected; peers that cannot support an active capability may remain read-only observers (parse and validate history; no propose, vote, relay, or consensus participation); a validator that cannot support the active set must leave the active validator set.
- The wire protocol version is bumped when the private-payload message type lands, and the protocol documentation is rewritten.

### 5. Private data collections (`glasschain-identity`, `glasschain-network`, `glasschain-storage`)

- Collection configuration carries membership separately from an optional collection endorsement policy (ADR-008): membership answers who may read, write, and receive private payloads; endorsement answers whose signatures a PDC write requires. Regulator organizations are members of every collection by default.
- Dissemination is point-to-point over the libp2p swarm (gossipsub topics per collection plus Kademlia discovery), with a pull-based reconciliation path so a peer offline at dissemination time can fetch payloads during the retention window.
- A transient pre-commit store in `glasschain-storage` holds payloads before commit (72-hour default window, configurable); post-commit pulls happen within the collection's retention; retention and purge are configured per collection; hash commitments persist forever.
- The transport-level TOFU gap closes for the payload path: certificate verification is attached to the node so private-payload delivery rests on verified peer identity.
- The consensus boundary is enforced at every boundary (admission, transport, storage, replay): private cleartext payloads, quantities, pricing, counterparties, raw evidence, and unbatched high-frequency telemetry never enter the globally replicated pending pool or blocks; the block carries only the collection reference and commitment plus approved public metadata.

### 6. Consensus (`glasschain-core`, `glasschain-network`, node, RPC, CLI)

- `ConsensusProvider` commit notifications carry a quorum certificate — the set of validator signatures attesting a block — from day one: the BFT implementation supplies a real one, the retained PoW implementation supplies a degenerate one. No consumer of commit notifications may depend on "the leader said so".
- A Tendermint/CometBFT-class BFT implementation lands behind the seam. Malachite is the staged, default-off candidate, gated on a GlassChain testnet, API stability, licensing/stewardship review, and a security audit; `tendermint-rs` supplies neutral types/light-client tooling only. If Malachite fails its gates, the seam must allow a different Tendermint-class engine without touching application logic.
- PoW is retained as a dev/test consensus implementation. The ledger becomes a single canonical chain: tests that assert fork resolution are rewritten to assert liveness and quorum behavior; mining commands and RPCs lose their meaning and are retired, with README and proto updated.
- One-organization-one-vote is the recorded v1 assumption. Validator-set churn mechanics, weighting, and membership-wide governance vote transport after bounding are future decisions, not this spec's work.
- The benchmark gate: an in-process madsim-based compact-workload bench at 200 and 300 validators in `glasschain-network`, measuring block latency/size, vote traffic, pending-pool backpressure, and recovery under WAN delay/partition for the real GlassChain workload (public records, state commitments, certification/audit anchors, NF-e hashes), plus private-data dissemination measured separately. No substitute benchmark qualifies.

### 7. Analytics read path (`glasschain-indexer`, `glasschain-rpc`)

- The provenance index and analytical flattener are wired into the RPC server state (which today holds only the node), backing `QueryAssetHistory` with provenance and adding a lineage query keyed by GTIN/lot/batch.
- Event-bus channels become bounded with an explicit backpressure policy (drop-oldest vs. block) and a test that fills the buffer.
- The existing `IndexerProvider`/`EventBusProvider` seams stand; the relational warehouse writer remains a separate downstream reference adapter (recipe already published), not an in-tree database.

### 8. Workflow engine (`glasschain-contracts`)

- A state-machine framework following the Corda statemachine decomposition: an explicit Action/Event/TransitionResult algebra, one type per transition, checkpoint persistence, and a triage component for stuck flows.
- Flows: RFQ → Quote → PO → Acceptance → Shipment → Receipt → Dispute → Settlement; recall, quarantine, and dispute as first-class flows replacing the recall simulation test; certification and audit as flows that emit signed canonical records referencing immutable lot commitments.
- Commitment semantics per ADR-005/006: flows reference lot commitments and never mutate source records.
- Contract/workflow packaging split: verification-only, deterministic contract code and I/O-driven workflow code ship as separate deployable modules.
- Existing offer→PO automation, inventory triggers, and approval gates are preserved and continue to produce purchase orders through canonical records.

### 9. Cross-cutting surface rules

- `PLUGIN_KIT.md` is updated when the `EndorsementProvider` (and any other new provider trait) lands; `README.md` is updated when the wire protocol, gRPC surface, or CLI changes; proto changes go through the existing build regeneration path.
- No new workspace dependency cycle: `glasschain-core` gains no dependency on identity, network, or rpc.

## Testing Decisions

- **Primary seam: the full-node integration suite** in `glasschain-network`'s tests. A `TestNode` harness builds multi-organization nodes with real components — storage backend, WASM executor, endorsement provider, indexer/event bus, and in-process gRPC server — and every acceptance scenario runs as a node-level scenario: submit → commit → observe, asserting on the committed chain and RPC responses rather than internal call patterns. This is the highest seam that exercises the real commit path and the home of the existing end-to-end tests.
- **What makes a good test:** external behavior only (what the node committed, what the RPC returned), deterministic, named after the observable outcome; no assertions on internal plumbing; each scenario identifies its actor.
- **Per-crate unit seams:** `glasschain-core` registry/capability-height lookups as pure functions; `glasschain-vm` host-op ABI tests proving `set_state` remains ephemeral and the persistence op is scoped/typed; `glasschain-identity` policy evaluation (nested `NOutOf`, distinct signers, forged labels) and channel/collection duties; `glasschain-storage` atomic apply including injected backend failure; `glasschain-rpc` server integration for the public surface (`VerifyEndorsement` returning a real evaluation, provenance-backed queries).
- **Prior art:** the existing node integration, chaos, madsim chaos, server integration, and SNCM compliance suites; the criterion benches in `glasschain-vm` and `glasschain-contracts`; the madsim harness as the substrate for the consensus benchmark.
- **Required regression scenarios** (from the ADR handoffs): approval-gate evaluation does not persist its output; duplicate write ops rejected; stale-tip candidate rejected with its write set; failed-apply recovery rebuilds without changing history; same-block policy-change conflict rejected; forged organization label rejected; distinct-signer counting; multi-key transactions; PDC membership vs. endorsement separation; future-height activation; old blocks unchanged by new capabilities; unsupported peer becomes read-only; unknown namespace rejected; private-cleartext leakage into the block rejected at all boundaries; fork-asserting tests rewritten to quorum/liveness assertions.
- **CI gates unchanged:** workspace check, test, and clippy against the committed baseline; new tests must not raise the warning baseline; integration suite runs in CI.

## Out of Scope

- EVM execution runtime, Solidity, or EVM compatibility inside the core engine (ADR-001); the compatibility seam stays unpopulated.
- SCITT as a transparency log, ZK validiums, data-availability committees, KERI/ACDC or DID/VC identity replacement, IPFS object storage, execution-level sharding, or a separate aBFT settlement core (ADR-002, ADR-004, ADR-010).
- In-tree PostgreSQL/ClickHouse writer; bidirectional gRPC streaming (existing server streams suffice).
- Accounts, balances, settlement economics, and fee sponsorship (deferred until a concrete onboarding-friction case exists).
- Role-specific endorsement principals and full RBAC; organization-member principals are the v1 baseline.
- The complete Brazilian regulator field catalog; regulator-specific fields use registered namespaces.
- SEFAZ/NF-e adapter, REST gateway, WebSocket surface, ERP/WMS/TMS adapters, and verifiable oracle bridge (Stage 5).
- Observability rollout (tracing/metrics), CLI `asset-trace`, and integration-test rehoming (roadmap cross-cutting follow-ups, not debt-gap items).
- Validator-set churn mechanics, validator weighting, and membership-wide governance vote transport after bounding (future decisions, ADR-002 open questions 4–5).
- Federated learning (a SHOULD, explicitly deferred).

## Further Notes

- The spec is the synthesis of the resolved Wayfinder map ([issue #15](https://github.com/dbbvitor/GlassChain/issues/15)) and ADRs 001–010 in `.agents/plans/`; the reconciliation memory (`.agents/memories/debt-list-reconciliation.md`) is authoritative over any external debt-list prose.
- The state-commitment aggregation ratio (ADR-004 open question 1) is unsized; measure it empirically as part of the commitment implementation rather than assuming a ratio.
- The wayfinder map's remaining fog — validator-set churn/weighting and membership-wide governance vote transport — is deliberately outside this spec's tickets.
- Working artifacts and tickets follow the repo conventions in `docs/agents/`: this spec is published to the GitHub tracker with `Status: ready-for-agent`.