//! AC4: the existing offer→PO automation, inventory triggers, and approval
//! gates remain fully functional and are exercised by the framework's tests.
//!
//! The automation crates are driven unchanged — the framework steers them from
//! outside: the test runtime feeds engine/watcher outputs back into flows as
//! events, exactly as a node would.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use glasschain_contracts::ContractEngine;
use glasschain_core::providers::in_memory::InMemoryStorageProvider;
use glasschain_core::{
    InventoryUpdate, PurchaseConditions, PurchaseOrder, SmartContractDef, StorageProvider,
    SupplyOffer, TransactionKind,
};
use glasschain_workflows::{
    Event, FlowOutcome, FlowRunner, FlowState, FlowTriage, Transition, TransitionResult,
};
use glasschain_workflows::{InventoryTrigger, WatcherService};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── WASM gate fixtures (same approval protocol as the automation) ────────────

fn gate_wasm_b64(approve: bool) -> String {
    let value = if approve { "1" } else { "0" };
    let wat = format!(
        r#"
(module
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "approve")
  (data (i32.const 7) "{value}")
  (func (export "execute")
    (call $set_state (i32.const 0) (i32.const 7) (i32.const 7) (i32.const 1))
  )
)
"#
    );
    let wasm = wat::parse_str(&wat).expect("fixture WAT must compile");
    BASE64_STANDARD.encode(wasm)
}

fn trigger(product_id: &str, wasm_code_b64: Option<String>) -> InventoryTrigger {
    InventoryTrigger {
        trigger_id: format!("trig-{product_id}"),
        product_id: product_id.to_owned(),
        owner_id: "buyer-1".to_owned(),
        reorder_threshold: 0,
        reorder_quantity: 25,
        seller_id: "seller-1".to_owned(),
        price_per_unit: 100,
        currency: "USD".to_owned(),
        active: true,
        wasm_code_b64,
    }
}

// ── 1. Offer → PO automation (unchanged) ─────────────────────────────────────

#[test]
fn offer_to_po_automation_remains_functional() {
    let mut engine = ContractEngine::new();
    engine
        .register_contract(SmartContractDef {
            contract_id: "c-1".to_owned(),
            buyer_id: "buyer-1".to_owned(),
            product_id: "SKU-1".to_owned(),
            conditions: PurchaseConditions {
                max_price_per_unit: 1_000,
                min_quantity: 1,
                max_quantity: 50,
                max_lead_time_days: 5,
                preferred_seller_id: None,
                currency: "USD".to_owned(),
                auto_execute: true,
            },
            wasm_code_b64: None,
        })
        .expect("contract must register");

    let offer = SupplyOffer {
        product_id: "SKU-1".to_owned(),
        product_name: "Widget".to_owned(),
        seller_id: "seller-1".to_owned(),
        quantity_available: 100,
        price_per_unit: 900,
        lead_time_days: 3,
        currency: "USD".to_owned(),
    };

    let generated = engine.evaluate_supply_offer(&offer, "offer-1");
    assert_eq!(generated.len(), 2, "purchase order + execution record");
    assert!(
        matches!(
            &generated[0].kind,
            TransactionKind::PurchaseOrder(po) if po.quantity == 50 && po.seller_id == "seller-1"
        ),
        "the engine must still auto-execute a matching offer"
    );
}

// ── 2. Inventory trigger automation (unchanged) ──────────────────────────────

#[test]
fn inventory_trigger_automation_remains_functional() {
    let mut watcher = WatcherService::new();
    watcher.add_trigger(trigger("SKU", None));

    let orders = watcher.on_inventory_update(&InventoryUpdate {
        product_id: "SKU".to_owned(),
        owner_id: "buyer-1".to_owned(),
        quantity_delta: -100,
        reason: "consumption".to_owned(),
    });
    assert_eq!(orders.len(), 1);
    assert!(
        matches!(&orders[0].kind, TransactionKind::PurchaseOrder(po) if po.quantity == 25),
        "the watcher must still reorder when the threshold is crossed"
    );
}

// ── 3. Approval gate automation (unchanged, through the watcher path) ────────

#[test]
fn approval_gate_automation_remains_functional() {
    let executor = glasschain_vm::WasmExecutionProvider::new().expect("wasmtime must init");
    let update = InventoryUpdate {
        product_id: "SKU-GATED".to_owned(),
        owner_id: "buyer-1".to_owned(),
        quantity_delta: -100,
        reason: "consumption".to_owned(),
    };

    // Approving gate → order emitted.
    let mut watcher = WatcherService::new();
    watcher.set_executor(Arc::new(executor));
    watcher.add_trigger(trigger("SKU-GATED", Some(gate_wasm_b64(true))));
    let orders = watcher.on_inventory_update(&update);
    assert_eq!(
        orders.len(),
        1,
        "an approving gate must not block the order"
    );

    // Denying gate → no order.
    let executor = glasschain_vm::WasmExecutionProvider::new().expect("wasmtime must init");
    let mut watcher = WatcherService::new();
    watcher.set_executor(Arc::new(executor));
    watcher.add_trigger(trigger("SKU-GATED", Some(gate_wasm_b64(false))));
    let orders = watcher.on_inventory_update(&update);
    assert!(orders.is_empty(), "a denying gate must suppress the order");
}

// ── 4. A flow consumes the automation's outputs as events ────────────────────

/// Minimal test-only flow tracking the first purchase order it sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum PoTrackingState {
    Waiting,
    Tracked { seller: String },
}

impl FlowState for PoTrackingState {
    fn step(&self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Tracked { .. } => "tracked",
        }
    }
}

struct TrackPurchaseOrder;

impl Transition<PoTrackingState> for TrackPurchaseOrder {
    fn name(&self) -> &'static str {
        "TrackPurchaseOrder"
    }

    fn matches(&self, state: &PoTrackingState, event: &Event) -> bool {
        matches!(state, PoTrackingState::Waiting)
            && matches!(
                event,
                Event::TransactionCommitted(TransactionKind::PurchaseOrder(_))
            )
    }

    fn apply(&self, state: &PoTrackingState, event: &Event) -> TransitionResult<PoTrackingState> {
        let Event::TransactionCommitted(TransactionKind::PurchaseOrder(po)) = event else {
            return TransitionResult::new(state.clone(), Vec::new(), false);
        };
        TransitionResult::new(
            PoTrackingState::Tracked {
                seller: po.seller_id.clone(),
            },
            Vec::new(),
            true,
        )
    }
}

#[test]
fn flow_consumes_automation_outputs_as_events() {
    // The runtime loop: the watcher (automation) produces a purchase order;
    // the node commits it and feeds it to the flow as an event.
    let mut watcher = WatcherService::new();
    watcher.add_trigger(trigger("SKU", None));
    let orders = watcher.on_inventory_update(&InventoryUpdate {
        product_id: "SKU".to_owned(),
        owner_id: "buyer-1".to_owned(),
        quantity_delta: -100,
        reason: "consumption".to_owned(),
    });
    let po: &PurchaseOrder = match &orders[0].kind {
        TransactionKind::PurchaseOrder(po) => po,
        other => panic!("expected a purchase order, got {other:?}"),
    };

    let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
    let triage = FlowTriage::new();
    let runner: FlowRunner<PoTrackingState> =
        FlowRunner::new("po_tracking", vec![Box::new(TrackPurchaseOrder)]);
    let FlowOutcome {
        state, completed, ..
    } = runner
        .handle(
            &storage,
            &triage,
            "flow-po",
            &PoTrackingState::Waiting,
            &Event::TransactionCommitted(TransactionKind::PurchaseOrder(po.clone())),
        )
        .unwrap()
        .expect("the flow must consume the automation's purchase order");
    assert!(completed);
    assert_eq!(
        state,
        PoTrackingState::Tracked {
            seller: "seller-1".to_owned(),
        }
    );
}
