# Demo — `glasschain-demo`

A local browser demonstration of a real GlassChain federation moving synthetic
supply-chain transactions. **A synthetic demo run is presentation, never
evidence** — see the glossary term in the repository root `CONTEXT.md`. Nothing
shown here establishes the [ADR-010](adr/adr-010-consensus-adoption.md) testnet,
security, or scalability gates; those live in
[`benchmarks/consensus-capacity.md`](benchmarks/consensus-capacity.md).

## Run it

```bash
cargo run --manifest-path demo/Cargo.toml          # binds 127.0.0.1:18850
# or: cargo run --manifest-path demo/Cargo.toml -- 127.0.0.1:19500
```

Open `http://127.0.0.1:18850/`. The page pulls `/api/bootstrap` (the session
capability token, same-origin) and the initial snapshot; **Start** launches the
synthetic runner, **Stop** aborts it, **Reset** clears the shared state. A
validator set is not required — the demo federation is six in-process nodes
(PoW dev/test, difficulty 1).

## What it demonstrates

- **Multi-company federation, synthetic workload, operator-tunable.** The
  federation comes from parameters, editable live — even mid run — through the
  page's form or `POST /api/params` (token-gated): companies per role
  (manufacturers/distributors/logistics/pharmacies/regulators/certifiers),
  evil-node count, lots per round, round interval. Topology changes rebuild
  the federation automatically at the next round boundary (the run and its
  counters survive; the status pill shows `rebuilding`). Each node is a real
  `Node` with its own identity, minted by one demo root organization — a
  simplification; the production trust model is ADR-011's federation trust
  store, not the demo's job.
- **Pending vs executed offers.** The offers sidebar splits in two: pending
  (awaiting buyer, with the quantity spinner) and executed (committed
  purchases plus filled offers). Every offer covers only **part of its
  500-unit lot** — the ladder (300/400/500) makes auto purchases partial lot
  buys, tracked with a `sold` counter ("auto-executed: 400 of the lot's 500
  units bought"); the remainder stays in the warehouse.
- **A staged pipeline — everyone holds stock.** Lots advance **one custody
  hop per round** through `RunState`-backed pipeline entries (anchor →
  manufacturer → distributor → logistics → pharmacy, certification when a
  lot arrives), so goods physically rest at every member between rounds.
  The WMS view now shows manufacturers, distributors and logistics each
  holding on-hand units alongside the pharmacies, exactly as asked. A lot's
  full journey takes ~4 rounds; certifications and audits commit only after
  the lot actually reaches the pharmacy. The lots table is append-only for the session (a
  high cap guards only extreme runs): no rows are shed mid-session, the
  layout is fixed-width with single-line ellipsized custody chains, and the
  table scrolls inside a bounded container — no jumping.
- **The contract match, end to end — with a human buyer.** The runner
  registers two real contracts on the same product, both owned by the first
  pharmacy: `auto-replenish` (price cap 1200, `auto_execute`) and
  `manual-review` (ceiling 2000, `auto_execute=false`). Rotating
  manufacturers submit `SupplyOffer`s; offers under the cap are matched and
  auto-executed by the engine; premium offers (every third, 1400) match only
  `manual-review` and are advertised as **awaiting a buyer decision**. The
  offers sidebar shows all recent offers; the user completes a pending one
  with a **quantity spinner** (partial buys: any amount from 1 up to the
  remaining units — the offer stays open with a `sold` counter until it is
  fully bought). Buy submits a real `PurchaseOrder` through the viewing
  pharmacy's own node (`POST /api/purchase`, token-gated) — admission, relay
  and commit are the same code path as everything else. Each member's drawer
  shows what it holds: stock on hand (committed custody), a demo cash
  balance (bookkeeping moved buyer→seller per committed purchase, labelled
  demo-only — the chain carries no balances), its advertised sell offers and
  its purchase history. The animated transaction dots flow only while the
  run is active; a stopped run freezes the picture. The contract-flow
  story is the engine's own behavior, not a staged simulation.
- **Lot inspection.** Clicking a lot row opens its drawer: the full
  committed custody timeline (event → custodian → block), its
  certifications/audits, its PDC payload status judged from the VIEWER's
  snapshot (own terms readable when the lot is yours; other members'
  terms never readable by you), and **its custody path highlighted in
  accent on the live graph** while the drawer is open. The drawer works
  for chain-less lots too (the certs loop no longer throws). Clicking a
  graph node opens the member's drawer WITHOUT switching the global
  view-as selector — drawers are viewer-scoped (`?as=X&viewer=V`).
- **Transaction-level graph.** Every recent transaction is a moving dot on
  its real edge (colored by kind: lot, custody, offer, purchase,
  attestation); hover shows what it is; clicking a dot opens the drawer with
  the transaction's id, kind, flow (from → to) and committed height.
  Clicking a node still opens the member's own-node view; dragging
  rearranges.
- **Synchronized scenario rounds with speed.** Each round commits
  `LOTS_PER_ROUND` (3) lots across rotating real companies through the full
  custody chain — manufacturer → distributor → logistics → pharmacy, each
  hop submitted by the handover company and confirmed in the miner's pool
  before the next dispatch (a lot cannot be received before it is
  dispatched) — plus a member-only `pricing` PDC payload disseminated to the
  whole collection, and public `quality_certification` +
  `audit_attestation` records from a rotating certifier. All of a round's transactions land in one mined block
  (dev-PoW, difficulty 1); a reference run sustained ~38 tx/s with commit
  p50 12 ms / p95 34 ms on a laptop — labelled dev-PoW, never compared to
  the ADR-010 gates.
- **Diversified private payloads.** Every lot now carries THREE
  origin-scoped payloads: the manufacturer's **pricing terms** (the
  12.5 % member discount), the distributor's **storage terms**
  (temperature range, humidity, retention, release conditions), and
  logistics' **transit terms** (route, transit window, cold-chain
  commitment) — each authored by its org, submitted as the lot crosses
  into its custody. Origin scoping applies per payload: you read your
  own class of terms; the regulator reads everything.
- **Zero trust, with evil nodes that attack for real — five attack kinds.**
  Every round the evil companies run: a private-payload attempt (membership
  gate / fail-closed #86), a forged certification with an undisclosed
  payload key (strict schema), a **lot anchor with a tampered commitment**
  (anchored-family hash check), a **duplicate transaction replay** (pool
  idempotency), and the under-metadata registration (admitted, trust score
  reduced). Every outcome is a real GlassChain code path. Every round:
  `QuimicaFalsa` (verified identity, deliberately not a `pricing` member)
  attempts a private payload (→ membership-gate rejection) and a forged
  certification with an undisclosed payload key (→ strict ADR-006 schema
  rejection); `DipFakeCerts` (no certificate verifier at all) attempts a
  private payload (→ fail-closed zero-trust gate #86) and submits a
  registration with missing core metadata (→ admitted, zero trust is not
  exclusion, but the trust score drops and the UI flags it). Every outcome
  shown is a real GlassChain code path — nothing staged, nothing simulated.
  Equivocation evidence would surface when the staged BFT engine ever emits
  it; the dev driver is PoW, so the UI labels that capability unavailable
  rather than fabricating proofs.
- **Per-member visibility, read from each member's own node.**
  `GET /api/snapshot?as=<company>` reports that member's own node: its chain
  height, and its own transient store — which `pricing` payloads the member
  actually holds. Regulator and supply-chain members hold the payloads; the
  certifiers and evil companies hold commitments only. The UI's visibility
  matrix renders the same chain per member.
- **Interactive, traffic-weighted graph.** One node per company (color by
  role, red for evil), edges whose width grows with the transactions that
  crossed them, moving dots for in-flight volume. Every honest member gets
  edges: custody hops through distributors, PDC dissemination fan-out to
  collection members (including the regulator), certification and contract
  handovers. Evil nodes carry no edges — nothing of theirs ever flowed
  anywhere; that absence is part of the story. Click a node to open that
  member's drawer (its own-node chain height, the private payloads it holds,
  and what it can/cannot see — with the reasoning); drag nodes to rearrange;
  hover for a tooltip. The Canvas2D baseline honors reduced motion; the
  WebGPU experiment stays behind the measured-budget gate.
- **Stock-driven buy pressure.** Purchase quantities scale with the
  system's absolute stock (every 5 000 sellable units = +1 pressure step,
  capped): scarce stock keeps the partial-lot ladder, piled-up stock buys
  full lots so goods reach the retail drain instead of piling up. The
  retail drain scales with each pharmacy's own stock for the same reason —
  the loop converges instead of accumulating forever.
- **Member drawer as an orders overview.** Opening a member's drawer now
  reads like the Stockify orders page: four KPI cards (concluded sell
  orders, pending sell orders, concluded buys, inbound lots in transit),
  an order-status donut (concluded sells / pending sells / buys), a
  recent buy & sell orders table (ref, type, counterparty, round, total,
  status badge), and   a warehouse/cash section and the origin-scoped PDC table
   (every lot
  whose committed custodian is the member: SKU, lot ref, units, acquired
  at block, source) — before the PDC inspection and warehouse sections.
  `org_snapshot` exposes `pending_inbound` (pipeline lots addressed to the
  member). Opening a member's drawer now
  reads like the Stockify orders page: four KPI cards (concluded sell
  orders, pending sell orders, concluded buys, inbound lots in transit),
  an order-status donut (concluded sells / pending sells / buys), and a
  recent buy & sell orders table with status badges and per-order
  totals — before the PDC inspection and warehouse sections. `org_snapshot` exposes
  `pending_inbound` (pipeline lots addressed to the member).
- **The viewing-as banner is always on and defaults to Public**: Public
  reads "the default view — the public chain only", a member view explains
  the origin scoping, Admin explains the privileged lens. Graph node
  clicks take priority over passing transaction dots (manufacturer nodes
  are the busiest, so dots used to steal the click), the drawer close
  button's ✕ is centered, and the PDC inspection is the drawer's last
  section in tabular form (lot, author, member price, list price, units,
  access badge).
- **Admin (demo) view.** The member selector gains an "Admin (demo) — sees
  everything" entry: the privileged lens with every payload's cleartext and
  every company's cash balance. The chain itself carries no global read;
  the lens is clearly labelled as demonstration-only. Public view stays
  commitments-only; member views stay origin-scoped. A left org/nav sidebar (brand, section anchors,
  the member selector with a "Return to public view" button, pending +
  executed offers, the theme toggle) beside the main content. Selecting a
  member now **visibly changes the page**: a viewing-as banner appears, the
  member's rows in the directory/bars/visibility matrix highlight, and its
  drawer opens. The WMS directory dropped its Status/Action columns (the
  row itself is the action), a **Purchases** table carries executed trade,
  and cards/panels breathe with larger gaps. The motion button is gone —
  the animation stays permanently reduced (slow, capped flow). Header KPIs; **Stock Level by
  Member** (horizontal bars, red flag for low-stock members); **Inventory
  Value Share** (conic-gradient donut with a percentage legend, small
  shares folded into "Others"); and an **Inventory Directory** table
  (member, type, lots, units, value, retail sold, status badge, Manage
  action) — every row clickable into the member drawer. Cards are gone:
  the reference layout is charts + directory. Header KPIs (members, total stock
  units, total inventory value = on-hand × $15.00 demo bookkeeping, total
  retail sold, low-stock alerts) above per-member cards: health badge
  (Optimal / Low stock), lots + on-hand units, stock value and sellable
  pool, `in · out · retail` movement row, and a "Manage member →" footer —
  clicking opens the member drawer. Data comes from committed chain events;
  value is demo bookkeeping (the chain carries no valuation).
- **WMS dashboard with retail flow.** A warehouse-management view derived
  from committed custody chains plus a per-company **sellable inventory**
  pool: pharmacies (every stocked member) sell 100 units per round on to end
  customers — a real committed `InventoryUpdate` (negative stock delta) that
  the live graph deliberately does not animate. Inventories accumulate
  first, drain slowly, and emptying one takes sustained rounds. Clicking a
  WMS member card opens the member drawer with its stock, sellable
  inventory, cash (demo bookkeeping, with retail revenue), and bought/sold
  summary. The cards show `in · out · sold retail` counters.
- **Resizable panels.** The lots table's scroll container has a drag handle
  at its edge — drag to resize the visible height.
- **PDC inspection per role.** The member drawer lists the `pricing`
  collection's disseminated payload count and the cleartexts THIS member
  holds — real member-only pricing terms as JSON (list price, the 12.5 %
  member discount, quantity, payment terms), rendered as readable fields;
  non-members see only opaque sha256 commitments with the reasoning spelled
  out. Payloads are written from the star center so every member holds
  every term (the engine disseminates one hop; the demo topology is a
  star).
- **Resizable drawer + resizable drawer tables.** The member drawer's
  body lives in a dedicated scroll strip (`drawer-scroll-outer`), so the
  left-edge resize handle never scrolls away; every table inside the
  drawer (orders, inventory-by-lot, PDC inspection) has its own top drag
  handle that adjusts its scroll height. The drawer layout is ONErule block (`#member-drawer {fixed, flex
  column, overflow hidden}` + `#drawer-scroll-outer { flex:1, min-height:
  0, overflow-y:auto, touch, overscroll-contain}` + `#drawer-content {
  padding: 1.8rem 2.4rem}` + a single `#drawer-close`) — an earlier
  splice had dropped the main dark-mode rule entirely (only a stray
  light-theme copy survived), which is why padding/scroll kept "coming
  back broken" across browsers. The scroll strip carries `min-height: 0`, the
  boot **self-heals a cached pre-restructure page** (rebuilding the
  wrapper if the served HTML lacks it), and drawer tables keep readable
  column widths (min-width with horizontal scroll inside their wrappers)
  instead of crushing on narrow screens. (the flex-shrink fix — without it the drawer
  clipped its content, killing scrolling and padding), and the drawer's
  tables/KPI grids size to the drawer's horizontal width with
  word-breaking cells — no horizontal overflow when the drawer is
  resized narrow. The cert drawer's related-chain lookup normalizes the lot
  reference case ("lot-1" vs "LOT-1") — that mismatch made every cert
  claim an uncommitted chain.
- **Trade alignment root fixed.** The misalignment survived every layout
  fix because a stray sibling-selector margin (`.chart-card + .chart-card`)
  pushed the second card down 1.4rem inside its own row. Scoped to the
  WMS charts where it belongs.
- **10 ms animation budget.** The renderer target-load budget drops from
  16.7 ms to 10 ms: Canvas2D must animate the densest state under 10 ms
  p95 or the step-5 WebGPU path activates. The renderer-info label shows
  the measured p95 and the met/missed verdict.
- **Higher parameter maxima.** Companies per role (manufacturers ≤6,
  distributors/logistics ≤5, pharmacies ≤6, regulators ≤3, certifiers ≤4,
  evil ≤5), lots/round ≤20, interval ≤10 s — form and server clamp in
  lockstep.
- **Private-data gate.** A member's cash and trade bookkeeping are
  member-private: visible to the member itself, the regulator, and Admin
  (demo) — the public lens and other members see the public chain only,
  with an explanatory note in the drawer. Verified live: mfg-2 viewed by
  mfg-1 → cash null/private_visible false; self and regulator → visible.
- **Trade columns equal-height.** The pending/executed columns are forced
  to the same grid row height (`grid-auto-rows: 1fr`, flex stacks) so the
  cards align regardless of content.
- **Spacing rhythm.** Section margins, card gaps, and chart-card spacing
  use one consistent rhythm (1.4rem); Trade's pending/executed cards are
  equal-height columns. Pending (with Buy) and
  executed (committed purchases + filled offers) sit in two chart-card
  columns in the main content — not the sidebar. The Inventory section's
  KPI and chart cards breathe with wider gaps.
- **Light/dark + reduced motion.** A header toggle switches light/dark
  (persisted in localStorage; default dark), and a Motion toggle pauses the
  animation entirely — reduced motion is the default (slower, capped flow;
  `prefers-reduced-motion` is honored by CSS as well). Every panel
  (metrics, lots, WMS, certifications, zero-trust, visibility, feed, offers
  sidebar) diff-renders: DOM rebuilds only when its content changed, so
  hover, selection and typed input survive the 500 ms tick. The Buy
  quantity field keeps the user's typed value per offer — it never snaps
  back to the maximum.
- **Design system — Linear × Apple.** Dark chassis (single indigo accent,
  hairline borders, radial accent glow behind the canvas) with Apple's type
  scale and spacing: frosted sticky header, uppercase micro-labels, tabular
  numerals, pill-to-8px radius buttons, dark member drawer. The old light
  theme is gone; the canvas is the visual center of gravity.
- **Explained interactions.** Every adversary outcome row carries a human
  explanation of which gate answered and why (membership gate, fail-closed
  #86 gate, strict ADR-006 validation, trust scoring); rows are clickable
  and open the explanation in the member drawer. Metric cards, controls and
  table headers carry tooltips. `prefers-reduced-motion` is honored.
- **Measured metrics.** Submitted/rejected counters, lots, block commit
  latency (p50/p95 over a rolling 64-block window), pending-pool depth and
  bytes, chain height, tx/s throughput. Values come from the headless runner,
  never from animation timestamps.
- **Presentation integrity.** The SSE stream is coalesced at a fixed 500 ms
  cadence (never one event per transaction); a reconnecting client refetches
  the authoritative snapshot — the presentation stream is not an audit log.

One engine caveat, honestly: the wire engine relays transactions one hop, so
the demo's star topology puts the block producer at the center (`FarmaGen`)
and every company dials it — a deployment property (mesh vs star), not a
consensus property.

## Security posture

Loopback-only bind; `Host` check on reads; state-changing commands
(`POST /api/run`) require a same-origin `Origin` header plus the per-run
capability token in `x-glass-auth` (no cookies, no token in URLs or logs).
Everything the page serves comes from the same origin; the response CSP
allows no external code. The demo is local-only: hosted multi-user access
needs authentication, TLS, tenant isolation, quotas, and retention — a
separate security scope (plan §4).

## Renderer policy

Canvas2D is the baseline. WebGPU is attempted **only** when the measured
Canvas2D frame time at the ~500-element target load misses the 16.7 ms p95
budget (the step-5 gate), and falls back automatically when no adapter can be
acquired or the device is lost. The measurement result should be recorded
here (browser/version, device, p95, budget met/missed) when the comparison is
run: _pending — the comparison has not been run on a real device yet; Canvas2D
is the shipping renderer until the gate is measured._

## Testing

Rust-side tests own every guarantee that matters (`cargo test --manifest-path
demo/Cargo.toml`): collection membership (evils and certifiers excluded), one
full synchronized round through a real federation (custody chains complete,
certification + audit committed, PDC payload disseminated), the evil attacks
dying on their real gates, the per-member org views served from each member's
own node, parameter sanitization + topology rebuild signalling, and the
contract engine matching offers into purchase orders. The browser UI is smoke-tested manually; JS test tooling is
deliberately not pulled in (plan §6, deferred with the CI work).

# Demo — `glasschain-demo`

A local browser demonstration of a real GlassChain federation moving synthetic
supply-chain transactions. **A synthetic demo run is presentation, never
evidence** — see the glossary term in the repository root `CONTEXT.md`. Nothing
shown here establishes the [ADR-010](adr/adr-010-consensus-adoption.md) testnet,
security, or scalability gates; those live in
[`benchmarks/consensus-capacity.md`](benchmarks/consensus-capacity.md).

## Run it

```bash
cargo run --manifest-path demo/Cargo.toml          # binds 127.0.0.1:18850
# or: cargo run --manifest-path demo/Cargo.toml -- 127.0.0.1:19500
```

Open `http://127.0.0.1:18850/`. The page pulls `/api/bootstrap` (the session
capability token, same-origin) and the initial snapshot; **Start** launches the
synthetic runner, **Stop** aborts it, **Reset** clears the shared state. A
validator set is not required — the demo federation is six in-process nodes
(PoW dev/test, difficulty 1).

## What it demonstrates

- **Multi-company federation, synthetic workload, operator-tunable.** The
  federation comes from parameters, editable live — even mid run — through the
  page's form or `POST /api/params` (token-gated): companies per role
  (manufacturers/distributors/logistics/pharmacies/regulators/certifiers),
  evil-node count, lots per round, round interval. Topology changes rebuild
  the federation automatically at the next round boundary (the run and its
  counters survive; the status pill shows `rebuilding`). Each node is a real
  `Node` with its own identity, minted by one demo root organization — a
  simplification; the production trust model is ADR-011's federation trust
  store, not the demo's job.
- **Pending vs executed offers.** The offers sidebar splits in two: pending
  (awaiting buyer, with the quantity spinner) and executed (committed
  purchases plus filled offers). Every offer covers only **part of its
  500-unit lot** — the ladder (300/400/500) makes auto purchases partial lot
  buys, tracked with a `sold` counter ("auto-executed: 400 of the lot's 500
  units bought"); the remainder stays in the warehouse.
- **A staged pipeline — everyone holds stock.** Lots advance **one custody
  hop per round** through `RunState`-backed pipeline entries (anchor →
  manufacturer → distributor → logistics → pharmacy, certification when a
  lot arrives), so goods physically rest at every member between rounds.
  The WMS view now shows manufacturers, distributors and logistics each
  holding on-hand units alongside the pharmacies, exactly as asked. A lot's
  full journey takes ~4 rounds; certifications and audits commit only after
  the lot actually reaches the pharmacy. The lots table is append-only for the session (a
  high cap guards only extreme runs): no rows are shed mid-session, the
  layout is fixed-width with single-line ellipsized custody chains, and the
  table scrolls inside a bounded container — no jumping.
- **The contract match, end to end — with a human buyer.** The runner
  registers two real contracts on the same product, both owned by the first
  pharmacy: `auto-replenish` (price cap 1200, `auto_execute`) and
  `manual-review` (ceiling 2000, `auto_execute=false`). Rotating
  manufacturers submit `SupplyOffer`s; offers under the cap are matched and
  auto-executed by the engine; premium offers (every third, 1400) match only
  `manual-review` and are advertised as **awaiting a buyer decision**. The
  offers sidebar shows all recent offers; the user completes a pending one
  with a **quantity spinner** (partial buys: any amount from 1 up to the
  remaining units — the offer stays open with a `sold` counter until it is
  fully bought). Buy submits a real `PurchaseOrder` through the viewing
  pharmacy's own node (`POST /api/purchase`, token-gated) — admission, relay
  and commit are the same code path as everything else. Each member's drawer
  shows what it holds: stock on hand (committed custody), a demo cash
  balance (bookkeeping moved buyer→seller per committed purchase, labelled
  demo-only — the chain carries no balances), its advertised sell offers and
  its purchase history. The animated transaction dots flow only while the
  run is active; a stopped run freezes the picture. The contract-flow
  story is the engine's own behavior, not a staged simulation.
- **Lot inspection.** Clicking a lot row opens its drawer: the full
  committed custody timeline (event → custodian → block), its
  certifications/audits, its PDC payload status judged from the VIEWER's
  snapshot (own terms readable when the lot is yours; other members'
  terms never readable by you), and **its custody path highlighted in
  accent on the live graph** while the drawer is open. The drawer works
  for chain-less lots too (the certs loop no longer throws). Clicking a
  graph node opens the member's drawer WITHOUT switching the global
  view-as selector — drawers are viewer-scoped (`?as=X&viewer=V`).
- **Transaction-level graph.** Every recent transaction is a moving dot on
  its real edge (colored by kind: lot, custody, offer, purchase,
  attestation); hover shows what it is; clicking a dot opens the drawer with
  the transaction's id, kind, flow (from → to) and committed height.
  Clicking a node still opens the member's own-node view; dragging
  rearranges.
- **Synchronized scenario rounds with speed.** Each round commits
  `LOTS_PER_ROUND` (3) lots across rotating real companies through the full
  custody chain — manufacturer → distributor → logistics → pharmacy, each
  hop submitted by the handover company and confirmed in the miner's pool
  before the next dispatch (a lot cannot be received before it is
  dispatched) — plus a member-only `pricing` PDC payload disseminated to the
  whole collection, and public `quality_certification` +
  `audit_attestation` records from a rotating certifier. All of a round's transactions land in one mined block
  (dev-PoW, difficulty 1); a reference run sustained ~38 tx/s with commit
  p50 12 ms / p95 34 ms on a laptop — labelled dev-PoW, never compared to
  the ADR-010 gates.
- **Diversified private payloads.** Every lot now carries THREE
  origin-scoped payloads: the manufacturer's **pricing terms** (the
  12.5 % member discount), the distributor's **storage terms**
  (temperature range, humidity, retention, release conditions), and
  logistics' **transit terms** (route, transit window, cold-chain
  commitment) — each authored by its org, submitted as the lot crosses
  into its custody. Origin scoping applies per payload: you read your
  own class of terms; the regulator reads everything.
- **Zero trust, with evil nodes that attack for real — five attack kinds.**
  Every round the evil companies run: a private-payload attempt (membership
  gate / fail-closed #86), a forged certification with an undisclosed
  payload key (strict schema), a **lot anchor with a tampered commitment**
  (anchored-family hash check), a **duplicate transaction replay** (pool
  idempotency), and the under-metadata registration (admitted, trust score
  reduced). Every outcome is a real GlassChain code path. Every round:
  `QuimicaFalsa` (verified identity, deliberately not a `pricing` member)
  attempts a private payload (→ membership-gate rejection) and a forged
  certification with an undisclosed payload key (→ strict ADR-006 schema
  rejection); `DipFakeCerts` (no certificate verifier at all) attempts a
  private payload (→ fail-closed zero-trust gate #86) and submits a
  registration with missing core metadata (→ admitted, zero trust is not
  exclusion, but the trust score drops and the UI flags it). Every outcome
  shown is a real GlassChain code path — nothing staged, nothing simulated.
  Equivocation evidence would surface when the staged BFT engine ever emits
  it; the dev driver is PoW, so the UI labels that capability unavailable
  rather than fabricating proofs.
- **Per-member visibility, read from each member's own node.**
  `GET /api/snapshot?as=<company>` reports that member's own node: its chain
  height, and its own transient store — which `pricing` payloads the member
  actually holds. Regulator and supply-chain members hold the payloads; the
  certifiers and evil companies hold commitments only. The UI's visibility
  matrix renders the same chain per member.
- **Interactive, traffic-weighted graph.** One node per company (color by
  role, red for evil), edges whose width grows with the transactions that
  crossed them, moving dots for in-flight volume. Every honest member gets
  edges: custody hops through distributors, PDC dissemination fan-out to
  collection members (including the regulator), certification and contract
  handovers. Evil nodes carry no edges — nothing of theirs ever flowed
  anywhere; that absence is part of the story. Click a node to open that
  member's drawer (its own-node chain height, the private payloads it holds,
  and what it can/cannot see — with the reasoning); drag nodes to rearrange;
  hover for a tooltip. The Canvas2D baseline honors reduced motion; the
  WebGPU experiment stays behind the measured-budget gate.
- **Stock-driven buy pressure.** Purchase quantities scale with the
  system's absolute stock (every 5 000 sellable units = +1 pressure step,
  capped): scarce stock keeps the partial-lot ladder, piled-up stock buys
  full lots so goods reach the retail drain instead of piling up. The
  retail drain scales with each pharmacy's own stock for the same reason —
  the loop converges instead of accumulating forever.
- **Member drawer as an orders overview.** Opening a member's drawer now
  reads like the Stockify orders page: four KPI cards (concluded sell
  orders, pending sell orders, concluded buys, inbound lots in transit),
  an order-status donut (concluded sells / pending sells / buys), a
  recent buy & sell orders table (ref, type, counterparty, round, total,
  status badge), and   a warehouse/cash section and the origin-scoped PDC table
   (every lot
  whose committed custodian is the member: SKU, lot ref, units, acquired
  at block, source) — before the PDC inspection and warehouse sections.
  `org_snapshot` exposes `pending_inbound` (pipeline lots addressed to the
  member). Opening a member's drawer now
  reads like the Stockify orders page: four KPI cards (concluded sell
  orders, pending sell orders, concluded buys, inbound lots in transit),
  an order-status donut (concluded sells / pending sells / buys), and a
  recent buy & sell orders table with status badges and per-order
  totals — before the PDC inspection and warehouse sections. `org_snapshot` exposes
  `pending_inbound` (pipeline lots addressed to the member).
- **The viewing-as banner is always on and defaults to Public**: Public
  reads "the default view — the public chain only", a member view explains
  the origin scoping, Admin explains the privileged lens. Graph node
  clicks take priority over passing transaction dots (manufacturer nodes
  are the busiest, so dots used to steal the click), the drawer close
  button's ✕ is centered, and the PDC inspection is the drawer's last
  section in tabular form (lot, author, member price, list price, units,
  access badge).
- **Admin (demo) view.** The member selector gains an "Admin (demo) — sees
  everything" entry: the privileged lens with every payload's cleartext and
  every company's cash balance. The chain itself carries no global read;
  the lens is clearly labelled as demonstration-only. Public view stays
  commitments-only; member views stay origin-scoped. A left org/nav sidebar (brand, section anchors,
  the member selector with a "Return to public view" button, pending +
  executed offers, the theme toggle) beside the main content. Selecting a
  member now **visibly changes the page**: a viewing-as banner appears, the
  member's rows in the directory/bars/visibility matrix highlight, and its
  drawer opens. The WMS directory dropped its Status/Action columns (the
  row itself is the action), a **Purchases** table carries executed trade,
  and cards/panels breathe with larger gaps. The motion button is gone —
  the animation stays permanently reduced (slow, capped flow). Header KPIs; **Stock Level by
  Member** (horizontal bars, red flag for low-stock members); **Inventory
  Value Share** (conic-gradient donut with a percentage legend, small
  shares folded into "Others"); and an **Inventory Directory** table
  (member, type, lots, units, value, retail sold, status badge, Manage
  action) — every row clickable into the member drawer. Cards are gone:
  the reference layout is charts + directory. Header KPIs (members, total stock
  units, total inventory value = on-hand × $15.00 demo bookkeeping, total
  retail sold, low-stock alerts) above per-member cards: health badge
  (Optimal / Low stock), lots + on-hand units, stock value and sellable
  pool, `in · out · retail` movement row, and a "Manage member →" footer —
  clicking opens the member drawer. Data comes from committed chain events;
  value is demo bookkeeping (the chain carries no valuation).
- **WMS dashboard with retail flow.** A warehouse-management view derived
  from committed custody chains plus a per-company **sellable inventory**
  pool: pharmacies (every stocked member) sell 100 units per round on to end
  customers — a real committed `InventoryUpdate` (negative stock delta) that
  the live graph deliberately does not animate. Inventories accumulate
  first, drain slowly, and emptying one takes sustained rounds. Clicking a
  WMS member card opens the member drawer with its stock, sellable
  inventory, cash (demo bookkeeping, with retail revenue), and bought/sold
  summary. The cards show `in · out · sold retail` counters.
- **Resizable panels.** The lots table's scroll container has a drag handle
  at its edge — drag to resize the visible height.
- **PDC inspection per role.** The member drawer lists the `pricing`
  collection's disseminated payload count and the cleartexts THIS member
  holds — real member-only pricing terms as JSON (list price, the 12.5 %
  member discount, quantity, payment terms), rendered as readable fields;
  non-members see only opaque sha256 commitments with the reasoning spelled
  out. Payloads are written from the star center so every member holds
  every term (the engine disseminates one hop; the demo topology is a
  star).
- **Resizable drawer + resizable drawer tables.** The member drawer's
  body lives in a dedicated scroll strip (`drawer-scroll-outer`), so the
  left-edge resize handle never scrolls away; every table inside the
  drawer (orders, inventory-by-lot, PDC inspection) has its own top drag
  handle that adjusts its scroll height. The drawer layout is ONErule block (`#member-drawer {fixed, flex
  column, overflow hidden}` + `#drawer-scroll-outer { flex:1, min-height:
  0, overflow-y:auto, touch, overscroll-contain}` + `#drawer-content {
  padding: 1.8rem 2.4rem}` + a single `#drawer-close`) — an earlier
  splice had dropped the main dark-mode rule entirely (only a stray
  light-theme copy survived), which is why padding/scroll kept "coming
  back broken" across browsers. The scroll strip carries `min-height: 0`, the
  boot **self-heals a cached pre-restructure page** (rebuilding the
  wrapper if the served HTML lacks it), and drawer tables keep readable
  column widths (min-width with horizontal scroll inside their wrappers)
  instead of crushing on narrow screens. (the flex-shrink fix — without it the drawer
  clipped its content, killing scrolling and padding), and the drawer's
  tables/KPI grids size to the drawer's horizontal width with
  word-breaking cells — no horizontal overflow when the drawer is
  resized narrow. The cert drawer's related-chain lookup normalizes the lot
  reference case ("lot-1" vs "LOT-1") — that mismatch made every cert
  claim an uncommitted chain.
- **Trade alignment root fixed.** The misalignment survived every layout
  fix because a stray sibling-selector margin (`.chart-card + .chart-card`)
  pushed the second card down 1.4rem inside its own row. Scoped to the
  WMS charts where it belongs.
- **10 ms animation budget.** The renderer target-load budget drops from
  16.7 ms to 10 ms: Canvas2D must animate the densest state under 10 ms
  p95 or the step-5 WebGPU path activates. The renderer-info label shows
  the measured p95 and the met/missed verdict.
- **Higher parameter maxima.** Companies per role (manufacturers ≤6,
  distributors/logistics ≤5, pharmacies ≤6, regulators ≤3, certifiers ≤4,
  evil ≤5), lots/round ≤20, interval ≤10 s — form and server clamp in
  lockstep.
- **Private-data gate.** A member's cash and trade bookkeeping are
  member-private: visible to the member itself, the regulator, and Admin
  (demo) — the public lens and other members see the public chain only,
  with an explanatory note in the drawer. Verified live: mfg-2 viewed by
  mfg-1 → cash null/private_visible false; self and regulator → visible.
- **Trade columns equal-height.** The pending/executed columns are forced
  to the same grid row height (`grid-auto-rows: 1fr`, flex stacks) so the
  cards align regardless of content.
- **Spacing rhythm.** Section margins, card gaps, and chart-card spacing
  use one consistent rhythm (1.4rem); Trade's pending/executed cards are
  equal-height columns. Pending (with Buy) and
  executed (committed purchases + filled offers) sit in two chart-card
  columns in the main content — not the sidebar. The Inventory section's
  KPI and chart cards breathe with wider gaps.
- **Light/dark + reduced motion.** A header toggle switches light/dark
  (persisted in localStorage; default dark), and a Motion toggle pauses the
  animation entirely — reduced motion is the default (slower, capped flow;
  `prefers-reduced-motion` is honored by CSS as well). Every panel
  (metrics, lots, WMS, certifications, zero-trust, visibility, feed, offers
  sidebar) diff-renders: DOM rebuilds only when its content changed, so
  hover, selection and typed input survive the 500 ms tick. The Buy
  quantity field keeps the user's typed value per offer — it never snaps
  back to the maximum.
- **Design system — Linear × Apple.** Dark chassis (single indigo accent,
  hairline borders, radial accent glow behind the canvas) with Apple's type
  scale and spacing: frosted sticky header, uppercase micro-labels, tabular
  numerals, pill-to-8px radius buttons, dark member drawer. The old light
  theme is gone; the canvas is the visual center of gravity.
- **Explained interactions.** Every adversary outcome row carries a human
  explanation of which gate answered and why (membership gate, fail-closed
  #86 gate, strict ADR-006 validation, trust scoring); rows are clickable
  and open the explanation in the member drawer. Metric cards, controls and
  table headers carry tooltips. `prefers-reduced-motion` is honored.
- **Measured metrics.** Submitted/rejected counters, lots, block commit
  latency (p50/p95 over a rolling 64-block window), pending-pool depth and
  bytes, chain height, tx/s throughput. Values come from the headless runner,
  never from animation timestamps.
- **Presentation integrity.** The SSE stream is coalesced at a fixed 500 ms
  cadence (never one event per transaction); a reconnecting client refetches
  the authoritative snapshot — the presentation stream is not an audit log.

One engine caveat, honestly: the wire engine relays transactions one hop, so
the demo's star topology puts the block producer at the center (`FarmaGen`)
and every company dials it — a deployment property (mesh vs star), not a
consensus property.

## Security posture

Loopback-only bind; `Host` check on reads; state-changing commands
(`POST /api/run`) require a same-origin `Origin` header plus the per-run
capability token in `x-glass-auth` (no cookies, no token in URLs or logs).
Everything the page serves comes from the same origin; the response CSP
allows no external code. The demo is local-only: hosted multi-user access
needs authentication, TLS, tenant isolation, quotas, and retention — a
separate security scope (plan §4).

## Renderer policy

Canvas2D is the baseline. WebGPU is attempted **only** when the measured
Canvas2D frame time at the ~500-element target load misses the 16.7 ms p95
budget (the step-5 gate), and falls back automatically when no adapter can be
acquired or the device is lost. The measurement result should be recorded
here (browser/version, device, p95, budget met/missed) when the comparison is
run: _pending — the comparison has not been run on a real device yet; Canvas2D
is the shipping renderer until the gate is measured._

## Testing

Rust-side tests own every guarantee that matters (`cargo test --manifest-path
demo/Cargo.toml`): collection membership (evils and certifiers excluded), one
full synchronized round through a real federation (custody chains complete,
certification + audit committed, PDC payload disseminated), the evil attacks
dying on their real gates, the per-member org views served from each member's
own node, parameter sanitization + topology rebuild signalling, and the
contract engine matching offers into purchase orders. The browser UI is smoke-tested manually; JS test tooling is
deliberately not pulled in (plan §6, deferred with the CI work).

## What is native GlassChain vs. demo-side bookkeeping

Everything the demo shows **on the chain** rides real GlassChain code paths:
`SupplyOffer`/`PurchaseOrder`/`AssetRegistration`/`CanonicalRecord`/
`InventoryUpdate` transactions, the contract engine's `offer_matches` +
auto-execute, PDC dissemination and its membership gate, strict ADR-006
schema validation, trust scoring, pending-pool idempotency (replay
blocking), and the fail-closed org-gated paths the evil companies attack.

The runner adds **presentation-only state that GlassChain itself does not
carry**. None of it is on-chain; none of it is consensus-visible:

| Demo-side | What it is | Why the chain has no equivalent |
|---|---|---|
| Demo cash (`RunState.cash`) | per-company balance bookkeeping, moved buyer→seller per committed purchase and on retail sales | GlassChain carries no currency balances or settlement |
| Sellable inventory (`RunState.inventory`) | a per-company trading pool gained at each handover, drained by retail | the chain records custody events, not stock balances |
| Stock value (on-hand × $15.00) | demo valuation shown in the WMS | the chain has no price oracle or valuation |
| `OfferEvent.sold` / offer fill counters | partial-fill tracking per offer | `PurchaseOrder`s are native; the *fill tracking against an offer* is demo bookkeeping |
| Buy pressure / retail drain scaling | simulation policy throttling production and accelerating sales | simulation policy, not a consensus behavior |
| Staged pipeline cadence (one hop per round) | the runner paces custody hops for watchability | GlassChain commits as fast as the driver submits |
| Star-center mining + PDC writing | the demo's star topology puts the block producer at the center (the engine relays one hop) | a deployment property (mesh vs star), not consensus |
| Bounded SSE snapshots, diff-rendered panels, resizers, drawers | presentation | browser concerns |

## Out of scope