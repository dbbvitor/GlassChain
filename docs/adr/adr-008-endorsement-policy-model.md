# ADR-008 — State-based endorsement policy model

**Status:** Accepted
**Date:** 2026-08-24
**Decision owner:** project owner
**Relates to:** §1.2–§1.4, §3.3, §5.1–§5.2 · [ADR-003](adr-003-privacy-model.md)
(PDC membership) · [ADR-007](adr-007-vm-state-semantics.md) (scoped write sets) ·
wayfinder [#22](https://github.com/dbbvitor/GlassChain/issues/22)

## Context

`glasschain-identity` already has an `EndorsementEngine` and an
`EndorsementPolicy`, but the current implementation is transaction-level: it
allows an organization-name list and a required count, counts signatures, and
is not enforced at the network commit path. It has no state-key metadata,
policy composition, or distinct-principal protection.

The resolved VM model gives every persistent state operation an explicit channel,
contract, key, and public/PDC visibility scope. ADR-003 separately defines PDC
membership and private-payload dissemination. Endorsement must authorize the
transaction or state change without confusing consensus finality, PDC
membership, or the identity label supplied by a caller.

Fabric's research result provides the smallest proven model that covers the
requirements: a deterministic signature-policy tree over MSP principals, with
key-level validation metadata that can add constraints to a contract default.

## Decision

### 1. Policy scope and precedence

GlassChain uses these policy scopes:

1. an explicit channel default;
2. an optional stricter contract default;
3. an optional PDC collection endorsement policy for a PDC write; and
4. an optional key-level policy for the fully scoped persistent key.

A transaction must satisfy every applicable policy. A more-specific policy may
add constraints but may not weaken a channel, contract, or collection
requirement. A transaction touching multiple persistent keys must satisfy all
of their effective policies. A channel without an explicit default policy is
not an allow-all channel; v1 policies must name at least one valid principal and
require at least one signature.

PDC membership answers who may read, write, and receive private payloads.
Collection endorsement answers whose signatures are required for a PDC write.
They are separate controls: being a collection member does not automatically
satisfy its endorsement policy, and a PDC write does not automatically require a
multi-party quorum when its configured policy does not require one.

### 2. Policy expression and principals

The canonical v1 expression is a small Fabric-style signature-policy tree:

- `SignedBy(principal)`;
- `NOutOf(required, rules)`; and
- local `AND`/`OR` convenience builders that serialize to `NOutOf`.

The persisted/wire representation is deterministic and contains no executable
policy code. Implicit `ANY`/`ALL`/`MAJORITY` policies and weighted BFT voting are
not additional v1 policy languages.

A v1 principal is a verified MSP organization member. The organization and
identity are derived from the authenticated certificate/credential, not trusted
from a caller-supplied `org_name`. Role-specific principals such as regulator,
auditor, or logistics operator are deferred to the RBAC work; deferring roles
does not defer cryptographic identity verification.

A qualifying endorsement signs the exact transaction and its committed write
set. Policy evaluation counts at most one signature per distinct principal.
Multiple nodes or identities from one organization cannot satisfy two distinct
organization principals, and replayed or duplicate signatures do not increase
the count.

### 3. Operations requiring stronger endorsement

The following v1 defaults apply on top of the ordinary effective policy:

| Operation | Endorsement rule |
|---|---|
| Any accepted persistent state write | Channel/contract/key policy; a simple write may use a one-signature policy |
| Public custody edge or lot-custody handoff | Cross-organization sender and receiving custodian, normally `2-of-2` |
| Recall, quarantine, or dispute transition | Explicit configured multi-party policy involving the affected custodian and an authorized authority |
| `QualityCertification` or `AuditAttestation` | Issuer signature by default; additional signers only when the applicable policy requires them |
| PDC write | Collection membership plus its configured endorsement policy; no blanket quorum rule |
| Threshold inventory operation | Explicit contract/key policy; no generic core quantity rule because quantities may be private |
| Settlement finalization | Deferred with the account/settlement model |
| Endorsement-policy change | Current effective policy plus the governance requirement defined by the channel/contract |

Certification and audit remain first-class append-only records referencing
immutable lot commitments. Their issuer signature is not an edit to the source
lot or custody transaction.

### 4. Policy lifecycle and commit-time enforcement

Policy metadata is committed in-band with the global chain and is versioned,
append-only state. A policy update:

- is itself a signed transaction;
- must satisfy the current effective policy and any governance requirement;
- activates only after its containing block commits;
- leaves historical blocks governed by the policy version effective at that
  height; and
- may explicitly clear a key-level policy only through the same authorization,
  returning to the applicable fallback policy after commit.

In v1, a block is rejected when one transaction changes a key's policy and a
later transaction in that same block writes the same key. This avoids
provider-specific within-block dependency ordering; a later block can use the
new policy deterministically.

Endorsement is evaluated at transaction/block admission against the exact
transaction and committed write set. If any applicable policy is unsatisfied,
the transaction is not committed and no partial write set is materialized.
Endorsement is application authorization; it is separate from Tendermint-class
BFT finality and its quorum certificate.

## Consequences

- The existing `EndorsementEngine` is a useful verification core, but its
  organization-name/count API must evolve into verified MSP principals,
  deterministic policy expressions, and distinct-signer counting.
- The network commit path must invoke endorsement before accepting a transaction
  or write set, while `glasschain-core` remains independent of identity. Neutral
  policy/request types belong behind the existing provider seam; certificate and
  MSP verification remain in `glasschain-identity`.
- Persistent VM keys from ADR-007 now have a precise policy target. Public and
  PDC visibility do not change the identity or append-only rules.
- PDC collection configuration must carry membership separately from optional
  collection endorsement, and policy metadata must be included in the same
  deterministic history used for replay.
- Ordinary onboarding can remain low-friction under an explicit one-signature
  policy, while custody and regulated transitions require the cross-party
  evidence the domain requires.
- Implementing this decision remains Stage 2 work; the policy model is settled,
  but enforcement, RBAC, channel wiring, and certificate-backed identity
  plumbing are not shipped.

## Implementation handoff

1. Define identity-neutral policy expressions, principals, scoped targets, and
   endorsement requests/results behind the `glasschain-core` provider seam.
2. Make `glasschain-identity` evaluate the expression against certificate-bound
   MSP principals, reject duplicate/replayed principals, and validate policy
   metadata.
3. Store channel/contract/collection/key policy metadata in committed history;
   enforce current-policy authorization and the same-block policy-update rule.
4. Invoke the provider from the network commit path before block acceptance and
   before VM write-set materialization.
5. Add tests for nested `NOutOf`, distinct organizations, forged organization
   labels, multi-key transactions, PDC membership versus endorsement, custody
   `2-of-2`, policy updates, same-block conflicts, and failed authorization with
   no partial state.

## Out of scope

- Role-specific principals and full RBAC; those remain a separate Stage 2 item.
- Weighted validator voting, FBA quorum slices, or a second consensus protocol.
- Automatic multi-party endorsement for every PDC write.
- Settlement economics, accounts, balances, or fee sponsorship.
- Changing the immutable certification/audit and lot-commitment semantics in
  ADR-005 and ADR-006.
