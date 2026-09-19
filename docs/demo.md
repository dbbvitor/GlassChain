# Demo — `glasschain-demo`

A local browser demonstration of a real GlassChain federation moving synthetic
supply-chain transactions. **A synthetic demo run is presentation, never
evidence** — see the glossary term in the repository root
[`CONTEXT.md`](../CONTEXT.md). Nothing shown here establishes the
[ADR-010](adr/adr-010-consensus-adoption.md) testnet, security, or scalability
gates; those live in
[`benchmarks/consensus-capacity.md`](benchmarks/consensus-capacity.md).

## Run it

```bash
cargo run --manifest-path demo/Cargo.toml          # binds 127.0.0.1:18850
# or: cargo run --manifest-path demo/Cargo.toml -- 127.0.0.1:19500
```

Open `http://127.0.0.1:18850/`. **Start** launches the synthetic runner,
**Stop** freezes the committed picture, **Reset** wipes the shared state
(behind a confirm dialog). The page pulls `/api/bootstrap` (the session
capability token and the initial configuration), renders coalesced 500 ms
snapshots from `/api/events`, and refetches the authoritative snapshot after
any stream gap — the stream is presentation, not an audit log.

The default federation is 15 in-process nodes — 3 manufacturers, 2
distributors, 2 logistics, 3 pharmacies, 1 regulator, 2 certifiers and 2 evil
companies — speaking PoW dev/test at difficulty 1. No validator set is
required. Each node is a real `Node` with its own TLS identity and
certificate, minted by one demo root organization: a simplification, since the
production trust model is ADR-011's federation trust store, not the demo's
job.

## The panels

Seven panels, one focus each, in the left rail:

| Panel | What it sells |
|---|---|
| **Overview** | Fleet KPIs, the live transaction graph, live-editable simulation parameters, committed activity feed |
| **Inventory** | Who holds what, stock bars and value share, the retail drain behind them |
| **Trade** | Contracts with their real conditions, pending/executed offers, partial buys, the purchase ledger |
| **Traceability** | Lot custody chains, verifiable lineage, certifications/audits, the tamper-evident block chain |
| **Trust & Security** | Per-member security posture, adversary outcomes with the gate that answered, the who-sees-what matrix |
| **Compliance** | SNCM schema validation, trust distribution, flat analytical records, fee schedules |
| **Performance** | Measured throughput and commit latency, the full metric grid, the renderer budget |

## What it demonstrates

### A staged, operator-tunable federation

- **Live parameters.** Companies per role, evil-node count, lots per round (up
  to 50) and the round interval (down to 0, back-to-back rounds) are editable
  from Overview even mid run (`POST /api/params`, token-gated). The server
  clamps every value; topology changes rebuild the federation automatically at
  the next round boundary and the run's counters survive (the status pill shows
  `rebuilding`). A **Stress preset** sets 50 lots/round with no pause and
  applies it in one click.
- **One custody hop per round.** `LOTS_PER_ROUND` (default 3) lots enter the
  pipeline each round and advance exactly one hop — manufacturer →
  distributor → logistics → pharmacy — so goods physically rest at every
  member between rounds and a lot's journey takes about four rounds.
  Certifications and audits commit the round after manufacture.
- **Everyone holds stock.** Inventory derives who holds what from committed
  custody events, not from memory. Every stocked member sells to end customers
  each round through a real committed `InventoryUpdate`; inventories
  accumulate first and drain slowly.

### Trade — real contracts, with a human in the loop

- **Four real contracts** are registered on the same product: one
  auto-executing contract (`auto-replenish`, pharmacy-1, price cap 1200) and
  one conditions-only contract per pharmacy, each with its own price band and
  ceiling — `value-review` (pharmacy-2, 1600), `manual-review` (pharmacy-1,
  2000) and `premium-review` (pharmacy-3, 2400). The Trade panel's
  **Contracts** table shows each one's real conditions, committed execution
  count and units purchased.
- **Offers vary per lot.** Cheap prices rotate across eight tiers
  ($8.50–$12.00) and premium prices across six ($13.50–$23.00); quantities
  cover varied slices of the 500-unit lot (100–500, larger on the premium
  bands) plus a stock-pressure step. Cheap offers are matched and
  auto-executed by the engine; every third offer is premium, matches exactly
  one pharmacy's price band and appears under **Awaiting buyer** — the bands
  never overlap, so a premium offer can only ever complete against one
  contract.
- **Partial buys.** The offer's `sold` counter tracks fills; a human buys any
  amount from 1 up to the remainder, and the offer stays open until it is
  fully bought. Completion submits a real `PurchaseOrder` through the
  offer's named buyer node against that buyer's contract.
- **Buy is a real transaction.** `POST /api/purchase` submits a real
  `PurchaseOrder` through the viewing pharmacy's own node — admission, relay
  and commit are the same code path as everything else.

### Traceability

- **Lot custody chains.** Every lot's committed provenance is a timeline of
  event → custodian → block, drawn in the lot drawer and highlighted in accent
  on the live graph while the drawer stays open.
- **Verifiable lineage.** Each in-flight lot is checked against the provenance
  index's `verify_lineage` — the mandatory custody events (`manufacture`,
  `dispatch`, `receive`) must appear in order for that asset id — and its flat
  analytical records are counted per batch. The lots table and the lot drawer
  show the per-lot verdict, and the Compliance panel rolls the check up
  fleet-wide.
- **Tamper-evident block chain.** The block table lists each recent block with
  its hash and its link to the previous block; the block drawer explains that
  altering any committed transaction breaks every later link, and that the
  verifier re-derives and checks the chain on sync and restart.
- **Certifications and audits.** Quality certifications and audit
  attestations are real `CanonicalRecord`s from rotating certifiers.

### Private data, scoped by origin

- Every lot carries several origin-scoped private payloads in the `pricing`
  collection: the manufacturer's **pricing terms**, the distributor's
  **storage terms**, logistics' **transit terms**, plus process, intake and
  temperature records and certification evidence / regulator notes.
- A member reads its own terms in cleartext; other members' terms stay opaque
  sha256 commitments; the regulator reads the whole collection. The filter is
  enforced **server-side** before serialization — UI hiding is not enforcement
  (a cross-member inspection is covered by a test).
- `GET /api/snapshot?as=<company>&viewer=<viewer>` reports each member's own
  node (its chain height and transient store). The **Who sees what** matrix is
  filled from those own-node views, and the member drawer lists the payloads
  the viewer may read with an access badge per row.

### Zero trust, with attacks that die on real gates

Every round each evil company attempts five kinds of attack; every outcome is
a real GlassChain code path, never a staged result:

1. **Private-payload smuggling** → the collection membership gate (a verified
   non-member) or the fail-closed org gate #86 (a node without a certificate
   verifier).
2. **Forged certification with an undisclosed payload key** → strict ADR-006
   schema validation.
3. **Lot anchor with a tampered commitment** → anchored-family hash check.
4. **Duplicate transaction replay** → pending-pool idempotency.
5. **Registration with missing core metadata** → admitted (zero trust is not
   exclusion), trust score reduced and visible in Compliance.

Every row in **Adversary outcomes** opens the drawer with the gate that
answered and why. Equivocation evidence would surface if the staged BFT engine
emitted it; the dev driver is PoW, so the panel stays empty and says so rather
than fabricating proofs. Evil nodes carry no graph edges — nothing of theirs
ever flowed anywhere, and that absence is part of the story.

### Security posture

The **Trust & Security** panel reads each member's own node after the
federation connects:

- **Certificate verifier** — whether the node enforces org paths fail-closed.
- **Certificate** — an X.509 member certificate issued by the demo Root CA,
  tied to the node's ed25519 key.
- **OCSP staple** — the demo mints a real issuer-signed `BasicResponse` per
  member and verifies it **locally** against the Root CA (ADR-017); the column
  reports the verification, not a hardcoded label.
- **Trust score** — the average `MetadataTrustScore` over the registrations
  the org itself originated, with the record count. A company with no
  registrations shows *undefined*, not assumed good; the evil under-metadata
  node's score drops below the standard threshold on-chain.
- **Channels** and **peer sessions** — the node's collection membership and
  its established TLS/TOFU connections.

The transport itself is TLS with certificate fingerprint pinning (TOFU) and a
fail-closed CRL; nothing in the UI bypasses it.

### Compliance

- **SNCM schema validation (ADR-006)** — every in-flight lot's registration is
  validated against the strict schema; the panel shows compliant vs
  non-compliant, the 0.7×/1.0× gas-fee schedule, and the trust drop the evil
  under-metadata registration earns.
- **Analytical records** — the flat projection of committed registrations with
  GTIN, batch, serial, custodian, event, trust score and the exact missing
  fields, so a flagged record stays visible and permanent.
- **Trust distribution** — standard-compliant vs low-trust records and the
  fleet average, straight from the flattener.

### Performance

- **Measured metrics.** Submitted/rejected counters, lots, chain height,
  throughput, block commit latency (p50/p95/p99 over a rolling 64-block
  window) and pending-pool depth/bytes — all from the headless runner, never
  from animation timestamps. Dev-PoW commit latency is labelled as such and
  never compared to the ADR-010 gates.
- **Where the round goes.** Every round is timed per phase — scenario
  production, private-payload dissemination, node submission, pool settle,
  PoW mining, chain projections, retail — and the Performance panel draws the
  breakdown as one stacked bar per round, with round and commit p50/p95/p99
  in the KPI grid and a per-phase percentile table (p50/p95/p99 over the
  rolling window). Runner admission is fanned out (24 concurrent submissions)
  and payload dissemination is bounded-concurrent (16), while compliance/trust
  projections fold only the new records, so sustained stress runs stay flat
  instead of degrading with history. A debug-build reference run on a laptop
  sustains **~610 tx/s** at 50 lots/round (1 800 lots, round p50 753 ms,
  dev-PoW commit p50 222 ms, projections flat at ~35 ms) — labelled dev-PoW,
  never compared to the ADR-010 gates.
- **Per-round charts.** Throughput and commit-latency bars over the rolling
  window make a slowdown visible without reading numbers.
- **Renderer budget.** The graph reports its own measured draw-time p99
  against the 10 ms target-load budget (see Renderer policy).

### The interface

- **The federation graph is shared by every panel**, above the content, with a
  collapse toggle. Every node tooltip carries the member's trust score and
  record count. Each transaction flows **once**, from origin to destination,
  keyed by its transaction id — as the backend's recent-transaction window
  slides, dots neither teleport nor replay mid-edge; they fade at the
  destination.
- **Viewing as.** The rail selector switches the whole page's lens: Public
  (the chain only), a member (origin-scoped, from that member's own node, with
  a banner explaining the scope), or Admin (demo) — the explicitly labelled
  privileged lens with every payload's cleartext and every cash balance.
  Selecting a member highlights its rows and opens its drawer.
- **Member drawer as an orders overview.** Four KPI cards, an order-status
  donut, a recent orders table with round, unit price and status badges, then
  warehouse and cash — including the member's trust score — (a member-private
  ledger: visible to the member, the regulator and Admin), inventory by lot,
  and the receivable PDC table (latest 40 rows with the total count, so a
  long-lived member never stalls the drawer).
- **Every row inspects.** Lots, certificates, blocks, transactions, adversary
  outcomes, contracts, purchases, security-posture rows and
  schema-validation rows all open a drawer with their real details and a
  human explanation; contract drawers list the purchases committed against
  them, and the posture/visibility rows open the member's own-node drawer.
- **Every table and the drawer are resizable** — drag the grip (or focus it
  and use arrow keys / PageUp/PageDown); sizes persist in `localStorage`.
- **Light and dark**, persisted; **reduced motion** honored from the OS
  preference (a fixed, calm picture) and panels diff-render so hover,
  selection and typed quantities survive the 500 ms tick. The Buy quantity
  field keeps the user's typed value per offer.

## Design

A Linear-style dark chassis with Apple's type and spacing and Material's state
layers: one indigo accent, hairline borders, surfaces instead of shadows,
tabular numerals, uppercase micro-labels, pill status chips, a frosted sticky
header, and one 4 px-based spacing scale. The graph is the visual center of
gravity: a themed canvas panel with the accent glow behind it. No CSS
framework, no webfont, no build step — one hand-written stylesheet of design
tokens.

## Security posture (bridge)

Loopback-only bind; `Host` check on reads; state-changing commands
(`POST /api/run`, `/api/params`, `/api/purchase`) require a same-origin
`Origin` header plus the per-run capability token in `x-glass-auth` (no
cookies, no token in URLs or logs). Everything the page serves comes from the
same origin; the response CSP allows no external code. The demo is local-only:
hosted multi-user access needs authentication, TLS, tenant isolation, quotas
and retention — a separate security scope (plan §4).

## Renderer policy

Canvas2D is the baseline: it draws edges, nodes, labels and the moving
transaction dots, and it measures its own draw-time **p99** against the
**10 ms** target-load budget (300 measured frames at the current load). A
**WebGPU layer (moving dots only, on a transparent overlay)** is attempted
only when that measured baseline p99 misses the budget, or when explicitly
forced for verification with `?renderer=webgpu` (`?renderer=canvas` pins the
baseline). Adapter failure or `device.lost` falls back to Canvas2D
automatically. The renderer label reports the active backend and the measured
p99, so the decision is evidence, not preference.

The real-device comparison is still pending; no WebGPU number has been
recorded here yet. The forced mode exists precisely so that comparison can be
run (browser/version, device, p99, budget met/missed) and written down.

## Testing

Rust-side tests own every guarantee that matters:

```bash
cargo test --manifest-path demo/Cargo.toml
```

They cover collection membership (evils and certifiers excluded), a full
synchronized run through a real federation (custody chains complete,
certification and audit committed, private payloads disseminated), the evil
attacks dying on their real gates, per-member own-node views and origin
scoping, parameter sanitization, contract matching into purchase orders,
partial manual buys, the staged pipeline, per-round phase timings, and the
sellable snapshot data:
block-window hash links, per-member OCSP staples verified locally, verifier
presence, contract conditions, compliance/lineage rollups and per-round
performance history. The browser UI is smoke-tested manually; JS test tooling
is deliberately not pulled in (plan §6). The demo package is excluded from the
root workspace and has its own lockfile.

## What is native GlassChain vs. demo-side bookkeeping

Everything the demo shows **on the chain** rides real GlassChain code paths:
`SupplyOffer`/`PurchaseOrder`/`AssetRegistration`/`CanonicalRecord`/
`InventoryUpdate` transactions, the contract engine's `offer_matches` +
auto-execute, PDC dissemination and its membership gate, strict ADR-006 schema
validation, trust scoring, the provenance index and analytical flattener,
OCSP staple minting/verification, pending-pool idempotency (replay blocking),
and the fail-closed org-gated paths the evil companies attack.

The runner adds **presentation-only state that GlassChain itself does not
carry**. None of it is on-chain; none of it is consensus-visible:

| Demo-side | What it is | Why the chain has no equivalent |
|---|---|---|
| Demo cash (`RunState.cash`) | per-company balance bookkeeping, moved buyer→seller per committed purchase and on retail sales | GlassChain carries no currency balances or settlement |
| Sellable inventory (`RunState.inventory`) | a per-company trading pool gained at each handover, drained by retail | the chain records custody events, not stock balances |
| Stock value (on-hand × $15.00) | demo valuation shown in Inventory | the chain has no price oracle or valuation |
| `OfferEvent.sold` / offer fill counters | partial-fill tracking per offer | `PurchaseOrder`s are native; the *fill tracking against an offer* is demo bookkeeping |
| Buy pressure / retail drain scaling | simulation policy throttling production and accelerating sales | simulation policy, not a consensus behavior |
| Staged pipeline cadence (one hop per round) | the runner paces custody hops for watchability | GlassChain commits as fast as the driver submits |
| Star-center mining + PDC writing | the demo's star topology puts the block producer at the center (the engine relays one hop) | a deployment property (mesh vs star), not consensus |
| Bounded SSE snapshots, diff-rendered panels, resizers, drawers | presentation | browser concerns |

One engine caveat, honestly: the wire engine relays transactions one hop, so
the demo's star topology puts the block producer at the center (the first
company) and every company dials it — a deployment property, not a consensus
property.

## Coverage and honest boundaries

The demo surfaces the ledger-facing, browser-reachable features: the chain and
its tamper-evidence, transactions and provenance, contracts and trade, private
data and its server-side scoping, identity/certificate/OCSP posture, the
zero-trust gates, compliance projections and measured performance.

Deliberately **not** surfaced here: the recall/dispute workflow state machine
and the broader workflow engine, endorsement-policy updates, WASM contract
execution through the VM, delegated/responder OCSP and on-chain revocation
(#74, parked), storage-provider internals, and the gRPC/CLI surfaces (a
browser cannot speak native gRPC; the demo bridge is intentionally narrow).
Those need their own scenarios and, in several cases, their own security
scope — not incidental demo scope.

## Out of scope

Desktop gpui, mandatory GPU access, browser-hosted validators, a full product
REST/WebSocket API, public multi-tenant hosting, production key management,
new consensus/schema behaviour, real-data demonstrations and a new benchmark
source of truth. See the plan
([`gui-demo-benchmark.md`](../.agents/plans/gui-demo-benchmark.md), Out of
scope) for the full boundary; remaining work is plan step 4 (fault scenarios)
and the browser-smoke CI.
