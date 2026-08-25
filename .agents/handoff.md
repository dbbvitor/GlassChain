# Handoff — GlassChain requirements-alignment programme

**Written:** 2026-08-25
**For:** the next agent session
**Branch:** `rework` · **Last commit:** `eea0f32` · all programme artifacts are **uncommitted**

---

## Mission

Bring GlassChain — a working supply-chain ledger (PoW + WASM, 11 crates, ~16k lines
of Rust) — into alignment with the **Hybrid Distributed Inventory System**
requirements: a Fabric/Corda/Thor hybrid combining permissioned identity and
endorsement (Fabric), workflow-first modelling and selective disclosure (Corda), and
an event/subscription surface with fee delegation (Thor).

The requirements describe roughly **three times the scope of what exists**. This is a
multi-quarter programme, not a task. Your job is to advance it one well-scoped,
verified step at a time.

---

## Read these first, in this order

| # | File | Why |
|---|---|---|
| 1 | `AGENTS.md` | **Canonical project rules.** Invariants, conventions, commands. Non-negotiable. |
| 2 | `.agents/plans/requirements-alignment.md` | The programme: 26-requirement traceability matrix + 7-stage roadmap |
| 3 | `.agents/plans/adr-002-consensus-finality.md` | **Resolved 2026-08-20** — BFT, Tendermint-class; the validator ladder lives in ADR-004 |
| 4 | `.agents/plans/adr-003-privacy-model.md` | **Resolved 2026-08-20** — Option A (private data collections); boundary/auditor/purge/reconciliation recorded |
| 5 | `.agents/memories/reference-architectures.md` | Fabric/Corda/Thor structural study + Repomix pack recipes |
| 6 | `.agents/plans/integration-completion.md` | Phase-level wiring plan; superseded in *scope*, still accurate on *state* |
| 7 | `.agents/plans/adr-001-execution-layer.md` | Resolved. Read for context on why WASM stayed |
| 8 | `.agents/plans/adr-004-scale-topology.md` | Resolved 2026-08-20 — single chain, off-chain state commitments, light-client ladder |
| 9 | `.agents/plans/adr-005-certification-and-audit.md` | Accepted 2026-08-20 — signed append-only certification/audit anchors reference immutable lot commitments |
| 10 | `.agents/plans/adr-006-canonical-schema-v1.md` | Accepted 2026-08-24 — 13 strict canonical record families, registered extensions, capability-controlled activation |
| 11 | `.agents/plans/adr-007-vm-state-semantics.md` | Accepted 2026-08-24 — explicit persistent writes, committed write sets, and scoped public/PDC visibility |
| 12 | `.agents/plans/adr-008-endorsement-policy-model.md` | Accepted 2026-08-24 — Fabric-style signature policies, scoped key constraints, and distinct verified principals |
| 13 | `.agents/memories/fabric-policy-shapes.md` | Fabric policy research distilled for GlassChain's endorsement and PDC decisions |
| 14 | `.agents/memories/participation-model.md` | **Read before touching the membership ladder.** The four senses of "validate", why bounding the validator set is a liveness requirement, and the four wrong answers every design pass reaches for first |
| 15 | `.agents/plans/adr-010-capability-versioning-policy.md` | Accepted capability/versioning policy, consensus input boundary, historical validation, peer downgrade, and compact BFT benchmark gate |

`CLAUDE.md` and `.github/copilot-instructions.md` are thin pointers to `AGENTS.md`.
When rules change, **edit `AGENTS.md`** and let the pointers stay pointers.

---

## Where the programme stands

### Done

- **ADR-001 (execution layer) — accepted.** WASM/Wasmtime stands. EVM compatibility
  demoted to a deferred SHOULD behind `ExecutionProvider`; Solidity out of scope.
  This removed the single largest cost item and unblocked ADR-003.
- **CI — shipped.** `.github/workflows/ci.yml`: `cargo check`, `cargo test`, and a
  clippy **no-new-warnings gate** against `.github/clippy-baseline.txt` (**2163**).
  Installs `protoc` (not vendored) and `rustup component add clippy` (not declared by
  `rust-toolchain.toml`). Verified to pass at baseline and to fail on an injected warning.
- **Certificate chain verification — shipped.** `glasschain-identity/src/cert_verifier.rs`
  now verifies the org Root CA's signature over a peer certificate via `rustls-webpki`
  (`ring` provider, matching what `glasschain-network` already selects for rustls).
  `VerificationLevel::Full` is the default. Tests cover a bit-flipped signature and a
  forged CA that reuses the victim's exact Distinguished Name — the attack the previous
  DN-comparison-only check accepted.
- **`.agents/` scaffold** — `plans/`, `tasks/`, `memories/`, each with a README and template.
- **VM state semantics — accepted.** [ADR-007](plans/adr-007-vm-state-semantics.md) chooses
  hybrid execution: `set_state` remains ephemeral, a separate host operation requests
  persistence, accepted write sets are committed with blocks, and replay rebuilds the
  materialized cache from those write sets. Public/PDC visibility is explicit.
- **State-based endorsement policy — accepted.** [ADR-008](plans/adr-008-endorsement-policy-model.md)
  chooses Fabric-style `SignedBy`/`NOutOf` policies over verified MSP organization
  members, channel/contract defaults plus scoped key constraints, distinct signer
  counting, and explicit multi-party rules for custody and regulated transitions.
- **Capability/versioning policy — accepted.** [ADR-010](plans/adr-010-capability-versioning-policy.md)
  makes the consensus boundary explicit, gates every consensus-visible or
  validation-affecting behavior with a network-wide committed capability set,
  preserves height-based historical validation, and downgrades unsupported peers
  to read-only rather than letting obsolete validators vote.

### Blocked on the requirement owner

**Nothing.** D2 resolved 2026-08-20 (wayfinder #16 → ADR-002: BFT,
Tendermint/CometBFT-class); D3 resolved 2026-08-20 (wayfinder #17 → ADR-003:
Fabric-style private data collections, with the boundary, auditor, purge, and
reconciliation answers recorded). D6 is resolved by ADR-010. Stages 1–2 are
unblocked; the consensus-swap execution plan incorporates the resolved BFT
implementation-path research from wayfinder #23 and remains gated by the
200/300-validator compact-workload testnet, stability, stewardship, and audit.

### Unblocked and waiting

Stage 1 of `requirements-alignment.md`: observability, schema registry + extension
namespaces, canonical schema expansion, and VM state write-set plumbing per
[ADR-007](plans/adr-007-vm-state-semantics.md). Stage 2 endorsement enforcement now
has a settled model in [ADR-008](plans/adr-008-endorsement-policy-model.md).

---

## Resolved architecture decisions — historical reasoning

Retained as context: the reasoning below is historical. D2 → ADR-002 (resolved),
D3 → ADR-003 (accepted), D4 → ADR-007 (accepted), D5 → ADR-008 (accepted), and
D6 → ADR-010 (accepted).

### D2 — Who operates the validator set?

§8.2 requires "immediate, deterministic transaction finality (preventing chain forks)".
The ledger uses PoW with longest-chain resolution, which forks *by design* — there is a
passing test named `test_concurrent_mining_longest_chain_wins`.

**Already settled:** "immediate" is literal. A block must be final the moment it
commits. That rejected Option A (keep PoW) and Option D (Thor-style finality gadget
layered over existing production). **Do not re-propose either.**

**Resolved 2026-08-20:** the validator set is zero-trust and includes every
participating organization in v1, including commercial rivals. Use
Tendermint/CometBFT-class BFT, with full participation through the practical
validator ceiling and an authenticated light-client ladder beyond it. The
consensus family remains unchanged as the membership ladder advances.

The implementation seam is still `ConsensusProvider`, and it must expose a
quorum certificate from the start. `PowConsensusProvider` may remain as a second
implementation for compatibility/testing, but production work replaces PoW and
updates every consumer of committed-block semantics. Target throughput and
validator-set sizing remain execution/research work, not open consensus-family
selection.

### D3 — Confirm the privacy model

**Resolved 2026-08-20:** ADR-003 accepts **Option A, Fabric-style private data
collections**: one global ordered chain, public hash commitments, and private
commercial payloads disseminated point-to-point to authorized collection members.
Regulator visibility, purge, and the default 72-hour transient reconciliation
window are settled there.

Implement it as **three subsystems, not one** (this is how Fabric splits it):

| Concern | Fabric | GlassChain home |
|---|---|---|
| Collection policy / membership | `core/common/privdata/` | `glasschain-identity` — extend `channel.rs` |
| Dissemination **and reconciliation** | `gossip/privdata/` | `glasschain-network` — **this is `LibP2pNode`'s job** |
| Ephemeral pre-commit storage | `core/transientstore/` | `glasschain-storage` — new transient store |

Do not skip reconciliation: a peer offline during dissemination must be able to pull
the payload later (Fabric's `reconcile.go`). Dissemination alone is insufficient.

---

## Verified state — trust this table, not prose elsewhere

Measured against the working tree. Line counts and paths confirmed.

| Component | Reality |
|---|---|
| Workspace | **11 crates** (not 10) |
| `glasschain-node/src/main.rs` | **639 lines** — already wires Sled, WASM, identity, network, gRPC. Not a skeleton |
| Autonomous watcher → WASM → signed PO | ✅ Complete, with stress tests (`test_madsim_1000_autonomous_triggers_stress`) |
| Watcher state replay from committed chain | ✅ `glasschain-network/src/node.rs` |
| gRPC auth interceptor | ✅ Wired (validates ed25519 MSP tokens, **not** X.509) |
| E2E recall simulation | ✅ Exists — `chaos_tests.rs:266` |
| Certificate chain verification | ✅ **Implemented** (this session) |
| `Node.cert_verifier` | ❌ `None` at `node.rs:269,338,369,404` — so the verifier constrains nothing at runtime |
| Endorsement enforcement | ❌ `glasschain-rpc/src/server.rs:553` — `"endorsement engine not yet wired to RPC layer"` |
| Provenance/flattener in RPC | ❌ `server.rs:387` — `ServerState` holds only `node` |
| Channel isolation | ❌ `Channel` referenced only by its own crate's tests |
| `LibP2pNode` | ❌ `libp2p_swarm.rs`, **716 lines**, unit-tested, **unreachable from any binary** — but ADR-003 makes it *required infrastructure*, not a deletion candidate |
| Account / balance / fee model | ❌ Does not exist anywhere in the workspace |
| Observability | ❌ `log` crate only — no metrics, no tracing |
| Test suite | **209 tests + 13 doctests = 222**, all passing |
| Clippy | **2163** warnings, at baseline |

---

## Invariants — breaking these fails review

From `AGENTS.md`; re-stated because they constrain almost every task here.

1. **No dependency cycles.** `glasschain-core` depends on nothing internal. If a lower
   crate needs behaviour from a higher one, **define a trait in core and inject the
   implementation**. Verify with `cargo tree -p glasschain-core -i glasschain-identity`.
2. **Provider traits are the seams** — `ConsensusProvider`, `StorageProvider`,
   `ExecutionProvider`, `NetworkProvider`, `IndexerProvider`, `EventBusProvider`
   (`glasschain-core/src/providers.rs`). Never bypass one.
3. **No `unsafe`** — `unsafe_code = "deny"` workspace-wide. New crates need `[lints] workspace = true`.
4. **Errors** — per-crate `error.rs` with a `thiserror` enum; propagate with `?`.
   No `unwrap()`/`expect()` in library code; allowed only inside `#[test]`.
5. **Currency is an integer in minor units** (`1500` = `$15.00`). Never a float.
6. **Identifiers ≥ 2 characters** (`id`, `tx`, `rx` fine; `x` is not).
7. **Tests for every behaviour change.** Unit tests in `#[cfg(test)] mod tests`;
   integration tests currently all in `crates/glasschain-network/tests/`.
8. **Never weaken security defaults.** `GLASSCHAIN_INSECURE_TLS=1` and the
   `insecure-tls` feature are local-debug escape hatches. Never add new env-var kill
   switches for security controls. Never commit keys, certs, or `.pem` files.
9. **Don't run `cargo fmt --all`** — there is a known formatting backlog and CI
   deliberately omits the gate. Format only files you touched:
   `cargo fmt -- crates/<crate>/src/<file>.rs`.

---

## Already rejected — do not re-propose

Each of these was evaluated and turned down for a specific reason.

| Proposal | Why it was rejected |
|---|---|
| Thor-style finality gadget over existing production (ADR-002 Option D) | Finality lags production by a quorum round; §8.2's "immediate" is literal |
| Keep PoW (ADR-002 Option A) | Probabilistic finality; cannot be tuned into deterministic |
| Replace Wasmtime with an EVM (`revm`) | Discards tested work to satisfy a SHOULD, and re-introduces the §3.2 conflict |
| Delete or feature-gate out `LibP2pNode` | ADR-003 makes it the required substrate for private-data dissemination |
| Endorsement check inside `glasschain-core`'s commit path | Requires `core → identity` — a dependency cycle |
| Drive the watcher from Sled write handles over `mpsc` | Breaks the replay invariant; nodes silently diverge after sync. The watcher **must** be fed committed ledger events only |
| Give the WASM VM the node's MSP signing key | Key-exfiltration hole. Guest code must never hold signing material. Current split is correct: WASM returns approve/deny, the node signs outside the sandbox |
| In-tree PostgreSQL/ClickHouse writer | Ship a **separate reference adapter crate** behind `IndexerProvider` instead |
| VRF/RANDAO sortition to pick a per-epoch voting committee | Rejected in ADR-002 (Algorand row) — sampling contradicts full participation, and GlassChain has no stake weighting to make committee capture improbable. "Jury duty" framing means **eligibility + rotation**, not random selection |
| Regional/local-first sharding, sub-clustered voting, cross-shard rollup finality | Same proposal as channel/sub-ledger sharding, rebranded. Forbidden by ADR-004 decision 1 and ADR-002 decision 4; breaks cross-region recall, which is the point of the ledger. See ADR-004's rejected-alternatives entry for the three breakages |
| Validator fee pool, gas-fee redistribution, or fee rebates for validators | **No account, balance, or fee model exists anywhere** (§2.4); ADR-008 defers settlement economics; gas is execution metering with `GasReport` deliberately kept out of the result. Fee rebates would also hand asymmetric transaction costs to whichever commercial rival can afford servers |
| Governance rights conditional on running a validator | Builds the exact privilege tier the model exists to avoid. Governance standing attaches to membership (CONTEXT.md); conditioning it on infrastructure disenfranchises smallholders and hands the rules of a compliance ledger to its largest commercial actors |
| Deriving `MetadataTrustScore` from validator uptime or block signatures | It scores **completeness of SNCM/Anvisa metadata on a `TraceableAsset`** — GTIN, batch, expiry, serial. It has no concept of nodes. Tying it to infrastructure would invert its purpose: a smallholder's lot would rate as less trustworthy because the farmer runs no server |
| "Validators get faster settlement" as an incentive | Under single-slot finality a block is final at commit for everyone, and a verifying member confirms it locally from the commit certificate. The only real asymmetry is one round of early proposal visibility — a front-running hazard given ADR-004 declines to claim fair ordering, not a feature to formalize |

---

## Your next actions

**D2 and D3 are both resolved (2026-08-20).** The scope call is also resolved:
no EVM runtime (only a decoupled compatibility seam), no in-tree warehouse,
bidirectional streaming is backlog, and fee sponsorship waits for a concrete
onboarding-friction case. Canonical schema v1 is now specified (ADR-006): 13
strict record families, registered extensions, and capability-controlled
activation. The Wayfinder map has no remaining frontier ticket: BFT research and
capability/versioning policy are resolved. VM mutation and endorsement-policy
decisions are resolved by [ADR-007](plans/adr-007-vm-state-semantics.md) and
[ADR-008](plans/adr-008-endorsement-policy-model.md). The remaining validator-set
churn/weighting and membership-wide governance aggregation after validator
bounding are future decisions, not silent assumptions. Then pick up implementation
work below.

**Then pick up work.** In priority order:

### Stage 2 — consensus swap and privacy cluster (unblocked)
Replace `PowConsensusProvider` behind `ConsensusProvider`, with a quorum certificate on
the seam. **Retain `PowConsensusProvider` as a second implementation** — all three
reference systems ship multiple consensus impls behind one seam, and a seam with one
implementation is unproven. Expect to rewrite `test_concurrent_mining_longest_chain_wins`
and `test_madsim_application_layer_partition_and_merge`, which *assert* fork resolution
and become invalid. `mine`/`mine-async` REPL commands and the `MineBlock` RPC lose their
meaning — README and `.proto` both change.

Capability/versioning is now settled by [ADR-010](plans/adr-010-capability-versioning-policy.md):
land it with the private-payload wire change, keep the active set network-wide and
height-based, preserve historical validation, enforce a strict public/private consensus
boundary, and downgrade unsupported peers to read-only. Benchmark the compact public
record/commitment workload at 200/300 validators; do not substitute a ZK-only or raw-event
benchmark.

### Stage 1 — foundations (independent of the swap), in this order

1. **Canonical schema expansion (§4.1)** — implement the 13 specified record
      families with strict v1 validation, including `QualityCertification`,
      `AuditAttestation`, `StateCommitment`, and an embedded evidence-manifest
      reference. Certification/audit records reference immutable lot commitments and
      never mutate source transactions ([ADR-005](plans/adr-005-certification-and-audit.md),
      [ADR-006](plans/adr-006-canonical-schema-v1.md)). **This is the data-model
      prerequisite for the Stage 3 workflow engine.**
2. **Schema registry + extension namespaces (§4.2, §4.3)** — implement the
      immutable network-wide registry, registered namespace schemas, and
      capability-controlled activation specified by [ADR-006](plans/adr-006-canonical-schema-v1.md).
3. **VM state write-set plumbing (§2.1)** — implement [ADR-007](plans/adr-007-vm-state-semantics.md):
      separate ephemeral approval output from explicit persistent set/delete operations,
      carry canonical scoped write sets in committed records, apply public/PDC visibility,
      materialize atomically, and rebuild from blocks without re-executing WASM.
4. **Observability (§8.1)** — `log` → `tracing`, Prometheus metrics, admin endpoint.
   **Shape matters:** one thin registry crate plus a `metrics.rs` *inside* each crate
   that owns instruments. Do **not** build a central metrics module every crate depends
   on — Thor and Fabric both colocate. Note this changes the logging convention in
   `AGENTS.md`; update it in the same change.

### Cheap, unblocked, worth doing alongside

- **`Node.cert_verifier` is `None` in all four constructors** (`node.rs:269,338,369,404`).
  The verifier now works but constrains nothing. Either wire it for org-mode nodes or
  state in code and docs that TOFU-only is deliberate. Today it reads as an oversight.
- **`GlasschainServer::with_auth(require_auth: false)`** yields a permissive
  interceptor. Document which deployment path gets strict mode.

---

## How to validate

```bash
cargo check --workspace --all-targets     # fast, use while iterating
cargo test --workspace                    # 209 tests + 13 doctests — must not regress
cargo clippy --workspace --all-targets    # must not exceed 2163 warnings
cargo fmt -- crates/<crate>/src/<file>.rs # touched files only
```

Exact clippy count, the way CI measures it:

```bash
cargo clippy --workspace --all-targets --message-format=json 2>/dev/null \
  | grep -c '"level":"warning"'
```

Also per `AGENTS.md`: update `README.md` when CLI flags, REPL commands, the wire
protocol, or the gRPC surface change, and `PLUGIN_KIT.md` when a provider trait changes.

---

## Traps that have already cost time

| Trap | What happens | Do this instead |
|---|---|---|
| Trusting a plan's claims about the codebase | An externally-authored plan listed 5 of 7 items as to-do that were already done, and 2 delivered "strengths" that were the actual gaps | **Verify against the working tree before planning.** `grep`, `wc -l`, read the file |
| Assuming `cargo fmt --check` passes | It does not — known backlog. A fmt gate fails on a clean tree | CI omits it deliberately. Format only touched files |
| Assuming `rust-toolchain.toml` provides clippy | It pins the channel only, not components | `rustup component add clippy` |
| Assuming `protoc` is vendored | `tonic-prost-build` shells out to it | `apt-get install -y protobuf-compiler` |
| Deleting "dead" code | `LibP2pNode` looked unreachable and deletable; it is the required substrate for a pending requirement | Check unimplemented requirements before removing anything |
| Adding a clippy warning | The CI gate is a hard count against 2163 | Re-run the JSON count before finishing. `const fn` and `#[must_use]` suggestions are the common accidental additions |
| Treating "light client" and "full non-voting node" as the same thing | Three separate design passes concluded that light clients can publish fraud proofs. They cannot — a light client trusts the quorum for validity and does not hold the state | Use the CONTEXT.md terms: **light client** verifies headers, **verifying member** re-executes. Fraud detection needs the latter |
| Drawing the participation model as a vertical stack of tiers | The next pass immediately attaches privileges (governance, fees, faster settlement) to the upper boxes, rebuilding the caste the design exists to avoid | State it as **four overlapping roles over one uniform member status**. See [`memories/participation-model.md`](memories/participation-model.md) |

---

## Cross-cutting, unscheduled

Not blocking anything, but they get more expensive the longer they wait.

- **Rehome cross-cutting integration tests.** Everything lives in
  `crates/glasschain-network/tests/`, including `sncm_compliance.rs`, which is not a
  network test. All three reference systems give scenario tests their own home; Corda
  partitions them for CI parallelism.
- **Flip `avoid-breaking-exported-api` once the SDK has consumers.** `clippy.toml`
  currently tells clippy to freely suggest breaking public signatures — correct pre-1.0,
  wrong afterwards. Corda gates public API changes in CI against a checked-in
  `api-current.txt`.
- **Commit the programme artifacts.** Everything under `.agents/`, `.github/workflows/`,
  `.github/clippy-baseline.txt`, `AGENTS.md`, `CLAUDE.md`, and
  `.github/copilot-instructions.md` is currently untracked.
