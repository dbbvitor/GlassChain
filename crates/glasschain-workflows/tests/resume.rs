//! Resume/checkpoint/triage tests for the workflow framework (ticket #40
//! AC1–AC3): determinism, no-loss/no-duplication resume, canonical-record
//! reference chains, and the stuck-flow triage view.

use glasschain_core::canonical::validate_record;
use glasschain_core::providers::in_memory::InMemoryStorageProvider;
use glasschain_core::{
    Block, CanonicalRecord, CoreError, InventoryUpdate, RecordSignature, StorageProvider,
    Transaction, TransactionKind,
};
use glasschain_workflows::{
    shipment_receipt_flow, Action, Checkpoint, CheckpointStore, Event, FlowOutcome, FlowRunner,
    FlowState, FlowTriage, ReceiptFlowState, Transition, TransitionResult,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn storage() -> Arc<dyn StorageProvider> {
    Arc::new(InMemoryStorageProvider::new())
}

/// Attach a placeholder signature — the runtime/endorsement layer's job
/// (#45); v1 validation only checks signature presence.
fn signed(record: &mut CanonicalRecord, signer: &str) {
    record.signatures.push(RecordSignature {
        signer: signer.to_owned(),
        signature_bytes: b"sig".to_vec(),
    });
}

/// A valid anchored `lot` record (commitment = canonical commitment).
fn lot_record(occurred_at: u64) -> CanonicalRecord {
    let payload = BTreeMap::from([
        ("lot_id".to_owned(), json!("LOT-1")),
        ("product_id".to_owned(), json!("SKU-1")),
        ("batch_number".to_owned(), json!("BATCH-1")),
    ]);
    let mut lot = CanonicalRecord::new(occurred_at, "lot", payload, "plant-1");
    let commitment = lot.commitment().unwrap();
    lot.commitment = Some(commitment);
    signed(&mut lot, "plant-1");
    lot
}

/// A valid `shipment` record referencing `lot_ref`.
fn shipment_record(lot_ref: &str, occurred_at: u64) -> CanonicalRecord {
    let payload = BTreeMap::from([
        ("lot_ref".to_owned(), json!(lot_ref)),
        ("from_org".to_owned(), json!("plant-1")),
        ("to_org".to_owned(), json!("receiver-1")),
    ]);
    let mut shipment = CanonicalRecord::new(occurred_at, "shipment", payload, "shipper-1");
    signed(&mut shipment, "shipper-1");
    shipment
}

/// A [`StorageProvider`] wrapper that fails `put_state` (from the
/// `put_state_limit`-th call onward) and optionally every `delete_state`,
/// simulating a backend outage mid-flow.
struct FailingStorage {
    inner: Arc<dyn StorageProvider>,
    put_state_limit: usize,
    fail_deletes: bool,
    put_count: AtomicUsize,
}

impl StorageProvider for FailingStorage {
    fn put_block(&self, block: &Block) -> Result<(), CoreError> {
        self.inner.put_block(block)
    }
    fn get_block(&self, index: u64) -> Result<Option<Block>, CoreError> {
        self.inner.get_block(index)
    }
    fn latest_block_index(&self) -> Result<Option<u64>, CoreError> {
        self.inner.latest_block_index()
    }
    fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError> {
        let count = self.put_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.put_state_limit {
            return Err(CoreError::Storage("simulated outage".to_owned()));
        }
        self.inner.put_state(key, value)
    }
    fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
        self.inner.get_state(key)
    }
    fn delete_state(&self, key: &str) -> Result<(), CoreError> {
        if self.fail_deletes {
            return Err(CoreError::Storage("simulated outage".to_owned()));
        }
        self.inner.delete_state(key)
    }
    fn name(&self) -> &'static str {
        "failing"
    }
}

fn failing(
    storage: Arc<dyn StorageProvider>,
    put_state_limit: usize,
    fail_deletes: bool,
) -> Arc<FailingStorage> {
    Arc::new(FailingStorage {
        inner: storage,
        put_state_limit,
        fail_deletes,
        put_count: AtomicUsize::new(0),
    })
}

// ── AC3: canonical records in → canonical records out ────────────────────────

#[test]
fn receipt_flow_emits_valid_record_referencing_immutable_lot_without_mutation() {
    let storage = storage();
    let triage = FlowTriage::new();
    let runner = shipment_receipt_flow("receiver-1", "receiver-org", "2026-01-15");

    let lot = lot_record(1_000);
    let shipment = shipment_record(&lot.record_id, 1_001);
    validate_record(&lot).expect("fixture lot must validate");
    validate_record(&shipment).expect("fixture shipment must validate");
    let shipment_before = serde_json::to_string(&shipment).unwrap();

    // Step 1: consume the lot → anchor its immutable commitment.
    let FlowOutcome {
        state,
        actions,
        completed,
    } = runner
        .handle(
            &storage,
            &triage,
            "flow-1",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(lot.clone()),
        )
        .unwrap()
        .expect("lot event must be handled");
    assert!(!completed);
    assert!(actions.is_empty());
    let ReceiptFlowState::LotAnchored {
        lot_ref,
        lot_commitment,
    } = state
    else {
        panic!("expected LotAnchored, got {state:?}");
    };
    assert_eq!(lot_ref, lot.record_id);
    assert_eq!(lot_commitment, lot.commitment.clone().unwrap());

    // Step 2: consume the shipment → the receipt action is delivered for the
    // runtime to execute.
    let FlowOutcome {
        actions, completed, ..
    } = runner
        .handle(
            &storage,
            &triage,
            "flow-1",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(shipment.clone()),
        )
        .unwrap()
        .expect("shipment event must be handled");
    assert!(!completed, "completion lands on the final ack");
    assert_eq!(actions.len(), 1);
    let Action::EmitRecord(receipt) = &actions[0] else {
        panic!("expected an emitted record, got {:?}", actions[0]);
    };

    // The emitted record is structurally a valid v1 delivery_receipt once the
    // runtime attaches signatures (endorsement, #45).
    let mut to_submit = receipt.clone();
    signed(&mut to_submit, "receiver-org");
    validate_record(&to_submit).expect("emitted receipt must validate");
    assert_eq!(to_submit.payload["shipment_ref"], json!(shipment.record_id));
    assert_eq!(to_submit.payload["receiver_id"], json!("receiver-1"));
    assert_eq!(to_submit.payload["received_at"], json!("2026-01-15"));

    // Reference chain to the immutable lot commitment: receipt → shipment →
    // lot (anchored commitment). Source records are untouched.
    assert_eq!(
        shipment.payload["lot_ref"],
        json!(lot.record_id),
        "shipment must still reference the anchored lot"
    );
    assert_eq!(
        serde_json::to_string(&shipment).unwrap(),
        shipment_before,
        "consuming a record must never mutate it"
    );

    // The runtime executes the receipt durably, then acknowledges.
    runner.ack(&storage, &triage, "flow-1", 1).unwrap();
    assert!(runner.current_state(&storage, "flow-1").unwrap().is_none());
    assert!(triage.entry("flow-1").is_none());
}

// ── AC1: transitions are deterministic ────────────────────────────────────────

#[test]
fn identical_inputs_produce_identical_outputs_across_runners() {
    let lot = lot_record(2_000);
    let shipment = shipment_record(&lot.record_id, 2_001);

    let run = || {
        let storage = storage();
        let triage = FlowTriage::new();
        let runner = shipment_receipt_flow("receiver-1", "receiver-org", "2026-01-15");
        runner
            .handle(
                &storage,
                &triage,
                "flow-d",
                &ReceiptFlowState::AwaitingLot,
                &Event::RecordCommitted(lot.clone()),
            )
            .unwrap()
            .expect("lot event must be handled");
        runner
            .handle(
                &storage,
                &triage,
                "flow-d",
                &ReceiptFlowState::AwaitingLot,
                &Event::RecordCommitted(shipment.clone()),
            )
            .unwrap()
            .expect("shipment event must be handled")
    };

    assert_eq!(run(), run(), "replay must reach the identical outcome");
}

// ── AC2: interrupted flows resume without loss or duplication ────────────────

#[test]
fn interruption_before_pending_save_loses_nothing() {
    let storage = storage();
    let triage = FlowTriage::new();
    let runner = shipment_receipt_flow("receiver-1", "receiver-org", "2026-01-15");
    let lot = lot_record(3_000);
    let shipment = shipment_record(&lot.record_id, 3_001);

    runner
        .handle(
            &storage,
            &triage,
            "flow-2",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(lot),
        )
        .unwrap()
        .expect("lot event must be handled");

    // Backend outage on the pending checkpoint write of the shipment step:
    // the flow must still be waiting on the lot, with nothing pending.
    let outage = failing(Arc::clone(&storage), 0, false);
    let err = runner
        .handle(
            &(outage as Arc<dyn StorageProvider>),
            &triage,
            "flow-2",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(shipment.clone()),
        )
        .expect_err("outage must surface as an error");
    assert!(matches!(
        err,
        glasschain_workflows::WorkflowError::Storage(_)
    ));

    // Retry on the healthy backend: the receipt is delivered exactly once.
    let outcome = runner
        .handle(
            &storage,
            &triage,
            "flow-2",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(shipment),
        )
        .unwrap()
        .expect("shipment event must be handled");
    assert_eq!(outcome.actions.len(), 1);
    runner.ack(&storage, &triage, "flow-2", 1).unwrap();
}

#[test]
fn interruption_before_ack_replays_deterministically_with_idempotent_sink() {
    let storage = storage();
    let triage = FlowTriage::new();
    let runner = shipment_receipt_flow("receiver-1", "receiver-org", "2026-01-15");
    let lot = lot_record(4_000);
    let shipment = shipment_record(&lot.record_id, 4_001);

    runner
        .handle(
            &storage,
            &triage,
            "flow-3",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(lot),
        )
        .unwrap()
        .expect("lot event must be handled");

    // The runtime receives the receipt and executes it (first delivery)…
    let outcome = runner
        .handle(
            &storage,
            &triage,
            "flow-3",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(shipment.clone()),
        )
        .unwrap()
        .expect("shipment event must be handled");
    let first_delivery = outcome.actions;
    assert_eq!(first_delivery.len(), 1);

    // …but the ack fails (outage): the checkpoint still says nothing ran.
    let outage = failing(Arc::clone(&storage), usize::MAX, true);
    let err = runner
        .ack(&(outage as Arc<dyn StorageProvider>), &triage, "flow-3", 1)
        .expect_err("outage must surface as an error");
    assert!(matches!(
        err,
        glasschain_workflows::WorkflowError::Storage(_)
    ));

    let store = CheckpointStore::new(Arc::clone(&storage));
    let saved: Checkpoint = store
        .load("flow-3")
        .unwrap()
        .expect("checkpoint must exist");
    assert_eq!(saved.next_action, 0, "no action is recorded as executed");
    assert!(
        saved.pending_event.is_some(),
        "the shipment transition is pending"
    );

    // Resume: the action is re-delivered (at-least-once) with the *same*
    // deterministic record id, so the ledger dedupes the effect.
    let outcome = runner
        .handle(
            &storage,
            &triage,
            "flow-3",
            &ReceiptFlowState::AwaitingLot,
            &Event::Resumed("operator intervention".to_owned()),
        )
        .unwrap()
        .expect("resume must re-deliver the pending action");
    assert_eq!(outcome.actions, first_delivery, "re-delivery is identical");
    let Action::EmitRecord(receipt) = &outcome.actions[0] else {
        panic!("expected an emitted record");
    };
    assert_eq!(receipt.record_id, format!("receipt:{}", shipment.record_id));

    runner.ack(&storage, &triage, "flow-3", 1).unwrap();
    assert!(
        runner.current_state(&storage, "flow-3").unwrap().is_none(),
        "completed checkpoint is cleared"
    );
}

#[test]
fn partial_ack_skips_executed_actions_on_resume() {
    let storage = storage();
    let triage = FlowTriage::new();
    let runner: FlowRunner<DoubleEmitState> =
        FlowRunner::new("double_emit", vec![Box::new(DoubleEmitTransition)]);
    let update = InventoryUpdate {
        product_id: "SKU".to_owned(),
        owner_id: "buyer-1".to_owned(),
        quantity_delta: -1,
        reason: "x".to_owned(),
    };

    // The transition emits two actions; the runtime executes the first and
    // acknowledges it before the outage.
    let outcome = runner
        .handle(
            &storage,
            &triage,
            "flow-6",
            &DoubleEmitState::Waiting,
            &Event::TransactionCommitted(TransactionKind::InventoryUpdate(update)),
        )
        .unwrap()
        .expect("update event must be handled");
    assert_eq!(outcome.actions.len(), 2);
    runner.ack(&storage, &triage, "flow-6", 1).unwrap();

    // Resume delivers only the not-yet-acknowledged action.
    let outcome = runner
        .handle(
            &storage,
            &triage,
            "flow-6",
            &DoubleEmitState::Waiting,
            &Event::Resumed("operator intervention".to_owned()),
        )
        .unwrap()
        .expect("resume must re-deliver the remaining action");
    assert_eq!(outcome.actions.len(), 1, "one action remains unexecuted");
    assert_eq!(outcome.actions[0], double_emit_action(2));

    runner.ack(&storage, &triage, "flow-6", 2).unwrap();
    assert!(runner.current_state(&storage, "flow-6").unwrap().is_none());
}

// ── Idle flows and unmatched events ───────────────────────────────────────────

#[test]
fn unmatched_event_and_resume_without_pending_work_are_noops() {
    let storage = storage();
    let triage = FlowTriage::new();
    let runner = shipment_receipt_flow("receiver-1", "receiver-org", "2026-01-15");
    let lot = lot_record(5_000);
    let shipment = shipment_record(&lot.record_id, 5_001);

    // A shipment before its lot matches no transition.
    let outcome = runner
        .handle(
            &storage,
            &triage,
            "flow-4",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(shipment),
        )
        .unwrap();
    assert!(outcome.is_none(), "unmatched events are ignored");

    // A resume with no pending work and no checkpoint is also a no-op.
    let outcome = runner
        .handle(
            &storage,
            &triage,
            "flow-4",
            &ReceiptFlowState::AwaitingLot,
            &Event::Resumed("poll".to_owned()),
        )
        .unwrap();
    assert!(outcome.is_none());

    // … and after a waiting checkpoint too.
    runner
        .handle(
            &storage,
            &triage,
            "flow-4",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(lot),
        )
        .unwrap()
        .expect("lot event must be handled");
    let outcome = runner
        .handle(
            &storage,
            &triage,
            "flow-4",
            &ReceiptFlowState::AwaitingLot,
            &Event::Resumed("poll".to_owned()),
        )
        .unwrap();
    assert!(outcome.is_none(), "waiting flows have nothing to resume");
}

// ── AC2: triage view surfaces stuck flows ────────────────────────────────────

#[test]
fn triage_surfaces_stuck_flows_and_resurfaces_after_restart() {
    let storage = storage();
    let triage = FlowTriage::new();
    let runner = shipment_receipt_flow("receiver-1", "receiver-org", "2026-01-15");
    let lot = lot_record(6_000);

    runner
        .handle(
            &storage,
            &triage,
            "flow-5",
            &ReceiptFlowState::AwaitingLot,
            &Event::RecordCommitted(lot),
        )
        .unwrap()
        .expect("lot event must be handled");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let entry = triage.entry("flow-5").expect("flow must be tracked");
    assert_eq!(entry.flow_kind, "shipment_receipt");
    assert_eq!(entry.step, "lot_anchored");
    assert!(
        triage.stuck_flows(now, 60).is_empty(),
        "a just-updated flow is not stuck"
    );

    // A flow whose last durable point is old is stuck; ordering is stable.
    triage.record("flow-old", "shipment_receipt", "lot_anchored", now - 120);
    triage.record("flow-older", "shipment_receipt", "lot_anchored", now - 360);
    let stuck = triage.stuck_flows(now, 60);
    let ids: Vec<&str> = stuck.iter().map(|e| e.flow_id.as_str()).collect();
    assert_eq!(ids, vec!["flow-old", "flow-older"]);

    // After a triage restart (fresh registry), driving the waiting flow
    // re-surfaces it with its *stored* timestamp, preserving staleness.
    let fresh_triage = FlowTriage::new();
    assert!(fresh_triage.entry("flow-5").is_none());
    let outcome = runner
        .handle(
            &storage,
            &fresh_triage,
            "flow-5",
            &ReceiptFlowState::AwaitingLot,
            &Event::Resumed("restart".to_owned()),
        )
        .unwrap();
    assert!(outcome.is_none());
    let store = CheckpointStore::new(Arc::clone(&storage));
    let saved = store
        .load("flow-5")
        .unwrap()
        .expect("checkpoint must exist");
    let resurfaced = fresh_triage.entry("flow-5").expect("flow must re-surface");
    assert_eq!(resurfaced.updated_at, saved.updated_at);
}

// ── FlowRunner/CheckpointStore plumbing ──────────────────────────────────────

#[test]
fn checkpoint_store_round_trips_and_deletes() {
    let storage = storage();
    let store = CheckpointStore::new(Arc::clone(&storage));

    let checkpoint = Checkpoint {
        flow_id: "flow-c".to_owned(),
        flow_kind: "shipment_receipt".to_owned(),
        state: json!({ "LotAnchored": { "lot_ref": "lot-1", "lot_commitment": "c1" } }),
        pending_event: None,
        next_action: 0,
        updated_at: 7,
    };
    store.save(&checkpoint).unwrap();
    assert_eq!(store.load("flow-c").unwrap(), Some(checkpoint));
    assert_eq!(CheckpointStore::key("flow-c"), "workflow:checkpoint:flow-c");

    store.delete("flow-c").unwrap();
    assert!(store.load("flow-c").unwrap().is_none());
}

// ── Double-emit flow fixture (partial-ack coverage) ──────────────────────────

/// A test-only flow that emits two deterministic transactions at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum DoubleEmitState {
    Waiting,
}

impl FlowState for DoubleEmitState {
    fn step(&self) -> &'static str {
        "waiting"
    }
}

struct DoubleEmitTransition;

impl Transition<DoubleEmitState> for DoubleEmitTransition {
    fn name(&self) -> &'static str {
        "DoubleEmit"
    }

    fn matches(&self, state: &DoubleEmitState, event: &Event) -> bool {
        matches!(state, DoubleEmitState::Waiting)
            && matches!(
                event,
                Event::TransactionCommitted(TransactionKind::InventoryUpdate(_))
            )
    }

    fn apply(&self, _state: &DoubleEmitState, _event: &Event) -> TransitionResult<DoubleEmitState> {
        TransitionResult::new(
            DoubleEmitState::Waiting,
            vec![double_emit_action(1), double_emit_action(2)],
            true,
        )
    }
}

/// One deterministic emission of the double-emit flow.
fn double_emit_action(index: u64) -> Action {
    Action::EmitTransaction(Transaction::with_id(
        format!("double-{index}"),
        TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU".to_owned(),
            owner_id: "buyer-1".to_owned(),
            quantity_delta: -index.cast_signed(),
            reason: "flow emission".to_owned(),
        }),
    ))
}
