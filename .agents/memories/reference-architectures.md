# Reference architecture study — Fabric, Corda, Thor

**Learned:** 2026-08-18
**Method:** structure-only Repomix packs of `hyperledger/fabric`, `corda/corda`,
`vechain/thor`. Directory trees only, no file contents.
**Updated:** full-content packs added — see [Pack recipes](#pack-recipes) below.
**Why it matters:** these three are the paradigm sources named in the GlassChain
requirements. Their structure encodes decisions we are still making in
[ADR-002](../plans/adr-002-consensus-finality.md) and
[ADR-003](../plans/adr-003-privacy-model.md).

## Scale and shape

| Repo | Language | Structure size | Organizing principle |
|---|---|---|---|
| Thor | Go | ~950 lines, 4.7k tokens | Flat, ~40 top-level packages named for domain concepts |
| Fabric | Go | ~3,600 lines, 19k tokens | Layered by architectural role (`common`/`core`/`orderer`/`msp`) |
| Corda | Kotlin | ~4,400 lines, 24k tokens | Module-per-Gradle-project, API/impl split |

Thor is by far the leanest and is the closest analogue to GlassChain's current
size and ambition. Fabric and Corda are both ~15+ year enterprise codebases whose
structure reflects governance needs GlassChain does not have yet.

## Findings that change our decisions

### 1. Thor separates block production from finality — ADR-002 Option D

`bft/` (`engine.go`, `justifier.go`, `casts.go`, `finality_zero_test.go`) is a
**separate top-level package from** `consensus/` (`poa_validator.go`,
`pos_validator.go`, `validator.go`), with `packer/` and `scheduler/` handling
proposal ordering.

VeChain did not replace its consensus to get deterministic finality — it **layered
a BFT finality gadget over PoA block production**. This is a fourth option for
ADR-002 that neither the original plan nor my ADR considered: GlassChain could
keep its existing block production and add a justifier that marks blocks final
once a validator quorum votes, rather than ripping out `PowConsensusProvider`.

Caveat: finality is deterministic but not *instant* — it arrives a quorum-round
after production.

> **Resolved 2026-08-18 — this option is REJECTED.** The requirement owner confirmed
> §8.2's "immediate" is literal: a block must be final at the moment it commits. That
> disqualifies the finality-gadget approach and any design that keeps part of the
> mining path. Do not re-propose it. The reasoning above is retained only because it
> would become relevant again if the requirement were ever revisited.

### 2. All three ship multiple consensus implementations behind one seam

- Fabric: `orderer/consensus/etcdraft/` (CFT) **and** `orderer/consensus/smartbft/` (BFT)
- Corda: `node/notary/jpa/`, `experimental/raft/`, `experimental/bftsmart/`
- Thor: `scheduler/poa_v1.go`, `poa_v2.go`, `pos.go` + separate `bft/`

This validates `ConsensusProvider` as the right seam, but it is also a warning:
a seam with one implementation is unproven. Whatever ADR-002 chooses, the PoW
implementation should be *retained* as the second implementation that keeps the
abstraction honest — not deleted.

### 3. Fabric splits private data across three concerns — ADR-003 refinement

Not one subsystem, but three:

| Concern | Location |
|---|---|
| Collection policy / membership | `core/common/privdata/` (`collection.go`, `membershipinfo.go`) |
| Dissemination to authorized peers | `gossip/privdata/` (`distributor.go`, `pull.go`, `reconcile.go`) |
| Ephemeral pre-commit storage | `core/transientstore/` |

ADR-003 treated private data collections as a single design. It should decompose
along these three lines — and note that Fabric's dissemination layer is built on
gossip, which is the same substrate `LibP2pNode` already provides.

`core/ledger/kvledger/txmgmt/privacyenabledstate/` also shows that private state
is a distinct state-DB concern, not a filter over the public one. Confirms
ADR-003's "largest blast radius" assessment.

### 4. Corda's flow engine is the Stage 3 blueprint

`node/services/statemachine/` is the concrete shape of a workflow engine:

- `transitions/` — one type per transition (`TopLevelTransition`,
  `StartedFlowTransition`, `UnstartedFlowTransition`, `ErrorFlowTransition`,
  `KilledFlowTransition`, `DeliverSessionMessageTransition`)
- `Action.kt` / `Event.kt` / `TransitionResult.kt` — explicit event→action algebra
- `FlowFiber.kt`, `FlowStateMachineImpl.kt`, `SubFlow.kt` — execution and nesting
- `StaffedFlowHospital.kt` — a dedicated error-triage component for stuck flows
- `interceptors/` — cross-cutting concerns (metrics, history dump, hospitalisation)
- `DBCheckpointStorage.kt` in `services/persistence/` — flows checkpoint to the DB

The reusable flow primitives live in `core/flows/`: `CollectSignaturesFlow`,
`FinalityFlow`, `SendTransactionFlow`/`ReceiveTransactionFlow`, `NotaryFlow`,
`@InitiatingFlow`/`@InitiatedBy`. Stage 3 should copy this decomposition rather
than inventing one.

### 5. Corda separates contracts from workflows as deployable units — §3.4

`finance/contracts/` and `finance/workflows/` are **separate Gradle modules**,
mirrored in every sample (`attachment-demo/contracts` + `/workflows`). Contract
code is verification-only and must be deterministic; workflow code drives I/O.
This is the CorDapp packaging answer, and it maps cleanly onto Rust crates.

### 6. Metrics are colocated per-package, not centralized — §8.1

Thor has `metrics/` for the Prometheus plumbing but a `metrics.go` *inside*
`chain/`, `txpool/`, `comm/`, `state/`, `logdb/`, `muxdb/`, `bft/`, and
`api/middleware/`. Fabric does the same (`core/endorser/metrics.go`,
`gossip/metrics/`, `orderer/common/broadcutter/metrics.go`).

Concrete guidance for GlassChain's observability work: one thin crate for the
registry, a `metrics.rs` in each crate that owns instruments. Do not build a
central metrics module that every crate depends on.

### 7. Thor's `api/subscriptions/` is the §2.3 answer, and it is small

`beat_reader.go`, `block_reader.go`, `event_reader.go`, `transfer_reader.go`,
`pending_tx.go`, `message_cache.go` — WebSocket event streams as one tightly
scoped package. Also `api/doc/` serves Swagger UI from `thor.yaml`. §2.3 and the
REST half of §7.1 are a much smaller job than the requirements list implies.

### 8. Fabric versions its consensus-critical logic — capability framework

`common/capabilities/` gates protocol features per channel, and validation logic
is versioned in parallel directories: `core/handlers/validation/builtin/v12/`,
`v13/`, `v20/`. Ledger rules cannot change retroactively without forking the
chain, so old rules are kept and selected by capability level.

GlassChain has a bare `PROTOCOL_VERSION` constant and no capability concept.
ADR-003 requires a wire-protocol change, so this becomes relevant sooner than it
looks.

### 9. Corda has a top-level `experimental/` — the `LibP2pNode` answer

`experimental/` holds `avalanche/`, `cpp-serializer/`, `quasar-hook/`, `blobwriter/`
— unproven work that ships in the repo without implying production readiness.
Corda also puts `raft/` and `bftsmart/` notaries under
`node/notary/experimental/`.

This is a cleaner resolution to GlassChain's unreachable-`LibP2pNode` problem than
either "wire it" or "delete it": mark it experimental, explicitly, so the status is
legible. (ADR-003 still promotes it to required infrastructure if Option A is
chosen — but if that decision slips, `experimental/` is the honest holding pattern.)

### 10. Test organization diverges sharply from ours

- Thor: `*_test.go` colocated, plus `test/` with `testchain/`, `testnode/`, `datagen/` builders
- Fabric: `integration/` with `nwo/`, a full network-orchestration framework
- Corda: `testing/` as *published modules* (`node-driver`, `test-utils`,
  `core-test-utils`, `cordapps`), plus source sets split
  `integration-test`, `integration-test1`, `integration-test2`, `smoke-test` —
  deliberately partitioned for CI parallelism

GlassChain puts every integration test in `crates/glasschain-network/tests/`,
including `sncm_compliance.rs`, which is not a network test at all. All three
references would place cross-cutting scenario tests in their own home.

### 11. Corda enforces API stability in CI — we do the opposite

`.ci/api-current.txt` plus `check-api-changes.sh` gate public API changes on every
PR. GlassChain's `clippy.toml` sets `avoid-breaking-exported-api = false`, i.e.
clippy is told to freely suggest breaking public signatures. That is the right
call *now* (pre-1.0, no downstream users) but should flip once the SDK has
consumers.

## Implication summary

The two structural mistakes to avoid, visible in all three references:

1. **Do not centralize what should be colocated** (metrics, tests, errors).
2. **Do not build a seam with one implementation and call it pluggable.**

And the one to steal outright: Corda's `statemachine/transitions/` decomposition
for Stage 3.

## Pack recipes

Full-content Repomix packs, for grepping implementation detail rather than just
structure. Repack with these ignore patterns — the defaults pull in enormous
amounts of noise that crowds out the actual source.

| Repo | Tokens | Ignore patterns |
|---|---|---|
| Thor | 1.5M | `api/doc/stoplight-ui/**,api/doc/swagger-ui/**,vendor/**,**/testdata/**` |
| Corda | 2.7M | `**/build/**,docs/**,**/*.jar,**/*.png,**/*.gif,gradle/**,**/gradle-wrapper*,**/*.csv,.ci/api-current.txt,detekt-baseline.xml,tools/demobench/**,tools/explorer/**,client/jfx/**` |
| Fabric | 3.7M | `vendor/**,**/testdata/**,**/mock/**,**/fake/**,**/fakes/**,docs/**,**/*.pb.go,CHANGELOG.md,release_notes/**,vagrant/**` |

What those patterns exclude, and why it matters:

- **Thor** — the Swagger/Stoplight UI bundles are ~1.4M tokens of vendored
  minified JS/CSS, roughly half the raw pack.
- **Corda** — `samples/simm-valuation-demo` ships historical LIBOR/fed-funds
  fixings as CSV, duplicated across two source sets: ~1.9M tokens. `api-current.txt`
  is another 130k, and `detekt-baseline.xml` 59k.
- **Fabric** — `CHANGELOG.md` alone is 349k tokens. Note the mock dirs are
  **`mock/` and `fake/` singular**, not the more common plural; a `**/mocks/**`
  pattern misses almost all of them.

### Where to look, by topic

| Question | Repo | Path |
|---|---|---|
| Finality gadget over block production | Thor | `bft/` (`engine.go`, `justifier.go`, `casts.go`) |
| CFT vs BFT ordering behind one seam | Fabric | `orderer/consensus/etcdraft/`, `orderer/consensus/smartbft/` |
| Private data: policy / dissemination / transient | Fabric | `core/common/privdata/`, `gossip/privdata/`, `core/transientstore/` |
| Private state as a separate DB | Fabric | `core/ledger/kvledger/txmgmt/privacyenabledstate/` |
| Capability-gated protocol versioning | Fabric | `common/capabilities/`, `core/handlers/validation/builtin/v12\|v13\|v20/` |
| Flow/workflow engine | Corda | `node/src/main/kotlin/net/corda/node/services/statemachine/` |
| Reusable flow primitives | Corda | `core/src/main/kotlin/net/corda/core/flows/` |
| Contract/workflow packaging split | Corda | `finance/contracts/` vs `finance/workflows/` |
| Flow checkpoint persistence | Corda | `node/.../services/persistence/DBCheckpointStorage.kt` |
| WebSocket event streams | Thor | `api/subscriptions/` |
| Per-crate metrics colocation | Thor | `metrics/` + `metrics.go` in `chain/`, `txpool/`, `bft/`, `state/` |
