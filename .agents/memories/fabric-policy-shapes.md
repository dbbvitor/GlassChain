# Fabric policy shapes — research memory

**Learned:** 2026-08-24
**Source:** research recorded on [wayfinder issue #20](https://github.com/dbbvitor/GlassChain/issues/20)

## Findings

### Signature policies

Fabric's reusable endorsement policy is a deterministic `SignaturePolicyEnvelope`:
a flat principal list (`MSPPrincipal`, organization/member and role identities)
plus a `Rule` tree containing `SignedBy` and `NOutOf`. `AND` and `OR` are
compositions of `NOutOf`. `NOutOf` counts distinct principals, so an `AND` over
Org A and Org B cannot be satisfied by two signatures from Org A.

Implicit-meta `ANY`/`ALL`/`MAJORITY` policies are channel-configuration shorthand
for org-counting defaults. They are useful for configuration but are not needed
as a second GlassChain wire language.

### State-based endorsement

Fabric stores a state-based policy as validation-parameter metadata on the key,
not as an unrelated side table. Validation uses key-level policy first and falls
back through collection/chaincode policy. The safe interpretation is that a
more-specific policy may add constraints but may not remove the applicable base
requirement. Policy changes are in-band, MVCC-tracked metadata writes.

A block containing a policy update and a later write to the same key needs an
explicit dependency-ordering rule. GlassChain chooses to reject that same-block
case in v1 rather than add speculative ordering machinery.

### Collection configuration

Collection membership controls who may read, write, and receive private data;
it is separate from endorsement, which controls whose signatures a write needs.
A collection can therefore have stricter endorsement than membership. Collection
configuration also governs dissemination counts, retention/block-to-live, and
member-only read/write behavior. GlassChain's public chain commitment and PDC
rules remain defined by ADR-003.

## Design consequence for GlassChain

[ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md) adopts the small
signature-policy tree over verified MSP organization members, with channel and
contract defaults, optional collection policy, and fully scoped key-level
constraints. It keeps roles for the RBAC work, separates PDC membership from
endorsement, and requires distinct principals/signatures over the exact
transaction plus committed write set.
