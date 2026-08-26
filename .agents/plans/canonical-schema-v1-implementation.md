# Plan — Canonical schema v1 registry (ticket #34)

**Ticket:** [#34 Canonical schema registry and strict v1 validation](https://github.com/dbbvitor/GlassChain/issues/34)
**Spec:** ADR-006 · ADR-005 · spec-close-debt-gap.md implementation decision 1
**Branch:** `main`

## Scope

The v1 data model end-to-end at the core seam, plus node-level proof:

1. **`glasschain-core/src/canonical.rs`** — new module:
   - 13 record families as a static, immutable registry keyed by
     `(schema_id, schema_version, schema_hash)`; each descriptor carries its
     required-field catalog, anchor requirement, status vocabulary, and
     signature requirement.
   - `CanonicalRecord` — the ADR-006 common envelope (record_id, schema_id,
     schema_version, occurred_at, issuer, required signature set, optional
     channel/PDC ref, registered extensions) + family payload.
   - `canonical_form()`/`commitment()` — deterministic canonical serialization;
     anchored families must carry `commitment == sha256(canonical_form)`.
   - Registered extension namespaces: descriptor with JSON-Schema-compatible
     subset (field types + required), no core-field shadowing, private
     namespaces admit only a commitment and require `pdc_ref`.
   - Legacy boundary: `TraceableAsset`-shaped payloads are rejected with an
     explicit migration path; `migrate_legacy_asset()` builds product+lot records.
2. **Wiring (one seam, both gates):** new `TransactionKind::CanonicalRecord`
   variant; `Ledger::add_transaction` (admission) validates records;
   `Ledger::validate_chain` + `try_replace_chain` (commit of locally mined and
   peer blocks) re-validate every record in every block.
3. **Tests:** unit (core) — accept/reject matrix, unknown namespace, shadowing,
   private-leakage rejection, commitment mismatch, status vocab, legacy
   boundary, determinism, historical version lookup. Integration
   (`glasschain-network/tests/canonical_schema.rs`) — node-level admission
   rejection, valid record commit, unknown namespace, private leakage, legacy
   input, two-node sync.

## Out of scope (later tickets)

- Capability-controlled activation / future-height (ticket #36).
- Signature *verification* (opaque in core; lands with endorsement, #37/#45).
- VM write sets (ticket #35), PDC dissemination (#47).

## Validation

`cargo check --workspace --all-targets --all-features --locked` →
`cargo test -p glasschain-core` and the new integration file →
`cargo test --workspace --all-targets --all-features --locked` →
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` →
`cargo fmt -- <touched files>`.