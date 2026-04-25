use crate::contract::{Contract, ContractStatus};
use crate::error::ContractError;
use glasschain_core::{
    ContractExecution, PurchaseOrder, SmartContractDef, SupplyOffer, Transaction, TransactionKind,
};
use std::collections::HashMap;

/// The smart-contract execution engine.
///
/// Stores all registered contracts in memory and evaluates them whenever a new
/// [`SupplyOffer`] is observed.  When a contract's conditions are satisfied the
/// engine produces the appropriate [`Transaction`]s (a [`PurchaseOrder`] and a
/// [`ContractExecution`] record) for submission to the ledger.
#[derive(Debug, Default)]
pub struct ContractEngine {
    /// Keyed by `contract_id`.
    contracts: HashMap<String, Contract>,
}

impl ContractEngine {
    /// Create an empty engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new contract from a ledger-committed [`SmartContractDef`].
    ///
    /// Returns `Err(ContractError::AlreadyExists)` if the contract ID has
    /// already been registered.
    pub fn register_contract(&mut self, def: SmartContractDef) -> Result<(), ContractError> {
        if self.contracts.contains_key(&def.contract_id) {
            return Err(ContractError::AlreadyExists(def.contract_id));
        }
        let id = def.contract_id.clone();
        self.contracts.insert(id, Contract::new(def));
        Ok(())
    }

    /// Retrieve a contract by ID.
    pub fn get_contract(&self, id: &str) -> Option<&Contract> {
        self.contracts.get(id)
    }

    /// Retrieve a mutable reference to a contract by ID.
    pub fn get_contract_mut(&mut self, id: &str) -> Option<&mut Contract> {
        self.contracts.get_mut(id)
    }

    /// Return an iterator over all contracts.
    pub fn contracts(&self) -> impl Iterator<Item = &Contract> {
        self.contracts.values()
    }

    /// Cancel a contract by ID, preventing further automatic executions.
    pub fn cancel_contract(&mut self, id: &str) -> Result<(), ContractError> {
        let contract = self
            .contracts
            .get_mut(id)
            .ok_or_else(|| ContractError::NotFound(id.to_owned()))?;
        contract.status = ContractStatus::Cancelled;
        Ok(())
    }

    /// Pause a contract by ID.
    pub fn pause_contract(&mut self, id: &str) -> Result<(), ContractError> {
        let contract = self
            .contracts
            .get_mut(id)
            .ok_or_else(|| ContractError::NotFound(id.to_owned()))?;
        if !contract.is_active() {
            return Err(ContractError::Inactive(id.to_owned()));
        }
        contract.status = ContractStatus::Paused;
        Ok(())
    }

    /// Resume a previously paused contract.
    pub fn resume_contract(&mut self, id: &str) -> Result<(), ContractError> {
        let contract = self
            .contracts
            .get_mut(id)
            .ok_or_else(|| ContractError::NotFound(id.to_owned()))?;
        if contract.status == ContractStatus::Paused {
            contract.status = ContractStatus::Active;
        }
        Ok(())
    }

    /// Evaluate a [`SupplyOffer`] against all active contracts and, for each
    /// matching contract with `auto_execute` enabled, generate the transactions
    /// required to record the automatic purchase.
    ///
    /// Returns the list of generated transactions (may be empty).
    pub fn evaluate_supply_offer(
        &mut self,
        offer: &SupplyOffer,
        offer_tx_id: &str,
    ) -> Vec<Transaction> {
        let mut generated = Vec::new();

        for contract in self.contracts.values_mut() {
            if !contract.is_active() {
                continue;
            }
            let conditions = &contract.definition.conditions;

            // Filter by product.
            if contract.definition.product_id != offer.product_id {
                continue;
            }

            // Filter by currency.
            if conditions.currency != offer.currency {
                continue;
            }

            // Check conditions.
            if offer.price_per_unit > conditions.max_price_per_unit {
                log::debug!(
                    "Contract {}: price {:.2} exceeds max {:.2}",
                    contract.id(),
                    offer.price_per_unit,
                    conditions.max_price_per_unit
                );
                continue;
            }
            if offer.lead_time_days > conditions.max_lead_time_days {
                log::debug!(
                    "Contract {}: lead time {} exceeds max {}",
                    contract.id(),
                    offer.lead_time_days,
                    conditions.max_lead_time_days
                );
                continue;
            }
            if offer.quantity_available < conditions.min_quantity {
                log::debug!(
                    "Contract {}: available qty {} below min {}",
                    contract.id(),
                    offer.quantity_available,
                    conditions.min_quantity
                );
                continue;
            }

            // Optional seller preference.
            if let Some(ref preferred) = conditions.preferred_seller_id {
                if *preferred != offer.seller_id {
                    continue;
                }
            }

            if !conditions.auto_execute {
                // Conditions met but auto-execution disabled: just log.
                log::info!(
                    "Contract {} conditions met for offer {} but auto_execute=false",
                    contract.id(),
                    offer_tx_id
                );
                continue;
            }

            // Determine order quantity (capped at contract max_quantity and
            // what the seller has available).
            let remaining_budget =
                conditions.max_quantity.saturating_sub(contract.quantity_purchased);
            if remaining_budget == 0 {
                contract.status = ContractStatus::Fulfilled;
                continue;
            }
            let order_qty = remaining_budget
                .min(offer.quantity_available)
                .min(conditions.max_quantity);

            let total_price = order_qty as f64 * offer.price_per_unit;
            let po_tx_id = uuid::Uuid::new_v4().to_string();

            let po_tx = Transaction::new(TransactionKind::PurchaseOrder(PurchaseOrder {
                product_id: offer.product_id.clone(),
                buyer_id: contract.buyer_id().to_owned(),
                seller_id: offer.seller_id.clone(),
                quantity: order_qty,
                agreed_price_per_unit: offer.price_per_unit,
                currency: offer.currency.clone(),
                contract_id: Some(contract.id().to_owned()),
            }));

            let exec_tx = Transaction::new(TransactionKind::ContractExecution(ContractExecution {
                contract_id: contract.id().to_owned(),
                purchase_order_tx_id: po_tx_id,
                buyer_id: contract.buyer_id().to_owned(),
                seller_id: offer.seller_id.clone(),
                product_id: offer.product_id.clone(),
                quantity: order_qty,
                total_price,
                currency: offer.currency.clone(),
            }));

            log::info!(
                "Contract {} auto-executed: {} × {} @ {:.2} {} from {} (total {:.2})",
                contract.id(),
                order_qty,
                offer.product_id,
                offer.price_per_unit,
                offer.currency,
                offer.seller_id,
                total_price
            );

            contract.quantity_purchased += order_qty;
            contract.execution_count += 1;

            if contract.quantity_purchased >= conditions.max_quantity {
                contract.status = ContractStatus::Fulfilled;
                log::info!("Contract {} fulfilled", contract.id());
            }

            generated.push(po_tx);
            generated.push(exec_tx);
        }

        generated
    }

    /// Load contract definitions that were recovered from ledger history.
    ///
    /// Silently skips already-registered contracts (idempotent).
    pub fn load_from_ledger(&mut self, def: SmartContractDef) {
        if !self.contracts.contains_key(&def.contract_id) {
            self.contracts
                .insert(def.contract_id.clone(), Contract::new(def));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{PurchaseConditions, SmartContractDef, SupplyOffer, TransactionKind};

    fn make_contract(
        id: &str,
        buyer: &str,
        product: &str,
        max_price: f64,
        max_lead: u32,
        min_qty: u64,
        max_qty: u64,
        auto_execute: bool,
    ) -> SmartContractDef {
        SmartContractDef {
            contract_id: id.into(),
            buyer_id: buyer.into(),
            product_id: product.into(),
            conditions: PurchaseConditions {
                max_price_per_unit: max_price,
                min_quantity: min_qty,
                max_quantity: max_qty,
                max_lead_time_days: max_lead,
                preferred_seller_id: None,
                currency: "USD".into(),
                auto_execute,
            },
        }
    }

    fn make_offer(
        seller: &str,
        product: &str,
        qty: u64,
        price: f64,
        lead: u32,
    ) -> SupplyOffer {
        SupplyOffer {
            product_id: product.into(),
            product_name: "Widget".into(),
            seller_id: seller.into(),
            quantity_available: qty,
            price_per_unit: price,
            lead_time_days: lead,
            currency: "USD".into(),
        }
    }

    #[test]
    fn test_register_and_retrieve_contract() {
        let mut engine = ContractEngine::new();
        let def = make_contract("c1", "buyer-1", "SKU-001", 15.0, 10, 10, 100, true);
        engine.register_contract(def.clone()).unwrap();
        let c = engine.get_contract("c1").unwrap();
        assert_eq!(c.id(), "c1");
        assert_eq!(c.buyer_id(), "buyer-1");
    }

    #[test]
    fn test_duplicate_registration_rejected() {
        let mut engine = ContractEngine::new();
        let def = make_contract("c1", "buyer-1", "SKU-001", 15.0, 10, 10, 100, true);
        engine.register_contract(def.clone()).unwrap();
        assert!(engine.register_contract(def).is_err());
    }

    #[test]
    fn test_matching_offer_auto_executes() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract("c1", "buyer-1", "SKU-001", 15.0, 10, 10, 100, true))
            .unwrap();
        let offer = make_offer("seller-1", "SKU-001", 50, 12.0, 7);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert_eq!(txs.len(), 2); // PurchaseOrder + ContractExecution
        // First transaction is the purchase order
        if let TransactionKind::PurchaseOrder(ref po) = txs[0].kind {
            assert_eq!(po.buyer_id, "buyer-1");
            assert_eq!(po.seller_id, "seller-1");
            assert_eq!(po.quantity, 50);
            assert_eq!(po.contract_id, Some("c1".into()));
        } else {
            panic!("expected PurchaseOrder");
        }
    }

    #[test]
    fn test_offer_price_too_high_not_executed() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract("c1", "buyer-1", "SKU-001", 10.0, 10, 10, 100, true))
            .unwrap();
        let offer = make_offer("seller-1", "SKU-001", 50, 15.0, 7); // price > max
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_offer_lead_time_too_long_not_executed() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract("c1", "buyer-1", "SKU-001", 15.0, 5, 10, 100, true))
            .unwrap();
        let offer = make_offer("seller-1", "SKU-001", 50, 10.0, 10); // lead > max
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_contract_fulfilled_after_max_quantity() {
        let mut engine = ContractEngine::new();
        // max_quantity = 30
        engine
            .register_contract(make_contract("c1", "buyer-1", "SKU-001", 15.0, 10, 10, 30, true))
            .unwrap();
        let offer = make_offer("seller-1", "SKU-001", 100, 10.0, 5);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert_eq!(txs.len(), 2);
        let c = engine.get_contract("c1").unwrap();
        assert_eq!(c.status, ContractStatus::Fulfilled);
        // Second evaluation should produce nothing
        let txs2 = engine.evaluate_supply_offer(&offer, "offer-tx-2");
        assert!(txs2.is_empty());
    }

    #[test]
    fn test_cancel_contract_prevents_execution() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract("c1", "buyer-1", "SKU-001", 15.0, 10, 10, 100, true))
            .unwrap();
        engine.cancel_contract("c1").unwrap();
        let offer = make_offer("seller-1", "SKU-001", 50, 10.0, 5);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_auto_execute_false_generates_no_transactions() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract(
                "c1", "buyer-1", "SKU-001", 15.0, 10, 10, 100, false,
            ))
            .unwrap();
        let offer = make_offer("seller-1", "SKU-001", 50, 10.0, 5);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_preferred_seller_filter() {
        let mut engine = ContractEngine::new();
        let mut def = make_contract("c1", "buyer-1", "SKU-001", 15.0, 10, 10, 100, true);
        def.conditions.preferred_seller_id = Some("preferred-seller".into());
        engine.register_contract(def).unwrap();

        // Wrong seller – should not match
        let offer_wrong = make_offer("other-seller", "SKU-001", 50, 10.0, 5);
        assert!(engine
            .evaluate_supply_offer(&offer_wrong, "tx-1")
            .is_empty());

        // Correct seller – should match
        let offer_right = make_offer("preferred-seller", "SKU-001", 50, 10.0, 5);
        assert_eq!(engine.evaluate_supply_offer(&offer_right, "tx-2").len(), 2);
    }
}
