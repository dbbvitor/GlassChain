# Handoff — GlassChain debt-gap spec: COMPLETE (through #48)

**Written:** after closing #48. `main` HEAD: `322356d`. All spec tickets
(#34–#49) are closed; the frontier is empty — future work comes from review
follow-ups and the ADR-010 adoption gates.

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
| #45 Endorsement enforcement | `e1e4b75` | `Transaction.endorsements` carriers (scoped target + signers over canonical tx bytes), `PolicyUpdate` kind + `PolicyHistory` replay (same-block rule, fail-closed `network-governance` fallback), capability-gated enforcement at mine/peer/sync/ledger-commit paths, operation defaults (custody 2-of-2, recall 2-of-2 issuer+`issued_by`, cert/audit issuer), `VerifyEndorsement` real evaluation |
| #42 BFT behind the seam | `044da66` | `BftConsensusProvider` (core, `bft` feature, default-off): real ed25519 attestations over block hash, `verify_certificate` (⅔+ distinct, fail-closed), capability-gated engine selection in `mine_async` (`bft_consensus` active at candidate height), node-level no-fork finality test, wire `glasschain/2`, adoption gates in README |
| #43 Purchase-to-settlement flows | `11a36ac` | `purchase_flow.rs` (buyer/seller role runners over one state machine; PO/shipment/receipt emissions, acceptance/dispute consumption; RFQ/Quote/Settlement are record-less by design), `attestation_flow.rs` (one parameterized cert/audit flow, anchored records with embedded evidence manifests), `Event::Woken` business wake-up (Resumed stays liveness-only), `FlowRunner` Send+Sync, two-org node-level E2E with interruption resume |
| #44 Recall/quarantine/dispute flows | `a0d1a51` | `recall_flow.rs`: recall lifecycle (append-only status trail issued→active→completed, anchors the CONFIGURED lot), `quarantine_flow`/`dispute_flow` custodian responses emitting `inventory_transformation` records (dispute reason stays off-chain by whitelist), legacy chaos recall simulation replaced by a three-org flow-driven E2E asserting the public trail on all chains |
| #46 PDCs on the wire | `f91f0f2` | `Message::PrivatePayload` (point-to-point, member-only), wire `/3` + Hello `org` field, `TransientStore` (storage), collection config with membership≠endorsement + regulator defaults, four-boundary enforcement (admission/transport/storage/replay) with node-level scenarios in `pdc_boundary.rs` + `protocol_security.rs` |
| #47 PDC distribution E2E | `fa69e2d` | pull reconciliation (`RequestPrivatePayload` + `reconcile_private_payloads`, wire `/4`), retention/purge (`retention_secs` default 72h, expiry envelopes + `purge_expired`), cert-verified delivery (Hello carries the org cert; Step 2.5 verifies CN == claimed org under the Root CA; payload path requires verified senders when a verifier runs; org-drift rejection); TLS stays transport-only self-signed |
| #49 Packaging split | `52234f7` | `WatcherService` moved to `glasschain-workflows` (I/O-driven layer) with its bench; contracts = deterministic layer (BTreeMap registry — the determinism invariant is literally true), approval gate public, test_wasm cross-crate (wat main dep); docs in README + AGENTS + PLUGIN_KIT |
| #48 Capacity gate | `322356d` | `consensus_capacity.rs` (#[ignore]-gated 200/300 + smoke): star topology, compact ADR-010 §7 workload, latency/size/cert/pool/fan-out metrics, app-layer partition recovery, PDC dissemination separate; evidence in `docs/benchmarks/consensus-capacity.md` (PoW cert 115B measured; staged BFT one-attestation cert 508B leader-side; no vote-gossip claims) |

## Working tree

Clean. All four gates green at `044da66` (run in **both** feature configs —
default and `--all-features` — since `bft` gates new code):
`cargo check --workspace --all-targets --all-features --locked` ✅
`cargo test --workspace --all-targets --all-features --locked` ✅ (24 suites)
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` ✅
`cargo fmt --all --check` ✅

## Next ticket: #47 (PDC dissemination end-to-end)

The frontier order after #46: **#47** (gossipsub/Kademlia dissemination, pull
reconciliation, retention/purge windows, cert-verified payload delivery
closing the self-asserted-org gap), #48, #49 (#34–#45, #42–#44, #46 closed).

#46 facts worth remembering:
- The authoritative collection endorsement policy is a COMMITTED `PolicyUpdate`
  with a collection-scoped `collection_policy` (enforced by the #45 engine,
  tested in endorsement.rs). `ChannelConfig.endorsement_policy` is a local
  DECLARATION only — never present it as an enforcement source (the review
  caught that false-doc pattern again).
- Capability gates for payloads use the NEXT height (`effective_set(next)`)
  because payloads are pre-commit artifacts for writes landing at tip+1; they
  may arrive before their block does.
- `peer_senders`/peer registry are keyed by the ADVERTISED listen addr; the
  receive path must use `current_stable_addr` for registry lookups, not the
  connection addr.
- A guest MUST compute private values at runtime — a value in a WASM data
  segment rides the committed ContractCreation tx. The test contracts do this
  via `i32.store` of an obfuscated constant.
- #47 must also: attach `CertChainVerifier` to the payload path (org is
  self-asserted until then), enforce membership on VM writes mined by relayed
  executions, add org-drift detection to `verify_or_register`, and add the
  retention/purge windows to `TransientStore` (`delete` is the hook).

#44 review finding worth remembering (fixed in the amend, generalizable):
- `RecallConfig.lot_ref` was write-only — the anchor transition anchored the
  FIRST committed lot record regardless of config, while the doc claimed lot
  scoping. Config-driven transitions must actually MATCH on their config
  (`record.record_id == config.lot_ref`); a write-only config field whose doc
  claims scoping is a HARD review finding pattern to grep for in every flow.

#43 facts worth remembering:
- The v1 registry has NO rfq/quote/acceptance/dispute/settlement families —
  those chain steps are flow states (record-less by design); every
  family-bearing step emits/consumes its record. Don't "fix" this by extending
  SCHEMA_V1.
- The runner swallows `Event::Resumed` for waiting flows (liveness signal
  only); business decision points need `Event::Woken(reason)`.
- Flow emissions are only exactly-once because hosts submit with
  `Transaction::with_id(record.record_id, …)` and keep `rfq_id`/`lot_ref`
  globally unique (record ids derive from them; the ledger silently drops
  duplicate tx ids). Documented on the purchase_flow module.
- `evidence_manifest` for cert/audit families must be an OBJECT:
  `{"manifest_commitment": <64-hex>}` (ADR-005 embedded manifest), not a
  string.
- Future node-hosted flow runtime needs a durable wake queue (`Woken` events
  arriving while un-acked pending work exists are dropped — fine for
  operator-driven hosts, recorded in the #43 plan).

#42 review findings worth remembering (both fixed in the amended commit):
- The Spec reviewer caught two **false doc claims** — README said the engine
  "gathers attestations from its configured validator set locally" (it doesn't:
  `attest` is local-signer-only, 1-validator set is its own quorum), and
  `validate_block`'s doc claimed quorum verification happens on a wire path and
  at commit time (neither exists). Docs claiming future work as present is the
  recurring failure mode on staged tickets — write docs against the shipped
  code, not the plan.
- Stale "arrives with #42" comments on the received/sync paths had to be
  rewritten when #42 actually shipped. Grep for a ticket number before closing
  it.

#45 review findings worth remembering (both fixed in the amended commit):
- The sync path (`Message::Chain` → `try_replace_chain`) was a full admission
  bypass — `Node::enforce_chain_endorsements` now walks the candidate chain
  (capability history + policy history + carrier evaluation) before adoption.
  Any future admission path must call an endorsement gate too.
- Record families have no channel/contract scope, so committed policies cannot
  reach them; `operation_default` (fail-closed on known families) is the only
  record-level enforcement. Recall's 2-of-2 degenerates to self-approval when
  envelope issuer == payload `issued_by` (ponytail-noted; configured
  multi-party policies for records need channel wiring).
- Known accepted gaps (surfaced to owner in the #45 evidence comment):
  `PolicyUpdate` is a full replacement (a more-specific scope can weaken a
  base layer — ADR-008 §1 non-weakening not enforced); no production node
  wiring attaches an endorsement provider yet (inert outside tests until a
  node/CLI flag or network default lands); peer-path write binding is
  aggregate, not per-transaction.

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
- Endorsement (#45): `Transaction.endorsements: Vec<TransactionEndorsement>` is `#[serde(default, skip_serializing_if = "Vec::is_empty")]` — pre-#45 tx JSON (and therefore block hashes) unchanged when empty. Signers sign `TransactionEndorsement::payload(tx)` = tx bytes with carriers cleared (never self-referential). `compute_write_set` returns `(aggregate, per_tx)`; per-tx attribution feeds the coverage check at mining. Enforcement gates: `submit_transaction` (admission), `enforce_block_endorsements` (mining + peer `Message::Block`), `enforce_chain_endorsements` (sync `Message::Chain`), `commit_mined_block` (structural replay). All are dormant unless `NodeState.endorsement` is set AND the `endorsement` capability is active at the candidate height.
- Workflows crate API: `FlowRunner::handle(storage, triage, flow_id, initial_state, event) -> Option<FlowOutcome { state, actions, completed }>` and `ack(storage, triage, flow_id, executed)` — the caller executes actions durably between handle and ack; ack is the only place the checkpoint advances.
- `TransactionKind` has `CanonicalRecord` and `CapabilityActivation` variants — exhaustive matches live in `glasschain-indexer/src/indexer.rs` (`kind_name`), `event_bus.rs`, `glasschain-rpc/src/server.rs`, `glasschain-node/src/main.rs`. Adding a variant breaks all of them.
- BFT (#42): `BftConsensusProvider` lives in `glasschain-core/src/bft.rs` behind the `bft` feature (`bft = ["dep:ed25519-dalek"]`, default off; `glasschain-network/bft` forwards it). `attest(block)` = one local ed25519 attestation (1-validator set is its own quorum); `verify_certificate` = the real ⅔+-distinct verification (HashSet-deduped keys, unknown validators fail-closed, signatures over `block.hash`). Node selection: `NodeState.consensus: Option<Arc<BftConsensusProvider>>` (concrete type — the sync `propose_block` can't express the node's pre-computed write set), engaged by `set_bft_consensus` + `bft_consensus` active at the candidate height. **PoW-coupled paths that reject BFT blocks:** peer `Message::Block` admission (`has_valid_pow`), `try_replace_chain` (sync), `restore_ledger`/`validate_chain` (restart); certificates are not persisted with blocks. All are adoption-gate work, documented in README.
- Both feature configs must stay green: CI's `--all-features` compiles the `bft` code, default builds compile the `#[cfg(not(feature = "bft"))]` fallbacks. Gated imports (`#[cfg(feature = "bft")] use ...`) are required or default builds warn on unused imports.

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
