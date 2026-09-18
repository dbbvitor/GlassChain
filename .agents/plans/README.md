# Plans

Current planning map, reviewed 2026-09-14. Frontier C (Consensus Capacity / BLST
Backend) **concluded**: `bls-signatures` now runs the audited `blst` backend
(ADR-015) with the sum-of-keys verify, and the 300-validator finality gate
passes (p50 3 996 ms, exact quorum every round — `docs/benchmarks/consensus-capacity.md`).
**Frontier B concluded 2026-09-14** (ADR-017 + its code: OCSP stapling on the
Hello, `AdminGate`-gated channel RPCs, `backup-scrub` CLI, per-Hello org
reauthorization). Next in the priority
sequence: **Frontier D (Browser Demo)**. Frontier A
concluded earlier: `EquivocationProof` carries
both dual-signed votes and verifies through the #95 context envelope.

| Plan | Concluded / available | Pending frontier |
|---|---|---|
| [OpenSSF compliance implementation](openssf-compliance-implementation.md) | Unblocked OpenSSF tickets #125–#128 implemented: per-file SPDX headers, governance docs (CONTRIBUTING/GOVERNANCE/CODE_OF_CONDUCT), SECURITY.md + threat model, coverage gate + fuzz harnesses, reproducible-build + release workflows; tickets resolved and closed 2026-09-15 | #129 — synthesize `docs/compliance/openssf-best-practices.md` + `.agents/plans/openssf-compliance.md` |
| [Requirements alignment](requirements-alignment.md) | Schema, write sets, workflows, PDC, analytics mechanisms shipped; decisions D1–D6 settled | Deployment authorization/recovery, integrations, governed feedback; FL remains deferred |
| [Source-comment debt](deferred-code-debt.md) | All seven markers (D1–D7) settled and benchmarked in code (#106–#109, #114–#115) | Work completed; maintain benchmarks and monitor regression |
| [Zero trust](zero-trust.md) | Consensus context-authenticated votes (#95, #99), live receipt journal (#96), historical QC verification on sync/restart (#97, PR #116), deadlines/queues (#98), fail-closed PDC (#86), possession proofs (#110), durable TOFU (#88), CRLs (ADR-013), cert-bound principals (#87), dual-sign `EquivocationProof` (Frontier A concluded); **Frontier B concluded (ADR-017 + code: Hello-carried OCSP staple verified locally, `AdminGate` channel RPCs, `backup-scrub`, per-Hello reauthorization)** | Residual plan shipped 2026-09-14 ([zero-trust-residual](zero-trust-residual.md)): identity/key custody (`--identity-file`, ADR-018), `channel-admin` CLI, `reload-trust-store` REPL hot-reload, durable equivocation evidence; #74 + delegated responders parked by decision |
| [Performance](performance.md) | Local 100/200 BFT measurements, drop counters, hybrid TLS (#105), WAN proxy (#108), D3 admission bench (#106), read-path memory baseline (#107), **BLS backend on audited `blst` (ADR-015, #121 merged); Step 0 concluded** (per-phase round timing, leader-quorum-loss + bandwidth WAN scenarios); durability decided (ADR-016); **Step 1 codec profiled — JSON stays the wire**; **Step 3 D3 incremental index — admission flat at ~0.19 ms**; **Step 6 installment** (bounded 8 000-tx pool, stats, handshake re-audit)  Step 6 batching (evidence-backed: ~1.5 MB blocks fail replication) then sweeps under hardware budgets; Step 7 is deployer evidence |
| [Latency opportunities](latency-opportunities.md) | Implemented and gated 2026-09-14: concurrent vote verify, priority lanes, height-bounded catch-up (wire `/7`; 1 004-block bootstrap 272 ms), reconnect backoff — 300-gate p50 4 612 → 4 117 ms; Step 5 fault profile + Step 6 saturation study recorded; gossip relay measured and **reverted** (negative result) | Speculation (#6) frozen pending the ADR-002 question |
| [Post-quantum readiness](post-quantum.md) | Algorithm discriminants shipped; negotiated X25519MLKEM768 hybrid TLS shipped (#105) behind `pq-tls` (retroactively grounded by ADR-015) | Migration and long-term archive profile decisions |
| [Browser demo](gui-demo-benchmark.md) | Web-app direction replaces desktop gpui; steps 1–3 + zero-trust slice shipped 2026-09-16/17 (axum bridge, 15-node multi-company runner with two evil companies attacking on real gates, per-member own-node views + visibility matrix, Canvas2D UI, `docs/demo.md`) | Plan steps 4–5 (fault scenarios, Canvas2D/WebGPU measured comparison), browser-smoke CI |

See [the latest report assessment](../memories/external-review-verdicts.md) for
accepted, corrected and deferred literature suggestions. Source debt lives once
in its inventory; other plans reference its D1–D7 identifiers.

[Artifact conventions](../README.md): retire completed implementation plans;
keep accepted decisions in [`docs/adr/`](../../docs/adr/), not this directory.
