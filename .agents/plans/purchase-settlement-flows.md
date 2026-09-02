# Plan — Purchase-to-settlement flows (ticket #43)

**Ticket:** [#43 Purchase-to-settlement flows](https://github.com/dbbvitor/GlassChain/issues/43)
**Builds on:** #40 workflow framework (`glasschain-workflows`), #34 canonical schema v1.
**Shipped:** commit `b189436`. Deviation from the plan below: operator decision
steps are driven by a new `Event::Woken(String)` business wake-up, not
`Resumed` — the runner treats `Resumed` as a pure liveness signal and swallows
it for waiting flows, so decision steps needed their own event variant.

## The record-mapping decision (the ticket's crux)

The v1 registry (ADR-006) is frozen at 13 families; there are **no RFQ, Quote,
Acceptance, Dispute, or Settlement families**. That is deliberate, not a gap:
RFQ/Quote carry pricing and commercial terms that ADR-010 §1 keeps off the
consensus-facing chain, and the `purchase_order` family is the negotiated
outcome's first public commitment. The chain therefore maps:

| Step | Record interaction | Where |
|---|---|---|
| RFQ | flow-initial state; terms stay off-chain (ADR-010 §1) | buyer flow config |
| Quote | flow state; off-chain quote acceptance wakes the flow (`Woken("quote-accepted:<qid>")`) | buyer flow |
| PO | **emits** `purchase_order` (product, buyer, seller, quantity, currency) | buyer flow |
| Acceptance | **consumes** committed `purchase_order` | seller flow |
| Shipment | **emits** `shipment` (lot_ref, from_org, to_org) | seller flow |
| Receipt | **emits** `delivery_receipt` (shipment_ref, receiver_id, received_at) | buyer flow |
| Dispute | **consumes/references** the `delivery_receipt` (dispute wake) | both |
| Settlement | terminal state referencing the committed PO/receipt | both |

Every step has a defined canonical-record interaction; the three bracket steps
are record-less by settled design (documented in code + evidence comment).

## Changes

### 1. `crates/glasschain-workflows/src/purchase_flow.rs` (new)
- `PurchaseFlowState` (role-tagged): `RfqIssued`, `QuoteAccepted`,
  `AwaitingShipment` (buyer), `AwaitingPurchaseOrder`, `PoAccepted`,
  `AwaitingReceipt` (seller), `Delivered`, `Disputed`, `Settled` (terminal).
- Transitions (one type each): `AcceptQuote` (Resumed wake),
  `CommitPurchaseOrder` (emits PO; deterministic `record_id = po:<rfq_id>`,
  `occurred_at` from config seed), `AcceptPurchaseOrder` (consumes PO),
  `ShipOrder` (emits shipment; `occurred_at = po.occurred_at + 1`),
  `RecordDelivery` (consumes shipment, emits receipt — receipt_flow pattern),
  `RaiseDispute` (Resumed), `Settle` (Resumed, terminal).
- `buyer_flow(config)` / `seller_flow(config)` constructors over one state
  machine; deterministic config (no wall clock).
- Unit tests: per-step emission/consumption, deterministic ids, checkpoint
  resume without duplication (handle → don't ack → re-handle re-delivers;
  ack advances; ledger dedup makes re-execution exactly-once).

### 2. `crates/glasschain-workflows/src/attestation_flow.rs` (new)
- One parameterized implementation, two flows: `certification_flow()` /
  `audit_flow()` over `quality_certification` / `audit_attestation`.
- Shape (receipt_flow's AnchorLot pattern): `AwaitingLot` → consume anchored
  `lot` (holds immutable commitment) → operator wake → emit the family record
  with required `lot_ref, issuer, scope, valid_from, valid_to, status,
  evidence_manifest`, status `"valid"`, `commitment` set (anchored family),
  `record_id = <family>:<lot_ref>`. Source lot never mutated (emit-only).
- Unit tests: anchored commitment present and correct; scope/validity/status
  carried; source record untouched.

### 3. Node-level scenario: `crates/glasschain-network/tests/purchase_settlement_scenario.rs`
- `#![cfg(feature)]`? No feature gate needed — workflows is a plain dep for
  tests (add `glasschain-workflows` as dev-dep of `glasschain-network`; no
  cycle: workflows deps are core/contracts/vm).
- Two nodes (`start(vec![seed])` pattern): maker org (seller) + buyer org.
- Maker commits `product` + anchored `lot`; buyer flow drives RFQ→Quote→PO;
  seller flow consumes PO (Acceptance) → ships; buyer consumes shipment →
  receipt; dispute → settlement on both sides; cert/audit flows emit
  referencing the lot.
- Flow emissions are submitted through the real nodes
  (`submit_transaction(Transaction::with_id(record_id, CanonicalRecord))`,
  runtime attaches the stand-in signature — endorsement layer's job in prod),
  committed via `mine()`, propagated via `Message::Block`; the flow host feeds
  committed records back as `Event::RecordCommitted` from `ledger_snapshot()`.
- Assertions: every record present on BOTH chains (sync), custody edge
  maker→buyer on the lot (shipment from/to + receipt receiver), cert/audit
  reference lot_ref, interrupted step resumes without duplicate records.

## Out of scope
- Triage restart-survival (storage `list`) — #40's ponytail marker; not in ACs.
- Recall/quarantine/dispute depth — #44. PDC private payloads — #46/#47.
- Node-hosted flow runtime — flows run over the seams (as in #40).
