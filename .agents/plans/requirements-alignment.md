# Requirements alignment — Hybrid Distributed Inventory System

**Status:** active; core mechanisms shipped, deployment and integration gaps remain
**Reviewed:** 2026-09-05 against `f7b434e`
**History:** [Debt-gap implementation record](../memories/debt-gap-handoff.md)
tracks the completed programme; closed tickets do not imply all requirements or
production adoption gates are met.

## Objective and accepted decisions

Build a permissioned supply-chain ledger combining Fabric-style identity and
endorsement, Corda-style workflows/selective disclosure, and event-driven
integration. Best-in-class latency/scalability is a goal constrained by zero
trust, Brazilian law and ICP-Brasil interoperability—not a certification claim.

| Decision | Accepted boundary |
|---|---|
| D1 — [ADR-001](../../docs/adr/adr-001-execution-layer.md) | WASM/Wasmtime stands; contracts are a MUST, EVM compatibility a deferred SHOULD, Solidity out of scope. |
| D2 — [ADR-002](../../docs/adr/adr-002-consensus-finality.md) | Tendermint/CometBFT-class quorum BFT; PoW remains the dev/test default, not production consensus. HotStuff speculation is not shipped. |
| D3 — [ADR-003](../../docs/adr/adr-003-privacy-model.md) | Public records/commitments, private payloads sent only to collection members; selective disclosure does not imply anonymization. |
| D4 — [ADR-007](../../docs/adr/adr-007-vm-state-semantics.md) | Explicit persistent WASM writes, committed write sets and replayable materialized state. |
| D5 — [ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md) | Deterministic policies over verified principals, distinct signer counting and scoped authorization; separate from BFT/PDC membership. |
| D6 — [ADR-010](../../docs/adr/adr-010-capability-versioning-policy.md) | Committed future-height capabilities, historical interpretation and separate wire compatibility. Production BFT adoption requires its testnet/API/licensing/audit gates. |

[ADR-009](../../docs/adr/adr-009-validator-eligibility.md) records membership and
rotation policy; [ADR-011](../../docs/adr/adr-011-federation-trust-store.md),
[ADR-012](../../docs/adr/adr-012-signature-binding.md),
[ADR-013](../../docs/adr/adr-013-certificate-revocation.md) and
[ADR-014](../../docs/adr/adr-014-bls-aggregated-certificates.md) cover trust stores,
governance authorization, CRLs and BLS certificates. Accepted policy is not
proof that every deployment control is automated or audited.

## Requirement traceability

This replaces the old pre-programme “3 met / 11 partial / 11 absent / 1 conflict”
count and obsolete assertions that workflows, schema, PDCs and vote rounds do not
exist. “Shipped” here describes code, not a full compliance or operational verdict.
Paths in the evidence column are relative to `crates/`.

| Req | Requirement | Current implementation / remaining work |
|---|---|---|
| 1.1 | MSP / X.509 identity | Partial: `glasschain-identity/src/cert_verifier.rs` verifies chains/intermediates/CRLs; `glasschain-node/src/main.rs` installs with `--org` + `--trust-store`. Persistent identity, certificate-bound endorsement principals and lifecycle operation remain. |
| 1.2 | RBAC | Not delivered as a complete role policy system. `glasschain-rpc/src/auth.rs` provides authentication; roles/permissions and production authorization need specification. |
| 1.3 | Multi-party endorsement | Engine and commit gates shipped (`core/src/endorsement.rs`, `identity/src/msp_policy.rs`, `network/src/node.rs`); activation/provider configuration, canonical-record scope and recall independence remain gaps D1/D2/D4. |
| 1.4 | Channels / private partitions | Partial: `identity/src/channel.rs`, member-gated PDC delivery/reconciliation over TCP exist; not a fully wired persistent channel-management service. libp2p remains experimental/unwired. |
| 2.1 | Smart contracts (MUST) | WASM execution, fuel/gas and committed explicit write sets shipped (`glasschain-vm`, `core/src/write_set.rs`); production limits/security remain validation work. |
| 2.2 | Canonical business events | Schema v1 and event mapping shipped (`core/src/canonical.rs`, `indexer/src/event_bus.rs`); external delivery guarantees and all business integrations are not implied. |
| 2.3 | WebSocket / log subscriptions | Partial: gRPC server streams exist; no WebSocket/REST gateway. |
| 2.4 | Sponsored transactions / fee delegation | Absent/deferred; no account, balance or fee model. |
| 3.1 | Workflow-first modelling | Engine/checkpoints and purchase/receipt/attestation/recall flows shipped in `glasschain-workflows`; unattended discovery and orchestration remain (D6). |
| 3.2 | Selective privacy | PDC boundaries/distribution/reconciliation shipped (`network/tests/pdc_boundary.rs`, `pdc_distribution.rs`); fail-open unconfigured trust and restart-safe deletion remain. |
| 3.3 | Legal commitment semantics | Technical commitment/certification/audit records and workflows exist. Legal effect, timestamp evidence and qualified-signature requirements require deployment-specific review, not a code-only “met”. |
| 3.4 | Domain app packaging | Contract/workflow crate split shipped; distribution/version compatibility with external consumers remains. |
| 4.1 | Canonical minimum schema | 13 v1 families and strict validation shipped (`core/src/canonical.rs`); do not add learning/model record families to immutable `SCHEMA_V1`. |
| 4.2 | Extension namespaces | Registry/validation machinery shipped; governed registration and consumer interoperability remain operational work. |
| 4.3 | Schema + policy registry | Registry, policy history and capability activation shipped (`core/src/canonical.rs`, `endorsement.rs`, `capability.rs`); real governance bootstrap and record-scope wiring remain. |
| 5.1 | Autonomous replenishment | Watcher/approval/PO path shipped; exercise it with active policies and distinct real principals before regulated operation. |
| 5.2 | Recall / quarantine / dispute flows | Flow transitions and scenarios shipped (`workflows/src/recall_flow.rs`, `network/tests/recall_flow_scenario.rs`); authority policy D2 and recovery D6 remain. |
| 6.1 | Off-chain analytics | Projections stay outside canonical ledger state; ingestion still costs node CPU/memory in `after_block_commit`, so isolation must be measured. |
| 6.2 | Indexer service | In-memory indexer/provenance/flattener and RPC read path shipped (`rpc/src/server.rs`); durable external indexing/export remains. |
| 6.3 | Relational / analytics warehouse | Not delivered; a separate reference adapter is the planned integration, not a database dependency of consensus. |
| 6.4 | AI-ready semantic feeds | Flattener/CSV/lineage exist, currently asset-registration-oriented; measure coverage, memory and export continuity before calling this a full feature pipeline. |
| 6.5 | Federated learning (SHOULD) | Absent and explicitly deferred behind a useful task, dataset, evaluation and privacy model; see learning-loop mapping below. |
| 7.1 | ERP/WMS/TMS, REST/gRPC/CSV | Partial: gRPC and CSV mechanisms exist; external adapters/REST remain, SDK is not a complete live network client. |
| 7.2 | Oracle connectivity | Not delivered; trusted input/authenticity and governance must be specified per integration. |
| 8.1 | Metrics, tracing, admin API | Partial diagnostics (logs, event streams, dropped-outbound counters, test metrics); no complete production observability/admin surface. |
| 8.2 | Deterministic finality | Staged BLS vote rounds exist, default-off. Local 100/200 measurements exist; 300 not passing, historical verification/production adoption gates remain. No longer an unresolved PoW architecture conflict. |

## Roadmap — remaining work, not a second shipped-feature checklist

### Stage 0 — decisions and evidence

Architecture decisions and CI infrastructure shipped. Keep production claims
behind ADR-010 gates. The benchmark record separates PoW propagation from staged
BFT finality; local measurements are not a WAN deployment or security audit.
Consult [performance.md](performance.md) for Step 0–7, not old validator-count
extrapolations. 300 is an operating target, not a proven upper bound.

### Stage 1 — foundations and observability

- [ ] Instrument admission, phase/QC verification, commit, persistence,
      propagation and analytics separately; reuse existing counters/events before
      creating another metrics crate or migrating all logging.
- [ ] Bound and measure mempool/history costs (source debt D3), and high-frequency
      flattener memory/query/replay costs (performance §5).
- [ ] Specify local durable acknowledgement and crash recovery before a persistent
      pilot. Existing sled logging does not need another WAL.

### Stage 2 — governance and privacy

- [ ] Complete [zero-trust.md](zero-trust.md)'s fail-closed deployment, historical
      verification and identity lifecycle gates; the verifier being installed is
      not the same as every principal/path being certificate-bound.
- [ ] Resolve governance bootstrap D1, recall authority/scope D2 and certificate
      registration D4 in [deferred-code-debt.md](deferred-code-debt.md).
- [ ] Enforce restart-safe private-payload deletion D5, including purge scheduling,
      replica/backup retention and failure handling.
- [ ] Define RBAC and channel-management operations before implementing them.
- [ ] Preserve current addressed TCP PDC transport; libp2p adoption is separate
      interoperability/PQ work, **not a prerequisite to already-shipped PDCs**.

### Stage 3 — workflows and governed feedback

- [ ] Restore triage discovery across restart (D6); checkpoint persistence alone
      cannot surface a stalled recall after a process restart.
- [ ] Test recall/quarantine/dispute and replenishment with the intended active
      endorsement policy and authority separation, not just happy-path transitions.
- [ ] Add a measured Sense/Decide/Adapt/Learn scenario below using existing flows
      and projections; do not create another workflow engine or mutate committed
      records when an off-chain recommendation changes.

### Stage 4 — economics and optional compatibility

No account/balance/fee/sponsorship implementation until a concrete requirement
justifies it. EVM compatibility remains a deferred adapter SHOULD; WASM stands.
Neither is needed to demonstrate the current supply-chain workflow.

### Stage 5 — integration and demonstration

- [ ] Build the [browser demo](gui-demo-benchmark.md): a synthetic headless Rust
      driver, same-origin demo HTTP/SSE bridge and accessible web UI. Evaluate
      optional WebGPU against Canvas2D; no browser keys or validators. This
      supersedes desktop gpui, remains unimplemented, and does not complete the
      product REST/WebSocket requirement or adoption-gate benchmark.
- [ ] Specify the first ERP/WMS/TMS integration, REST/WebSocket surface or oracle
      consumer before choosing an adapter. Preserve gRPC as the existing path.

### Stage 6 — analytics and learning

- [ ] Reference warehouse/export adapter outside consensus; establish bounded
      batches, lag/replay semantics and retention before choosing a database.
- [ ] Validate semantic feed coverage, lineage/CSV correctness and performance
      with existing `AssetRegistration` fixtures plus canonical-record controls.
- [ ] Federated learning stays a SHOULD, gated as below. Heavy parameters and
      training stay off-chain if ever adopted; no default IPFS dependency.

### Cross-cutting gates

Keep tests and public API compatibility proportionate to actual consumers; a
repository-wide test move is not required for this plan. Regulatory deployment
requires legal basis, data classification (including linkable hashes/identifiers),
access/retention controls and evidence/credential profiles. Schema completeness
scores do not certify ANVISA, LGPD or ICP-Brasil compliance.

## DLT-LFL mapping — useful lens, not a new mandatory framework

The supplied report gives no primary citation establishing DLT-LFL or BC-FL as a
mandatory architecture or regulation. Use the four phases to find missing
feedback, not to promote §6.5 from SHOULD to MUST.

| Phase | Existing mechanism | Minimal planned validation / gap |
|---|---|---|
| **Sense** | Committed ledger events, `indexer/src/event_bus.rs`, provenance and `flattener.rs` | Replay/lag/coverage checks; verify the flattener does not silently skip the workload being evaluated. Sensor truth and private access remain separate concerns. |
| **Decide** | Contract rules, `contracts/src/approval_gate.rs`, watcher conditions, flow transition tables | Deterministic rules and authority checks; a threshold baseline before any model. Treat external recommendations as untrusted inputs. |
| **Adapt** | `workflows/src/watcher.rs`, `purchase_flow.rs`, `recall_flow.rs`, runner/checkpoint actions | Changes go through normal transactions, endorsements and regulator policy; no training process may directly mutate ledger state or authorize a recall. |
| **Learn** | Off-chain lineage/CSV and recorded workflow outcomes | Evaluation/reporting is possible; automated model training or policy retraining is **not implemented**. Start with offline outcome comparison and human-approved, versioned policy changes. |

First scenario: synthetic demand shift or recall, measured against a fixed-rule
baseline. Record false alerts, stockout/recall response time, authority decisions
and recovery behaviour. A proposed policy version is reviewed, tested and
activated through existing governance; roll back a policy prospectively, never
rewrite historical transactions. This closes a useful feedback loop without ML.

**Before federated learning:** identify the decision owner, a measurable task,
representative authorized dataset, cross-org trust model, evaluation baseline,
retention budget and measurable advantage over the deterministic baseline.
Model updates/gradients may reveal training data; off-chain storage, hashes and
federation alone do not provide privacy. Specify access controls, poisoning/
Sybil defences, reproducibility and secure aggregation/differential privacy if
the threat model requires them. Legal review must address personal-data processing
and automated decisions where applicable.

If evidence later justifies training, use an off-chain adapter consuming authorized
committed events. Prefer controlled storage already in use; content addressing
proves integrity, not confidentiality, availability or deletion. Only approved
public commitments may be anchored using existing schemas where semantically
valid, or a governed extension—not raw parameters or new core `SCHEMA_V1` families.

## Validation and ownership

Each open item needs a responsible owner and a runnable acceptance check before
implementation. D1–D7's detailed tests live in the [source-comment debt plan](deferred-code-debt.md).
WAN/resource tests live in [performance.md](performance.md); archival evidence
research lives in [post-quantum.md](post-quantum.md). The README points users to
these plans without reproducing an easily stale issue-status table.
