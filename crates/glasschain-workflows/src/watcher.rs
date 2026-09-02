//! Watcher service for inventory-threshold-based contract triggering.
//!
//! I/O-driven automation (ticket #49's packaging split): the watcher observes
//! committed [`InventoryUpdate`] events and autonomously emits `PurchaseOrder`
//! transactions — the workflow half of the Corda contracts/workflows split.
//! The deterministic contract layer (registry, matching, approval gate)
//! remains in `glasschain-contracts`.
//!
//! The [`WatcherService`] observes World State changes (specifically
//! [`InventoryUpdate`][glasschain_core::InventoryUpdate] transactions) and
//! autonomously submits `PurchaseOrder` transactions when an inventory level
//! drops below a configured threshold.
//!
//! ## Event-Condition-Action (ECA) Model
//!
//! ```text
//! Event:     InventoryUpdate(product_id="SKU-001", delta=-50)
//! Condition: inventory["SKU-001"]["owner-1"] < threshold (e.g. 100 units)
//! Action:    Submit PurchaseOrder(product_id="SKU-001", qty=reorder_qty)
//! ```
//!
//! Triggers are registered via [`WatcherService::add_trigger`] and evaluated
//! on every call to [`WatcherService::on_inventory_update`].
//!
//! ## Optional WASM Gating
//!
//! Each trigger may carry a base64-encoded WASM module in `wasm_code_b64`.
//! When an [`ExecutionProvider`] is registered via [`WatcherService::set_executor`],
//! the WASM module is executed before any `PurchaseOrder` is emitted.  The
//! contract must write `approve = b"1"` via the `set_state` host function for
//! the order to proceed.
//!
//! ## State Persistence
//!
//! [`WatcherService::serialize_state`] / [`WatcherService::restore_from_bytes`]
//! round-trip the runtime inventory and fire-count state through
//! [`WatcherStateSnapshot`], enabling seamless recovery after a node restart
//! without requiring a full chain replay.

use glasschain_contracts::approval_gate::{ApprovalGate, ApprovalGatePolicy, GateDecision};
use glasschain_core::{
    ExecutionProvider, InventoryUpdate, PurchaseOrder, Transaction, TransactionKind,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

// ── InventoryTrigger ──────────────────────────────────────────────────────────

/// Defines when and how the watcher should auto-generate a purchase order.
///
/// Derive [`Default`] so that callers constructing the struct with field
/// initialiser syntax can use `..Default::default()` to zero-fill any fields
/// added in the future (including `wasm_code_b64`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InventoryTrigger {
    /// Unique trigger identifier.
    pub trigger_id: String,
    /// Product to watch.
    pub product_id: String,
    /// Owner whose inventory level is monitored.
    pub owner_id: String,
    /// When inventory drops **at or below** this level, the trigger fires.
    pub reorder_threshold: i64,
    /// Quantity to order when the trigger fires.
    pub reorder_quantity: u64,
    /// Preferred seller for the auto-generated purchase order.
    pub seller_id: String,
    /// Agreed unit price in minor currency units (e.g. cents for USD).
    pub price_per_unit: u64,
    /// ISO-4217 currency code.
    pub currency: String,
    /// Whether this trigger is currently active.
    pub active: bool,
    /// Optional base64-encoded WASM module used to gate `PurchaseOrder` emission.
    ///
    /// When `Some`, the module is executed through the registered
    /// [`ExecutionProvider`] before a `PurchaseOrder` is generated.  The WASM
    /// contract **must** call `set_state("approve", "1")` for the order to
    /// proceed.  Any other value — or the absence of the key — causes the
    /// trigger to be skipped for this inventory event.
    ///
    /// If no executor has been registered via [`WatcherService::set_executor`]
    /// but this field is `Some`, the trigger fires unconditionally (useful in
    /// development / test environments without a full VM stack).
    pub wasm_code_b64: Option<String>,
}

// ── WatcherStateSnapshot ──────────────────────────────────────────────────────

/// Serializable snapshot of the watcher's runtime state.
///
/// Used to persist inventory levels and fire counts across node restarts
/// via the `StorageProvider` backend.
///
/// **Triggers are intentionally excluded** from the snapshot: they are
/// reconstructed via [`WatcherService::add_trigger`] during chain replay on
/// startup, which is the canonical source of truth for trigger definitions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WatcherStateSnapshot {
    /// Per-product per-owner inventory levels.
    pub inventory: HashMap<String, HashMap<String, i64>>,
    /// Per-trigger fire counts (for unique tx ID generation).
    pub trigger_fire_counts: HashMap<String, u64>,
}

// ── WatcherService ────────────────────────────────────────────────────────────

/// The Watcher service.
///
/// Tracks per-owner inventory levels and fires registered triggers when
/// levels fall below their configured thresholds.
///
/// `WatcherService` does **not** derive [`Default`] because
/// `Arc<dyn ExecutionProvider>` does not implement [`Default`].  A manual
/// implementation is provided instead, delegating to [`WatcherService::new`].
pub struct WatcherService {
    /// Active triggers keyed by `trigger_id`.
    triggers: HashMap<String, InventoryTrigger>,
    /// Running inventory totals: `inventory[product_id][owner_id] = level`.
    inventory: HashMap<String, HashMap<String, i64>>,
    /// Per-trigger fire counter to guarantee unique transaction IDs across
    /// repeated firings of the same trigger.
    trigger_fire_counts: HashMap<String, u64>,
    /// Optional WASM execution provider for trigger gating.
    executor: Option<Arc<dyn ExecutionProvider>>,
}

impl fmt::Debug for WatcherService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatcherService")
            .field("triggers", &self.triggers)
            .field("inventory", &self.inventory)
            .field("trigger_fire_counts", &self.trigger_fire_counts)
            .field(
                "executor",
                if self.executor.is_some() {
                    &"<set>"
                } else {
                    &"<none>"
                },
            )
            .finish()
    }
}

impl Default for WatcherService {
    fn default() -> Self {
        Self::new()
    }
}

impl WatcherService {
    /// Create a new empty watcher service.
    #[must_use]
    pub fn new() -> Self {
        Self {
            triggers: HashMap::new(),
            inventory: HashMap::new(),
            trigger_fire_counts: HashMap::new(),
            executor: None,
        }
    }

    /// Register a WASM execution provider for trigger gating.
    ///
    /// When set, any [`InventoryTrigger`] with a `wasm_code_b64` payload will be
    /// executed through this provider before a `PurchaseOrder` is generated.
    /// Triggers without `wasm_code_b64` are always executed unconditionally.
    pub fn set_executor(&mut self, executor: Arc<dyn ExecutionProvider>) {
        self.executor = Some(executor);
    }

    /// Register an inventory-level trigger.
    pub fn add_trigger(&mut self, trigger: InventoryTrigger) {
        self.triggers.insert(trigger.trigger_id.clone(), trigger);
    }

    /// Deactivate a trigger by ID.
    pub fn disable_trigger(&mut self, trigger_id: &str) {
        if let Some(t) = self.triggers.get_mut(trigger_id) {
            t.active = false;
        }
    }

    /// Re-enable a previously disabled trigger.
    pub fn enable_trigger(&mut self, trigger_id: &str) {
        if let Some(t) = self.triggers.get_mut(trigger_id) {
            t.active = true;
        }
    }

    /// Return the current inventory level for `(product_id, owner_id)`.
    #[must_use]
    pub fn inventory_level(&self, product_id: &str, owner_id: &str) -> i64 {
        self.inventory
            .get(product_id)
            .and_then(|m| m.get(owner_id))
            .copied()
            .unwrap_or(0)
    }

    // ── State persistence ─────────────────────────────────────────────────────

    /// Capture the current runtime state as a [`WatcherStateSnapshot`].
    ///
    /// The snapshot captures inventory levels and trigger fire counts —
    /// everything needed to resume operation after a node restart without
    /// replaying the chain.
    #[must_use]
    pub fn to_snapshot(&self) -> WatcherStateSnapshot {
        WatcherStateSnapshot {
            inventory: self.inventory.clone(),
            trigger_fire_counts: self.trigger_fire_counts.clone(),
        }
    }

    /// Serialize the current state as a JSON byte vector for storage.
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if serialisation fails (unlikely for
    /// the plain `HashMap` types held by this struct).
    pub fn serialize_state(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&self.to_snapshot())
    }

    /// Restore inventory and fire-count state from a previously captured snapshot.
    ///
    /// Triggers are **not** restored from the snapshot — they must be
    /// re-registered via [`add_trigger`][Self::add_trigger] (they come from
    /// chain replay on startup).
    pub fn apply_snapshot(&mut self, snapshot: WatcherStateSnapshot) {
        self.inventory = snapshot.inventory;
        self.trigger_fire_counts = snapshot.trigger_fire_counts;
    }

    /// Deserialize a state snapshot from JSON bytes and apply it.
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if the bytes are not valid JSON matching
    /// [`WatcherStateSnapshot`].
    pub fn restore_from_bytes(&mut self, data: &[u8]) -> Result<(), serde_json::Error> {
        let snapshot: WatcherStateSnapshot = serde_json::from_slice(data)?;
        self.apply_snapshot(snapshot);
        Ok(())
    }

    // ── Core ECA loop ─────────────────────────────────────────────────────────

    /// Process an [`InventoryUpdate`] transaction and return any auto-generated
    /// [`PurchaseOrder`] transactions.
    ///
    /// This is the core ECA (Event-Condition-Action) evaluation loop:
    /// 1. **Event** — an `InventoryUpdate` arrives.
    /// 2. **Condition** — is the new level at or below any trigger threshold?
    /// 3. **Action** — generate a `PurchaseOrder` and return it for submission.
    ///
    /// Each firing of a trigger increments an internal counter that is embedded
    /// in the transaction ID, guaranteeing unique IDs across repeated firings.
    ///
    /// When a trigger carries a `wasm_code_b64` module **and** an executor has
    /// been registered, the module is executed and must approve the order (see
    /// the module-level documentation for the exact approval protocol).
    // Keep the event-condition-action evaluation in one transaction path.
    #[allow(clippy::too_many_lines)]
    pub fn on_inventory_update(&mut self, update: &InventoryUpdate) -> Vec<Transaction> {
        // Apply the inventory delta.
        let level = self
            .inventory
            .entry(update.product_id.clone())
            .or_default()
            .entry(update.owner_id.clone())
            .or_insert(0);
        *level += update.quantity_delta;
        let new_level = *level;

        log::debug!(
            "WatcherService: inventory[{}][{}] = {}",
            update.product_id,
            update.owner_id,
            new_level
        );

        // Collect triggers that should fire into a local Vec so that we can
        // subsequently mutate `trigger_fire_counts` without conflicting with
        // the shared borrow on `self.triggers`.
        let firing_triggers: Vec<InventoryTrigger> = self
            .triggers
            .values()
            .filter(|t| {
                t.active
                    && t.product_id == update.product_id
                    && t.owner_id == update.owner_id
                    && new_level <= t.reorder_threshold
            })
            .cloned()
            .collect();

        let mut orders = Vec::new();
        for trigger in firing_triggers {
            // Increment the per-trigger fire counter and capture it so the
            // transaction ID is unique across multiple firings.
            let fire_count = self
                .trigger_fire_counts
                .entry(trigger.trigger_id.clone())
                .or_insert(0);
            *fire_count += 1;
            let count = *fire_count;

            log::info!(
                "WatcherService: trigger '{}' fired (#{}) — level {} ≤ threshold {}; ordering {}",
                trigger.trigger_id,
                count,
                new_level,
                trigger.reorder_threshold,
                trigger.reorder_quantity,
            );

            // ── WASM gate (optional) ──────────────────────────────────────────
            //
            // If the trigger carries a WASM module, execute it with the current
            // inventory level as pre-populated state.  The contract must set
            // `approve = b"1"` to allow the PurchaseOrder to proceed.
            if let Some(ref wasm_b64) = trigger.wasm_code_b64 {
                if let Some(ref exec) = self.executor {
                    // Provide the current inventory level as context.
                    let mut world_state: HashMap<String, Vec<u8>> = HashMap::new();
                    world_state.insert(
                        "inventory_level".to_string(),
                        new_level.to_string().into_bytes(),
                    );
                    world_state.insert(
                        "threshold".to_string(),
                        trigger.reorder_threshold.to_string().into_bytes(),
                    );
                    world_state.insert(
                        "product_id".to_string(),
                        trigger.product_id.as_bytes().to_vec(),
                    );

                    let initial_state = Ok(world_state);
                    let gate =
                        ApprovalGate::new(exec.as_ref(), ApprovalGatePolicy::InventoryTrigger);
                    match gate.evaluate(&trigger.trigger_id, wasm_b64, initial_state) {
                        GateDecision::Approved => {
                            log::debug!(
                                "WatcherService: trigger '{}' WASM approved the PurchaseOrder",
                                trigger.trigger_id
                            );
                        }
                        GateDecision::Denied { reason } => {
                            log::warn!(
                                "WatcherService: trigger '{}' WASM gate denied the PurchaseOrder: {reason}",
                                trigger.trigger_id
                            );
                            continue;
                        }
                    }
                }
                // If no executor is configured but wasm_code_b64 is set, fall
                // through (treat as approved) so the trigger still fires in
                // dev/test environments without a full VM stack.
            }

            let tx = Transaction::with_id(
                format!(
                    "watcher:{}:{}:{}:{}",
                    trigger.trigger_id, update.product_id, update.owner_id, count,
                ),
                TransactionKind::PurchaseOrder(PurchaseOrder {
                    product_id: trigger.product_id.clone(),
                    buyer_id: trigger.owner_id.clone(),
                    seller_id: trigger.seller_id.clone(),
                    quantity: trigger.reorder_quantity,
                    agreed_price_per_unit: trigger.price_per_unit,
                    currency: trigger.currency.clone(),
                    contract_id: Some(trigger.trigger_id.clone()),
                }),
            );
            orders.push(tx);
        }
        orders
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_contracts::test_wasm::{approving_wasm_b64, denying_wasm_b64};
    use glasschain_vm::WasmExecutionProvider;
    use std::sync::Arc;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a minimal trigger with `price_per_unit` expressed in minor
    /// currency units (e.g. cents: 1000 = $10.00).
    ///
    /// `wasm_code_b64` defaults to `None` via `..Default::default()`, so all
    /// callers that don't need WASM gating require no change.
    fn make_trigger(
        id: &str,
        product: &str,
        owner: &str,
        threshold: i64,
        reorder_qty: u64,
    ) -> InventoryTrigger {
        InventoryTrigger {
            trigger_id: id.into(),
            product_id: product.into(),
            owner_id: owner.into(),
            reorder_threshold: threshold,
            reorder_quantity: reorder_qty,
            seller_id: "supplier-1".into(),
            price_per_unit: 1000,
            currency: "USD".into(),
            active: true,
            ..Default::default()
        }
    }

    fn inv_update(product: &str, owner: &str, delta: i64) -> InventoryUpdate {
        InventoryUpdate {
            product_id: product.into(),
            owner_id: owner.into(),
            quantity_delta: delta,
            reason: "test".into(),
        }
    }

    // ── Inventory accumulation ────────────────────────────────────────────────

    #[test]
    fn test_inventory_accumulates() {
        let mut svc = WatcherService::new();
        svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 200));
        svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", -50));
        assert_eq!(svc.inventory_level("SKU-001", "pharmacy-1"), 150);
    }

    // ── Trigger firing ────────────────────────────────────────────────────────

    #[test]
    fn test_trigger_fires_below_threshold() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t1", "SKU-001", "pharmacy-1", 100, 500));

        // Stock above threshold — should NOT fire.
        let orders = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 200));
        assert!(orders.is_empty());

        // Drain below threshold — should fire.
        let orders = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", -150));
        assert_eq!(orders.len(), 1);
        if let TransactionKind::PurchaseOrder(ref po) = orders[0].kind {
            assert_eq!(po.quantity, 500);
            assert_eq!(po.product_id, "SKU-001");
            assert_eq!(po.buyer_id, "pharmacy-1");
        } else {
            panic!("expected PurchaseOrder");
        }
    }

    #[test]
    fn test_trigger_fires_at_exact_threshold() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t1", "SKU-001", "pharmacy-1", 100, 300));
        // Set inventory exactly at threshold — trigger must fire (≤).
        let orders = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 100));
        assert_eq!(orders.len(), 1);
    }

    #[test]
    fn test_disabled_trigger_does_not_fire() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t1", "SKU-001", "pharmacy-1", 100, 300));
        svc.disable_trigger("t1");
        let orders = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 50));
        assert!(orders.is_empty());
    }

    #[test]
    fn test_re_enable_trigger_fires() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t1", "SKU-001", "pharmacy-1", 100, 300));
        svc.disable_trigger("t1");
        svc.enable_trigger("t1");
        let orders = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 50));
        assert_eq!(orders.len(), 1);
    }

    #[test]
    fn test_trigger_only_fires_for_matching_product() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t1", "SKU-001", "pharmacy-1", 100, 300));
        // Update for a different product — should NOT fire.
        let orders = svc.on_inventory_update(&inv_update("SKU-999", "pharmacy-1", 50));
        assert!(orders.is_empty());
    }

    #[test]
    fn test_multiple_triggers_same_product() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t1", "SKU-001", "pharmacy-1", 100, 200));
        svc.add_trigger(make_trigger("t2", "SKU-001", "pharmacy-1", 50, 100));

        // Inventory at 80 — only t1 fires (threshold 100 ≥ 80).
        let orders = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 80));
        assert_eq!(orders.len(), 1);

        // Inventory drops to 30 — t2 fires too.
        let orders2 = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", -50));
        assert_eq!(orders2.len(), 2);
    }

    // ── Transaction ID uniqueness across repeated firings ─────────────────────

    #[test]
    fn test_repeated_trigger_generates_unique_tx_ids() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t1", "SKU-001", "pharmacy-1", 100, 300));

        // First firing — inventory starts at 0 and is set to 50, which is ≤ 100.
        let orders1 = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 50));
        assert_eq!(orders1.len(), 1);

        // Restock above threshold so the trigger is no longer active at this level.
        svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 200));

        // Second firing — drain back below the threshold.
        let orders2 = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", -200));
        assert_eq!(orders2.len(), 1);

        assert_ne!(
            orders1[0].id, orders2[0].id,
            "repeated trigger firings must produce unique transaction IDs"
        );
    }

    // ── WASM gating ───────────────────────────────────────────────────────────

    /// A WASM module that writes `approve = "1"` unconditionally.
    /// A trigger that carries a WASM module and whose WASM gate approves the
    /// order (sets `approve = "1"`) should produce a `PurchaseOrder`.
    #[test]
    fn test_wasm_approved_trigger_fires() {
        let mut svc = WatcherService::new();
        let executor = Arc::new(WasmExecutionProvider::new().expect("wasmtime engine"));
        svc.set_executor(executor);

        let trigger = InventoryTrigger {
            wasm_code_b64: Some(approving_wasm_b64()),
            ..make_trigger("t-approve", "SKU-WASM", "owner-1", 100, 250)
        };
        svc.add_trigger(trigger);

        // Drop inventory below threshold — WASM approves → PO must be emitted.
        let orders = svc.on_inventory_update(&inv_update("SKU-WASM", "owner-1", 50));
        assert_eq!(
            orders.len(),
            1,
            "approving WASM gate must allow PurchaseOrder to be generated"
        );
        if let TransactionKind::PurchaseOrder(ref po) = orders[0].kind {
            assert_eq!(po.quantity, 250);
            assert_eq!(po.product_id, "SKU-WASM");
        } else {
            panic!("expected PurchaseOrder, got {:?}", orders[0].kind);
        }
    }

    /// A trigger whose WASM gate sets `approve = "0"` must NOT produce any
    /// `PurchaseOrder`, even though the inventory condition is met.
    #[test]
    fn test_wasm_denied_trigger_skipped() {
        let mut svc = WatcherService::new();
        let executor = Arc::new(WasmExecutionProvider::new().expect("wasmtime engine"));
        svc.set_executor(executor);

        let trigger = InventoryTrigger {
            wasm_code_b64: Some(denying_wasm_b64()),
            ..make_trigger("t-deny", "SKU-WASM", "owner-1", 100, 250)
        };
        svc.add_trigger(trigger);

        // Drop inventory below threshold — WASM denies → no PO.
        let orders = svc.on_inventory_update(&inv_update("SKU-WASM", "owner-1", 50));
        assert!(
            orders.is_empty(),
            "denying WASM gate must suppress PurchaseOrder generation"
        );
    }

    /// When `wasm_code_b64` is set on a trigger but no executor has been
    /// registered, the trigger must fire unconditionally (dev/test fast-path).
    #[test]
    fn test_invalid_wasm_gate_skips_trigger() {
        let mut svc = WatcherService::new();
        let executor = Arc::new(WasmExecutionProvider::new().expect("wasmtime engine"));
        svc.set_executor(executor);

        let trigger = InventoryTrigger {
            wasm_code_b64: Some("not-base64".into()),
            ..make_trigger("t-invalid", "SKU-INVALID", "owner-1", 100, 300)
        };
        svc.add_trigger(trigger);

        let orders = svc.on_inventory_update(&inv_update("SKU-INVALID", "owner-1", 50));
        assert!(orders.is_empty());
    }

    #[test]
    fn test_wasm_no_executor_fires_unconditionally() {
        let mut svc = WatcherService::new();
        // Deliberately do NOT call svc.set_executor(…).

        let trigger = InventoryTrigger {
            // Use the denying WASM bytes — but since there's no executor the
            // bytes are never evaluated and the trigger should still fire.
            wasm_code_b64: Some(denying_wasm_b64()),
            ..make_trigger("t-noexec", "SKU-NOEXEC", "owner-1", 100, 300)
        };
        svc.add_trigger(trigger);

        let orders = svc.on_inventory_update(&inv_update("SKU-NOEXEC", "owner-1", 50));
        assert_eq!(
            orders.len(),
            1,
            "trigger with wasm_code_b64 but no executor must fire unconditionally"
        );
    }

    // ── State persistence ─────────────────────────────────────────────────────

    /// Serializing the watcher state and restoring it into a fresh instance
    /// must reproduce the same inventory levels.
    #[test]
    fn test_serialize_deserialize_state() {
        let mut svc = WatcherService::new();
        svc.on_inventory_update(&inv_update("SKU-001", "owner-a", 500));
        svc.on_inventory_update(&inv_update("SKU-001", "owner-a", -120));
        svc.on_inventory_update(&inv_update("SKU-002", "owner-b", 200));

        let bytes = svc.serialize_state().expect("serialize must succeed");
        assert!(!bytes.is_empty(), "serialized state must not be empty");

        let mut restored = WatcherService::new();
        restored
            .restore_from_bytes(&bytes)
            .expect("restore must succeed");

        assert_eq!(
            restored.inventory_level("SKU-001", "owner-a"),
            380,
            "SKU-001/owner-a level must be 500 - 120 = 380 after restore"
        );
        assert_eq!(
            restored.inventory_level("SKU-002", "owner-b"),
            200,
            "SKU-002/owner-b level must be 200 after restore"
        );
        // A product/owner that was never updated must still read as 0.
        assert_eq!(
            restored.inventory_level("SKU-999", "owner-x"),
            0,
            "unknown product/owner must read as 0"
        );
    }

    /// After firing a trigger, the fire count must survive a
    /// serialize → restore round-trip, ensuring that IDs generated after a
    /// node restart never collide with IDs generated before it.
    #[test]
    fn test_snapshot_preserves_fire_counts() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t-count", "SKU-COUNT", "owner-1", 100, 50));

        // Fire the trigger once.
        let orders = svc.on_inventory_update(&inv_update("SKU-COUNT", "owner-1", 50));
        assert_eq!(orders.len(), 1, "trigger must fire on first update");

        // Snapshot, then restore into a fresh service.
        let bytes = svc.serialize_state().expect("serialize must succeed");
        let mut restored = WatcherService::new();
        restored
            .restore_from_bytes(&bytes)
            .expect("restore must succeed");

        // The fire count for "t-count" must be ≥ 1 in the restored instance.
        let snapshot = restored.to_snapshot();
        let count = snapshot
            .trigger_fire_counts
            .get("t-count")
            .copied()
            .unwrap_or(0);
        assert!(
            count >= 1,
            "restored fire count for 't-count' must be >= 1, got {count}"
        );

        // Re-register the trigger in the restored watcher and fire it again.
        // The new transaction ID must differ from the one before the snapshot.
        restored.add_trigger(make_trigger("t-count", "SKU-COUNT", "owner-1", 100, 50));
        // Restock so inventory rises above threshold, then drain again.
        restored.on_inventory_update(&inv_update("SKU-COUNT", "owner-1", 200));
        let orders2 = restored.on_inventory_update(&inv_update("SKU-COUNT", "owner-1", -200));
        assert_eq!(orders2.len(), 1, "trigger must fire again after restore");

        assert_ne!(
            orders[0].id, orders2[0].id,
            "tx IDs generated across a snapshot boundary must be unique"
        );
    }

    #[test]
    fn test_restore_from_invalid_bytes_errors() {
        let mut svc = WatcherService::new();
        // Bytes that are not valid JSON matching WatcherStateSnapshot.
        assert!(svc.restore_from_bytes(b"not json").is_err());
    }
}
