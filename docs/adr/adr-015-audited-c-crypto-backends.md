# ADR-015 — Audited C cryptographic backends are accepted, feature-gated

**Status:** Accepted
**Date:** 2026-09-13
**Decision owner:** project owner
**Relates to:** [ADR-014](adr-014-bls-aggregated-certificates.md) (BLS
aggregation — `blst` revisit condition) · [ADR-013](adr-013-certificate-revocation.md)
(revocation) · [#85](https://github.com/dbbvitor/GlassChain/issues/85)
(backend policy review) · post-quantum plan action 1 (`X25519MLKEM768`) ·
performance plan Step 4 (300-validator gate)

## Context

Two independent "ready to start" actions were blocked on the same unanswered
policy question ([#85](https://github.com/dbbvitor/GlassChain/issues/85)):

| Action | Needs | Why |
|---|---|---|
| `X25519MLKEM768` post-quantum key exchange (shipped behind `pq-tls`, #105) | `aws-lc-rs` — C and assembly | `ring` ships no post-quantum group; harvest-now-decrypt-later exposure |
| 300-validator BFT finality gate (performance Step 4) | `blst` — C, ~10× faster pairings | The gate does not pass on the pure-Rust `pairing` backend (first vote 34.8 s; the 299 × 202-pairing precommit re-verification herd exceeds the scaled phase budget — `docs/benchmarks/consensus-capacity.md`) |

ADR-014 rejected `blst` "only with a measured need". That condition was met on
2026-09-04 and is recorded in ADR-014's amendment note. The remaining question
was policy: does the workspace accept audited C cryptographic backends at all,
and under what conditions? Deciding per-crate would produce the worst outcome —
accepting C for transport while refusing it for consensus, on no principle
either plan could state.

## Decision

1. **Audited C cryptographic backends are accepted** when all of the following
   hold:
   - the implementation is **industry-audited** (`blst` — Supranational,
     used by Ethereum consensus; `aws-lc-rs` — AWS, FIPS modules in scope);
   - adoption is **feature-gated by default** where a maintained pure-Rust
     alternative exists (`pq-tls` for `aws-lc-rs`; the `blst` backend selection
     for `bls-signatures`), so the default build keeps its smaller supply-chain
     footprint;
   - the **CI matrix proves the build** on Ubuntu, macOS and Windows before
     merge — the genuine unknown for any C dependency;
   - the **RustSec/`cargo audit` posture is clean** and stays monitored by the
     existing CI dependency-audit gate.

2. **`unsafe_code = "deny"` is unaffected.** It is a lint on workspace crates,
   not on the dependency graph; `wasmtime` and `ring` were already in the
   graph. The real costs of a C backend are supply-chain surface, build time
   and cross-platform CI — item 1 prices those in.

3. **`blst` (not `blst-portable`) is the selected variant.** The CI matrix is
   standard x86_64/ARM runners; `blst-portable` is a fallback only if a real
   target fails to build or run.

4. **The pure-Rust `pairing` backend for `bls-signatures` is retired.** There
   is no dual-backend shim: pre-release, the slower path is deleted, not
   maintained behind a fallback feature (AGENTS.md compatibility rule).

5. **This decision does not reverse ADR-014's other rulings.** Plain BLS
   multisig with PoP registration, the quorum-certificate wire shape, the
   ed25519 scope boundary and capability gating are unchanged; the backend swap
   keeps signatures byte-identical (the `bls-signatures` test vectors pin both
   backends to the same hash-to-curve output).

## Consequences

- `glasschain-core` drops its direct `bls12_381` dependency and the hand-rolled
  `verify_same_message_multisig` over `bls12_381` pairings — the same-message
  check moves to a blst-native form. Less hand-rolled pairing arithmetic on the
  critical verification path.
- The transport half (`aws-lc-rs`) needs no new work: `pq-tls` (#105) already
  ships it feature-gated; this ADR is its retroactive policy basis.
- The next crate that wants a C backend cites this ADR instead of reopening
  the policy question.
- Any claim of the 300-validator gate passing still requires recorded
  before/after benchmark evidence under unchanged quorum assumptions
  (performance plan validation gates) — accepting the backend does not assume
  the speedup.
