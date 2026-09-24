# Kani scope and evidence

kani-verifier 0.68.0 / CBMC 6.11.0. The curated harnesses run in `ci.yml`'s
`kani` job on every PR and push; the Verus proofs (`make verus`, same PR/push
tier) cover the zero-trust logic that Verus can model.

```
cargo kani -p glasschain-core -p glasschain-identity --default-unwind 16 \
  --output-format=terse --sarif kani.sarif \
  -Z concrete-playback --concrete-playback=print
```

Five harnesses verify (measured warm, 2m40s for both crates):

- `glasschain-core`: ISO-8601 structural check, allocation-free proof-of-work
  prefix predicate, expiry-date trust-score contribution.
- `glasschain-identity`: `ocsp::minimal_be` (serial comparison) and
  `ocsp::read_tlv` (DER framing never over-reads).

Local helpers: `make kani` (same command), `make kani-coverage`
(`--coverage -Z source-coverage`, writes `target/kani/**/kanicov_*` JSON for
gap analysis).

## Zero-trust tool assignment

Policy: Verus first for zero-trust logic it can express; Kani where Verus has
no leverage; tests + mutation for the primitives underneath (ADR-019).

| Surface | Decision code | Verdict |
|---|---|---|
| Endorsement policy algebra (`core/endorsement.rs`) | `validate`, `evaluate`, `required_count`, `covers` | Verus (pure policy tree) |
| Trust score (`core/asset.rs`) | `MetadataTrustScore::compute`, `is_valid_iso8601_date` | Verus (bounded ints; Kani has parity today) |
| BFT quorum/bitmap/context (`core/{bft,consensus}.rs`) | `QuorumCertificate::validate`, `verify_certificate`, vote/context messages | Verus — quorum/bitmap kernels proved 2026-09-24 (`proof_arith` in `bft.rs`); context framing deferred (panic-on-length-cast path); BLS assumed |
| TOFU pin transition (`network/node.rs`) | `PeerRegistry::verify_or_register` | Verus (HashMap state machine; ed25519 assumed) |
| Private-payload gate (`network/node.rs`) | `private_peer_trusted`, `payload_targets`, `Channel::is_member` | Verus (membership conjunction; hash assumed) |
| Channel/membership rules (`identity/channel.rs`, `msp_policy`) | `is_member`, height-bounded authorization | Verus (modulo ed25519) |
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

## Verus toolchain notes

- Verified with the pinned release `0.2026.09.20.aef82ed`;
  `cargo verus verify -p glasschain-vm -p glasschain-core --all-features
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
