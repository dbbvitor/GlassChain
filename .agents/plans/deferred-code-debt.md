# Plan — Source-comment debt and review follow-ups

**Status:** reviewed; work below is planned, not implemented
**Reviewed:** 2026-09-05 against `f7b434e`
**Scope:** all tracked source `TODO` and `ponytail:` comments, plus documentation-only matches.

## Goal and method

Make every deliberate shortcut actionable without implementing speculative fixes.
Keep this inventory authoritative; the programme, zero-trust and performance
plans link here instead of maintaining separate lists. This review also updates
those plans for the external HotStuff/PQC/learning-loop report and makes the root
README a navigation overview. No runtime behaviour changes in this effort.

Repomix packed all 97 Rust files without compression; searching
`\bTODO\b|ponytail:` found **7 markers: 6 ponytail + 1 TODO**. A repository-wide
`git grep -n -i -E 'TODO|ponytail:'` additionally found quotations in docs and
three `todo!()` placeholders in `PLUGIN_KIT.md`, not executable workspace code.
Line numbers below identify the reviewed revision; symbols are durable anchors.

## Inventory, grouped by file

| ID / source | What was simplified / ceiling | Disposition, trigger and smallest next step | Acceptance check |
|---|---|---|---|
| D1 — `crates/glasschain-core/src/endorsement.rs:395`, `PolicyHistory::default_policies` | Fixed `network-governance` fallback; unconfigured scopes fail closed. | **Keep the safe fallback.** Before multi-org deployment, document/test genesis principal provisioning and authorized `PolicyUpdate` bootstrap. Trace `operation_default` separately: capability activations still explicitly require the fixed principal; a scoped update must not be advertised as replacing that rule. Programme Stage 2 / zero-trust. | Unknown/missing governance signer rejected; authorized scoped update applies from the next block; replay derives the same policy. No allow-all bootstrap exception. |
| D2 — `crates/glasschain-core/src/endorsement.rs:533`, `operation_default` | Recall's intended two-party approval degenerates when envelope issuer and `issued_by` name the same principal; canonical record family/channel policy wiring is incomplete. | **Decision before regulated recall deployment.** Define whether a regulator may issue a unilateral recall or two distinct organizations are required, then bind record scope to committed policy. Do not invent a second authority to satisfy a count, and do not block a legally authorized emergency recall by assumption. Programme Stage 2; independent of ML. | Same-principal case exercises the explicitly chosen policy; duplicated signatures never manufacture another principal; wrong-scope endorsements and policy downgrades rejected at admission, commit and historical validation. |
| D3 — `crates/glasschain-core/src/ledger.rs:78`, `Ledger::add_transaction` | Rebuild capability history for canonical records/activations; scan all committed and pending transaction IDs. Cost grows with history and pool size. | **Measure now, optimize the shared path when attributed.** Performance Step 0/3/6: increasing history and burst/duplicate workloads. Prefer one rebuildable capability/ID index at the owning layer, not competing caches or a DAG as a first fix. | Before/after admission p95/p99 and memory versus height/pool size; duplicate IDs, activation boundaries, sync/restart, failed proposal restoration and direct `Ledger` callers retain identical results. |
| D4 — `crates/glasschain-identity/src/msp_policy.rs:10`, `MspEndorsementProvider` | Trusted key→principal registration is directory-based, not derived from verified certificates. | **Open decision:** [Certificate-bound MSP principals](https://github.com/dbbvitor/GlassChain/issues/87). Reuse verifier/CRL machinery; specify org binding, proof of key possession, expiry/revocation and height-based historical authorization before wiring remote principals. | Wrong-org, unknown key, revoked/expired credential rejected for new authorization; committed historical authorization remains deterministically verifiable. Never consult mutable current-time CRLs to reinterpret old blocks. |
| D5 — `crates/glasschain-storage/src/transient.rs:55`, `TransientStore::expiry_index` (module docs lines 19–23 give the upgrade) | Expiry index is in memory. Persisted payloads cannot be enumerated for purge after restart. | **Required before persistent private-data deployment.** Choose a bounded prefix scan or atomically maintained durable expiry index using the existing storage seam; no new storage engine. Also wire a purge schedule and deletion-error reporting—an API alone is not a retention guarantee. Stage 2 / zero-trust. | Persist payload, reopen store, expire, purge **without first reading the key**, and verify underlying key deletion; live records survive; interrupted deletes retry safely. Test both backends as applicable, and document backup/replica retention separately. |
| D6 — `crates/glasschain-workflows/src/triage.rs:26`, `FlowTriage` | Progress inventory vanishes on restart; a known flow is rediscovered only when driven again. | **Required for unattended purchase/recall recovery.** Enumerate saved checkpoints at startup into the triage view; reuse the narrow storage scan considered for D5 if both consumers need it. Do not create a generic query subsystem. Stage 3. | Reopen persistent checkpoints with a fresh triage instance; discover a waiting/stuck flow without a new event; preserve its timestamp; completed flows absent; discovery does not replay side effects. |
| D7 — `crates/glasschain-network/tests/madsim_chaos.rs:631` | TODO for madsim-tokio interception of real socket partitions; current test partitions at application level. | **Alternative deferred, outcome partly shipped:** [TCP-level fault injection](https://github.com/dbbvitor/GlassChain/issues/70) closed via `tcp_partition.rs`. Add WAN delays/jitter/bandwidth to that proxy first (performance Step 0). Revisit simulator-runtime migration only if pinned Tokio support and deterministic fault schedules add coverage the proxy cannot provide. | Established TLS session severed; advertised/reconnect addresses cannot bypass faults; partition + repair converges without conflicting finalization. Label real TCP wall-clock tests separately from deterministic simulated-network tests. |

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
