# ADR-010 — Capability and versioning policy

**Status:** Accepted 2026-08-25
**Date:** 2026-08-25
**Decision owner:** project owner
**Relates to:** [ADR-002](adr-002-consensus-finality.md), [ADR-003](adr-003-privacy-model.md), [ADR-004](adr-004-scale-topology.md), [ADR-006](adr-006-canonical-schema-v1.md), [ADR-008](adr-008-endorsement-policy-model.md) · wayfinder [What capability/versioning policy does GlassChain adopt?](https://github.com/dbbvitor/GlassChain/issues/25)

## Context

GlassChain currently has a bare `PROTOCOL_VERSION` string and no capability
registry. The accepted privacy, schema, commitment, endorsement, and future BFT
decisions change wire or validation semantics. Applying those changes in place
would make peers disagree about the meaning of committed history.

The BFT scaling review also exposed an important boundary: 70M member identities
do not imply 70M validators or 70M raw events in the consensus path. The global
chain must remain useful for public provenance while private commercial data,
raw evidence, and high-frequency telemetry stay outside global replication.

## Decision

### 1. Consensus-facing data boundary

The consensus-facing pending pool and globally replicated blocks may contain:

- approved public canonical records;
- public custody edges and permitted legal identity fields;
- lot, batch, and commitment identifiers;
- timestamps, recalls, and NF-e hashes;
- signed certification and audit anchors;
- explicitly public VM write sets; and
- `StateCommitment` batch anchors with the required signatures.

They must not contain private quantities, pricing, counterparties, raw evidence,
high-frequency telemetry, or other cleartext private data not permitted by the
canonical schema. Private payloads use the PDC/off-chain path and are represented
on the global chain by commitments and approved public metadata.

This is not a commitment-only ledger: public canonical facts remain first-class
records. The core does not require SCITT, a ZK validity proof, a data-availability
committee, or a second settlement chain for v1.

### 2. Capability scope

Capabilities gate every consensus-visible or validation-affecting behavior,
including:

- private-payload/PDC wire and validation support;
- canonical schema v1, registered namespace schemas, and deprecation rules;
- `StateCommitment` and related batch-anchor semantics;
- endorsement-policy enforcement;
- the future Tendermint/CometBFT-class BFT consensus activation; and
- any later change to block, transaction, replay, or admission semantics.

A capability is not a license to add a deferred architecture. SCITT, ZK
validiums, KERI, DID/VC replacement, IPFS, execution sharding, and a separate
aBFT settlement core remain outside v1.

### 3. Network-wide scope

The active capability set is network-wide and part of committed chain history.
Channels and PDCs remain privacy and membership boundaries; they are not
independent consensus-rule domains. A capability activation is a signed,
append-only control-plane record, not a fourteenth business-schema record family.

Schema identity/versioning remains separate: every canonical record uses the
immutable `(schema_id, schema_version, schema_hash)` identity defined by ADR-006.
The capability set decides when a registered schema or validation version may be
used for new blocks.

### 4. Activation

A capability activation record contains the capability identifier, immutable
version/hash, and a future activation height. It is accepted and finalized by
the normal Tendermint-class BFT process using the `>2/3` current-validator
finality threshold. The activation block is validated using the capability set
that was active before the transition; the new set starts at the declared
height. An activation cannot change rules midway through its own block.

The activation record must be authorized under the network's governance policy
and cannot be an operator-only configuration switch. In v1, all member
organizations are validators, so the governance electorate and active validator
set coincide and the one-organization/one-vote assumption makes the threshold
operationally equivalent. If the validator set is bounded later, governance
standing remains attached to membership, including light-client members; the
transport and aggregation of that membership-wide vote are a separate future
decision and may not silently become validator-only authority.

### 5. Historical validation and replay

Validation selects the capability, schema, and validation-logic version effective
at each block height. Existing blocks retain their historical meaning and are
never reinterpreted under the newest rules. Versioned validators are preserved
rather than mutated in place.

Capability and policy changes are append-only. A policy/capability transition
cannot authorize a write in the same block by changing the rules underneath it;
the later block uses the new policy. Replay derives the same capability history
from committed blocks before rebuilding materialized state.

### 6. Wire compatibility and peers

`PROTOCOL_VERSION` remains a wire-encoding compatibility gate, separate from
ledger capabilities. The private-payload protocol change bumps the wire version.
The `Hello` handshake must enforce incompatible versions and advertise the
capabilities a peer supports. Connection negotiation is informative and
admission-related; it cannot activate ledger semantics outside committed
history.

A peer that cannot support an active capability may remain a TLS-protected,
read-only observer only when it can parse and validate the relevant history. It
may not propose, vote, relay active writes, or participate in consensus. A
validator that cannot support the active set must leave the active validator
set. A peer that cannot decode later blocks must disconnect or resync through a
compatible light-client path.

### 7. Scale evidence

The BFT implementation must be measured with GlassChain's actual compact
workload at 200 and 300 validators: public records, state commitments,
certification/audit anchors, NF-e hashes, block latency and size, vote traffic,
pending-pool backpressure, private-data dissemination separately, and recovery
under WAN delay or partition. Raw 70M-event ingestion and ZK-only proof
verification are not v1 benchmark targets.

Malachite remains a staged, default-off candidate behind `ConsensusProvider`.
Adoption requires a GlassChain testnet, API and stability evidence, licensing and
stewardship review, and a security audit. `tendermint-rs` may provide neutral
Tendermint types or light-client tooling; it is not a consensus engine.

## Consequences

- The capability registry and activation history become prerequisites for the
  private-payload wire change, strict schema activation, endorsement enforcement,
  state commitments, and the BFT swap.
- Historical validation can coexist with newer peers without silently changing
  the meaning of old blocks.
- The network must distinguish public canonical data from private PDC payloads
  at admission, transport, storage, and replay boundaries.
- Unsupported peers can remain observable without being allowed to vote under
  obsolete rules, while the validator set remains bounded for BFT liveness.
- The capability record, governance authorization, and ordinary BFT finality are
  separate concepts: membership authorizes network changes, validators order and
  finalize them, and channels/PDCs control private-data access.
- The concrete record envelope, capability names, governance aggregation after
  validator bounding, and state-commitment Merkle/cadence format are
  implementation or later-decision work; this ADR fixes their safety boundary,
  not their wire layout.

## Implementation handoff

1. Add identity-neutral capability and activation types behind the existing core
   provider seams without adding a dependency from `glasschain-core` to identity
   or network.
2. Store the active capability history with committed blocks and expose a
   deterministic lookup by height for admission, validation, and replay.
3. Add capability/version advertisement and actual mismatch handling to the
   network handshake; keep private payload dissemination separate from consensus
   broadcast.
4. Bump and document the wire protocol version when the private-payload message
   is introduced.
5. Integrate strict schema registry activation, state-commitment validation, and
   endorsement enforcement with the same historical capability lookup.
6. Add compatibility tests for old blocks, future-height activation, same-block
   transitions, unsupported peers, unknown namespaces, and private-data leakage.
7. Add the 200/300-validator compact-workload benchmark before assigning a
   production throughput claim.

## Out of scope

- EVM execution, Solidity, or an EVM-compatible core runtime.
- SCITT as a replacement transparency log, ZK validiums, DACs, KERI/ACDC,
  `did:webvh`, IPFS, execution sharding, or a separate aBFT settlement core.
- In-tree PostgreSQL/ClickHouse storage or bidirectional gRPC streaming.
- Accounts, balances, settlement economics, or fee sponsorship.
- A final validator-set churn, weighting, or membership-wide governance
  aggregation design after the validator set is bounded.
