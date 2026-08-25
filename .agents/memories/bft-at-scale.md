# BFT consensus at 200+ validators — research memory

**Learned:** 2026-08-24
**Source:** research recorded on [wayfinder issue #23](https://github.com/dbbvitor/GlassChain/issues/23) (part of #15)
**Branch:** `research/bft-at-scale` — see final report for commit.
**Scope:** production evidence (Q1), throughput evidence (Q2), Rust implementation paths for `ConsensusProvider` (Q3), and a brief comparison record for committee/async-BFT designs (Q4). Does **not** reopen ADR-002 (consensus family is settled: Tendermint/CometBFT-class, full participation, 200+ validators, seconds-level commits).

## Verification notes

- Cosmos Hub data is **live chain state** pulled 2026-08-24 from the public Cosmos Hub REST node `cosmos-rest.publicnode.com` (CometBFT/Cosmos-SDK endpoints). Not a secondary explorer summary.
- Malachite and tendermint-rs facts come from the projects' own READMEs and GitHub tags **fetched 2026-08-24**.
- Papers cited are the primary protocol papers/abstracts (arXiv / IACR ePrint / official project docs).
- **No public benchmark exists for CometBFT-class consensus at exactly 180–300 validators.** Everything in this file about that range is extrapolation from (a) the live 200-validator Cosmos Hub, (b) Malachite's published 100-validator experiments, or (c) the O(n²) vote-gossip model. Where a number is extrapolated or preliminary, it is labelled.

---

## Q1 — Production evidence at 100–300 voting validators

### Actual validator counts (primary, live)

- **Cosmos Hub runs 200 bonded validators today** (the network's `max_validators` param is `200`). Verified 2026-08-24:
  - Bonded validators returned: `200` — `GET /cosmos/staking/v1beta1/validators?status=BOND_STATUS_BONDED`
  - Staking params: `"max_validators": 200` — `GET /cosmos/staking/v1beta1/params`
  - So the issue's "~180" lead corresponds to an earlier cap; the current live cap is **200**. This is the largest continuously-running Tendermint/CometBFT production deployment and the single strongest data point for "200 validators works in production."
- The live set is dominated by professional/exchange/enterprise operators (Binance, Coinbase×2, Kraken, Upbit, Blockdaemon, Kiln, Figment, P2P.org, Everstake, stake.fish, Informal Systems, etc.), i.e. at this scale the "operational burden" is borne by institutional staking ops. This matches GlassChain's MSP-identified-institution validator model.

### Achieved block time / finality latency (primary, live)

- Cosmos Hub block time sampled **2026-08-24** from consecutive block timestamps:
  - 12-block span → ≈ **5.5 s/block**; 24-block span → ≈ **5.6 s/block**.
  - (Heights 32,651,573 / −12 / −24 via `GET /cosmos/base/tendermint/v1beta1/blocks/{h}`.)
- Finality model: Tendermint-class has **single-slot, deterministic finality at commit** — a block is final the moment its precommit quorum (⅔+) is gathered. Effective commit latency at Cosmos Hub ≈ one block interval (~5.5 s) plus gossip/propagation. This satisfies ADR-002's "final at the moment it commits; seconds-level is acceptable."

### What is known vs. not publicly benchmarked as N grows 180 → 300

**Known (primary grounds):**
- The Tendermint algorithm's vote step is **all-to-all**: every validator gossips its prevote/precommit to the rest, so message complexity scales **O(n²)** per round (Tendermint paper, arXiv:1807.04938, Buchman/Kwon/Milošević, 2018). This is the theoretical scaling bound and the reason sub-second block times harden as n grows.
- 200 validators is proven in long-running production (Cosmos Hub). Block propagation and vote aggregation, not transaction execution, are the dominant latency terms at this scale (CometBFT design; amplified by the O(n²) gossip). Malachite's own 100-validator experiments put finalization at ~780 ms with 1 MB blocks before propagation effects.

**Not publicly benchmarked / open unknowns:**
- No published end-to-end CometBFT benchmark at 300 validators. Cosmos Hub is *demand*-limited (low real TPS) and *not* a throughput or latency stress test, so its steady 5.5 s reflects typical operation, not a ceiling.
- The realistic failure-mode envelope (jailing/slashing on downtime, sentry/HSM ops, partition behavior) is operationally observed on Cosmos but not published as a benchmark as n → 300.
- CometBFT's README claims performance "up to 10k transactions per second (TPS)" — a headline, **no methodology, workload, or topology is given**, and it is not tied to any validator count. Do not cite it as a measured N=200 number.

**Confidence:** HIGH that 200 validators is production-proven with ~5.5–6 s blocks (live data + mature chain); **MEDIUM–LOW** on anything claimed for 300 validators or on "10k TPS" — absent a published benchmark, treat 300 validators and CometBFT TPS as unsized risks requiring our own testnet measurement.

---

## Q2 — Throughput evidence at 150–300 validators

There is **no single trustworthy TPS number** at 150–300 validators from a primary source. What exists, with test conditions:

| Source (primary) | Claim | Test conditions | Validator count | Caveat |
|---|---|---|---|---|
| **Malachite README** (informalsystems/malachite → now under Circle, fetched 2026-08-24) | Avg finalization latency **780 ms at 100 validators, 1 MB blocks**; up to **2.5 blocks/s** or **~13.5 MB/s ≈ 50,000 tps** | "Early experiments"; no full methodology published yet | **100** | Preliminary, **not externally audited**; TPS is *extrapolated from MB/s* ("depending on setup"). Not a 200–300-validator number. |
| **CometBFT README** (cometbft/cometbft) | "up to 10k TPS" | None disclosed | Unspecified | Headline only; do not quote without workload/topology context. |
| **Cosmos Hub (live)** | ~5.5–6 s blocks, low real on-chain TPS (demand-limited) | Mainnet | 200 | Demonstrates *capacity headroom* (throughput is not the constraint today), not a peak. |

### Likely bottlenecks at 150–300 validators (primary-grounded model)

1. **Vote gossip (dominant):** O(n²) prevote/precommit fan-out per round (Tendermint paper, arXiv:1807.04938). This sets the floor on commit latency and grows fastest with n.
2. **Block propagation:** every validator must receive and verify the full block; with sentry-node topologies this is tree/relay-delivered, so it adds to latency but is not O(n²).
3. **Signature handling/aggregation:** each validator signs its votes (ed25519); verifying ⅔+ = ~200–300 signatures per block on every node is CPU-cheap relative to gossip bandwidth but must be batched/parallelized; commit size grows linearly with n and is embedded in every block.

**Recommendation for the requirement owner:** do not write a bare "high throughput" number into §8.2. Size throughput from a testnet we run (see execution sketch, Q3/Q5): at ~200 validators, GB/s of block bandwidth and tens-of-k tx/s are plausible per the Malachite 100-validator extrapolation, but only a self-run benchmark at the real validator count can replace "unquantified."

---

## Q3 — Rust implementation paths behind `ConsensusProvider`

### Candidate comparison

| Path | What it is | Maturity | License | Maintenance | Per-height validator-set changes |
|---|---|---|---|---|---|
| **Malachite** (Informal Systems → now **Circle**, Apache-2.0) | State-of-the-art **Tendermint implementation in Rust** (split crates, `informalsystems-malachitebft-*` on crates.io) | **Pre-1.0 (latest tag v0.5.0)**; README: "alpha software… has not been externally audited" | Apache-2.0 | Active; team joined **Circle** to build L1 "Arc" | Yes — it follows Tendermint, where the validator set is committed per height from application validator updates (same model as CometBFT); supports the per-height change GlassChain would need if the set is ever bounded. |
| **tendermint-rs** (informalsystems/tendermint-rs, Apache-2.0) | **Client/light-client/ABCI toolkit**, NOT a consensus engine: `tendermint`, `tendermint-abci`, `tendermint-light-client`, `tendermint-rpc`, `tendermint-proto`, `tendermint-p2p` | Pre-1.0 but stable & widely used (Hermes/IBC relayer) | Apache-2.0 | Active | Not applicable — it has no engine; it does light-client verification and tracks validator-set changes for clients. |
| **CometBFT / Tendermint** (Go) | The battle-tested reference engine (Cosmos Hub at 200 validators) | Production | Apache-2.0 | Active (Cosmos Labs / Interchain) | Yes (ABCI `ValidatorUpdates` per EndBlock) — but it's **Go**, not a Rust library path; would be an out-of-process ABCI subprocess, breaking the all-Rust workspace. |
| **Other Rust Tendermint/HotStuff-class** | **None with serious production evidence at 200+.** Sui's Rust Narwhal/Bullshark is production but is *async DAG* class (Q4), not Tendermint-class. | — | — | — | — |

### Key distinctions (the crux of integrate vs. build)

- **tendermint-rs cannot be the consensus engine.** It has no consensus implementation; it is the *client/light-client/ABCI* companion layer. This is the most important correction for the execution plan: trying to "integrate tendermint-rs as ConsensusProvider" is a category error. Its real role is complementary — reuse its `tendermint` types, `tendermint-proto`, and `tendermint-light-client` for headers/verification once GlassChain speaks the Tendermint wire types.
- **Malachite is the only serious Tendermint-class Rust engine**, and it is *also* the natural successor/peer of CometBFT (same Informal Systems lineage), giving the closest integration fit: exact Tendermint algorithm → produces a real quorum certificate, which is exactly what ADR-002's seam wants (attestation set on `ConsensusProvider`).

### Recommendation: integrate vs. build

**Recommendation: integrate Malachite behind `ConsensusProvider` as the primary path, but gate it on a staged validation gate; keep "build-own" as the explicit fallback.**

Rationale:
- **Integrate (lean):** Malachite is purpose-built for this, is Apache-2.0, is actively maintained, implements the exact settled algorithm, and supports per-height validator-set changes. Building a Tendermint from scratch in Rust (fine — but it's the algorithm the very same engineers took years to harden, and GlassChain has no demonstrated consensus-engineering need to deviate).
- **Why it's not an unconditional integrate:** it is **alpha / unaudited / pre-1.0**, and its steward is now a **commercial company (Circle)** building a competing L1; an unconditional production dependency on it is a vendor-custody risk.

**Risk-reduced adoption plan:**
1. Implement `ConsensusProvider` with Malachite behind it, behind a default-off feature/flag (parallel to retained `PowConsensusProvider`, per ADR-002).
2. Run **our own testnet at 200 validators** (the real target count) and measure commit latency, TPS, and the O(n²) gossip ceiling — this is the only way to replace the extrapolated numbers.
3. Go/no-go gate: engage once Malachite is audited and crosses a stable (non-alpha) release, or if Circle's stewardship proves unreliable. If the gate fails → **build-own** a Tendermint-class engine on tendermint-rs types + Malachite's public spec (the seam makes this a contained swap, not a rewrite).

**Risks to record:**
- Malachite alpha/unaudited → consensus is the highest-stakes component; a consensus bug is catastrophic and hard to test out.
- Vendor custody under Circle → potential roadmap/forks divergence from the open CometBFT ecosystem and from GlassChain's needs; ability to self-maintain if abandoned.
- Stability of its API/ABCI (CometBFT ABCI 2.0) as it approaches 1.0 — expect breaking changes pre-1.0.
- No published benchmark at 200–300 validators → we must generate the evidence ourselves.

---

## Q4 — Brief comparison (why the settled Tendermint-class choice holds) — not candidates

Documented primary grounding for why committee/async designs were rejected in #16/ADR-002 (does not reopen it):

| Design | Primary source | Production scale | Why it doesn't fit GlassChain |
|---|---|---|---|
| **Algorand VRF sortition** | "Algorand" (Chen & Micali, arXiv:1607.01341) | Mainnet runs a small, curated relay/participation set (well under 200 full voting nodes) | Committee/**sortition sampling** per round — random subset actually votes, contradicting the **full-participation** requirement (ADR-002 decision 3). Open-membership problem GlassChain doesn't have. |
| **HoneyBadgerBFT (async)** | "The Honey Badger of BFT Protocols" (Miller et al., ePrint 2016/199) | Research: "over a hundred nodes" on WAN, tens of k tx/s | Fully-asynchronous BFT, fixed-known set; tested ~100 nodes, **no production at 200+**; liveness/MPC complexity not warranted. |
| **Narwhal & Tusk (async DAG)** | arXiv:2105.11827 | Research: WAN, >130k tx/s (Narwhal-HotStuff) / 160k tx/s (Tusk) at tens of nodes | Async DAG-class; **benchmarks at ~50 nodes**, far from 200; different participation/ordering model. |
| **Bullshark (sync/async DAG)** | arXiv:2201.05677 | Research: "125,000 tx/s … for a deployment of **50 parties**" | Same: research-scale at 50 nodes; DAG ordering ≠ single-slot Tendermint finality model. |
| **Hashgraph** | Swirlds whitepaper TR-2016-01; Hedera docs | Hedera mainnet council **≤ 39 nodes** | Governed council, far below 200; async/gossip-of-gossip model; no evidence at 200+. |

Bottom line for the record: the committee (sortition) and async-BFT families either (a) sacrifice full participation (Algorand's sampled committee), (b) have no production evidence at 200+ (all of them), or (c) solve an open-membership problem GlassChain does not have. The **partially-synchronous, full-participation, single-slot-finality Tendermint-class choice in ADR-002 is the only one with production evidence at ~200 validators** (Cosmos Hub). Confirmed, not reopened.

---

## Source list (primary)

- Cosmos Hub live REST (fetched 2026-08-24): validators list, staking params, block timestamps — `cosmos-rest.publicnode.com` (Cosmos-SDK / CometBFT endpoints).
- CometBFT README + docs: `github.com/cometbft/cometbft` — "up to 10k TPS"; ABCI 2.0; maintained by Cosmos Labs (Interchain).
- Malachite README + GitHub tags (informalsystems/malachite, now under Circle/Arc): alpha/unaudited; 780 ms @ 100 validators, 1 MB blocks; ~13.5 MB/s ≈ 50k tps; Apache-2.0; v0.5.0.
- tendermint-rs README (informalsystems/tendermint-rs): libraries only (no engine); Apache-2.0; Tendermint Core v0.34.21 compatibility.
- Tendermint paper — Buchman, Kwon, Milošević, "The latest gossip on BFT consensus," arXiv:1807.04938 (2018/2019): O(n²) vote gossip; single-slot finality.
- Algorand — Chen & Micali, arXiv:1607.01341 (2016/2017): VRF sortition/committee.
- HoneyBadgerBFT — Miller, Xia, Croman, Shi, Song, IACR ePrint 2016/199 (2016): async BFT, >100 nodes research scale.
- Narwhal & Tusk — Danezis et al., arXiv:2105.11827 (2022): DAG async BFT, WAN throughput, tens of nodes.
- Bullshark — Spiegelman et al., arXiv:2201.05677 (2022): "125k tx/s … 50 parties."
- Hashgraph — Swirlds whitepaper TR-2016-01 (Baird, 2016); Hedera docs (council ≤ 39 nodes).
- ADR-002 (`GlassChain/.agents/plans/adr-002-consensus-finality.md`, resolved 2026-08-20): the settled family decision this research supports.

## Confidence / unknowns summary

- **HIGH:** Cosmos Hub = 200 bonded validators, ~5.5–6 s blocks (live data); Malachite alpha/unaudited/pre-1.0 under Circle; tendermint-rs is a client toolkit, not an engine; no other serious Rust Tendermint-class path.
- **MEDIUM:** Malachite's ~50k tps is an extrapolation at 100 validators; valid as a ceiling *only* with that context.
- **UNKNOWN / blocker:** **no public benchmark at 180–300 validators for any Tendermint-class engine.** CometBFT "10k TPS" has no disclosed methodology. GlassChain must run its own 200-validator testnet to produce the §8.2 sizing number — this is the explicit blocker to replacing "high throughput" with a measured figure.

## Follow-up review — 2026-08-25

The BFT scaling review was accepted as a guardrail, not as a reopening of ADR-002.
The 70M-member design uses a bounded validator set and authenticated light-client
and verifying-member roles; it does not submit 70M raw events directly to the BFT
core. The compact consensus workload is approved public canonical records and
commitment envelopes. Private commercial data, raw evidence, and unbatched
high-frequency telemetry stay in PDC/off-chain paths.

The global chain is therefore not commitment-only: public custody edges, permitted
identity fields, lot/batch identifiers, timestamps, recalls, NF-e hashes,
certification/audit anchors, public write sets, and state commitments remain
first-class records. SCITT, ZK validiums, data-availability committees,
KERI/DID replacement, IPFS, execution sharding, and an aBFT settlement core are
not v1 requirements. Merkle commitments and MSP signatures are sufficient for
the current tamper-evidence requirement.

Malachite remains a staged, default-off `ConsensusProvider` candidate. Its
pre-1.0 maturity, stewardship, licensing, audit, and GlassChain 200/300-validator
compact-workload testnet remain adoption gates. `tendermint-rs` is complementary
type and light-client tooling, not a consensus engine. The capability and
historical-versioning policy that carries these boundaries is recorded in
[ADR-010](../plans/adr-010-capability-versioning-policy.md).
