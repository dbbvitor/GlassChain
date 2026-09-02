# Handoff — GlassChain

**Written:** 2026-09-02
**Branch:** `main` · **Commit:** `fed76c7` · working tree **clean**

---

## Mission

Bring GlassChain — a working supply-chain ledger — into alignment with the
**Hybrid Distributed Inventory System** requirements: a Fabric/Corda/Thor hybrid
combining permissioned identity and endorsement (Fabric), workflow-first
modelling and selective disclosure (Corda), and an event/subscription surface
(Thor). Multi-quarter programme; advance it one well-scoped, verified step at a
time.

## Read these first, in this order

| # | File | Why |
|---|---|---|
| 1 | `AGENTS.md` | **Canonical project rules.** Invariants, conventions, commands. Non-negotiable — this handoff does not restate them. |
| 2 | `.agents/plans/requirements-alignment.md` | The programme: 26-requirement traceability matrix + 7-stage roadmap |
| 3 | `.agents/memories/debt-gap-handoff.md` | What #34–#49 actually shipped, plus the architecture facts and review pitfalls they established. **The densest useful file in the repo.** |
| 4 | `.agents/memories/external-review-verdicts.md` | Which external-review claims are load-bearing, which are stale, and the verified residual gaps |
| 5 | [`docs/adr/`](../docs/adr/) | Nine accepted ADRs. Read the one covering your area before designing. Moved out of `.agents/plans/` on 2026-09-02 — they are shipped documentation, not agent scratch. |
| 6 | `.agents/memories/participation-model.md` | Read before touching the membership ladder. Four senses of "validate"; the four wrong answers every design pass reaches for first |

## Verified state — trust this table, not prose elsewhere

Measured against the working tree at `fed76c7` on 2026-09-02.

| Thing | Reality |
|---|---|
| Workspace | **12 crates**, 93 `.rs` files, ~37.3k lines |
| `cargo fmt --all --check` | ✅ passes with zero files needing formatting. The old "there is a known formatting backlog" claim is **retired**; `AGENTS.md`'s "format only the files you touched" convention still stands |
| `cargo check --workspace --all-targets --all-features --locked` | ✅ passes |
| `cargo test --workspace --all-targets --all-features --locked` | ✅ **546 tests** (537 passed, 9 ignored, 0 failed) across 29 harnesses + 10 doctest harnesses |
| `cargo clippy ... -- -D warnings` | ✅ **zero diagnostics**. The `.github/clippy-baseline.txt` no-new-warnings gate is **retired and deleted** — clippy is clean, hold it there |
| Programme tickets | #16–#49 **all closed**. Map #15 destination reached |
| Canonical schema v1, VM write sets, capabilities, endorsement engine + enforcement, quorum certificate, staged BFT, PDCs on the wire + reconciliation, analytics read path, workflow flows | ✅ **shipped** — see `debt-gap-handoff.md` for the per-ticket commit and detail |

Run both feature configs when touching consensus: default **and**
`--all-features` (the `bft` feature gates real code paths).

## Residual verified gaps

Not fog — each confirmed against the code on 2026-09-02, and each now filed.
Detail and the reason each was left alone in
`.agents/memories/external-review-verdicts.md`.

1. **Certificate verification is inert in production** ([#57](https://github.com/dbbvitor/GlassChain/issues/57)). `glasschain-node`
   builds an `Organization` (root CA in hand) but never calls
   `Node::set_cert_verifier`, so `cert_verifier` is `None` at runtime. The #47
   PDC org gate is `verification_required = cert_verifier.is_some()` — so the
   private-payload path **fails open to the self-asserted `Hello` org** outside
   tests. Blocked on a real decision, not a missing line: each node self-issues
   its own org CA, so a single-org verifier would reject every cross-org peer.
   Needs a federation trust-store / CA-distribution decision first.
2. **No certificate revocation** ([#58](https://github.com/dbbvitor/GlassChain/issues/58)). No CRL or OCSP anywhere
   (`cert_verifier.rs`), and chains are single-hop (no intermediates).
3. **No endorsement provider wired at node startup** ([#59](https://github.com/dbbvitor/GlassChain/issues/59)). The engine is inert
   outside tests until a node/CLI flag or network default lands.
4. **`record.signatures` / `CapabilityActivation.signatures` are count-only**
   ([#60](https://github.com/dbbvitor/GlassChain/issues/60)).
   The endorsement engine verifies `Transaction.endorsements`, never these. No
   ticket will fix it incidentally — binding them needs its own decision.
5. **`madsim_chaos.rs` TCP-level fault injection.** Long-standing `TODO`;
   partitions are simulated at the application layer. Not filed — test-harness
   fidelity, not a product gap.
6. **The peer wire protocol is JSON, and it costs ~5× on the hot path**
   ([#62](https://github.com/dbbvitor/GlassChain/issues/62)). `serde_json`
   renders `Vec<u8>` as decimal arrays, so `Attestation`'s 96 bytes of key and
   signature become ~393. Certificates are **not persisted with blocks yet**, so
   a `serde_bytes` attribute plus a wire-version bump fixes it now, and gets
   expensive once the format is in committed history and light-client proofs.
7. **There is no BFT finality measurement anywhere**
   ([#62](https://github.com/dbbvitor/GlassChain/issues/62)). The committed
   capacity gate measures *PoW mine latency* on the dev engine with a degenerate
   certificate and, in its own words, "no cross-validator vote rounds exist to
   measure." Do not cite its p50 33–36 ms as BFT latency — a previous revision of
   the performance plan did exactly that. Producing a real number is the blocking
   first step of `.agents/plans/performance.md`.

Also open, not gaps: [#61](https://github.com/dbbvitor/GlassChain/issues/61)
(`glasschain-demo`, a gpui visual demo and benchmark harness — plan in
`.agents/plans/gui-demo-benchmark.md`).

## Already rejected — do not re-propose

| Proposal | Why it was rejected |
|---|---|
| Keep PoW (ADR-002 Option A) | Probabilistic finality; §8.2's "immediate" is literal |
| Thor-style finality gadget over production (ADR-002 Option D) | Finality lags production by a quorum round |
| **DAG-BFT (Narwhal/Bullshark/Mysticeti) as a consensus-family swap** | Orthogonal, not a family change: Narwhal is a *mempool* layer designed to compose with partial-sync BFT ordering. No reusable standalone Rust DAG-BFT crate exists (Sui's is monorepo-embedded; standalone Narwhal repos archived). Sits behind `ConsensusProvider` — carried as step 6 of `.agents/plans/performance.md` under a **measured trigger**, not rejected |
| **HotStuff-1 / SBFT fast paths as a family change** | In-family speculative latency optimizations of the same partially-synchronous quorum BFT. ADR-preserving, and a live candidate (step 5) now that best-in-class latency is a stated goal — sequenced behind measurement and the wire encoding, because a fast path saves one hop while JSON inflates every message on every hop |
| **Raising the ~300-validator ceiling by protocol optimization** | A category error, settled in [#62](https://github.com/dbbvitor/GlassChain/issues/62) and `docs/consensus.md` §10.2. No production system runs deterministic per-round ⅔ finality with all `n` participating beyond ~209; every larger network changed the participation model, decoupled finality, or weakened it — all three already rejected by ADR-002. **But `n` is the wrong axis to sell on**: compete on latency, throughput, and ladder reach. 300 mutually-distrusting validators is already best-in-class for a permissioned ledger (Fabric orders on crash-fault-only Raft) |
| **zk-X509 / ZK certificate-chain proofs** | Solves a *public-chain* metadata-leak problem GlassChain does not have — no certificate is ever committed to the chain. Also a single-author unreviewed preprint, testnet-only |
| Replace Wasmtime with an EVM (`revm`) | Discards tested work to satisfy a SHOULD; re-introduces the §3.2 conflict |
| Delete or feature-gate out `LibP2pNode` | ADR-003 makes it required substrate for private-data dissemination |
| Endorsement check inside `glasschain-core`'s commit path | Requires `core → identity` — a dependency cycle |
| Drive the watcher from Sled write handles over `mpsc` | Breaks the replay invariant; nodes silently diverge after sync. The watcher **must** be fed committed ledger events only |
| Extending `SCHEMA_V1` with rfq/quote/acceptance/dispute/settlement families | Those chain steps are flow *states*, record-less by design |
