# Integration Completion Plan

**Status:** superseded in scope — see [`requirements-alignment.md`](requirements-alignment.md)
**Date:** 2026-08-18
**Baseline:** `eea0f325877c42069908e8a778cf798e898f2299` (branch `rework`)
**Supersedes:** an externally-authored plan whose premises did not match this repo (see Appendix).

> **Note:** this plan remains accurate about the repo's state, but it assumed the
> goal was "finish wiring what exists". The full requirements list makes it
> **Stage 1 of a larger program**, and reverses its Phase 3 recommendation
> (libp2p should be wired, not removed — selective disclosure needs it).

## Goal

Close the real gap between GlassChain's built-and-tested crates and a node that
actually uses them — without breaking the architectural invariants in `AGENTS.md`
(no dependency cycles, provider-trait seams, replayable automation state).

---

## Verified starting state

Checked against the working tree, not assumed. `✓` = wired and exercised.

| Component | Reality |
|---|---|
| Node orchestration | ✓ `glasschain-node/src/main.rs` (639 lines) already constructs `SledStorageProvider`, `WasmExecutionProvider`, `Organization`, `Node`, and `GlasschainServer` |
| Autonomous watcher → VM → signed tx | ✓ Complete. `node.rs:591` `set_executor`; `node.rs:951-996` collects, signs, persists watcher orders; `watcher.rs` runs WASM approve/deny |
| Watcher state replay | ✓ `node.rs:540-580` — storage snapshot with chain-replay fallback |
| gRPC auth interceptor | ✓ Wired. `server.rs:634-642` attaches `MspAuthInterceptor` to all three services |
| E2E recall simulation | ✓ Exists: `chaos_tests.rs:266 test_recall_simulation_manufacturer_to_pharmacy` |
| Partition / crash-rejoin tests | ✓ Exist in `madsim_chaos.rs` (application-layer only — see Phase 5) |
| Endorsement enforcement | ✗ `server.rs:553` returns `"endorsement engine not yet wired to RPC layer"` |
| Indexer / provenance / flattener in RPC | ✗ `server.rs:387` — `ServerState` holds only `node`; the comment says wiring is deferred |
| Channel isolation | ✗ `Channel` used only in its own crate's tests |
| Certificate signature verification | ✓ **Implemented** — `rustls-webpki` chain check, `Full` by default |
| `CertChainVerifier` on the node | ✗ `None` in all four `Node` constructors |
| `LibP2pNode` | ✗ ~700 lines, unit-tested, **unreachable from any binary** |
| CI | ✓ `.github/workflows/ci.yml` — check, test, clippy no-new-warnings gate |

**The headline finding (at the time of writing):** the two components most
confidently described as delivered strengths — X.509 verification and the libp2p
swarm — were respectively *unimplemented at the crypto layer* and *unreachable*.
The first has since been fixed. The second is still unreachable, and ADR-003 has
since made it required infrastructure rather than a deletion candidate.

---

## Phase 0 — CI safety net ✅ **DONE**

*Rationale: five phases of cross-crate integration with no automated verification is
exactly how the current doc/reality drift happened. This is the cheapest phase and
it de-risks every phase after it.*

- [x] `.github/workflows/ci.yml` — `cargo check --workspace --all-targets`,
      `cargo test --workspace`, `cargo clippy --workspace --all-targets` on the
      pinned 1.95 toolchain, with a cargo cache. Installs `protoc` (not vendored)
      and `rustup component add clippy` (not declared by `rust-toolchain.toml`).
- [x] Record the current clippy warning count as a committed baseline
      (`.github/clippy-baseline.txt`, 2163) so "do not add new warnings" becomes
      mechanically checkable rather than advisory.
- [x] Do **not** add `cargo fmt --check` yet — the repo has a known formatting
      backlog (`AGENTS.md`). Deliberately omitted, with a comment saying why.

**Validation:** verified locally to pass at baseline and to fail on an injected
warning. **Size:** S.

---

## Phase 1 — Close the security gaps presented as features

*Rationale: `README.md` advertised "certificate-fingerprint verification" while the
verifier that would enforce chain trust never checked a signature and was off by
default. 1.1 has closed that; 1.2 and 1.3 remain, and they are what still keep the
verifier from constraining anything at runtime.*

- [x] **1.1** ~~Implement the TODO at `glasschain-identity/src/cert_verifier.rs:264`.~~
      **Done.** `CertChainVerifier` now builds a one-hop path to the org Root CA via
      `rustls-webpki` and verifies the CA signature over the peer certificate.
      `VerificationLevel::Full` is the default; `Structural` is retained for tests
      and is explicitly documented as non-security-bearing.
- [x] **1.2** TOFU-only is the deliberate default for `Node.cert_verifier` (currently
      `None` at `node.rs:269,338,369,404`). The peer handshake remains fingerprint-
      pinned and address-bound; do not enable a local-organization CA verifier until
      a shared or multi-organization trust model is chosen. `AGENTS.md` and `README.md`
      document this decision.
- [ ] **1.3** Audit `GlasschainServer` auth defaults — `with_auth(require_auth: false)`
      yields a permissive interceptor. Document which deployment path gets strict mode.

**Validation:** new tests asserting a tampered / wrong-signer certificate is
*rejected*. This is the acceptance criterion — a passing parse is not enough.
**Size:** M. **Blocks:** Phase 2, Phase 4.

---

## Phase 2 — Endorsement enforcement, without a dependency cycle

*Rationale: the gap is real, but it cannot be implemented where the superseded plan
put it. `glasschain-identity` depends on `glasschain-core`, so core cannot call
`endorsement.rs`. That edge would be a cycle and is forbidden by `AGENTS.md`.*

- [ ] **2.1** Define `EndorsementProvider` in `glasschain-core/src/providers.rs`
      (input: `&Transaction` + collected signatures; output: satisfied / rejected + reason).
- [ ] **2.2** Implement it for `EndorsementEngine` in `glasschain-identity`.
- [ ] **2.3** Inject at the node and invoke in the block-commit path in
      `glasschain-network/src/node.rs` — which *already* depends on identity, so no
      new edge is introduced.
- [ ] **2.4** Replace the `server.rs:553` stub with a real evaluation result.

**Invariant:** `glasschain-core` must not gain a dependency on `glasschain-identity`.
Verify with `cargo tree -p glasschain-core -i glasschain-identity` returning nothing.
**Validation:** N-of-M policy test — a block with insufficient org signatures is
rejected at commit, not at ingress only. **Size:** M.

---

## Phase 3 — Resolve the libp2p fork in the road

*Rationale: private-data dissemination needs addressed point-to-point messaging
and peer discovery, which is the intended role of the gossipsub + Kademlia swarm.
The current TCP path remains the active transport until the privacy model is
confirmed and libp2p is wired with parity tests.*

- [x] Mark `LibP2pNode` experimental and currently unwired in `libp2p_swarm.rs`,
      `PLUGIN_KIT.md`, and `README.md`. Do not delete or feature-gate it out.
- [ ] After D3, wire it behind `--transport libp2p` in `glasschain-node`, with an
      integration test at parity with the TCP path.

**Do not** run both transports silently. **Size:** L for the wiring phase.

---

## Phase 4 — Analytics read path (in-tree, no external database)

- [ ] **4.1** Wire `ProvenanceIndex` + `AnalyticalFlattener` into `ServerState`,
      resolving the deferral note at `glasschain-rpc/src/server.rs:387`. Note
      `glasschain-network/src/node.rs:12` already uses `InMemoryIndexer` and
      `InMemoryEventBus`, so the indexer is wired at the *node* level — it is the
      provenance/flattener read path through RPC that is missing.
- [ ] **4.2** Back `QueryAssetHistory` with the provenance index; add a lineage
      query keyed by GTIN / batch / serial.
- [ ] **4.3** Convert `event_bus.rs` to bounded channels with an explicit
      backpressure policy (drop-oldest vs. block) and a test that fills the buffer.

**Explicitly out of scope: an in-tree PostgreSQL/ClickHouse writer.**
`PLUGIN_KIT.md` §5 already ships the SQLx adapter as a *recipe for downstream
implementers*, and `IndexerProvider` / `EventBusProvider` exist precisely so the
database lives outside this workspace. Adding a live DB pulls in a service
dependency, container fixtures, and integration-test infrastructure into a repo
that as of Phase 0 has only just acquired CI. Keep the seam; publish an example
adapter in a separate repo if one is needed.

Any latency target (e.g. "<50 ms lineage query") needs a criterion bench to be
meaningful — this repo already uses criterion in `glasschain-vm` and
`glasschain-contracts`. Add one or drop the number.
**Size:** M.

---

## Phase 5 — Channel isolation

- [ ] Thread the authenticated caller's channel ID from the gRPC interceptor
      (Phase 1.3) into the storage key namespace, so `StorageProvider` reads and
      writes are scoped per channel.
- [ ] Test: a caller authorized for channel A cannot read channel B's state.

Sequenced after Phase 2 because both need the same authenticated-identity context
plumbed into the commit path. **Size:** M.

---

## Phase 6 — Test hardening and CLI

- [ ] **6.1** Resolve `madsim_chaos.rs:571`. The chaos tests today simulate
      partitions at the *application layer*; the file's own TODO documents the
      migration to `madsim-tokio` for real TCP-level fault injection. Until then,
      claims of verified behaviour "under node churn" are overstated.
- [ ] **6.2** Add `asset-trace` to `glasschain-cli`. It does not exist — the CLI has
      exactly three subcommands (`identity-gen`, `contract-deploy`, `ledger-inspect`).
      This is new work, not polish.
- [ ] **6.3** *Extend*, don't rebuild, `test_recall_simulation_manufacturer_to_pharmacy`
      — add the third distinct organization and the endorsement assertion from Phase 2.

**Size:** M.

---

## Sequencing

```
Phase 0 (CI) ─┬─> Phase 1 (crypto/security) ─┬─> Phase 2 (endorsement) ──> Phase 5 (channels)
              │                              └─> Phase 4 (analytics)
              └─> Phase 3 (libp2p decision)  ────────────────────────────> Phase 6 (tests/CLI)
```

Phase 0 first and alone. Phases 1 and 3 can run in parallel — different crates,
disjoint write sets.

## Validation for every phase

```bash
cargo check --workspace --all-targets
cargo test --workspace          # 209 tests baseline — must not regress
cargo clippy --workspace --all-targets   # 2163 warnings — must not rise
```

Plus, per `AGENTS.md`: update `README.md` when the gRPC surface or CLI changes, and
`PLUGIN_KIT.md` when a provider trait changes (Phase 2 adds one).

---

## Appendix — corrections to the superseded plan

| Claim | Verified reality |
|---|---|
| Baseline `afd2b98f0c7…` | Not an object in this repository (`git cat-file` fails). HEAD is `eea0f32` |
| "10-crate workspace" | 11 crates |
| "`glasschain-node`: basic skeleton" | 639 lines already wiring storage, VM, identity, network, and gRPC |
| "Watcher must be bridged to the VM to sign and broadcast" | Already done — `node.rs:591`, `node.rs:951-996`, plus stress tests |
| "Needs an E2E recall integration test" | `chaos_tests.rs:266` already implements one |
| "`auth.rs` needs wiring" | Already wired at `server.rs:634-642` (it validates ed25519 MSP tokens, not X.509 — the X.509 path is the actual gap) |
| "X.509 verifiers" listed as a delivered strength | `cert_verifier.rs:264` did no signature verification, and is `None` by default — **signature verification has since been implemented**; the `None` default remains |
| "libp2p fully implemented" listed as a strength | Unreachable from any binary |

### Rejected proposals and why

1. **Endorsement check inside `glasschain-core`'s commit path** — would require
   `core → identity`, a dependency cycle. Replaced by Phase 2's provider trait.
2. **Driving the watcher from `Sled` write handles over `mpsc`** — the watcher is
   deliberately driven by *committed ledger events* and rebuilt by chain replay
   (`node.rs:540-580`). Feeding it raw storage writes makes its state
   non-deterministic and non-replayable, so nodes would silently diverge after a
   sync or chain replacement. It also adds a needless `contracts → storage` edge.
3. **Giving the WASM VM access to the node's MSP signing key** — a key-exfiltration
   hole. Untrusted, gas-metered guest code must never hold signing material. The
   current split is correct: WASM returns approve/deny, and the *node* signs outside
   the sandbox (`node.rs:962-989`). This would be a security regression, not a feature.
4. **In-tree PostgreSQL writer** — see Phase 4.
5. **No mention of CI** — the single highest-leverage missing piece.
