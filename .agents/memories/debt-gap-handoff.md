# Handoff — GlassChain debt-gap implementation (through ticket #41)

**Written:** after closing #39, #40, #41. `main` HEAD: `77d574c`.

## The loop (established and working — do not redesign it)

`plan file → implement (ponytail-lazy, tests) → full gates → commit to main →
two-axis code review (two parallel sub-agents, Spec + Standards) → fix hard
findings → amend commit → record evidence on issue via MCP → close issue`.

Standing preference is **ponytail** (laziest correct solution; stdlib/native/seams
before new code). No re-asking the user — continue to the next frontier ticket.

## Completed & committed to `main`

| Ticket | Commit | What |
|---|---|---|
| #34 Canonical schema v1 | `490517a` | 13 record families, `(schema_id,version,hash)` registry, strict validation at admission + commit + peer-block path |
| #35 VM typed result | `7958a3a` | `ExecutionResult` (ephemeral vs `PersistentWrite`), `persist_state` ABI, duplicate-scope rejection |
| #36 Capability registry | `950340a` | static v1 registry, future-height activation, height-selected validation, handshake gate, read-only observers |
| #37 Endorsement seam | `6e52b93` | identity-neutral `EndorsementProvider` + `PolicyExpression` tree, `MspEndorsementProvider`, distinct-principal counting |
| #38 Quorum certificate | `2aaea50` | `CommitNotification` + `QuorumCertificate` on `ConsensusProvider`, mining RPC/REPL retired, fork tests rewritten |
| #39 Analytics read path | `2830938` | provenance/flattener wired into RPC; bounded drop-oldest event bus; lineage queries; boundary-anchored canonical-key filters |
| #40 Workflow framework | `86339a2` | new crate `glasschain-workflows`: Action/Event/TransitionResult, one type per transition, checkpoint store over `StorageProvider`, handle/ack split (no-loss + at-least-once + deterministic ids), triage view |
| #41 Committed write sets | `77d574c` | `Block.write_set` hash-covered, `StorageProvider::apply_block` atomic boundary with shared `validate_tip_chain`, PDC commitment redaction (`PersistentWrite::block_form`), node executes `ContractExecution` at mining, `rebuild_world_state` (no WASM re-execution) |

## Working tree

Clean. All four gates green at `77d574c`:
`cargo check --workspace --all-targets --all-features --locked` ✅
`cargo test --workspace --all-targets --all-features --locked` ✅ (22 suites)
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` ✅
`cargo fmt --all --check` ✅

## Next ticket: #45 (endorsement enforcement at the commit path)

**Fetched and ready.** All blockers (#36, #41, #37) are closed. Considerably
smaller than #42 (BFT, explicitly "later, gate-heavy") — the frontier order
after #45: #42, #43/#44 (both unblocked by #40), #46→#47 (PDCs), #48, #49.

#45 acceptance criteria, condensed:
1. Endorsement evaluated at admission against the exact transaction **and its
   committed write set**, before write-set materialization; unsatisfied policy
   rejects with no partial state.
2. Operation defaults: custody handoffs 2-of-2 (sender + receiving custodian);
   recall/quarantine/dispute multi-party; certification/audit issuer signature;
   PDC writes = collection membership + collection policy.
3. Policy metadata committed in-band, versioned, append-only; policy update is
   a signed tx satisfying the current effective policy, activates only after
   its block commits; historical blocks keep their height-effective policy;
   key-level policies cleared only through the same authorization.
4. A block changing a key's policy AND writing the same key later in the same
   block is rejected; new policy applies from the next block.
5. `VerifyEndorsement` RPC returns a real evaluation.
6. Node-level scenarios: failed authorization w/ no partial state, multi-key
   txs, distinct-signer counting, PDC membership vs endorsement separation.

Existing pieces to build on (from #37): `glasschain_core::endorsement` has
`PolicyExpression` (SignedBy / NOutOf / AND / OR), `EndorsementRequest`,
`EndorsementEvaluation`, `EndorsementProvider`; `glasschain-identity` has
`MspEndorsementProvider`. From #41: `Block.write_set`, `apply_block`,
`compute_write_set`, `rebuild_world_state`. From #36: capability/height
selection (spec says "historical blocks under the policy version effective at
their height"). From #34: `CanonicalRecord` families (`quality_certification`,
`audit_attestation` require `issuer`; `delivery_receipt` is the custody
handoff record).

Spec decision 3 in `.agents/plans/spec-close-debt-gap.md` + ADR-008
(`.agents/plans/adr-008-endorsement-policy-model.md`) are the settled answers;
do not re-litigate them.

## Context / conventions (non-negotiable)

- Rust workspace, 12 crates, Rust **1.95** pinned (`rust-toolchain.toml`), edition 2021, Tokio async.
- **No `unsafe`** (`unsafe_code = "deny"` workspace-wide). New crates add `[lints] workspace = true`.
- **No `unwrap`/`expect` in library code** — only `#[test]`. (`providers.rs` has grandfathered `.expect("lock poisoned")` — match the file, don't add new ones elsewhere. `Block`/`Transaction` constructors have grandfathered clock `.expect`s — leave them.)
- Errors: per-crate `thiserror` enum, propagate with `?`. Logging: `log` crate in libraries, inline format captures (`log::warn!("bad addr {addr:?}: {e}")`).
- Serialization: serde derive; peer wire protocol is JSON. Currency = integer minor units. Identifiers ≥ 2 chars.
- Dependency direction (no cycles): `glasschain-core` depends on nothing internal. New crates: `glasschain-workflows` depends on core (+ dev-deps on contracts/vm). Seams are provider traits in core.
- Docs rules: README.md for CLI/REPL/wire/gRPC changes; PLUGIN_KIT.md for provider-trait changes (updated for `apply_block` in #41); plans to `.agents/plans/<slug>.md`.
- Targeted `#[allow]`s need a one-line justification comment (AGENTS.md).
- Clippy runs `all + pedantic + nursery + cargo` at warn, CI uses `-D warnings`. Watch for: `significant_drop_tightening` (nursery, fires on match-arm-shaped lock use — restructure or justified allow), `redundant_clone`, `doc_markdown`, `too_long_first_doc_paragraph`, `too_many_lines`, `cast_possible_wrap` (use `i.cast_signed()`).
- Never run `cargo run -p glasschain-node` (REPL blocks on stdin). Integration tests live in `crates/glasschain-network/tests/`.

## Architecture facts the last three tickets established

- `Block` now has `write_set: Vec<PersistentWrite>` included in `calculate_hash`; `Block::new` = empty set, `Block::with_write_set` = the mining path. `Ledger::new` genesis is hand-built with `write_set: Vec::new()`.
- `StorageProvider::apply_block(&Block)` — atomic block+state boundary; default sequential fallback; InMemory (block+state locks) and Sled (multi-tree transaction) override. All route the chain check through `validate_tip_chain(block, tip: Option<&Block>)` → `CoreError::InvalidBlock` on stale candidates (sled maps transaction aborts to `InvalidBlock`, real sled errors to `Storage`).
- `PersistentWrite::{block_form, state_key, apply_to_cache}`; `state_key()` = `ws:<channel>:<contract>:<key>`. PDC values are SHA-256 commitments in blocks AND in the world-state cache until #46/#47 deliver the private payload.
- Node: `NodeState.world_state` cache + `executor`; `compute_write_set` runs at `mine_async` (failed executions accept no writes — deterministic); `after_block_commit` does `storage.apply_block` + cache mirror (chain stays authoritative on failure); `rebuild_world_state` heals; `start()` persists missing chain blocks (a fresh node's genesis) through `apply_block`.
- Workflows crate API: `FlowRunner::handle(storage, triage, flow_id, initial_state, event) -> Option<FlowOutcome { state, actions, completed }>` and `ack(storage, triage, flow_id, executed)` — the caller executes actions durably between handle and ack; ack is the only place the checkpoint advances.
- `TransactionKind` has `CanonicalRecord` and `CapabilityActivation` variants — exhaustive matches live in `glasschain-indexer/src/indexer.rs` (`kind_name`), `event_bus.rs`, `glasschain-rpc/src/server.rs`, `glasschain-node/src/main.rs`. Adding a variant breaks all of them.

## Tracker workflow

GitHub repo `dbbvitor/GlassChain` via MCP tools (`issue_read`, `issue_write`,
`add_issue_comment`) — **not** the `gh` CLI. Close with `issue_write` (state
`closed`, reason `completed`) then post an evidence comment (AC-by-AC evidence,
validation commands, review summary). Commit to `main` directly — no branches
or PRs (established pattern under `/implement`).

## Pitfalls / things that burned earlier agents

- **Review loop is mandatory** — every ticket so far had at least one hard finding the two-axis review caught (e.g. #39's GTIN prefix cross-match regression; #40's checkpoint-before-durability loss window → handle/ack split; #41's sled `Storage`-vs-`InvalidBlock` inconsistency). Don't skip it to save time.
- **`server_integration.rs`** (`crates/glasschain-rpc/tests/`) uses `node.mine().await.unwrap()` + `start_server(node.clone())`; the old `mine_block()` helper and `MineBlock` RPC are retired — don't reintroduce them.
- The flattener's `ingest_indexed_block(&IndexedBlock::from(block), &txs)` takes two args; `indexed_transactions_of(block)` is now fallible (`Result`).
- madsim chaos tests assert the **no-fork** model; grep for `longest`/`longer chain` before declaring any consensus-related ticket done.
- `calculate_hash` covers `write_set` — any test tampering a block's write set without recomputing fails `is_valid()`.
- Sled's `TransactionalTree` has no `.last()`; use `.get()` + conflict detection (see `sled_backend.rs::apply_block`).
- Lock guards in atomic sections trip `significant_drop_tightening`; the justified `#[allow]` on `InMemoryStorageProvider::apply_block` explains why the guards must live to fn end.
- Repo has 12 crates now; README's crate count is stale (no one has fixed it — don't touch unless updating README anyway).
