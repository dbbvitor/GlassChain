//! Watcher service for inventory-threshold-based contract triggering (Phase 4).
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

use glasschain_core::{
    InventoryUpdate, PurchaseOrder, Transaction, TransactionKind,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Defines when and how the watcher should auto-generate a purchase order.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Agreed unit price for the auto-generated purchase order.
    pub price_per_unit: f64,
    /// ISO-4217 currency code.
    pub currency: String,
    /// Whether this trigger is currently active.
    pub active: bool,
}

/// The Watcher service.
///
/// Tracks per-owner inventory levels and fires registered triggers when
/// levels fall below their configured thresholds.
#[derive(Debug, Default)]
pub struct WatcherService {
    /// Active triggers keyed by `trigger_id`.
    triggers: HashMap<String, InventoryTrigger>,
    /// Running inventory totals: `inventory[product_id][owner_id] = level`.
    inventory: HashMap<String, HashMap<String, i64>>,
}

impl WatcherService {
    /// Create a new empty watcher service.
    pub fn new() -> Self {
        Self::default()
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
    pub fn inventory_level(&self, product_id: &str, owner_id: &str) -> i64 {
        self.inventory
            .get(product_id)
            .and_then(|m| m.get(owner_id))
            .copied()
            .unwrap_or(0)
    }

    /// Process an [`InventoryUpdate`] transaction and return any auto-generated
    /// [`PurchaseOrder`] transactions.
    ///
    /// This is the core ECA (Event-Condition-Action) evaluation loop:
    /// 1. **Event** — an `InventoryUpdate` arrives.
    /// 2. **Condition** — is the new level at or below any trigger threshold?
    /// 3. **Action** — generate a `PurchaseOrder` and return it for submission.
    pub fn on_inventory_update(
        &mut self,
        update: &InventoryUpdate,
    ) -> Vec<Transaction> {
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

        // Evaluate all active triggers.
        let mut orders = Vec::new();
        for trigger in self.triggers.values() {
            if !trigger.active {
                continue;
            }
            if trigger.product_id != update.product_id {
                continue;
            }
            if trigger.owner_id != update.owner_id {
                continue;
            }
            if new_level <= trigger.reorder_threshold {
                log::info!(
                    "WatcherService: trigger '{}' fired — level {} ≤ threshold {}; ordering {}",
                    trigger.trigger_id,
                    new_level,
                    trigger.reorder_threshold,
                    trigger.reorder_quantity
                );
                let tx = Transaction::with_id(
                    format!("watcher:{}:{}:{}", trigger.trigger_id, update.product_id, update.owner_id),
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
        }
        orders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            price_per_unit: 10.0,
            currency: "USD".into(),
            active: true,
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

    #[test]
    fn test_inventory_accumulates() {
        let mut svc = WatcherService::new();
        svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 200));
        svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", -50));
        assert_eq!(svc.inventory_level("SKU-001", "pharmacy-1"), 150);
    }

    #[test]
    fn test_trigger_fires_below_threshold() {
        let mut svc = WatcherService::new();
        svc.add_trigger(make_trigger("t1", "SKU-001", "pharmacy-1", 100, 500));

        // Stock above threshold – should NOT fire.
        let orders = svc.on_inventory_update(&inv_update("SKU-001", "pharmacy-1", 200));
        assert!(orders.is_empty());

        // Drain below threshold – should fire.
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
        // Set inventory exactly at threshold.
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
        // Update for a different product – should NOT fire.
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
}
