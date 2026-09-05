# Plans

Current planning map, reviewed 2026-09-05. These are objectives and acceptance
gates, not claims that implementation or regulatory approval is complete.

| Plan | Concluded / available | Pending frontier |
|---|---|---|
| [Requirements alignment](requirements-alignment.md) | Schema, write sets, workflows, PDC and analytics mechanisms shipped; decisions D1–D6 settled | Deployment authorization/recovery, integrations, governed feedback; FL remains deferred |
| [Source-comment debt](deferred-code-debt.md) | Seven markers inventoried; TCP fault proxy exists; stale consensus quotes corrected | D1–D6 acceptance work; D7 WAN extension before optional simulator migration |
| [Zero trust](zero-trust.md) | TLS/TOFU, optional verifier, CRLs, endorsement and staged BFT mechanisms exist | Consensus context/history verification, bounded receipts, fail-closed PDC and identity lifecycle |
| [Performance](performance.md) | Local 100/200 BFT measurements, base64/BLS, drop counters and scheduling fixes | 300 gate, safety-first WAN/resource tests, admission/mempool costs, gated fast paths |
| [Post-quantum readiness](post-quantum.md) | Algorithm discriminants shipped; provider/evidence research recorded | Negotiated hybrid TLS tests/selection; migration and archive profile decisions |
| [Browser demo](gui-demo-benchmark.md) | Web-app direction replaces desktop gpui; WebGPU evaluated as optional acceleration | Browser/bridge spike, headless runner, accessible UI, measured renderer choice; no implementation yet |

See [the latest report assessment](../memories/external-review-verdicts.md) for
accepted, corrected and deferred literature suggestions. Source debt lives once
in its inventory; other plans reference its D1–D7 identifiers.

[Artifact conventions](../README.md): retire completed implementation plans;
keep accepted decisions in [`docs/adr/`](../../docs/adr/), not this directory.
