# ADR-006 — Canonical schema v1 and extensibility

**Status:** Accepted
**Date:** 2026-08-24
**Decision owner:** project owner
**Relates to:** §2.2, §3.3, §4.1–§4.3, §5.2, §6.4 · [ADR-003](adr-003-privacy-model.md) ·
[ADR-004](adr-004-scale-topology.md) · [ADR-005](adr-005-certification-and-audit.md) ·
[ADR-007](adr-007-vm-state-semantics.md) ·
wayfinder [#19](https://github.com/dbbvitor/GlassChain/issues/19)

## Context

GlassChain currently validates only six `TraceableAsset` fields through the
compile-time `SNCM_SCHEMA` constant. Its transaction enum also covers only the
initial offer, purchase, contract, inventory, and asset-registration paths.
The requirements and later decisions add custody events, batch commitments,
private evidence, certification, and audit processes.

The schema must be strict enough for every peer to reach the same result, while
remaining extensible for partner-specific data. It must also preserve the
privacy and immutability rules already settled in ADR-003, ADR-004, and
ADR-005.

## Decision

### 1. Canonical v1 record families

Schema v1 includes these 13 record families:

1. `PartyIdentity` / organization
2. Product / SKU
3. Lot / batch
4. Inventory threshold / trigger policy
5. Purchase order
6. Shipment
7. Transit event
8. Delivery receipt
9. Inventory transformation
10. Recall
11. `QualityCertification`
12. `AuditAttestation`
13. `StateCommitment` / batch anchor

`EvidenceManifest` is an embedded manifest structure referenced by certification
and audit records, not a separate public entity. `InventoryThreshold` is a policy
record, not a custody event.

`StateCommitment` is the canonical anchor for a batch of off-chain events. It is
not the same thing as a VM write set: persistent contract state is commit metadata
with the explicit scope and visibility rules in [ADR-007](adr-007-vm-state-semantics.md),
while telemetry and evidence remain off-chain.

These are canonical data records, not a requirement to add one transport variant
per record family. The transport envelope and any `TransactionKind` mapping are
implementation work, but must preserve the record identity, schema version,
signatures, and immutable references below.

### 2. Common record envelope

Each canonical record carries, directly or through its signed envelope:

- `record_id`;
- `schema_id`;
- `schema_version`;
- the canonical record hash where an anchor is required;
- `occurred_at`;
- originating/issuing MSP identity;
- the required signature set;
- an optional channel/PDC reference; and
- registered namespaced extensions.

A record's identity and schema version are part of its signed canonical form.
Records are append-only once anchored.

### 3. Public and private data

The public lot record contains identity, custody, and commitment metadata. Raw
quantities and commercial terms remain in the applicable PDC/private payload;
the public `StateCommitment` binds that private batch to the public lot record.
Raw telemetry and evidence remain off-chain. A PDC-scoped VM write likewise
keeps its private value out of the globally replicated block and exposes only its
commitment there, as defined by ADR-007.

Certifications and audit records follow ADR-005: they reference the immutable lot
commitment, and their public anchors contain the evidence-manifest commitment,
issuer signatures, scope, validity, and status. They never edit the source lot,
custody, purchase, or shipment transaction.

### 4. Strict validation

Canonical v1 validation is strict at canonical ingress and commit:

- every required field must be present and valid, or the record is rejected;
- optional fields may be absent;
- validation is deterministic and does not depend on partner extension semantics;
- the existing metadata nudge/trust score may remain as a quality signal, but it
  cannot make an invalid canonical record valid; and
- certification, audit, recall, custody, and state-commitment records must fail
  without their required issuer, scope, reference, signature, validity, or
  commitment fields.

The schema validator must not silently accept a different interpretation of a
record on different peers.

### 5. Registered extension namespaces

Partner-specific fields are carried under registered namespaces. Each namespace
has an immutable schema descriptor and version (JSON-Schema-compatible
validation is the intended representation):

```json
{
  "extensions": {
    "urn:partner:cooperative-x": {
      "schema_version": "1",
      "value": {}
    }
  }
}
```

Unknown namespaces are rejected for canonical v1 records. Extensions cannot
override, shadow, or redefine core fields. Their canonical serialized values are
included in the record commitment; a private extension value may be kept in a
PDC with only its commitment public.

### 6. Registry and activation

Schema identity/versioning and protocol capability activation are separate but
cooperating concerns:

- the schema registry is network-wide and immutable by `(schema_id, version,
  schema_hash)`;
- old schema versions remain available to validate historical records;
- the capability mechanism controls when a schema version is accepted for new
  blocks and when an older version is deprecated; the network-wide activation and
  historical-version rules are defined by [ADR-010](adr-010-capability-versioning-policy.md); and
- no schema change may retroactively change the meaning of an existing block.

This gives the network one canonical schema vocabulary without making private
channels separate consensus domains.

### 7. NF-e mapping

NF-e semantics are reused for the shipment, transit-event, delivery-receipt, and
recall records. The SEFAZ adapter translates NF-e events into canonical records;
it does not invent a second event vocabulary.

## Consequences

- Stage 1 can replace `SNCM_SCHEMA` with a deterministic registry and add strict
  validation without changing the privacy model.
- Stage 3 can build workflows over stable record references rather than designing
  its own transaction vocabulary.
- Public indexers can project lot, custody, certification, and audit history
  without retrieving private quantities or evidence.
- Adding a record type or changing a required field is a versioned schema and
  capability decision, not an in-place edit.
- Existing non-canonical or legacy asset inputs need an explicit migration or
  compatibility boundary; they must not be silently treated as valid v1 records.
- Canonical records and VM write sets remain distinct: record validation governs
  the network vocabulary, while VM write-set validation governs execution-state
  scope, visibility, and atomic materialization.

## Out of scope

- The complete Brazilian regulator field catalog; schema implementation must
  validate the required v1 fields, while regulator-specific additions use
  registered namespaces.
- A particular JSON Schema validator dependency or database implementation.
- EVM ABI/schema compatibility; any future EVM adapter remains outside the core
  engine per ADR-001.
