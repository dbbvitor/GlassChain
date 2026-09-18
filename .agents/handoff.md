# Handoff — GlassChain

**Reviewed:** 2026-09-14
**Source baseline:** `main` / `origin/main` at `4560f33` (PR #121 / ADR-015 `blst` backend, merged).
**Latest working sessions (2026-09-14):** performance steps worked in order —
Step 0 (per-phase round timing + D7 scenarios, ADR-016 durability decision),
Step 1 (codec profiled — JSON stays the wire), Step 3 (incremental D3
admission index: flat ~0.19 ms from ~21 ms at 10k), Step 6 installments
(bounded 8 000-tx pool, stats, priority lanes, handshake re-audit, batching:
4 000-tx slice — the previously failing 9 000-tx probe now sustains, 740 KB
blocks converge 8/8, p50 5 275 ms under 9 000-tx offered load), the
latency-opportunity plan with all gated items executed (concurrent vote
verification, lanes, height-bounded catch-up wire `/7`, reconnect backoff —
300-gate p50 4 612 → 4 117 ms), scale table complete (10/100/200/300 →
194 ms/1.1 s/2.5 s/4.1 s), §5 read-path gaps closed (lagging-subscriber
drops receiver-observable; burst-vs-steady sub-ms/block), Step 5 fault
profile recorded (fail-closed 3.01 s, heal 173 ms), #6 declined for now
(ADR-002 amendment note; rollback design doc exists), block-relay gossip
measured and reverted. Step 7's in-repo half shipped (validator-set churn across heights — ADR-009 reconfiguration exercised through the driver); Open: node-level peak-RSS harness; 400/500 sweeps under hardware budgets; Step 7's deployment half (failure-domain placement evidence, fleet participation) is deployer work.

## Start here

1. Read [AGENTS.md](../AGENTS.md) for repository rules, then the
   [plan index](plans/README.md) for concluded versus pending work.
2. Previous priority sequence: **Frontier C (Consensus Capacity / BLST Backend)
   → Frontier B (RBAC & Operational Tail) → Frontier D (Browser Demo)**.
   Frontier A concluded: `EquivocationProof` carries both dual-signed votes
   and verifies through the #95 context envelope. **Frontier C concluded
   2026-09-13** (ADR-015: audited `blst` backend, sum-of-keys verify,
   300-validator gate passing); Frontier B concluded 2026-09-14 via
   [ADR-017](../../docs/adr/adr-017-deployment-trust-and-retention.md)
   (OCSP stapling only, MSP admin RBAC, backup scrubbing, #74 deferred);
   next frontier is **Frontier D**.
3. Read [zero-trust §8](plans/zero-trust.md) for consensus safety invariants.
4. Read [source-comment debt](plans/deferred-code-debt.md) for settled D1–D7
   markers and benchmarks.
5. For the visual product, use the [browser demo plan](plans/gui-demo-benchmark.md).
   Web app replaces desktop gpui; Canvas2D baseline, optional WebGPU.

## Current state — code, decisions and evidence are different

| Area | Concluded / available | Still pending |
|---|---|---|
| Workspace | 12 Rust crates; wire `glasschain/6`; 17 accepted ADRs (ADR-016 durability, ADR-017 deployment trust); D1–D7 settled | No browser package or demo bridge exists |
| Ledger/execution | Schema v1, capability/policy history, explicit WASM write sets and replay | Production durability acknowledgement and historical security gates |
| Consensus | PoW dev/test default; BLS driver with context-authenticated votes (#95, #99), live receipt journal (#96), full historical QC verification on sync/restart (#97, PR #116), absolute phase deadlines/bounded queues/distinct voters (#98), dual-sign `EquivocationProof` (Frontier A concluded) | Production audit/testnet/APIs (ADR-010) |
| Identity/privacy | TLS/TOFU with durable pins & signed rotation (#88), opt-in verifier with fail-closed private paths (#86), session-bound possession proofs (#110), CRLs/intermediates (ADR-013), cert-bound MSP principals with height authorization (#87, D4), fail-closed governance fallback (D1), issuer-signed recall (D2), restart-safe purge (D5) & triage discovery (D6); **Frontier B concluded via ADR-017 + code (2026-09-14)**: OCSP staple minted per member, stapled on `Hello` and verified locally (no responder egress, CRL fallback fail-closed); `AdminGate`-gated channel-management RPCs over certificate-bound admin principals; `glasschain backup-scrub` retention sweep for storage copies; per-Hello org reauthorization (downgrade on failed re-verification) | Residual plan **shipped 2026-09-14** ([zero-trust-residual](plans/zero-trust-residual.md)): `--identity-file` durable custody (ADR-018 — the same identity key/cert/Root CA across restarts, pins keep verifying), `glasschain channel-admin` CLI client, `reload-trust-store` REPL hot-reload with `AdminGate` hot-swap, durable equivocation evidence via the state seam; #74 + delegated responders parked by decision |
| Workflows/read path | Checkpointed flow engine, purchase/recall flows, triage API with restart discovery (D6), provenance/flattener/event bus and RPC queries; D3 baseline measured (#106) | Unattended external integration, durable external indexer adapter, bounded projection costs |
| Measurements | BFT finality on `blst` (ADR-015): p50 1 145 ms at 100 / 4 612 ms at 300 (2026-09-14) with the per-phase decomposition recorded; the 300 gate passes and verify is no longer the wall (fan-out is). D3 admission bench (~21 ms at 10k); read-path memory baseline (#107); D7 WAN scenarios (proxy #108 + leader-quorum loss + bandwidth budget); Step 0 marked done | Long-run fleet memory; 400/500 sweep is out of scope; slow-CPU/disk WAN scenario |
| PQ readiness | Discriminants shipped; negotiated X25519MLKEM768 hybrid TLS behind `pq-tls` shipped (#105) | Long-term archive evidence / migration policy; no guaranteed quantum-safe lifetime |
| Demonstration | **Frontier D in progress 2026-09-17 (uncommitted):** `demo/` package — axum bridge + 15-node multi-company runner + Canvas2D UI; zero-trust evil nodes (membership/schema/fail-closed rejections all real), per-member own-node views + visibility matrix, throughput metrics; Rust tests own the guarantees; docs `docs/demo.md` | Plan steps 4–5 (faults, renderer comparison); browser-smoke CI |

The seven source markers (D1–D7) are fully settled (#106–#109, #114–#115):
D1 governance bootstrap documented; D2 recall issuer-signed; D3 admission cost
benchmarked; D4 cert-bound principals shipped; D5 restart-safe purge shipped;
D6 triage discovery shipped; D7 WAN proxy profiles shipped.

## Pending frontiers — what to do next and how to finish

### A. Consensus safety residual (concluded)

**Status:** core safety mechanisms shipped in #95, #96, #97, #98, #99, and PR
#116; Frontier A concluded with the `EquivocationProof` format migration:
the proof carries both conflicting votes in full and `verify()` rides the
dual-sign context envelope (`domain || chain-id || height || round || phase ||
block-hash`) introduced in #95, so evidence can no longer be assembled from
votes of different contexts. Residual evidence-path hardening beyond the
journal is future work (see zero-trust §8).

### B. Deployment trust, privacy and recovery (concluded 2026-09-14)

Code items D1–D6 and #86–#88 shipped with tests. Frontier B concluded via
[ADR-017](../../docs/adr/adr-017-deployment-trust-and-retention.md) **plus its
code** (implementation plan: [frontier-b-tail](frontier-b-tail.md)):
- **OCSP stapling** — the org Root CA mints an issuer-signed OCSP
  `BasicResponse` per member certificate; the node staples it on every
  `Hello` (`ocsp_response_der`), receivers verify it **locally**
  (`CertChainVerifier::verify_ocsp_staple`); a revoked staple fails the
  session closed, absent/invalid/expired staples fall back to the fail-closed
  CRL path. No outbound responder queries.
- **Operator RBAC** — member certs carry the admin role as subject OU;
  `AdminGate` gates `CreateChannel`/`AddChannelMember`/`RemoveChannelMember`
  on `NodeService`; without a verifier these fail closed.
- **Backup scrubbing** — `glasschain backup-scrub` runs the D5 sweep over a
  copied storage directory before archival.
- **Reauthorization** — the TOFU `Known` path assigns the fresh Hello's
  org-verification result (upgrade or downgrade).
- **On-chain revocation registry (#74)** — remains deferred.

Next active frontier: **Frontier D (Browser Demonstration)**.

### C. Transport and performance (concluded 2026-09-13)

Hybrid TLS negotiation shipped (#105). Step 0 prerequisites (WAN proxy #108,
D3 admission bench #106, read-path memory baseline #107) are complete.

- **Step 4 (BLS backend, issue #85) concluded via ADR-015:** the audited `blst`
  C backend replaces the pure-Rust `pairing` path; the same-message verify is
  the sum-of-keys PopScheme check over `blstrs` — two pairing terms at any
  quorum size. Measured: pure-Rust 80.0 ms → blst sum-of-keys 1.78 ms at
  quorum 201 (`cargo bench -p glasschain-core --bench bft_verify`).
- **Completion evidence:** the 300-validator finality gate now passes with
  exact-quorum certificates every round — p50 1 096 ms (100) / 2 397 ms (200) /
  3 996 ms (300), before/after in `docs/benchmarks/consensus-capacity.md`.
  Remaining round cost is mesh replication, not verification.
- Residual performance work stays on the performance plan: Step 1 codec
  profiling, D3 admission rebuild optimization, Steps 5+ research only.

### D. Browser demonstration (in progress — steps 1–3 + zero-trust slice 2026-09-17)

[Browser demo](https://github.com/dbbvitor/GlassChain/issues/61) steps 1–3 +
the docs half of step 6 shipped on the working tree (uncommitted): `demo/`
standalone package (excluded from the workspace, own lockfile) with an axum
bridge (loopback, Origin+token-gated commands, bounded SSE snapshots),
a headless **15-node multi-company runner** (3 manufacturers, 2 distributors,
2 logistics, 3 pharmacies, regulator, 2 certifiers) driving synchronized
rounds of 3 lots each (lot anchor → custody → member-only pricing payload →
quality_certification + audit_attestation → PoW mine at difficulty 1; one
block per round; reference run ~38 tx/s, commit p50 12 ms / p95 34 ms), and
a vanilla-JS Canvas2D UI with the WebGPU experiment behind the measured-budget
gate (p95 ≤ 16.7 ms at ~500 elements — real-device comparison pending).

**Contract + parameters + graph slice (2026-09-17, second pass):** live-editable
simulation parameters (companies per role, evil count, lots/round, interval —
`POST /api/params`, sanitized and clamped; topology changes rebuild the
federation mid run at a round boundary via a version counter), a real
contract flow (`auto-replenish` contract registered in setup; rotating
manufacturers submit SupplyOffers; the engine's `offer_matches` + auto-execute
generate real PurchaseOrders surfaced in an offers table), traffic-weighted
canvas edges (width ∝ sqrt of per-pair tx count, with moving dots), and an
interactive graph (click node = view as that member via its own-node
snapshot, drag to rearrange, hover tooltip, one node per company).

**Cert drawer + scrollable tables (2026-09-17):** cert rows clickable →
`openCertDrawer` (details + the related lot's custody chain, highlighted
on the graph). Long tables (purchases/certs/security/visibility/directory)
wrapped in `.table-scroll` (max-height + overflow-y). The drawer's
counterparty SELL/BOUGHT list became a table with status badges
(`sellsList`/`buysList` renamed — the KPI block reuses `sells`/`buys`).

**Stale-build check + drawer padding (2026-09-17):** the served assets were
verified directly (curl style.css → `padding: 1.6rem 2.6rem`; curl app.js →
the process branch present) — the user's screenshot was a stale build.
Belt-and-braces: `#drawer-content` also carries horizontal padding, and
the PDC table's describeTerms call is throw-safe.

**Related-scope + process steps + more kinds (2026-09-17):** org views
are server-scoped to RELATED payloads (author == org OR the payload's
lot was manufactured by the org — `lot_maker` map built from
`state.lots`, `OrgPayload.lot` parsed server-side from the ledger
cleartext). The manufacturer records **intermediate process steps**
(mixing, packaging) as same-member `InventoryUpdate` chain txs — no
member-to-member transfer — plus a `process` payload kind; the
distributor adds an `intake` checklist payload and logistics a
`temperature_log` payload at their hops (multiple kinds per org type).
Drawer padding: the deep padding was in the CSS but the user's build was
stale — bumped to 2.6rem to force a diff.

**Five payload kinds (2026-09-17):** the certifier disseminates private
certification_evidence (findings/samples/lab ref/next audit) and the
regulator private compliance notes (kind=regulator_notes, authored
regulator-1) when the cert/audit records anchor. `describeTerms` and
KIND_LABEL are kind-aware; drawer padding widened (2.4rem horizontal).
Live: admin lens {pricing 15, certification_evidence 12, regulator_notes
12, storage 12, transit 15}; regulator-1 reads its own notes + all
collection kinds (membership).

**Skeleton rollback (2026-09-18):** the three-piece skeleton broke
mobile rendering per user report — ROLLED BACK. Restored state:
`drawer-scroll-outer` wrapper (boot self-heal rebuilds it on cached
pages), padding on `#drawer-content` (1.8rem 2.4rem), scroll strip
flex:1/min-height:0/overflow-y:auto WITH `-webkit-overflow-scrolling:
touch` + `overscroll-behavior: contain`, and the absolute
`.drawer-resize` handle strip. All drawer-resize/drawer-main/
drawer-main-block CSS removed.

**Structural drawer rebuild (2026-09-18):** after two failed padding/
scroll rounds on mobile, the drawer became three structural pieces:
`#drawer-resize` (flex strip, ew-resize cursor, separate from the scroll
direction) + `#drawer-main` (relative column holding the pinned close
button and `#drawer-scroll`) + `#drawer-scroll` (flex column's
`flex: 1; min-height: 0; overflow-y: auto` + touch attributes). Padding
lives on `#drawer-content` (1.8rem 2.4rem) — present in every drawer
regardless of content height or structure. JS self-heal rebuilds this
skeleton on cached pages. If a user STILL sees breakage: the served
binary predates the build — verify with
`curl :18850/style.css | grep drawer-scroll` (must show the new block).

**Ownership-scoped PDC lists + single scroll surface (2026-09-18):** the
member drawer lists ONLY the payloads the viewer authored (all readable
— ownership scopes the list; the regulator/admin see the whole
collection). `.drawer-scroll` lost its 18em cap — no nested scroll
traps; the drawer's scroll-outer is the single scrolling surface.

**Admin drawer crash fix (2026-09-17):** the `pricing`
collection now carries THREE payload kinds per lot, each authored by its
own org — `pricing` (manufacturer), `storage` (distributor: temp/humidity/
retention), `transit` (logistics: route/window/cold-chain) — submitted at
the matching custody hop from the star center; origin-scoped visibility
unchanged. Adversarial attacks diversified to five real code paths:
payload smuggling (membership/fail-closed), unknown-payload-key forgery
(schema), tampered-commitment lot anchor (anchored-family hash check),
duplicate transaction replay (pool idempotency), and the under-metadata
registration (trust scoring). `docs/demo.md` gained a **"What is native
GlassChain vs. demo-side bookkeeping"** table: demo cash, sellable
inventory, stock valuation, offer-fill counters, buy-pressure/retail
scaling, the staged-pipeline cadence, and the star topology are
demo-side; all on-chain transactions and gates are native.

**Rich lot PDC info (2026-09-17):** the lot inspector's PDC section
lists ALL payloads for the lot (up to three kinds) as a table — kind,
author, readable terms (describeTerms), truncated commitment hash,
access badge — plus a dissemination summary line. Role messages adapt
(own / regulator-style readable / commitments-only / public). Drawer
padding deepened (1.6rem 2rem) and width raised to 30em.

**Drawer flex-clipping fix (2026-09-17):** the scroll strip lacked
`min-height: 0` — the classic flex-column trap. The strip grew to its
content height, the drawer's `overflow: hidden` clipped everything
(no scrolling, the bottom padding invisible). Fixed + drawer tables/KPIs
now fit the drawer width (`width: 100%` + `word-break` cells).

**Padding-on-content (2026-09-17):** the drawer padding moved onto
`#drawer-content` itself (the element every drawer opens into — present
in both the restructured and legacy layouts), eliminating the
structure-dependent padding entirely; the scroll strip only scrolls.
Drawer tables get `min-width: 24em` so narrow screens scroll them
horizontally inside their wrappers instead of crushing columns.

**Dead-rule purge (2026-09-18):** found the underlying damage this
round: the `.drawer-resize`/`.drawer-scroll` rules from the rolled-back
skeleton were targeting elements removed from the HTML — deleted.
style.css is now single-source for the drawer: exactly one
`#member-drawer` block, one `#drawer-scroll-outer` block, one
`#drawer-content` padding rule, one `#drawer-close`.

**style.css drawer-rule restore (2026-09-18):** the MAIN `#member-drawer`
layout rule was missing from style.css (splice-shredded; only a stray
light-theme copy + duplicate `#drawer-close` remained) — dark mode had
NO drawer layout, which is the root of every recent "persisted" report.
Rebuilt as one clean block; purge of stale duplicates followed. Verify
served assets before debugging: `curl :18850/style.css` must contain
exactly one `#member-drawer {`, one `#drawer-close {`, one
`#drawer-content { padding`.

**Server-described PDC rows (2026-09-18):** `OrgPayload` gained
`kind` + `summary` computed server-side in `org_snapshot` (from the
ledger cleartext — the server always sees it). The drawer table renders
`value.kind`/`value.summary` directly; commitment rows now describe
themselves (kind + terms summary + hash) instead of "—". Lot/cert
drawers render `lotPayload.summary`. The JS `describeTerms` client
parser is now legacy (kept for the fallback path).

**Drawer scroll hardening (2026-09-18):** the scroll strip abandoned the
flex arrangement for an explicit absolute pin (top/left/right/bottom:
0, height 100%) with `-webkit-overflow-scrolling: touch` +
`overscroll-behavior: contain` — mobile touch scrolling queued properly,
and no flex ancestor can clip it.

**Stale-page self-heal (2026-09-17):** the user's "problems persisted"
screenshot matched a browser-cached PRE-restructure page (old HTML: no
`drawer-scroll-outer` wrapper + new CSS = clipped, unscrollable,
unpadded drawer). The boot now self-heals: if `#drawer-scroll-outer` is
missing it rebuilds the wrapper (moves `#drawer-content` inside) and the
CSS handles both structures (`#member-drawer > #drawer-content` is also
a scroll strip with the deep padding). Verified with a stale-page
harness. If a user still reports stale assets: hard-reload the page —
assets are served `no-store`.

**Drawer resize structural fix (2026-09-17):** the resize handle was an
absolutely-positioned child of a SCROLLABLE drawer — it scrolled away
with content, so dragging became impossible once the body grew. The
drawer is now a flex column with a dedicated non-scrolling
`drawer-scroll-outer` strip for the body and the handle pinned to the
drawer itself. Drawer tables each got a top drag handle wired through
`scrollWrap` (max-height drag). Gotcha: a later splice re-declared
`drawerShell` (duplicate identifier) — dedupe by line index.

**SSE refresh viewer fix (2026-09-17):** `refreshOrgView` (the SSE-tick
path) fetched the member view WITHOUT the viewer param and overwrote
`orgSnapshotData` every 500 ms — so the lot inspector's lookup source had
no cleartext and lots always read "has not been disseminated". Fixed
(`?as=X&viewer=X` on that path too); the cert/lot drawers got honest
fallbacks when a payload hasn't arrived yet.

**PDC inspection kind-aware (2026-09-17):** the member/cert/lot drawers'
PDC inspection lists the WHOLE collection with per-row access badges
(the earlier origin-only list hid the diversity). The table gained a
Kind column with per-kind detail rendering
(`describeTerms(terms)` in app.js). Kind data is only visible in
cleartext — commitments don't carry it — so a member sees its own kind
rows readable and the others as commitment rows (tests assert readable
kinds ⊆ own-author). The tests module lost `use super::*;` to a splice —
restored (63 test compile errors was the symptom).

**Skeleton rollback (2026-09-18):** the three-piece skeleton broke
mobile rendering per user report — ROLLED BACK. Restored state:
`drawer-scroll-outer` wrapper (boot self-heal rebuilds it on cached
pages), padding on `#drawer-content` (1.8rem 2.4rem), scroll strip
flex:1/min-height:0/overflow-y:auto WITH `-webkit-overflow-scrolling:
touch` + `overscroll-behavior: contain`, and the absolute
`.drawer-resize` handle strip. All drawer-resize/drawer-main/
drawer-main-block CSS removed.

**Structural drawer rebuild (2026-09-18):** after two failed padding/
scroll rounds on mobile, the drawer became three structural pieces:
`#drawer-resize` (flex strip, ew-resize cursor, separate from the scroll
direction) + `#drawer-main` (relative column holding the pinned close
button and `#drawer-scroll`) + `#drawer-scroll` (flex column's
`flex: 1; min-height: 0; overflow-y: auto` + touch attributes). Padding
lives on `#drawer-content` (1.8rem 2.4rem) — present in every drawer
regardless of content height or structure. JS self-heal rebuilds this
skeleton on cached pages. If a user STILL sees breakage: the served
binary predates the build — verify with
`curl :18850/style.css | grep drawer-scroll` (must show the new block).

**Ownership-scoped PDC lists + single scroll surface (2026-09-18):** the
member drawer lists ONLY the payloads the viewer authored (all readable
— ownership scopes the list; the regulator/admin see the whole
collection). `.drawer-scroll` lost its 18em cap — no nested scroll
traps; the drawer's scroll-outer is the single scrolling surface.

**Admin drawer crash fix (2026-09-17):** `adminData` was block-scoped
inside `if (admin)` but referenced in the PDC section below — Admin-mode
member drawers threw `adminData is not defined` (the member harness in
admin mode caught it). Hoisted to `let adminData = null`. The admin lens
also carries `private_visible: true` now.

**10 ms budget + private gate + param maxima (2026-09-17):** (1) the Trade
misalignment root was `.chart-card + .chart-card` margin pushing the
second card down inside its row — scoped to `.wms-charts + .chart-card`.
(2) The renderer budget is 10 ms p95 at the target load (WebGPU gate
re-evaluated against it). (3) SimParams maxima raised in form and
sanitize lockstep (37-node ceiling). (4) `private_visible` gate: cash +
trade bookkeeping visible only to the member, the regulator, and Admin —
the public lens and other members get null/empty with an explanatory
note in the drawer.

**Viewer-carrying page view + generic resizers (2026-09-17):** THE root
cause of "cannot read PDC info on lots": `refetchSnapshot` fetched the
member view WITHOUT the viewer param — `viewer == None` means no
cleartext anywhere, so the lot inspector always said "not disseminated".
Fixed (`?as=X&viewer=X`). The cert drawer shares the viewer-scoped PDC
lookup. Trade columns switched to the flex equal-height pattern. Every
`.table-scroll` gets an auto-attached resize handle (lots keeps its
explicit one).

**Drawer resize + cert-chain fix (2026-09-17):** the member drawer got a
left-edge resize handle (width drag) and its inner tables wrapped in
`.drawer-scroll` (bounded scroll). Cert drawer related-chain lookup
normalizes lot_ref case (`lot-1` certs vs `LOT-1` lots — the case
mismatch made EVERY cert claim an uncommitted chain).

**Origin certification + lot PDC terms (2026-09-17):** certification +
audit now issue the round after a lot ENTERS the pipeline (stage-agnostic
— GxP certification covers the manufactured batch, not the pharmacy
arrival). The lot inspector shows the readable terms inline when the
impersonated role may read them (own lots / regulator / admin), with
role-appropriate messages otherwise. Gotcha: `openMemberDrawer`'s
counterparty table declares `sellsList`/`buysList` — the KPI block owns
`sells`/`buys`; re-splicing the drawer can silently drop the
declarations (the live-data member harness caught it).

**Drawer UX fixes (2026-09-17):** (1) manufacturer nodes were unclickable
— tx dots spawning from them won the hit test; nodes now take priority
over dots in mouseup/hover. (2) The banner is always visible and defaults
to Public with reflecting text (member/admin variants). (3) Drawer close
✕ centered via grid place-items. (4) PDC inspection is the drawer's last
section, rendered as a table (lot / author / member price / list price /
units / access badge) with a summary line; a member with no authored
payloads shows an explanatory empty state.

**Lot-drawer crash fix + node clicks (2026-09-17):** openLotDrawer threw
for chain-less lots (`list` scoped inside the stage branch, then used by
the certs loop) — clicking such lots did nothing; hoisted. The lot PDC
check now matches JSON terms by lot number (the old placeholder-string
comparison never matched). Node clicks open the viewer-scoped drawer
without touching the view-as selector (viewers see the clicked member
through their own rights).

**Drawer inventory-by-lot (2026-09-17):** the member drawer gained an
inventory table — every lot whose committed custodian is the member
(SKU, lot ref, units 500, acquired-at block, source manufacturer),
client-side from `latest.lots`; the orders table gained a Round column.
Note: the staged pipeline needs ~4 rounds before a pharmacy holds lots —
early snapshots legitimately show empty inventory tables.

**Drawer orders-overview (2026-09-17):** the member drawer is a mini
Stockify orders page — 4 KPI cards (concluded sells, pending sells,
concluded buys, inbound lots from `pending_inbound` = pipeline entries
addressed to the member), an order-status conic donut with legend, and a
recent orders table (sell/buy rows with status badges). Trade columns get
`grid-auto-rows: 1fr` + flex stacks — genuinely equal heights now.

**Viewer-scoped drawers (2026-09-17):** `org_snapshot(shared, org,
viewer)` — the drawer for member B through viewer V lists ONLY B-authored
payloads, readable iff viewer==B (transient-checked) / regulator / admin;
`viewer == None` (public) reads nothing. Member page views list only
their own payloads (no cross-member rows at all). Drawer summary:
"`org`'s payloads on the chain: N · readable by <viewer>: K". A member
view now lists only its own payloads — no cross-member commitment rows.

**Admin view + alignment (2026-09-17):** `?as=admin` is the demo's
privileged lens — all payload cleartexts + a `cash_all` map (clearly
labelled; the chain carries no global read). The selector appends an
"Admin (demo)" option; in admin view the member drawer's PDC inspection
reads the admin snapshot and cash comes from `cash_all`. Trade columns
are equal-height (stretch), Inventory card spacing standardized
(`.wms-charts + .chart-card` margin + section rhythm).

**Origin-scoped PDC + offers reflow (2026-09-17):** payloads carry their
author (`PayloadLedger` = (collection, commitment, cleartext, author)) and
`org_snapshot` is origin-scoped — a member's own terms in cleartext
(verified against its own transient store), other members' terms as
commitments (`payload: None`), the regulator everything. The drawer
renders other members' entries as "commitment only" with the reasoning.
Pending/executed offers moved out of the sidebar into two side-by-side
chart-cards in the Trade section; Inventory section gaps widened.

**Stockify shell (2026-09-17):** the page is a two-column shell — a left
org/nav sidebar (brand, section anchors, member selector + "Return to
public view", pending/executed offers, theme toggle) and the main content
(header + KPIs + charts + directory + lots + trade tables). Selecting a
member shows a viewing-as banner, highlights `tr[data-member]` /
`.wms-bar-row` rows, and opens the drawer — the dropdown now visibly
changes the page. Motion button removed (permanently reduced flow,
`dataset.motion = "flow"`), directory Status/Action columns removed
(rows clickable), gaps widened, a Purchases table in the Trade section,
and the render harness gained classList/querySelectorAll stubs + the
member-view path (refetchSnapshot + updateViewBanner are exported).

**Real pricing terms in PDC payloads (2026-09-17):** the `pricing`
payload cleartext is now `pricing_terms(seq, pressure)` — JSON with list
price, 12.5 % member discount, quantity, currency, payment terms — written
from the star center so every member holds every term (one-hop
dissemination property, same pattern as the PDC benchmark). The drawer
renders the JSON as fields. Tests assert `member_price_per_unit` in
payloads, not placeholder strings.

**Staged pipeline (2026-09-17):** `SharedRun.pipeline: Vec<PipelineLot>`
(seq, stage 0..3, rotation, certifier, certified) — each round advances
each lot ONE hop (dispatch/receive submitted by the handover member),
new lots enter at `lots_per_round`/round, certification+audit submit the
round AFTER a lot reaches stage 3 (pharmacy), entries retire when
certified. Lot statuses: manufactured / at distributor / in transit /
complete. Consequence: upstream members hold stock too (the WMS now
shows stock spread across every role) and no intra-lot ordering races
remain (one hop per round per lot makes provenance order correct by
construction). `run_rounds(shared, n)` is the multi-round test seam;
staged tests need 5-6 rounds to see pharmacy stock.

**WMS reference layout + lot inspection (2026-09-17):** the WMS section is
now the Stockify layout — KPIs, "Stock Level by Member" horizontal bars
(clickable), "Inventory Value Share" conic-gradient donut with legend, and
an "Inventory Directory" table (click/Manage → member drawer). Lots rows
are clickable → lot drawer (custody timeline, certs, PDC info for the
current view) that highlights the lot's custody path in accent on the
live graph while open (`highlightPath`, cleared on drawer close).

**WMS Stockify redesign (2026-09-17):** `WmsSummary` header KPIs
(members/total units/total value = on-hand × RETAIL_PRICE/total sold
retail/low-stock count = sellable below one retail drain) + cards with
health badges, stock-value and sellable stats, alert pills, movement row
and "Manage member →" footer (click = member drawer). WmsRow gained
`stock_value_minor` + `sellable_units`.

**Stock-scaled buy pressure (2026-09-17):** `buy_pressure(RunState)` =
1 + system_stock / 5 000 (capped at 2); `offer_quantity(seq, pressure)` =
min(500, ladder × pressure) — scarce stock keeps the 300/400/500 partial
ladder, piled-up stock buys full 500-unit lots, pushing goods to the
retail drain; retail per pharmacy scales with the same pressure. The
partial-manual test asserts against the offer's actual quantity (pressure
changes it — never hardcode ladder numbers in tests).

**Retail flow + resizable + PDC inspection (2026-09-17):** pharmacies sell
100 units/round from tracked sellable inventory (`RunState.inventory`,
gained at each handover) to customers via real committed
`InventoryUpdate`s — deliberately slow (a warehouse drains only over many
rounds), revenue into demo cash, NOT animated on the graph; the WMS cards
are clickable (member drawer: inventory, cash, bought/sold summary) and
show `in · out · sold retail`; the lots scroll container got a drag
handle (`#lots-resize`); pending/executed offers sit side by side; the
member drawer's PDC inspection lists the collection's payload count with
the cleartexts the member holds (commitments-only for non-members); the
dot flow period is 2400 ms. Gotcha: retail revenue changes buyer cash —
cash tests must be delta-based around the movement they check.

**Sidebar split + partial auto lots + stable tables (2026-09-17):** the
offers sidebar separates pending (Buy spinner) from executed (purchases +
filled offers); the offer ladder now varies the offered quantity
(300/400/500 of the 500-unit lot) so auto purchases are partial lot buys
with a `sold` counter filled by the committed-block scan (match by seller +
price); the lots table is append-only (LOTS_CAP 500, practically
unbounded for a session), fixed-layout with ellipsized chains in a
scroll container. Tests: 13 (auto fill marking, ladder variety, no
downsizing).

**Render-blank fix (2026-09-17):** the diff-render rewrite dropped
`renderIfChanged`'s definition, so the first snapshot threw inside the SSE
handler and the page stayed blank while the backend ran — the exact
"can't see results" report. Restored, plus: the SSE and refetch handlers
wrap rendering in try/catch (a render bug logs and keeps the stream alive,
never blanks the page), and app.js exports the render functions. A Node
harness stubs the DOM and drives two real snapshots through renderDOM +
the visibility matrix — verified 12 lots / 10 WMS / 8 offers render and
the second tick skips rebuilds.

**Theme + motion + stable-input slice (2026-09-17, sixth pass):** light/dark
toggle (`gc-theme` in localStorage; light palette mirrors the dark one),
a Motion pause toggle (reduced motion default: slower capped flow,
`dataset.motion` drives the canvas), and section diff-rendering
(`renderIfChanged` per panel) so nothing rebuilds while unchanged —
hover/click/typing survive ticks. The Buy quantity input preserves the
user's typed value per offer (`typedQty` map) and is never snapped back.

**Partial-buy + member-holdings slice (2026-09-17, fifth pass):** manual buys
carry a quantity (any amount up to the offer's remainder — `OfferEvent.sold`
tracks partial fill, the note switches to "partially bought … still on offer"
and finally "fully bought"); demo cash bookkeeping (`RunState.cash`,
CASH_START per company, moved buyer→seller per committed purchase, labelled
demo-only); the member drawer now shows stock on hand, cash, sell offers and
purchase history; transaction dots flow only while the run is active (a
stopped run freezes); the graph click bug fixed (mousedown had claimed every
click as a drag — clicks now arm a candidate that becomes a drag only on
movement). Gotcha: never hold a `SharedRun.state` guard across an await —
even in tests it deadlocks (tokio::sync::Mutex fairness + the awaiting task
order); a test guard at function scope deadlocked the whole test.

**Transaction-flow + manual-buy slice (2026-09-17, fourth pass):** a
per-transaction ledger (`RunState.transactions`) drives the graph: every dot
is a real tx flowing along its edge (kind-colored), hover explains it, click
opens a details drawer (id, kind, flow, height). A sticky offers sidebar
shows all advertised offers; premium offers (every third, priced above the
auto cap) match only the new `manual-review` contract (`auto_execute=false`)
and wait for a human: `POST /api/purchase {offer_tx_id, buyer}` completes
them as the viewing pharmacy through a real PurchaseOrder. Edge counters no
longer evict (bounded by company pairs); offers cap 80 with committed-row
dedupe so the manual purchase row survives.

**Polish + BI slice (2026-09-17, third pass):** Apple-grade design system
(frosted sticky header, hairline cards, tabular numerals, pill controls,
dark glowing canvas, slide-over member drawer with backdrop blur), a WMS
dashboard derived from committed custody chains (per-company lots-on-hand,
units, cumulative receipts/dispatches, bar fills), and fully explained
interactions — every security row carries a Rust-side `explanation` naming
the gate that answered, expandable on click and mirrored in the member
drawer; metric cards/controls/table headers carry tooltips.

**Zero-trust slice (2026-09-17):** two evil companies attack every round and
every rejection is a real code path — `QuimicaFalsa` (verified, not a
`pricing` member) dies on the membership gate and on strict ADR-006 schema
validation; `DipFakeCerts` (no verifier) dies on the fail-closed gate (#86)
and its under-metadata registration is admitted with a flagged trust score.
Per-member views (`?as=<company>`) read each member's own node (chain height
+ transient store), and the UI has a visibility matrix + security feed.
Equivocation events are surfaced if the staged BFT engine ever emits them;
the UI labels that capability unavailable (no simulated proofs).

Engine caveat documented: the wire engine relays transactions one hop, so the
star center (`FarmaGen`) is the block producer; every company dials it.
Gotcha recorded: never construct nodes with `127.0.0.1:0` — `listen_addr()`
echoes the configured string; reserve concrete ports via
`stash_prebound_listener` (the repo's prebound-listener pattern), and give
the star center its address explicitly (BTreeMap order is alphabetical, not
insertion order). Custody hops submit sequentially with per-hop pool
confirmation (`wait_for_ids` on exact tx ids — pool depth counting is fooled
by extra traffic, and provenance preserves relay arrival order, so unordered
batching shuffled lot chains).

Rust tests own the guarantees: collection membership, one full round through
the real federation, the evil attacks, and per-member org views. Docs:
`docs/demo.md`, README quick start, `docs/README.md` index, glossary term
**synthetic demo run** in `CONTEXT.md`; decisions recorded in the plan's
"Settled decisions" section. Remaining: plan steps 4–5 (fault scenarios +
renderer comparison), browser-smoke CI. Note: demo Rust gates are separate
(`cargo test --manifest-path demo/Cargo.toml`); workspace gates do not cover
it. This can be built without waiting
for speculative consensus, FL, an archive TSA or a production REST gateway, but
must label the staged engine and unresolved privacy/recovery guarantees honestly.

### E. Deferred research

PQ archive evidence needs trusted time, preserved validation material, renewal,
retention and legal/profile review. Learning starts with offline outcomes against
a rules baseline; FL remains a SHOULD. Neither changes `SCHEMA_V1` or bypasses
endorsement. Use the relevant plans rather than inventing a new platform now.

## Validation and PR procedure

Local validation completed on this branch, 2026-09-12 (worktree target dir,
Rust 1.98.1 — the branch also pins the toolchain, so the gates below ran on
it):

- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets --all-features --locked`: passed.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D
  warnings`: passed, zero diagnostics (three 1.98 lint fixes: the SDK client
  constructor is now sync and infallible, and two CLI log borrows dropped).
- `cargo test --workspace --lib --bins --tests --all-features --locked`:
  passed — 589 tests across 33 harnesses with parallel harnesses (the
  new shared port-band allocator removes the serial constraint; bench
  executions run in the new Benchmarks workflow instead of the test gate).
- `cargo bench -p glasschain-core`: passed in release on 1.98.1 (the
  bench.yml command shape); the vm/workflows benches share the same shape.
- Coverage-specific: `cargo tarpaulin -p glasschain-network --all-features
  --lib --tests --locked --engine llvm` passed twice after fixing the
  instrumented-timing flakiness in the BFT vote collector tests — the
  collector now skips verification for duplicate copies of an already-counted
  voter (§8.4 flood relief), count-invariant tests use a generous window
  (the collector still exits at quorum), and the deadline-control asserts
  its structural bound (never ≥ 3) rather than an exact count.
- Large ignored scale/WAN gates were not re-run; prior numbers remain dated evidence.

For the PR, verify all local links, marker coverage and whitespace; fetch origin
and confirm no conflicts. `.github/workflows/ci.yml` filters docs-only changes,
so manually dispatch **CI on the final branch SHA** to exercise all platforms,
coverage and dependency audit. Inspect CodeQL/code-quality checks too; a local
pass is not remote green. Remote statuses belong on the PR, not a permanent
“all CI green” claim here. Do not weaken rules or suppress a failing check.

On resumption, read the PR's live checks and compare its base to `origin/main`.
If GitHub's external analysis service fails, record its exact run/error and stop
short of claiming merge readiness. Keep the PR open; merge only on explicit request.

## OpenSSF compliance track (2026-09-15)

Implemented the unblocked OpenSSF tickets (#125–#128) on the working tree —
uncommitted, see `.agents/plans/openssf-compliance-implementation.md` for the
full list. Per-file SPDX+copyright headers landed on all 108 `crates/**/*.rs`;
new workflows: `fuzz.yml`, `reproducible.yml`, `release.yml`, plus
`codecov.yml` (≥90% blocking) and a DCO check job in `ci.yml`. GitHub issue
writes are blocked on a valid oauth token (current one 401s) — resolution
comments/closures and the #123 map update are pending that.

**Update (2026-09-15, later):** `gh` re-authenticated — all pending tracker
writes done: #125–#128 claimed + resolved + closed, map #123 updated,
`main` branch protection enabled with required checks (Format, Clippy,
Test ×3, Code coverage, DCO sign-off, Security audit, `codecov/project`).
Remaining: #129 (synthesis) is now unblocked.
