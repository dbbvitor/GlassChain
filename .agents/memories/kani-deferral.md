# Kani deferral for the scoring/schema/capability surface

Ticket #166 aimed to bring the **full `glasschain-core` crate** under Kani
(#157). kani-verifier 0.68.0 / CBMC 6.11.0 cannot process that surface on this
codebase; the job gates the heap-free predicate that does verify, and the rest
is deferred with the evidence below (the map allows a forced infeasibility
exemption with evidence).

## What fails, and how

- **SHA-256 intrinsics**: any harness reaching `capability_hash` (so
  `CapabilitySet::genesis`, `lookup_capability`) pulls in
  `llvm.x86.sha256msg1/msg2/rnds2`, `llvm.x86.ssse3.pshuf.b.128` and
  `xgetbv`, which Kani lists as unsupported constructs. The verification then
  fails if those are reachable.
- **Heap-using predicates**: `MetadataTrustScore::compute` (pushes onto
  `Vec<String>`) and `schema::validate_asset` (builds `format!` messages)
  make CBMC abort with `CBMC failed with status 15` during propositional
  reduction, after the goto-instrument pass. This reproduces per harness.
- **System time**: `Block::new` calls `SystemTime::now`, a foreign call Kani
  cannot model; a harness that needs a `Block` must build the literal.
- `cargo kani -p glasschain-core --harness proofs::trivial` verifies in 0.03s,
  so the crate itself is processable — the failures are per-harness and
  reachability-driven.

## What verifies

`proofs::iso8601_check_is_total_and_bounded` (the date-structural predicate)
verifies; the weekly job runs exactly that harness with `--default-unwind 16`
on the command line (`[package.metadata.kani.flags]` is ignored by
kani-verifier 0.68.0: the same value there still unwinds unboundedly).

## Revisit trigger

When a Kani/CBMC release processes the allocation and x86-intrinsic paths (or
when the affected crates expose heap-free wrappers for the scoring and schema
predicates), re-add harnesses for `MetadataTrustScore::compute`,
`schema::validate_asset`, and `Block::has_valid_pow`, then widen the job
command back to `cargo kani -p glasschain-core`.
