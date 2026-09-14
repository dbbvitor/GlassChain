# Plans

Current planning map, reviewed 2026-09-13. Frontier C (Consensus Capacity / BLST
Backend) **concluded**: `bls-signatures` now runs the audited `blst` backend
(ADR-015) with the sum-of-keys verify, and the 300-validator finality gate
passes (p50 3 996 ms, exact quorum every round — `docs/benchmarks/consensus-capacity.md`).
Next in the priority sequence: **Frontier B (RBAC & Operational Tail) → Frontier
D (Browser Demo)**. Frontier A concluded earlier: `EquivocationProof` carries
both dual-signed votes and verifies through the #95 context envelope.

| Plan | Concluded / available | Pending frontier |
|---|---|---|
| [Requirements alignment](requirements-alignment.md) | Schema, write sets, workflows, PDC, analytics mechanisms shipped; decisions D1–D6 settled | Deployment authorization/recovery, integrations, governed feedback; FL remains deferred |
| [Source-comment debt](deferred-code-debt.md) | All seven markers (D1–D7) settled and benchmarked in code (#106–#109, #114–#115) | Work completed; maintain benchmarks and monitor regression |
| [Zero trust](zero-trust.md) | Consensus context-authenticated votes (#95, #99), live receipt journal (#96), historical QC verification on sync/restart (#97, PR #116), deadlines/queues (#98), fail-closed PDC (#86), possession proofs (#110), durable TOFU (#88), CRLs (ADR-013), cert-bound principals (#87), dual-sign `EquivocationProof` (Frontier A concluded) | OCSP stapling/verification, deployment RBAC, and replica backup retention (Frontier B) |
| [Performance](performance.md) | Local 100/200 BFT measurements, drop counters, hybrid TLS (#105), WAN proxy (#108), D3 admission bench (#106), read-path memory baseline (#107), **BLS backend on audited `blst` with sum-of-keys verify (ADR-015); 300-validator gate passing** | Step 1 codec profiling measured first; D3 admission optimization (one rebuildable index at the owning layer); Step 5+ in-family latency candidates |
| [Post-quantum readiness](post-quantum.md) | Algorithm discriminants shipped; negotiated X25519MLKEM768 hybrid TLS shipped (#105) behind `pq-tls` (retroactively grounded by ADR-015) | Migration and long-term archive profile decisions |
| [Browser demo](gui-demo-benchmark.md) | Web-app direction replaces desktop gpui; Canvas2D baseline + WebGPU evaluated | Browser/bridge spike, headless runner, accessible UI, measured renderer choice (Frontier D, queued after C and B) |

See [the latest report assessment](../memories/external-review-verdicts.md) for
accepted, corrected and deferred literature suggestions. Source debt lives once
in its inventory; other plans reference its D1–D7 identifiers.

[Artifact conventions](../README.md): retire completed implementation plans;
keep accepted decisions in [`docs/adr/`](../../docs/adr/), not this directory.
