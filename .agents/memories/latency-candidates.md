# Latency candidates and liveness-ops gates (Steps 5 + 7 research, 2026-09-14)

Findings from a primary-source check of the external report against the
HotStuff-1 v3 abstract (arXiv:2408.04728v3), SBFT (Gueta et al., DSN 2019)
and the SoK/BFT-evaluation secondary literature
([SoK 2303.11045](https://arxiv.org/pdf/2303.11045),
[BFT evaluation framework](https://web.cs.ucdavis.edu/~peisert/research/2021-ICPADS-BFT.pdf),
[Bedrock](https://www.usenix.org/system/files/nsdi24spring_prepub_amiri.pdf)).
Nothing here authorizes shipping a different protocol (ADR-002 governs).

## Verified terminology (report was right where it mattered)

- **Prefix speculation dilemma** — real (HotStuff-1 paper). Speculative
  confirmation is hard for streamlined protocols because they cannot halt
  and roll back like stable-leader protocols; HotStuff-1 is the first
  streamlined protocol to resolve it.
- **Slotting** — real, but the paper's term is plain *slotting*, not
  "Adaptive Slotting". Leaders dynamically drive as many decisions as
  network delays allow before view timers expire; it mitigates
  rationally-slow and malicious leaders. Do not adopt the report's name.

## Corrections to the report

- **"No-Gap Rule" / "permissive 2f+1 prefix votes"** — not found in the
  primary text we could verify; invented naming. If prefix speculation is
  ever designed, pull the actual invariants from HotStuff-1 §prefix
  sections, not from the report. Flagged: unverified.
- **HotStuff-1 latency claim** — the paper's precise claim: client
  confirmations one phase early (two network hops faster than HotStuff-2),
  with linear communication maintained *against faults*. The "3Δ" framing
  is the report's, not the paper's.
- **SBFT fast-path requirement**: the fast path needs *all* replicas
  participating — Bedrock records that a single faulty replica falls back
  to a slow path needing two more phases; the C-collector fast-path
  threshold is 3f+c+1 and the execution collector f+1. "Zero faults on fast
  path" is the right conclusion, imprecise phrasing.
- **O(n²) SBFT slow path** — confirmed by HotStuff's own comparison table
  (SBFT normal O(n), view change O(n²)).
- **"Reactor thread starvation / RCU locks / arena allocators"** — spec
  language from elsewhere. The Step-0/Step-4 measurements already
  attribute GlassChain's round: verification is 1.8 ms, codec <1 %, and the
  cost is the two vote-collection phases over the point-to-point mesh —
  wire fan-out, not thread/mutex pathology. Driver remediation here means
  dissemination shape (Step 6), not crypto worker pools.
- Governance gates (unweakened quorums, no reputation weighting,
  authenticated-signer-only telemetry) are already GlassChain policy —

see the plan §6 Step 7 wording; the report restates them correctly.

## Consequences for the plan

- Step 5 stays research-only. A HotStuff-1-style candidate needs: real
  driver measurements under faults (view changes, leader loss — the D7
  leader-quorum-loss scenario gives the first data point), a
  rollback/prefix-fork design doc, and an explicit ADR-002 decision before
  any client-visible speculative confirmation exists. Grounded first step:
  profile the driver under faults with the new phase timer — not invented
  reactor-work.
- Step 7 needs no research. What remains there is operational: multi-domain
  placement enforcement and authenticated-signer participation metrics are
  deployer evidence, not code. The report invents nothing to add there.
