# Plan — Source-comment debt and review follow-ups

**Status:** completed; all seven markers (D1–D7) settled and benchmarked in code (#106–#109, #114–#115)
**Reviewed:** 2026-09-12 against `7143c0c`
**Scope:** all tracked source `TODO` and `ponytail:` comments, plus documentation-only matches.

## Goal and method

Make every deliberate shortcut actionable without implementing speculative fixes.
Keep this inventory authoritative; the programme, zero-trust and performance
plans link here instead of maintaining separate lists. This review also updates
those plans for the external HotStuff/PQC/learning-loop report and makes the root
README a navigation overview. No runtime behaviour changes in this effort.

Searching `\bTODO\b|ponytail:` across `crates/` finds **3 remaining source markers (2 ponytail + 1 TODO)**:
1. `crates/glasschain-core/src/endorsement.rs:395` (D1 — network-governance fallback; settled & documented in #115)
2. `crates/glasschain-core/src/ledger.rs:78` (D3 — capability rebuild admission cost; benchmarked in #106)
3. `crates/glasschain-network/tests/madsim_chaos.rs:631` (D7 — madsim-tokio migration; alternative TCP proxy shipped in #70/#108)

The 4 other original ponytail markers (D2 in `endorsement.rs`, D4 in `msp_policy.rs`, D5 in `transient.rs`, D6 in `triage.rs`) were retired upon shipping their implementations (#109, #114, #115).
`PLUGIN_KIT.md` additionally contains three `todo!()` sketch placeholders in example documentation code.
All items are fully visible, tracked, and verified below.

## Inventory, grouped by file

| ID / source | What was simplified / ceiling | Disposition, trigger and smallest next step | Acceptance check |
|---|---|---|---|
| D1 — `crates/glasschain-core/src/endorsement.rs`, `PolicyHistory::default_policies` | Fixed `network-governance` fallback; unconfigured scopes fail closed. | **Shipped (2026-09-10):** the fixed fail-closed fallback stays; provisioning (register the governance key under `network-governance`) and the authorized `PolicyUpdate` bootstrap are documented in `docs/operations.md`, and tests prove the activation rule is separate from scoped updates. No allow-all exception, no flag. | Unknown/missing governance signer rejected; authorized scoped update applies from the next block; replay derives the same policy (`scoped_update_does_not_relax_the_activation_rule`, `policy_bootstrap_replay_is_deterministic`). |
| D2 — `crates/glasschain-core/src/endorsement.rs`, `operation_default` | Recall's intended two-party approval degenerates when envelope issuer and `issued_by` name the same principal; canonical record family/channel policy wiring is incomplete. | **Shipped (2026-09-10, owner decision):** the chain does **not** arbitrate recall authority — `recall` requires only the issuing organization's signature and `issued_by` stays informational. Downstream visibility runs through the public record plus the shipped quarantine/dispute workflows (`recall_flow_scenario`); no second authority is invented. | Wrong-signer recalls rejected at admission; issuer-signed recalls commit (`recall_registration_requires_only_the_issuing_org`); duplicated signatures still never manufacture another principal (provider counting tests). |
| D3 — `crates/glasschain-core/src/ledger.rs:78`, `Ledger::add_transaction` | Rebuild capability history for canonical records/activations; scan all committed and pending transaction IDs. Cost grows with history and pool size. | **Baseline measured (2026-09-09, `cargo bench -p glasschain-core --bench ledger_admission`, release, 64-admission bursts):** ~214 µs/admission at 100 committed records, ~2.3 ms at 1 000, ~21 ms at 10 000 — linear in history; duplicate-ID admission pays the same rebuild (no short-circuit); a 1 000-record history with a growing pending pool measured the same as a fresh burst (~136 ms/burst), so the pool arm is minor at that scale. Optimize the shared rebuildable capability/ID index at the owning layer when attributed; not competing caches or a DAG. | Before/after admission p95/p99 and memory versus height/pool size; duplicate IDs, activation boundaries, sync/restart, failed proposal restoration and direct `Ledger` callers retain identical results. |
| D4 — `crates/glasschain-identity/src/msp_policy.rs`, `MspEndorsementProvider` | Trusted key→principal registration is directory-based, not derived from verified certificates. | **Shipped (2026-09-10, #87):** `register_certificate` derives the principal from a certificate verified against the org anchor (chain, subject O, validity, CRL — once, at registration) plus a possession proof; entries carry valid-from/revoked-at heights and `evaluate(expression, request, height)` authorizes by height only (no wall clock or mutable CRL on replay). Local directory provisioning stays for trusted embedders; remote wiring/registration and a chain-derived registry (adjacent to #74) remain. | Wrong-org, unknown key, revoked/expired credential rejected for new authorization; committed historical authorization remains deterministically verifiable — covered in `msp_policy` tests (certificate registration, possession, go-forward revocation) and the trait-level height plumbing. |
| D5 — `crates/glasschain-storage/src/transient.rs:55`, `TransientStore::expiry_index` (module docs lines 19–23 give the upgrade) | Expiry index is in memory. Persisted payloads cannot be enumerated for purge after restart. | **Shipped (2026-09-10):** `StorageProvider::list_state_keys(prefix)` and a storage-scanning `purge_expired` that discovers persisted payloads without a prior read; per-key delete failures are logged and retried next sweep; the node runs a startup + 300 s retention sweep. | Persist payload, reopen store, expire, purge **without first reading the key**, and verify underlying key deletion; live records survive; interrupted deletes retry safely — covered for both in-memory and sled backends. Document backup/replica retention separately. |
| D6 — `crates/glasschain-workflows/src/triage.rs:26`, `FlowTriage` | Progress inventory vanishes on restart; a known flow is rediscovered only when driven again. | **Shipped (2026-09-10):** `Checkpoint.step` is persisted at save time and `FlowTriage::discover(storage)` enumerates checkpoints into a fresh view with stored timestamps, read-only (the scan D5 needed is shared). | Reopen persistent checkpoints with a fresh triage instance; discover a waiting/stuck flow without a new event; preserve its timestamp; completed flows absent; discovery does not replay side effects — covered in `workflows/tests/resume.rs`. |
| D7 — `crates/glasschain-network/tests/madsim_chaos.rs:631` | TODO for madsim-tokio interception of real socket partitions; current test partitions at application level. | **Alternative shipped and extended:** [TCP-level fault injection](https://github.com/dbbvitor/GlassChain/issues/70) closed via the proxy, now shared (`tests/common/proxy.rs`) with seedable WAN profiles: per-direction latency/jitter/bandwidth shaping, mid-scenario `set_profile`, partition/repair, asymmetric links. D7 scenarios shipped: no-fault 4-node baseline; asymmetric 200 ms/±80 ms link (applied post-handshake); partition-then-repair with time-without-quorum measured; a BFT vote round with the leader's WAN-shaped link (200 ms/±80 ms) reaching quorum with no conflicting tips. **Measured finding:** mesh formation through a ≥~120 ms/chunk-shaped relay stalls (~4 × 5 s reconnect cycles) while 60–80 ms one-way converges in ~2 s — the dial/hello path needs a budget audit before larger profiles are usable (performance Step 0 follow-up). Revisit simulator-runtime migration only if pinned Tokio support and deterministic fault schedules add coverage the proxy cannot provide. | Established TLS session severed; advertised/reconnect addresses cannot bypass faults; partition + repair converges without conflicting finalization. Label real TCP wall-clock tests separately from deterministic simulated-network tests (`real_tcp_wan_*`). |

**Trigger quality:** all six ponytail comments have a contextual upgrade path
(0 entirely missing), but D5's “when a real deployment needs it” is too vague
and D6 names closed workflow tickets as a future trigger. The concrete deployment
gates above supersede those weak/stale triggers. D7 remains an optional method,
not proof that TCP fault coverage is absent. Do not equate a closed parent ticket
with completion of its leftover comments.

## Important distinctions from the code

- **D3 is on the node path too:** `Node::submit_transaction` calls
  `Ledger::add_transaction`; generated transactions, retry restoration and peer
  admission also call it. `NodeState`'s incremental capability cache fixed one
  replay path, not the independent rebuild inside `Ledger`. Measure the actual
  end-to-end path rather than assuming the earlier cache removed every scan.
- **D5 is physical retention, not an expired-read bypass:** `get()` reads the
  persisted `expires_at` and rejects expired payloads even with an empty index.
  It returns *before* recording an expired key, so reading one does not repair
  purge discovery. The currently surviving guarantee is denial of reads, not
  deletion after restart. Backups, storage compaction and copies at other members
  require their own retention controls; deleting a key is not certified erasure.
- **D6 does not mean checkpoints are missing:** `CheckpointStore` persists them
  under `workflow:checkpoint:` and `FlowRunner::handle` can resume by ID. Missing
  discovery prevents an operator from reliably finding idle workflows.
- **D1 is not equivalent to D2:** a fail-closed governance fallback should stay;
  an ambiguous independent-approval requirement needs a policy decision.

## Sequence and completion

1. **Before a regulated/persistent pilot:** resolve D1/D2/D4 authorization and
   D5 retention; recover unattended flows with D6. Named ownership and privacy/
   regulatory review are acceptance inputs, not claims this plan has obtained.
2. **Before performance claims:** benchmark D3 and the WAN extension under D7,
   alongside the read-path memory scenario in [performance.md](performance.md).
3. **At implementation:** update the relevant marker in the same change as its
   acceptance check. Leave explicit deferrals in place until their trigger is met.

D5 and D6 can share a small storage enumeration decision, but have separate
acceptance tests: privacy purge and operational triage are different outcomes.
If `StorageProvider` changes, update `PLUGIN_KIT.md` and all implementations.
No code, comment-only source edits, new dependencies or new issue batch is
required merely to register this inventory.

## Documentation-only matches

- **Corrected in this documentation change:** `docs/consensus.md` no longer
  quotes deleted single-attestation/advisory-signature markers as current; it
  describes BLS rounds and ADR-012. These were documentation defects, not extra
  live source markers.
- `docs/privacy-and-identity.md` and `docs/workflows-and-contracts.md` quote D2,
  D4–D6: retain limitations and link this plan instead of promising shipped fixes.
- Historical memories preserve what was true at their recorded commit. Do not
  count quoted markers there as new debt or use them as current-state authority.
- `PLUGIN_KIT.md` contains **three `todo!()` sketch placeholders** (two consensus
  methods, one PostgreSQL adapter). They are not runnable implementations. The
  Raft sketch is not an approved zero-trust production consensus alternative
  (ADR-002); the warehouse adapter belongs to Stage 6. Do not scaffold either
  just to remove a textual TODO match.

## Validation for this documentation pass

- Re-scan packed source and tracked files; account for all seven source markers.
- Check local links and compare plan statuses with their referenced symbols.
- Run the required workspace test/clippy commands; report actual results, not
  inherited claims that a docs-only change implies green gates.
