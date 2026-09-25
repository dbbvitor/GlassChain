# Kani scope and evidence

kani-verifier 0.68.0 / CBMC 6.11.0. The curated harnesses run in `ci.yml`'s
`kani` job on every PR and push; the Verus proofs (`make verus`, same PR/push
tier) cover the zero-trust logic that Verus can model.

```
cargo kani -p glasschain-core -p glasschain-identity --default-unwind 16 \
  --output-format=terse --sarif kani.sarif \
  -Z concrete-playback --concrete-playback=print
```

Seven harnesses verify (measured warm, ~3 min for both crates):

- `glasschain-core`: ISO-8601 structural check, allocation-free proof-of-work
  prefix predicate, expiry-date trust-score contribution, and
  `canonical_is_hex64_rejects_short_strings` (the 64-character width gate).
- `glasschain-identity`: `ocsp::minimal_be` (serial comparison),
  `ocsp::read_tlv` (DER framing never over-reads), and
  `ocsp::read_generalized` (a `GeneralizedTime` body is total).

Local helpers: `make kani` (same command), `make kani-coverage`
(`--coverage -Z source-coverage`, writes `target/kani/**/kanicov_*` JSON for
gap analysis).

## Zero-trust tool assignment

Policy: Verus first for zero-trust logic it can express; Kani where Verus has
no leverage; tests + mutation for the primitives underneath (ADR-019).

| Surface | Decision code | Verdict |
|---|---|---|
| Endorsement policy algebra (`core/endorsement.rs`) | `validate`, `evaluate`, `required_count`, `covers` | **Neither tool today** (evidence below) — tests + mutation |
| Trust score (`core/asset.rs`) | `MetadataTrustScore::compute`, `is_valid_iso8601_date` | Verus — score arithmetic proved 2026-09-24 (`asset::trust_proofs`: exact 20/10 formula, `<= 100`, standard gate); Kani keeps the ISO-8601 structural parity |
| BFT quorum/bitmap/context (`core/{bft,consensus}.rs`) | `QuorumCertificate::validate`, `verify_certificate`, vote/context messages | Verus — quorum/bitmap kernels (`bft::proof_arith`) and certificate admission (`consensus::cert_proofs`: acceptance iff names the block and is degenerate-or-complete) proved 2026-09-24; the bitmap expansion (`expand_signers`/`signers_in_range`, exact set bits) and the pure round kernels (`rounds::{proposer_slot, receipt_action, should_retain}`) proved 2026-09-25 (#176); context framing deferred; BLS assumed |
| TOFU pin transition (`network/node.rs`) | `PeerRegistry::verify_or_register` → `core::pin::decide` | Verus — `spec_decide` gate proved 2026-09-24 (`pin.rs`: poisoned/NodeId/Org reject, rotate only with a valid proof under the pinned key); ed25519 assumed |
| Private-payload gate (`network/node.rs`) | `private_peer_trusted`, `payload_targets`, `Channel::is_member` | Verus — `channel::private_payload_allowed` (the fail-closed conjunction) and the slice membership `channel::contains_str` proved 2026-09-25 (#176); the hash lookups stay behind the seam |
| MSP height-window authorization (`identity/msp_policy.rs`) | `MspEndorsementProvider::evaluate` bounds checks | Verus — `authz_proofs` proved 2026-09-24 (registered-before-use, go-forward revocation); ed25519 assumed |
| Channel membership (`identity/channel.rs`) | `Channel::is_member` | Verus — the `HashSet<String>` was replaced by a `Vec<String>` and `is_member` routes through `contains_str`, proved 2026-09-25 (#176) |
| OCSP DER codec (`identity/ocsp.rs`) | `minimal_be`, `read_tlv`, `read_generalized` | Kani (slices/parsers) — `minimal_be`/`read_tlv` proved |
| Signed message encoders (`identity/possession.rs`) | `org_possession_message`, `tofu_pin_message`, `msp_registration_message` | Kani attempted; CBMC times out (below) — tests + mutation |
| Canonical record rules (`core/canonical.rs`) | `is_hex64`, `is_present`, `matches_type`, `validate_record_with` | Kani (predicates); JSON/`format!` body deferred |
| Capability history (`core/capability.rs`) | `effective_set`, `apply`, `validate_block` | Kani via `crypto::sha256` stub if a harness needs it |
| Cert chain / CRL / AdminGate (`identity/cert_verifier.rs`, `rpc/auth.rs`) | `verify_cert_der`, `verify_ocsp_staple`, `AdminGate::authorize` | Neither (webpki/ring/SystemTime/x509) — tests + mutation |

## What works

- Fully heap-free predicates (byte arrays, `&str` slices).
- Bounded symbolic heap: one symbolic dimension (e.g. the eight digits of a
  date) with concrete allocations everywhere else.
- `read_tlv` with a symbolic buffer length: pure slice arithmetic, 10s.
- `read_generalized` on a fully symbolic 15-byte body: fast (the time-crate
  conversions are oracle-free arithmetic).
- Short-string width gates on heap-free predicates (`is_hex64`).

## Manual codec safety battery (2026-09-24)

The zero-annotation autoharness tier is toolchain-blocked (below), so the
substitute is hand-written panic-freedom harnesses over untrusted-input
codecs. Two landed; four targets are blocked with evidence:

- **`wire::base64_decode`**: CBMC cannot finish the base64 engine even at a
  4-symbolic-byte input (`status 15`). Stubbing the engine would make the
  harness vacuous.
- **`ocsp::parse_staple`**: the nested `read_sequence` `Vec` allocations over
  symbolic bytes hang CBMC at 4 and 8 bytes. `read_tlv` (the single-element
  core) stays the verified unit.
- **64-byte `is_hex64` exactness**: symbolic UTF-8 validation over 64 bytes
  hangs; the cheap, useful half is the short-input rejection harness.
- **`verify_ed25519` shape gate**: even though a ≤8-byte key makes the dalek
  call unreachable at runtime, its codegen kills `goto-instrument` (the same
  kill as autoharness). The gate stays test/mutation-pinned.

**Re-attempted 2026-09-25 (#176)** on the same kani-verifier 0.68.0 / CBMC
6.11.0, one harness at a time with `--default-unwind 16 -Z unstable-options
--harness-timeout 5m`. Outcomes unchanged: `base64_decode` → `CBMC failed with
status 15`; 64-byte `is_hex64` exactness and `parse_staple` → `CBMC timed out`
(5m); `verify_ed25519` → `goto-instrument exited with status 15`. The four
harnesses were removed again; the recorded trigger (a Kani/CBMC release that
stops killing `goto-instrument` and finishes these formulas) still gates a
retry.

## Autoharness: not a gate

`cargo kani autoharness -Z autoharness` (0.68.0) is unusable on this
workspace as a CI step. Evidence:

- Whole-crate runs on `glasschain-vm` and `glasschain-contracts` kill the
  compiler: `kani-compiler/src/intrinsics.rs:243` panics on a `catch_unwind`
  signature mismatch (`match output.kind() == Int(I32)`); `catch_unwind`
  appears in dependency monomorphizations (wasmtime), not in workspace code.
- On `glasschain-core` (compiles fine) the run ends with
  `error: goto-instrument exited with status 15` after a few successes, even
  with `-j 1 -Z unstable-options --harness-timeout 5m`; the failed process is
  not reported as a harness result. Whole-crate runs also spend most
  verification time on serde-generated visitors (`::_::` modules), which is
  noise, so any sweep needs `--include-pattern`/`--exclude-pattern` anyway.
- Autoharness defaults to a 60s per-harness timeout and its own unwind bound;
  the manual harnesses it also runs need `--default-unwind 16` explicitly.
- `--exclude-pattern` does not filter the crate's manual harnesses.

**Re-attempted 2026-09-25 (#176)**, still on 0.68.0 (the latest release).
Raising the timeout does not help: none of these failures is a CBMC
verification timeout, and `--harness-timeout` does not govern compilation or
`goto-instrument`. `glasschain-core` again ended at
`goto-instrument exited with status 15` after ~30 generated suites verified
(with `--harness-timeout 10m`, `-j 1`); a rerun on the warm incremental cache
died earlier with a second compiler bug —
`kani-compiler/src/kani_middle/analysis.rs:29:45: called Option::unwrap() on
a None value`. A GitHub-hosted runner uses the same toolchain, so the
compiler panics and the unimplemented `catch_unwind` (#267) reproduce there;
only the `goto-instrument` crash could be memory-influenced (this box had
~3 GB free), and testing that half would need an advisory, non-gating run on
a fresh runner. Not a gate; the revisit trigger below stands.

Useful part: the generated-harness table lists every skipped function with a
reason (`Missing Arbitrary implementation`, `Generic Function`, ...), which is
a cheap inventory of what autoharness could cover once it stops crashing.

## Symbolic heap limits

- Six independently symbolic `Option<String>` asset fields abort CBMC
  (`status 15` during propositional reduction). Bounded symbolic heap
  verifies.
- The signed message encoders (symbolic-length `Vec<u8>` builders) exceed the
  per-harness budget even at field length ≤ 4; they are pinned by unit tests
  (exact bytes) and mutation coverage instead.
- `schema::validate_asset` never finishes: the `format!` violation messages
  explode the formula.
- `Block::new` reads the system clock; harnesses call predicate helpers
  directly.

## SHA-256 and the clock

Any `crypto::sha256` / `capability_hash` / `Block::calculate_hash` path pulls
in `llvm.x86.sha256msg1/msg2/rnds2` and `xgetbv`, which Kani reports as
unsupported. When a hash-adjacent structural property is worth proving, stub
the wrapper — `#[kani::stub(crate::crypto::sha256, stub)]` — and document the
proof as "hash assumed". `SystemTime::now` is a foreign call; stubbing foreign
functions is supported, but no harness has needed it yet.

## Toolchain notes

- `[package.metadata.kani.flags]` is ignored by 0.68.0: `--default-unwind 16`
  must be on the command line or `str::from_utf8`'s validation loop unwinds
  without bound.
- `--sarif` and `-Z concrete-playback --concrete-playback=print` work with
  `--output-format=terse`; playback prints only on failure.
- `--coverage -Z source-coverage` writes JSON under `target/kani/` — the
  per-line `FULL`/`NONE` report is for local harness development, not CI.

## Revisit triggers

- Autoharness returns to CI when a Kani release stops killing
  `goto-instrument` and stops hitting the `catch_unwind` ICE; then scope it
  with `--include-pattern` (serde `::_::` visitors excluded) and assert a
  non-zero verified count.
- Hash-path harnesses when a property needs them; `crypto::sha256` is the
  stub seam.
- Verus is the tool for the zero-trust modules in the table above (#170
  roadmap); Kani picks up what Verus cannot model.

## Endorsement policy algebra: neither tool today (2026-09-24)

The zero-trust policy tree (`PolicyExpression::{validate, evaluate,
required_count}`) was attempted in both tools. Evidence:

- **Verus**: `Principal` and `PolicyExpression` are serde-derived types
  declared outside `verus!`. Specs cannot pattern-match an external type —
  `#[verifier::external_type_specification]` + `external_body` makes it
  opaque ("pattern constructor for an opaque datatype") — and declaring the
  types inside `verus!` instead makes Verus ICE on serde's generated
  `deserialize::visit_enum` items (`VerusErasureCtxt has not been
  initialized`). Making `vstd` unconditional in `glasschain-core` was tried
  for this and reverted. Revisit path: hand-written `Serialize`/`Deserialize`
  impls in a `verus!`-declared enum (wire format pinned by the existing
  round-trip tests), or an upstream fix.
- **Kani**: construction alone hangs CBMC. A harness that only builds
  `PolicyExpression::NOutOf { required: 1, rules: vec![signed_by("org-a")] }`
  and calls `required_count()` does not finish in 120s; a leaf-only
  `SignedBy` harness verifies in 0.38s. The recursive heap tree
  (`Vec<Self>` + `String` fields + recursive drop glue) is beyond CBMC, the
  same limit class as the symbolic `Option<String>` blow-up above.
- **HashSet**: the production `evaluate` signature takes `&HashSet<Principal>`,
  which would additionally need vstd's key-model assumption
  (`assume(obeys_key_model::<Principal>())`, sanctioned by vstd for custom
  keys) — against the no-cheat gate — and a `HashSet` probe timed out at
  600s under Kani.

What the attempt did buy: an unvalidated `NOutOf { required: 0, rules: [...] }`
evaluated **true** (zero-of-n is an allow-all shape) because the guard only
covered empty rules. `evaluate` now fails closed on `required == 0`, pinned
by `test_zero_required_never_evaluates_true`.

## Verus toolchain notes

- Proved modules: `glasschain-vm` gas, `glasschain-core` BFT
  quorum/bitmap (`bft::proof_arith`), certificate admission
  (`consensus::cert_proofs`: acceptance iff the certificate names the block
  and is degenerate-or-complete), the TOFU pin decision (`pin`, with
  `spec_decide` as the total model the exec function is proved equal to),
  trust-score arithmetic   (`asset::trust_proofs`: `trust_score_value` sums
  20/10-point flags and `is_standard_score` is the ≥80 gate), the MSP
  height-window authorization (`identity/msp_policy.rs::authz_proofs`), and
  the #176 residues: the bitmap expansion (`bft::proof_arith::expand_signers`),
  the consensus-round kernels (`glasschain-core/src/rounds.rs`), and channel
  membership plus the private-payload gate (`glasschain-identity/src/channel.rs`).
  `vstd` is unconditional in `glasschain-core` (and now `glasschain-identity`)
  now that non-`bft` modules need it.

- Verified with the pinned release `0.2026.09.24.b9416fa` (upgraded from
  `0.2026.09.20.aef82ed` on 2026-09-25; the newest release with x86-linux and
  macOS assets). The `vstd` crate stays at the newest published snapshot
  (`0.0.0-2026-09-20-0158` — no newer snapshot is on crates.io), so the
  binary and crate versions differ by design.
- Verus 2026 releases isolate loop bodies: facts from outside a loop are
  invisible inside it unless the loop invariant carries them — every loop
  invariant in the proofs keeps its own bounds. `cargo verus verify -p
  glasschain-vm -p glasschain-core -p glasschain-identity --all-features
  --locked` (the `cargo_verus` guide's Verus-relevant Cargo options come
  before the `--` separator). `make verus` adds `-- --expand-errors`;
  `cargo verus focus` is the local iteration loop (skips deps).
- The `integer_ring`/Singular mode is deliberately unused: the guide pins
  Singular **4.3.2** (4.4.0 is known incompatible; this box has 4.4.1), and
  the BFT obligations are inequality/linear arithmetic the default solver
  closes. Add Singular only if a ring-equality lemma appears.
- The `verus` CI job runs the Verus LLM guide's cheat check over `crates/`
  (bare `assume(`/`admit(`, `external_body`, `axiom`); `kani::assume` is a
  Kani harness constraint and is exempt by the pattern.
- Ghost erasure: `spec fn`/`proof fn` and contract clauses vanish from
  normal builds; `validator_count` in `bitmap_contains` exists only in the
  contract, hence the targeted `#[allow(unused_variables)]`. No
  `verus_only`-gated code is used (the erasure guide only sanctions it for
  `use` statements and config attributes, and cfg-gated *code* is
  unsound).
- `verusdoc` is not wired up: it needs Verus built from source (`vargo build
  -p verusdoc`) plus manual rustdoc flags. Specs are documented in prose on
  the kernels and in ADR-019 instead.
- Planned Verus surfaces keyed by `HashMap`/`HashSet<String>` (channel
  membership, private-payload gating) will need vstd's key-model assumption
  (`assume(obeys_key_model::<String>())` — the primitive axioms do not cover
  `String`) with the narrow cheat-check exemption, or a route through slices
  as the TOFU decision did. Budget for it before starting those slices.
- Workflow: for a new or churning module, write the Kani harness before the
  Verus spec — it finds panics and boundary violations cheaply and keeps the
  spec you eventually write honest. The ownership corpus is stable enough
  that the zero-trust set went Verus-first on purpose (ADR-019).
