# Kani scope for glasschain-core

kani-verifier 0.68.0 / CBMC 6.11.0 verify three predicates in
`crates/glasschain-core/src/proofs.rs`: the ISO-8601 structural check, the
allocation-free proof-of-work prefix predicate, and the expiry-date
contribution to the trust score. The weekly `deep-checks.yml` job runs the
whole module (`cargo kani -p glasschain-core --default-unwind 16`). Expansion
beyond these is deferred with the evidence below.

## What works

- Fully heap-free predicates (byte arrays, `&str` slices).
- Bounded symbolic heap: one symbolic dimension (for example the eight digits
  of a date) with concrete allocations everywhere else.

## What does not

- **SHA-256 via x86 intrinsics**: any `capability_hash` path pulls in
  `llvm.x86.sha256msg1/msg2/rnds2`, `ssse3.pshuf.b.128` and `xgetbv`; Kani
  lists them as unsupported and verification fails if they are reachable.
- **Unbounded symbolic heap**: six independently symbolic `Option<String>`
  asset fields abort CBMC with `status 15` during propositional reduction.
  The allocation pattern itself is not the problem — bounded symbolic heap
  verifies.
- **`schema::validate_asset`**: no result within ten minutes; the `format!`
  violation messages explode the formula.
- **`Block::new`**: reads `SystemTime::now`, an unsupported foreign call;
  harnesses call the predicate helpers directly instead.
- `[package.metadata.kani.flags]` is ignored by 0.68.0 — `--default-unwind 16`
  must be on the command line, otherwise `str::from_utf8`'s validation loop
  unwinds without bound.
- CBMC crashed (`status 139`, segfault) on `slice::repeat` reached through the
  old allocating `has_valid_pow`; that predicate is allocation-free now.

## Revisit trigger

When a Kani/CBMC release supports the x86 intrinsics and finishes the schema
report, add harnesses for the capability lookups and `validate_asset`, then
widen the job. Until then Verus is the tool for those modules (#170).
