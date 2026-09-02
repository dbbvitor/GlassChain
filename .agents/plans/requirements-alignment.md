# Requirements Alignment — Hybrid Distributed Inventory System

**Status:** active — D1–D6 resolved; Stage 0 complete; the debt-gap programme
(#34–#49) shipped Stages 1–3 and the staged BFT/PDC/analytics work
**Date:** 2026-08-24 · **Reviewed:** 2026-09-02
**Baseline:** `fed76c7` on `main`, working tree clean. What actually shipped
per ticket is recorded in
[`../memories/debt-gap-handoff.md`](../memories/debt-gap-handoff.md); read that
before re-planning anything below as if it were still open.
**Note:** `integration-completion.md` is deleted — it was superseded by this
plan and its state claims all shipped. Its one reversed recommendation (libp2p)
is preserved below. VM state semantics and endorsement policy are settled in
[ADR-007](../../docs/adr/adr-007-vm-state-semantics.md) and
[ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md).

---

## Headline

The 26 requirements describe a system roughly **three times the scope of what exists**.
GlassChain today is a solid supply-chain ledger with PoW + WASM. The requirements
describe Fabric's governance *plus* Corda's workflow/privacy model *plus* VeChain's
event/subscription surface and fee-delegation economics.

Counted against verified code: **3 met, 11 partial, 11 absent, 1 architectural
conflict.**

The conflict matters more than the absences. It cannot be satisfied by adding
code — it invalidates code that already exists and is tested.

---

## The decisions that gate everything

### D1. Execution layer — ✅ **RESOLVED** → [ADR-001](../../docs/adr/adr-001-execution-layer.md)

The requirement was clarified into three items of differing strength: smart
contract support is a **MUST** (already met by `WasmExecutionProvider`), EVM
compatibility is a **SHOULD** (deferred behind `ExecutionProvider`), and Solidity
is **not a requirement**.

**WASM/Wasmtime stands.** §2.1 moves from CONFLICT to met, the largest single cost
item in the programme is removed, and — because EVM's global state trie was the
reason "EVM-compatible" and "no global ledger replication" were mutually
exclusive — **D3 is unblocked**.

### D2. Deterministic finality (§8.2) vs. Proof-of-Work — ✅ **RESOLVED** → [ADR-002](../../docs/adr/adr-002-consensus-finality.md)

§8.2 requires "immediate, deterministic transaction finality (preventing chain
forks)". The ledger uses PoW with longest-chain resolution — which is probabilistic
and forks *by design*. There is a passing test called
`test_concurrent_mining_longest_chain_wins` that asserts fork resolution works.

PoW cannot be tuned into deterministic finality. This requires a BFT/CFT consensus
(Raft for crash-fault, IBFT/PBFT for byzantine).

**Resolved 2026-08-18: "immediate" is literal.** Lagged finality is not acceptable,
which rejects the Thor-style finality-gadget option and rules out retaining any
part of the mining path in production. A fork must be unable to form, not merely
be resolved afterwards.

**Resolved 2026-08-20:** the validator set is zero-trust and includes every
participating organization in v1, including commercial rivals. Use
Tendermint/CometBFT-class BFT, with full participation through the practical
validator ceiling and an authenticated light-client ladder beyond it. See
[ADR-002](../../docs/adr/adr-002-consensus-finality.md) and [ADR-004](../../docs/adr/adr-004-scale-topology.md).

**Implementation consequence:** `ConsensusProvider` in `glasschain-core/src/providers.rs`
is the right seam, but the production implementation must replace PoW with the
selected BFT provider. `PowConsensusProvider` may remain as a second implementation
for compatibility/testing; the swap changes what "committed" means for every other
subsystem.

With the consensus family settled, endorsement, channels, and the workflow engine
can be implemented against the committed-block semantics.

### D3. Global state (§6.1) vs. selective disclosure (§1.4, §3.2) — ✅ **RESOLVED** → [ADR-003](../../docs/adr/adr-003-privacy-model.md)

§3.2 requires sensitive terms be shared "strictly on a need-to-know basis…avoiding
global ledger replication". This *used to* conflict with §2.1: EVM contracts assume a
globally consistent state trie, whereas Corda deliberately has no global state, which
is *why* it achieves selective disclosure. **ADR-001 dissolved that conflict** by
demoting EVM compatibility to a SHOULD.

Today every peer receives every transaction as JSON over a broadcast TCP mesh.
Selective disclosure is not a feature to add on top — it changes what the transport
does.

**Resolved 2026-08-20:** choose Fabric-style private data collections. One global
ordered chain carries public custody commitments and hashes; private commercial
payloads are disseminated point-to-point to authorized collection members. Regulator
visibility, purge, and the default 72-hour transient reconciliation window are
specified in [ADR-003](../../docs/adr/adr-003-privacy-model.md).

---

### D4. VM state mutation semantics — ✅ **RESOLVED** → [ADR-007](../../docs/adr/adr-007-vm-state-semantics.md)

The VM uses a hybrid state model. Existing invocation-local output remains
ephemeral and continues to drive approval decisions; a separate explicit host
operation is required for persistent contract-state writes. Accepted writes are
represented in the committed block, replayed from the chain, and materialized
atomically by storage. Every persistent write has explicit channel, contract,
key, and public/PDC visibility scope. High-frequency telemetry and evidence
remain off-chain `StateCommitment`/PDC data under ADR-003 and ADR-004.

This closes the debt-list gap without making `set_state` persist by default or
putting an EVM runtime in core. The implementation work is now a roadmap item,
not an open architectural decision.

---

### D5. State-based endorsement policy — ✅ **RESOLVED** → [ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md)

Endorsement is application authorization, separate from Tendermint-class BFT
finality and PDC membership. v1 uses a deterministic Fabric-style signature
policy tree (`SignedBy`/`NOutOf`) over verified MSP organization members, with
channel and contract defaults, optional collection endorsement, and fully scoped
key-level constraints. More-specific policies may tighten but not weaken their
applicable base policy; distinct principals are counted once.

Default stronger protections cover cross-organization custody handoffs and
configured recall, quarantine, and dispute transitions. Certification and audit
records remain issuer-signed append-only processes; PDC writes do not receive an
automatic multi-party requirement. Policy changes are committed and versioned,
and a same-block policy update plus write to that key is rejected in v1.

This settles the model; Stage 2 still has to wire certificate-backed evaluation
through `EndorsementProvider`, channel/PDC policy storage, and the commit path.

### D6. Capability/versioning and the consensus boundary — ✅ **RESOLVED** → [ADR-010](../../docs/adr/adr-010-capability-versioning-policy.md)

The BFT scaling review is accepted as a constraint on the chosen architecture,
not as a reason to replace it with ZK validiums, SCITT, KERI, execution shards,
or a second settlement core. The 70M-member horizon is handled by bounded BFT
validators, authenticated light clients, and off-chain PDC/event aggregation.

The consensus-facing pending pool admits approved public canonical records and
commitment envelopes. It rejects private quantities, pricing, counterparties, raw
evidence, high-frequency telemetry, and other unapproved cleartext private data.
Public custody edges, permitted legal identity fields, lot/batch identifiers,
timestamps, recalls, NF-e hashes, certification/audit anchors, public write sets,
and state commitments remain first-class global records.

Every consensus-visible or validation-affecting behavior is capability-gated:
PDC protocol support, canonical schema and registered namespaces, state
commitments, endorsement enforcement, and future BFT activation. The active set
is network-wide and committed in history. Signed activations name immutable
versions/hashes and future heights; historical validation remains height-based,
and `PROTOCOL_VERSION` remains a separate wire-compatibility gate.

Unsupported peers may observe only when they can parse and validate compatible
history; they cannot propose, vote, relay active writes, or participate in
consensus. Governance approval remains a membership right. In v1 the governance
and validator sets coincide for the `>2/3` threshold; future validator bounding
must preserve light-client governance standing.

The BFT capacity claim remains unquantified until a GlassChain testnet measures
the compact public-record/commitment workload at 200 and 300 validators. Malachite
is a staged, default-off `ConsensusProvider` candidate, not an unconditional
production dependency.

---

## Traceability matrix

Verified against the working tree.

| Req | Requirement | Status | Evidence |
|---|---|---|---|
| 1.1 | MSP / X.509 identity | **Partial** | `glasschain-identity` exists; certificate chain verification **implemented** (`cert_verifier.rs`, webpki, `Full` by default). Remaining: verifier is `None` in all four `Node` constructors; no auditor/logistics roles |
| 1.2 | RBAC | **Absent** | `MspAuthInterceptor` is authentication only — no roles, no policy evaluation |
| 1.3 | Multi-party endorsement | **Partial; model specified** | `EndorsementEngine` complete + tested; state/key policy model is defined by [ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md), but certificate-backed enforcement remains unwired (`server.rs:553` stub) |
| 1.4 | Channels / private partitions | **Partial** | `Channel` type exists, unwired; transport broadcasts globally; collection membership and endorsement are separate per [ADR-003](../../docs/adr/adr-003-privacy-model.md) and [ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md) |
| 2.1 | Smart contracts (MUST) | **MET** | `WasmExecutionProvider` + gas metering + reentrancy guard; state persistence semantics are defined by [ADR-007](../../docs/adr/adr-007-vm-state-semantics.md). EVM compat is a deferred SHOULD; Solidity out of scope — [ADR-001](../../docs/adr/adr-001-execution-layer.md) |
| 2.2 | Canonical business events | **Partial** | `NodeEvent` + `event_bus` exist; not a canonical schema over all business actions. Signed certification/audit anchors are now required first-class events — [ADR-005](../../docs/adr/adr-005-certification-and-audit.md) |
| 2.3 | WebSocket / log subscriptions | **Partial** | gRPC `SubscribeToEvents` server stream only; no WebSocket, no REST |
| 2.4 | Sponsored transactions / fee delegation | **Absent** | **No account, balance, or fee model exists at all** |
| 3.1 | Workflow-first modeling | **Absent** | 6 transaction kinds, no state machine. RFQ/Quote/Acceptance/Shipment/Receipt/Dispute/Settlement do not exist; the VM contract-state boundary is settled by [ADR-007](../../docs/adr/adr-007-vm-state-semantics.md) for the future workflow engine |
| 3.2 | Selective privacy | **Absent** | Global JSON broadcast to all peers |
| 3.3 | Legal commitment semantics | **Partial** | Certification and audit are now defined as signed, append-only processes referencing immutable lot commitments; full workflow semantics remain Stage 3 — [ADR-005](../../docs/adr/adr-005-certification-and-audit.md) |
| 3.4 | Domain app packaging | **Partial** | WASM contracts + `PLUGIN_KIT` provider traits |
| 4.1 | Canonical minimum schema (9 entity types) | **Specified; implementation pending** | Canonical v1 defines 13 record families, including state commitments, `QualityCertification`, and `AuditAttestation`; strict field catalog remains to be implemented — [ADR-006](../../docs/adr/adr-006-canonical-schema-v1.md) |
| 4.2 | Extension namespaces | **Specified; implementation pending** | Registered, versioned namespace schemas; unknown namespaces rejected and core fields cannot be overridden — [ADR-006](../../docs/adr/adr-006-canonical-schema-v1.md) |
| 4.3 | Schema + policy registry | **Specified; implementation pending** | Immutable network-wide `(schema_id, version, hash)` registry; capability activation controls new-block use and historical versions remain valid — [ADR-006](../../docs/adr/adr-006-canonical-schema-v1.md), [ADR-010](../../docs/adr/adr-010-capability-versioning-policy.md) |
| 5.1 | Autonomous replenishment | **MET** | Watcher → WASM → signed PO, with stress tests. Needs the Stage 2 endorsement enforcement from [ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md) for "subject to approval policies" |
| 5.2 | Recall / quarantine / dispute flows | **Partial** | A recall *test* exists; no flow engine, quarantine, or dispute; future flows consume immutable lot commitments and certification/audit status without mutating source transactions |
| 6.1 | Off-chain analytics | **MET** (by design) | Ledger is source of truth; analytics live outside |
| 6.2 | Indexer service | **Partial** | In-memory indexer built; not wired into RPC |
| 6.3 | Relational / analytics warehouse | **Absent** | See revised position below |
| 6.4 | AI-ready semantic feeds | **Partial** | `AnalyticalFlattener` + CSV exist, unwired; future projections must join immutable lot commitments to public certification/audit anchors without requiring private evidence |
| 6.5 | Federated learning (SHOULD) | **Absent** | Correctly deferred |
| 7.1 | ERP/WMS/TMS, REST/gRPC/CSV | **Partial** | gRPC + CSV; **no REST**, no adapters |
| 7.2 | Oracle connectivity | **Absent** | — |
| 8.1 | Observability (metrics, tracing, admin API) | **Absent** | `log` crate only — no metrics, no tracing, no OTel |
| 8.2 | Deterministic finality | **CONFLICT** | PoW longest-chain forks by design |

---

## Revised position on two earlier recommendations

**1. libp2p: reversed.** In the (since-deleted) `integration-completion.md` I recommended feature-gating
or removing `LibP2pNode` because no binary can reach it. **§1.4 and §3.2 reverse
that.** Selective disclosure needs addressed point-to-point messaging and peer
discovery — precisely what the unused gossipsub + Kademlia swarm provides. The
broadcast TCP mesh cannot deliver §3.2. `LibP2pNode` moves from dead code to
**the intended substrate**, and Stage 2 should wire it rather than delete it.

**2. Analytics warehouse: softened.** I previously rejected an in-tree PostgreSQL
writer. §6.3 makes a warehouse a stated MUST, so the reconciliation is: ship a
**separate reference adapter crate** implementing `IndexerProvider`, optional and
feature-gated, rather than coupling the node to a database. That satisfies §6.3
without putting a service dependency and container fixtures in the critical path of
the node itself.

---

## Revised roadmap

### Stage 0 — Decisions and safety net *(blocking, small)*

- [x] **D1** (execution layer) — resolved: [ADR-001](../../docs/adr/adr-001-execution-layer.md). WASM stands.
- [x] **D2** (consensus / finality) — resolved: Tendermint/CometBFT-class BFT,
      with full participation through the practical validator ceiling and a
      light-client ladder beyond it. [ADR-002](../../docs/adr/adr-002-consensus-finality.md).
- [x] **D3** (privacy model) — resolved: Fabric-style private data collections,
      public custody commitments, private commercial payloads, default regulator
      visibility, and configurable purge/reconciliation. [ADR-003](../../docs/adr/adr-003-privacy-model.md).
- [x] **D4** (VM state semantics) — resolved: explicit persistent writes, committed
      write sets, and explicit public/PDC scope. [ADR-007](../../docs/adr/adr-007-vm-state-semantics.md).
- [x] **D5** (endorsement policy) — resolved: deterministic signature policies over
      verified MSP principals, scoped defaults/overrides, distinct signers, and
      explicit custody/regulatory protections. [ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md).
- [x] **CI** — `.github/workflows/ci.yml`: fmt, check, test, clippy, coverage, and a
      RustSec audit. The clippy no-new-warnings baseline gate has been **retired**:
      clippy is clean at `-D warnings`, so `.github/clippy-baseline.txt` was deleted
      2026-09-02.

Stages 2–4 are unblocked now that D2 and D3 are resolved. Stage 1 remains the
schema prerequisite for the workflow engine.

### Stage 1 — Foundations after the resolved architecture decisions *(can start immediately)*

These can proceed independently of the consensus implementation and use the
resolved WASM, BFT, privacy, and VM-state boundaries:

- [x] **Certificate verification** — **done.** `CertChainVerifier` now performs a
      real chain check via `rustls-webpki` (ring), and defaults to
      `VerificationLevel::Full`. Regression tests cover a bit-flipped signature and
      a forged CA that reuses the victim's Distinguished Name — the attack the old
      structural-only check accepted. Remaining §1.1 gaps: `Node.cert_verifier` is
      still `None` in all four constructors, and there are no auditor/logistics roles.
- [ ] **Observability (§8.1)** — migrate `log` → `tracing`, add Prometheus metrics
      and an admin endpoint. Note this changes the logging convention in `AGENTS.md`.
      **Shape:** one thin registry crate plus a `metrics.rs` *inside* each crate that
      owns instruments. Do not build a central metrics module every crate depends on —
      Thor and Fabric both colocate ([study §6](../memories/reference-architectures.md)).
- [ ] **Schema registry + extension namespaces (§4.2, §4.3)** — implement the
      immutable network-wide registry and capability-controlled activation defined
      in [ADR-006](../../docs/adr/adr-006-canonical-schema-v1.md); validate registered namespace
      schemas and reject unknown namespaces.
- [ ] **Canonical schema expansion (§4.1)** — implement the 13 specified record
      families with strict v1 validation, including `QualityCertification`,
      `AuditAttestation`, `StateCommitment`, and the embedded evidence-manifest
      reference. Certification/audit records reference immutable lot commitments
      and never mutate source transactions ([ADR-005](../../docs/adr/adr-005-certification-and-audit.md)).
      This remains the data-model prerequisite for the Stage 3 workflow engine.
- [ ] **VM state write-set plumbing (§2.1)** — implement [ADR-007](../../docs/adr/adr-007-vm-state-semantics.md):
      separate ephemeral output from explicit persistent set/delete operations,
      carry canonical scoped write sets in committed records, apply public/PDC
      visibility correctly, materialize atomically, and rebuild from blocks
      without re-executing WASM. Keep high-frequency state commitments off-chain.

### Stage 2 — Governance and privacy *(unblocked)*

- [ ] Consensus replacement per D2, behind `ConsensusProvider`. Expose a **quorum
      certificate** on the seam from the start so a later Raft→BFT swap is additive
      rather than a rewrite of every commit consumer (ADR-002 consequences).
- [ ] Endorsement enforcement per [ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md),
      via a new `EndorsementProvider` trait in `glasschain-core`, implemented in
      `glasschain-identity`, and invoked from `glasschain-network`. **Core must not
      depend on identity** — verify with `cargo tree -p glasschain-core -i glasschain-identity`.
- [ ] RBAC (§1.2) layered on the interceptor: roles after certificate-backed
      identity, then policy evaluation. Organization-member principals are the v1
      endorsement baseline; role-specific principals remain this implementation's
      follow-up.
- [ ] Wire `LibP2pNode`; implement channels (§1.4) and private data collections (§3.2).
      Split the work along Fabric's three concerns — policy (`identity`),
      dissemination *and reconciliation* (`network`/libp2p), transient pre-commit
      store (`storage`) — per [ADR-003](../../docs/adr/adr-003-privacy-model.md).
- [ ] Capability/versioning mechanism, landed with the wire-protocol change rather
      than after it: network-wide committed capabilities, future-height activation,
      historical validation, read-only downgrade for unsupported peers, and a strict
      public/private consensus boundary ([ADR-010](../../docs/adr/adr-010-capability-versioning-policy.md)).

> If D3 slips, mark `LibP2pNode` **experimental** rather than deleting or
> feature-gating it. Corda carries a top-level `experimental/` for exactly this
> status ([study §9](../memories/reference-architectures.md)).

### Stage 3 — Workflow engine *(largest single build; after Stage 1 schema work)*

**Blueprint: Corda's `node/services/statemachine/`** — copy the decomposition
rather than inventing one ([study §4](../memories/reference-architectures.md)):
one type per transition, an explicit `Action`/`Event`/`TransitionResult` algebra,
fiber + sub-flow execution, checkpoint persistence, and a dedicated triage
component for stuck flows.

- [ ] State-machine framework for multi-step bilateral/multilateral flows (§3.1).
- [ ] Flows: RFQ → Quote → PO → Acceptance → Shipment → Receipt → Dispute → Settlement.
- [ ] Recall, quarantine, dispute flows (§5.2) as first-class flows, replacing the
      current recall *simulation test*.
- [ ] Commitment semantics (§3.3), including the strict canonical records and
      signed certification/audit processes specified by [ADR-005](../../docs/adr/adr-005-certification-and-audit.md)
      and [ADR-006](../../docs/adr/adr-006-canonical-schema-v1.md).
- [ ] **Contract/workflow packaging split (§3.4)** — Corda ships `finance/contracts`
      and `finance/workflows` as separate deployable modules: contract code is
      verification-only and deterministic, workflow code drives I/O. This maps
      directly onto separate Rust crates and answers §3.4, which is otherwise
      unaddressed in this roadmap.

This is a subsystem on the scale of Corda's flow framework. It should not be
estimated as a phase alongside "wire the indexer".

### Stage 4 — Economics *(reshaped by ADR-001)*

ADR-001 removed the EVM runtime from this stage. What remains:

- [ ] Account/balance model — deferred until a concrete onboarding-friction case
      justifies protocol economics; no account, balance, or fee type exists today.
- [ ] Fee delegation / sponsorship (§2.4) — follows the account/balance model and
      remains out of current scope.
- [ ] Optional EVM-compatibility adapter behind `ExecutionProvider` — no EVM
      runtime in GlassChain and no dependency of `glasschain-core`; revisit only
      if the §2.1 SHOULD is explicitly promoted.

### Stage 5 — Integration surface

- [ ] REST gateway (§7.1) and WebSocket event streams (§2.3). **Smaller than it
      looks** — Thor's entire subscription surface is one tightly scoped package
      (`api/subscriptions/`: beat/block/event/transfer readers, pending-tx, message
      cache), with Swagger served from a checked-in OpenAPI file
      ([study §7](../memories/reference-architectures.md)).
- [ ] ERP/WMS/TMS adapters (§7.1).
- [ ] Verifiable oracle bridge (§7.2).

### Stage 6 — Analytics

- [ ] Wire `ProvenanceIndex` + `AnalyticalFlattener` into `ServerState` (§6.2, §6.4).
- [ ] Bounded event-bus channels with an explicit backpressure policy.
- [ ] Reference warehouse adapter crate (§6.3), out of the node's critical path.
- [ ] §6.5 federated learning is a SHOULD — defer explicitly.

### Cross-cutting, unscheduled

Not blocking any stage, but they get more expensive the longer they wait:

- [ ] **Rehome cross-cutting integration tests.** Everything lives in
      `crates/glasschain-network/tests/`, including `sncm_compliance.rs`, which is
      not a network test. All three references give scenario tests their own home
      and Corda partitions them for CI parallelism
      ([study §10](../memories/reference-architectures.md)).
- [ ] **Flip `avoid-breaking-exported-api` once the SDK has consumers.** `clippy.toml`
      currently tells clippy to freely suggest breaking public signatures — correct
      pre-1.0, wrong afterwards. Corda gates public API changes in CI against a
      checked-in `api-current.txt` ([study §11](../memories/reference-architectures.md)).

---

## Open questions

1. ~~Is §2.1 (EVM/Solidity) a real constraint?~~ **Answered** — smart contracts are
   the MUST and are already met; EVM is a SHOULD; Solidity is out of scope. See
   [ADR-001](../../docs/adr/adr-001-execution-layer.md).
2. ~~Byzantine or crash-fault tolerance?~~ **Answered** — every member org is a
   zero-trust validator in v1; Tendermint/CometBFT-class BFT is selected, with a
   light-client ladder at national scale. See [ADR-002](../../docs/adr/adr-002-consensus-finality.md)
   and [ADR-004](../../docs/adr/adr-004-scale-topology.md).
3. ~~What must stay globally visible for regulatory traceability?~~ **Answered** —
   custody edges, identities, GTIN/batch/lot identifiers, timestamps, and recalls
   are public; commercial terms, quantities, and client relationships are private.
   See [ADR-003](../../docs/adr/adr-003-privacy-model.md).
4. ~~What is the target deployment scale?~~ **Answered** — the design horizon is
   70M entities; on-chain load is commitments and approved public records rather
   than raw events. Exact capacity remains unquantified until the 200/300-validator
   compact-workload testnet specified by [ADR-010](../../docs/adr/adr-010-capability-versioning-policy.md).
5. **Is there a delivery deadline?** The honest read is a multi-quarter program for a
   team. If the horizon is weeks, scope must be cut to a defensible subset —
   Stage 0 + Stage 1 + §5.1 hardening would be a coherent, demonstrable slice.
6. ~~Where do VM state mutations land?~~ **Answered** — hybrid explicit persistence,
   committed write sets, and explicit public/PDC scope. See [ADR-007](../../docs/adr/adr-007-vm-state-semantics.md).
7. ~~What does state-based endorsement require?~~ **Answered** — Fabric-style signature
   policies over verified MSP principals, scoped defaults and key constraints, distinct
   signers, and explicit custody/regulatory protections. See [ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md).
