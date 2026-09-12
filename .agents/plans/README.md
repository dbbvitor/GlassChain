# Plans

Current planning map, reviewed 2026-09-12. Active priority sequence:
**Frontier C (Consensus Capacity / BLST Backend) → Frontier B (RBAC & Operational Tail) → Frontier D (Browser Demo)**.
Frontier A concluded: `EquivocationProof` now carries both dual-signed votes and verifies through the #95 context envelope.

| Plan | Concluded / available | Pending frontier |
|---|---|---|
| [Requirements alignment](requirements-alignment.md) | Schema, write sets, workflows, PDC, analytics mechanisms shipped; decisions D1–D6 settled | Deployment authorization/recovery, integrations, governed feedback; FL remains deferred |
| [Source-comment debt](deferred-code-debt.md) | All seven markers (D1–D7) settled and benchmarked in code (#106–#109, #114–#115) | Work completed; maintain benchmarks and monitor regression |
| [Zero trust](zero-trust.md) | Consensus context-authenticated votes (#95, #99), live receipt journal (#96), historical QC verification on sync/restart (#97, PR #116), deadlines/queues (#98), fail-closed PDC (#86), possession proofs (#110), durable TOFU (#88), CRLs (ADR-013), cert-bound principals (#87), dual-sign `EquivocationProof` (Frontier A concluded) | OCSP stapling/verification, deployment RBAC, and replica backup retention (Frontier B) |
| [Performance](performance.md) | Local 100/200 BFT measurements, drop counters, hybrid TLS (#105), WAN proxy (#108), D3 admission bench (#106), read-path memory baseline (#107) | **ACTIVE FRONTIER (C):** Step 3 BLS verification / pairing backend experiment (`blst` / issue #85) to unblock 300-validator gate; gated fast paths |
| [Post-quantum readiness](post-quantum.md) | Algorithm discriminants shipped; negotiated X25519MLKEM768 hybrid TLS shipped (#105) behind `pq-tls` | Migration and long-term archive profile decisions |
| [Browser demo](gui-demo-benchmark.md) | Web-app direction replaces desktop gpui; Canvas2D baseline + WebGPU evaluated | Browser/bridge spike, headless runner, accessible UI, measured renderer choice (Frontier D, queued after C and B) |

See [the latest report assessment](../memories/external-review-verdicts.md) for
accepted, corrected and deferred literature suggestions. Source debt lives once
in its inventory; other plans reference its D1–D7 identifiers.

[Artifact conventions](../README.md): retire completed implementation plans;
keep accepted decisions in [`docs/adr/`](../../docs/adr/), not this directory.
