# ADR-003 — Privacy model and selective disclosure

**Status:** **Accepted 2026-08-20** — Option A confirmed; boundary, auditor,
purge, and reconciliation answers in the Decision section
**Date:** 2026-08-18
**Relates to:** §1.4, §3.2, §6.1 · [`requirements-alignment.md`](../../.agents/plans/requirements-alignment.md) D3 ·
[ADR-008](adr-008-endorsement-policy-model.md) (endorsement separate from membership)

## Context

§1.4 requires private data partitions; §3.2 requires sensitive commercial terms
(pricing, volumes, counterparties) be shared "strictly on a need-to-know basis…
avoiding global ledger replication".

Today every peer receives every transaction, in full, as JSON over a broadcast TCP
mesh (`glasschain-network/src/protocol.rs`). `glasschain-identity/src/channel.rs`
defines a `Channel` type, but it is referenced only by its own crate's tests. There
is no selective disclosure of any kind.

This is not a feature that can be layered on top of the current transport — it
changes what the transport does.

**ADR-001 unblocked this decision.** The original requirements treated EVM
compatibility as a MUST, which would have excluded a Corda-style model because
EVM contracts assume a globally consistent state trie. With EVM retained only as
an optional adapter, both privacy models were considered; Option A is now
accepted.

## Options

**A. Fabric-style private data collections.** One global chain. Private payloads
are exchanged point-to-point between authorized members; only a hash commitment is
written to the shared ledger. Non-members can verify that a transaction occurred
and has not been tampered with, without seeing its contents.

**B. Corda-style — no global ledger.** Each transaction is shared only with its
counterparties and a notary. There is no chain that everyone holds.

**C. Status quo — global broadcast.** Fails §1.4 and §3.2.

## Recommendation

**Option A.**

It preserves the architecture that already works and is tested: a single verifiable
chain, `ProvenanceIndex` custody lineage, `MetadataTrustScore`, SNCM schema
validation, and the analytics/flattener pipeline (§6.1–§6.4) all assume a globally
ordered chain they can index. Option B would invalidate most of that — under Corda's
model there is no global block stream for an indexer to consume, and regulatory
traceability (an Anvisa/SNCM recall spanning parties who were never counterparties)
becomes structurally harder.

Option A also composes cleanly with ADR-002's Tendermint/CometBFT-class BFT
choice: one canonical ordered chain, with confidential payloads beside it.

### Option A decomposes into three subsystems, not one

The reference study showed Fabric does not implement private data as a single
feature. It splits into three packages with different lifetimes and failure modes,
and GlassChain should mirror the split rather than build one monolith:

| Concern | Fabric | GlassChain home |
|---|---|---|
| Collection policy / membership | `core/common/privdata/` | `glasschain-identity` — extends `channel.rs` |
| Dissemination to authorized peers | `gossip/privdata/` (`distributor.go`, `pull.go`, `reconcile.go`) | `glasschain-network` — **this is `LibP2pNode`'s job** |
| Ephemeral pre-commit storage | `core/transientstore/` | `glasschain-storage` — new transient store, distinct from the committed state DB |

Two consequences fall out of this that were not visible before:

1. Fabric's dissemination layer is built on **gossip** — the same substrate
   `LibP2pNode` already provides via gossipsub + Kademlia. This is independent
   confirmation of the promotion below, not just an argument of convenience.
2. `core/ledger/kvledger/txmgmt/privacyenabledstate/` is a *distinct state DB*,
   not a read filter over the public one. Private state must be a separate keyspace
   in `StorageProvider`, which confirms this ADR's "largest blast radius" claim.

The `pull.go`/`reconcile.go` pair is also a requirement we would otherwise have
missed: a peer that was offline when a private payload was disseminated must be
able to fetch it later. Dissemination alone is not sufficient.

## Decision

**2026-08-20, requirement owner — wayfinder ticket
[#17](https://github.com/dbbvitor/GlassChain/issues/17).**

**Option A confirmed.** One global ordered chain; hash commitments public;
payloads disseminated point-to-point to collection members. (ADR-004's edge/PDC
layer already assumed this shape; this decision retroactively grounds it.)

The privacy sub-questions:

1. **Public/private boundary.** Public (committed on-chain): that a custody
   transfer occurred, custodian org identities (CNPJ-grade), GTIN/batch/lot
   identifiers, timestamp (BFT Time), and recall notices in full. Private (PDC
   payload): pricing, payment terms, **quantities**, and commercial-relationship
   details (client lists). Rationale: a recall must be traversable by parties
   who were never counterparties; volumes and terms are the commercially
   sensitive set.
2. **Auditor visibility.** Regulator/auditor orgs (Anvisa, MAPA) are
   policy-level members of every collection by default. Load-bearing fact:
   regulators already receive full pricing through NF-e, so per-collection
   audit grants would only create recall blind spots.
3. **Purge.** Private payloads expire per collection via a configurable
   retention policy (Fabric `blockToLive` analogue), tied to the product
   class's legal record-keeping shelf life. Hash commitments persist forever;
   after purge, a late auditor can prove a payload's existence and consistency
   but not read its contents. (GDPR/LGPD alignment.)
4. **Reconciliation window.** Two windows: the transient pre-commit store holds
   payloads **72 hours** (default, configurable — flagged by the owner as
   subject to change); post-commit, late peers pull private state from
   collection peers for as long as the collection's retention (question 3)
   allows.
5. **Certification and audit evidence.** Raw evidence for signed certification
   and audit processes remains in the applicable PDC/off-chain evidence store.
   The global chain receives only the evidence-manifest commitment and public
   issuer/scope/validity/status fields, as specified by [ADR-005](adr-005-certification-and-audit.md).
6. **Membership and endorsement are separate.** Collection membership controls
   who may read, write, and receive a private payload. A collection may impose a
   stricter endorsement policy on a write, but PDC membership alone is not an
   endorsement and a PDC write does not automatically require a multi-party
   quorum. Policy scope and composition are defined by [ADR-008](adr-008-endorsement-policy-model.md).
7. **The consensus boundary is explicit.** The globally replicated pending pool and block may carry approved public canonical records and commitment envelopes, but never private quantities, pricing, counterparties, raw evidence, high-frequency telemetry, or other unapproved cleartext private data. This does not turn the chain into a commitment-only ledger: public custody edges, permitted legal identity fields, lot/batch identifiers, timestamps, recalls, NF-e hashes, certification/audit anchors, and public write sets remain first-class records. The capability and historical-versioning rules are defined by [ADR-010](adr-010-capability-versioning-policy.md).

## Consequences

- **`LibP2pNode` is promoted from dead code to required infrastructure.** Selective
  disclosure needs addressed point-to-point messaging and peer discovery — exactly
  what the existing, currently unreachable gossipsub + Kademlia swarm provides.
  This reverses the recommendation in `integration-completion.md` Phase 3 to
  feature-gate or delete it.
- The wire protocol gains a private-payload message type alongside the existing
  `Transaction` / `Block` broadcast; `PROTOCOL_VERSION` must be bumped and the
  README protocol section rewritten.
- **A capability/versioning mechanism becomes necessary sooner than expected.**
  Fabric gates protocol features per channel via `common/capabilities/` and keeps
  validation logic in parallel versioned directories (`builtin/v12/`, `v13/`,
  `v20/`), because ledger rules cannot change retroactively without forking the
  chain. GlassChain has a bare `PROTOCOL_VERSION` constant and no capability
  concept. Since this ADR forces a wire-protocol change, the mechanism to make that
  change without invalidating existing blocks should land alongside it.
- `Channel` graduates from an unused type to the authorization boundary, and
  `StorageProvider` keys must be namespaced per channel so a member of channel A
  cannot read channel B's state.
- Transaction payloads split into a public commitment and a private body. This
  touches `TransactionKind` and every consumer of it — the largest blast radius
  of any decision in this programme.
- Certification and audit anchors are public facts that reference immutable lot
  commitments; their evidence manifests point to private/off-chain evidence and
  never turn the original transaction into a mutable record.
- State-based endorsement must address the explicit channel/contract/key scope
  from [ADR-007](adr-007-vm-state-semantics.md), while PDC membership and
  collection endorsement remain separate controls per [ADR-008](adr-008-endorsement-policy-model.md).
- The in-memory TOFU peer registry is insufficient once payload confidentiality
  depends on peer identity. This raised the priority of certificate verification in
  `cert_verifier.rs`, which is **now implemented** — but the verifier is still not
  attached to `Node`, so the TOFU gap remains open at the transport layer.

## Open questions

All privacy sub-questions answered 2026-08-20 — see Decision. Collection membership
and endorsement remain separate controls; policy composition is specified by
[ADR-008](adr-008-endorsement-policy-model.md). Per-product-class
retention values (question 3) and the final transient-window value (question 4)
are execution-time configuration, not open design questions.
