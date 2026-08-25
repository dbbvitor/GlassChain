# ADR-005 — Certification and audit attestations

**Status:** Accepted
**Date:** 2026-08-20
**Relates to:** §3.3, §4.1, §5.2, §6.1 · [ADR-003](adr-003-privacy-model.md)
(private evidence) · [ADR-004](adr-004-scale-topology.md) (global commitments) ·
wayfinder [#19](https://github.com/dbbvitor/GlassChain/issues/19)

## Context

Supply-chain certification and audit are evidence-bearing processes: quality
certificates, regulatory inspections, custody audits, and similar attestations
must be verifiable without exposing commercial or personal evidence globally.
They also refer to a lot or batch whose original transaction and commitment are
immutable.

Treating certification as an update to the original transaction would destroy
append-only provenance and make it unclear which issuer signed which version of
the lot state. Treating raw evidence as a public transaction would violate the
privacy boundary in ADR-003.

## Decision

1. **Certification and audit are first-class signed processes.** They are
   represented by their own canonical records and public chain anchors. They are
   not modifications, overwrites, or status fields on the original lot,
   custody, purchase, or shipment transaction.
2. **Every record references an immutable lot commitment.** The reference is to
   the lot/batch commitment identifier and its committed hash; the source
   transaction remains unchanged. A later correction, renewal, suspension, or
   revocation is a new signed record or append-only status event referencing the
   certification/audit record and the same lot commitment.
3. **The global chain stores the verifiable anchor, not raw evidence.** The
   public anchor contains at least:
   - certification or audit record identifier and type;
   - referenced lot/batch commitment;
   - evidence-manifest commitment (hash or Merkle root);
   - issuer MSP identity and signature(s);
   - certification/audit scope;
   - validity interval; and
   - current status, with status changes themselves signed and anchored.
4. **Raw evidence remains private and off-chain.** Evidence files, sensor
   readings, inspection documents, photos, and personal/commercial details live
   in the applicable PDC or authorized off-chain evidence store. The evidence
   manifest contains content-addressed references and descriptive metadata
   needed to verify the public commitment, but not the private evidence itself.
5. **Verification is layered.** Any peer can verify the global signature,
   issuer identity, lot commitment reference, manifest commitment, scope,
   validity, and public status. Only authorized PDC members can retrieve the
   underlying evidence and validate the manifest's private entries.

## Canonical schema implications

Schema v1 includes certification and audit records as first-class entity types:

- `QualityCertification` — an issuer's signed claim that a lot satisfies a
  defined certification scope for a validity interval.
- `AuditAttestation` — a signed record of an audit or inspection process and its
  outcome/status for a referenced lot commitment.
- `EvidenceManifest` is the manifest structure referenced by either record; it
  is not a public copy of the evidence payload.

The minimum public fields and extension rules belong in the runtime schema
registry. Partner-specific evidence metadata may use registered namespaces but
must not change the meaning of the public anchor or lot commitment.

## Consequences

- The indexer can project immutable lot commitments and their certification/audit
  history without accessing private evidence.
- A certification expiry or revocation does not rewrite historical custody or
  purchase transactions; it adds a signed, ordered fact that downstream readers
  must interpret with the validity/status rules.
- PDC membership and evidence retention follow ADR-003. Evidence purge does not
  remove the public issuer signature or manifest commitment; it may make the
  private contents unavailable after the retention window.
- Workflow and recall flows can reference certification/audit status as inputs,
  but must not mutate the source lot transaction.

## Out of scope

- A particular evidence database or object-storage vendor.
- Zero-knowledge proofs; a manifest commitment is sufficient until a requirement
  needs proof of contents without disclosure.
- The complete certification taxonomy or Brazilian regulator-specific field list;
  those are schema-v1 work, not a reason to weaken this invariant.
