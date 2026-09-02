# Plan — Recall, quarantine, and dispute flows (ticket #44)

**Ticket:** [#44 Recall, quarantine, and dispute as first-class flows](https://github.com/dbbvitor/GlassChain/issues/44)
**Builds on:** #40 framework, #43 flows (`Event::Woken`, Send+Sync runners).

## Design

### 1. `glasschain-workflows/src/recall_flow.rs` (new) — three first-class flows

**`recall_flow(config)` — the issuer's recall lifecycle** (emits the Recall
record family; `lot_ref, reason, status, issued_by`; status vocabulary
issued/active/completed):

```text
AwaitingLot ─(anchored lot)→ LotAnchored{lot_ref, lot_commitment}
LotAnchored ─(Woken "recall:<reason>")→ emits recall{status:"issued"} → Recalled
Recalled    ─(Woken "activate")→        emits recall{status:"active"}  → RecallActive
RecallActive ─(Woken "complete")→       emits recall{status:"completed"} → Completed
```

Record ids: `recall:<lot_ref>`, `recall:<lot_ref>:active`,
`recall:<lot_ref>:completed` — append-only status trail, source records never
mutated.

**`quarantine_flow(config)` / `dispute_flow(config)` — custodian responses**
(one shared `RecallResponseState` machine, two first-class tables):

```text
WatchingLot ─(RecordCommitted recall for the held lot)→ RecallObserved
RecallObserved ─(Woken "quarantine")→ emits inventory_transformation{lot_ref,
                                        transformation_type:"quarantine"} → terminal
RecallObserved ─(Woken "dispute:<reason>")→ emits inventory_transformation{
                                              transformation_type:"disputed"} → terminal
```

Custodian flows trigger on the PUBLIC recall record — a custodian is never the
recall's counterparty, demonstrating traversability. The dispute reason stays
in flow state (the `inventory_transformation` payload whitelist admits only
`lot_ref` + `transformation_type`, so commercial/dispute detail cannot enter
the chain — ADR-010 §1 by construction). Record id: `transformation:<lot_ref>:<kind>`.

### 2. Node-level scenario: `crates/glasschain-network/tests/recall_flow_scenario.rs`

Three fully-connected nodes (manufacturer, distributor, pharmacy):
1. Manufacturer anchors product + anchored lot; custody chain manufacturer →
   distributor → pharmacy (shipment + delivery_receipt per hop).
2. Manufacturer's recall flow issues and activates the recall.
3. Distributor's quarantine flow observes the public recall → quarantines.
4. Pharmacy's dispute flow observes the recall → disputes.
5. AC4: the recall issuance is interrupted (re-delivery before ack, single
   execution + ack) — no duplicate records.
6. Assertions on ALL THREE chains: the public recall trail (recall issued +
   active + both custody/status records) is present everywhere; no duplicates;
   the dispute reason string appears in no committed payload; no
   `purchase_order` (commercial) record exists anywhere.

### 3. Replace the old simulation

Delete `test_recall_simulation_manufacturer_to_pharmacy` from
`crates/glasschain-network/tests/chaos_tests.rs` (legacy
AssetRegistration/InventoryUpdate single-node simulation) — replaced by the
flow-driven scenario above.

## Out of scope
- Recall traversal via the provenance index (legacy-AssetRegistration-based;
  canonical-record ingest is analytics work, not gated by this ticket).
- Endorsement-authorized recalls (governance wiring — separate).
