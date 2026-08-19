use crate::approval_gate::{ApprovalGate, ApprovalGatePolicy, GateDecision};
use crate::contract::{Contract, ContractStatus};
use crate::error::ContractError;
use glasschain_core::{
    Block, ContractExecution, PurchaseOrder, SmartContractDef, SupplyOffer, Transaction,
    TransactionKind,
};
use std::collections::HashMap;

/// The smart-contract execution engine.
///
/// Stores all registered contracts in memory and evaluates them whenever a new
/// [`SupplyOffer`] is observed.  When a contract's conditions are satisfied the
/// engine produces the appropriate [`Transaction`]s (a [`PurchaseOrder`] and a
/// [`ContractExecution`] record) for submission to the ledger.
#[derive(Default)]
pub struct ContractEngine {
    /// Keyed by `contract_id`.
    contracts: HashMap<String, Contract>,
    /// Optional WASM execution provider for contract-level custom logic.
    executor: Option<std::sync::Arc<dyn glasschain_core::ExecutionProvider>>,
}

impl std::fmt::Debug for ContractEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContractEngine")
            .field("contracts", &self.contracts)
            .field("executor", &self.executor.as_ref().map(|_| "<executor>"))
            .finish()
    }
}

struct PurchasePlan {
    purchase_order: Transaction,
    execution: Transaction,
    quantity: u64,
    total_price: u64,
}

impl ContractEngine {
    /// Create an empty engine.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a WASM execution provider.
    ///
    /// When set, contracts that carry a `wasm_code_b64` payload are evaluated
    /// through this provider before the standard Rust condition matching runs.
    pub fn set_executor(
        &mut self,
        executor: std::sync::Arc<dyn glasschain_core::ExecutionProvider>,
    ) {
        self.executor = Some(executor);
    }

    /// Register a new contract from a ledger-committed [`SmartContractDef`].
    ///
    /// # Errors
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
    #[must_use]
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

    fn offer_matches(contract: &Contract, offer: &SupplyOffer) -> bool {
        let conditions = contract.conditions();
        if contract.product_id() != offer.product_id || conditions.currency != offer.currency {
            return false;
        }
        if offer.price_per_unit > conditions.max_price_per_unit {
            log::debug!(
                "Contract {}: price {} exceeds max {}",
                contract.id(),
                offer.price_per_unit,
                conditions.max_price_per_unit
            );
            return false;
        }
        if offer.lead_time_days > conditions.max_lead_time_days {
            log::debug!(
                "Contract {}: lead time {} exceeds max {}",
                contract.id(),
                offer.lead_time_days,
                conditions.max_lead_time_days
            );
            return false;
        }
        if offer.quantity_available < conditions.min_quantity {
            log::debug!(
                "Contract {}: available qty {} below min {}",
                contract.id(),
                offer.quantity_available,
                conditions.min_quantity
            );
            return false;
        }
        conditions
            .preferred_seller_id
            .as_ref()
            .is_none_or(|preferred| preferred == &offer.seller_id)
    }

    fn prepare_purchase(
        contract: &Contract,
        offer: &SupplyOffer,
        offer_tx_id: &str,
    ) -> Option<PurchasePlan> {
        let conditions = contract.conditions();
        let remaining_budget = conditions
            .max_quantity
            .saturating_sub(contract.quantity_purchased);
        if remaining_budget == 0 {
            return None;
        }

        let quantity = remaining_budget
            .min(offer.quantity_available)
            .min(conditions.max_quantity);
        let total_price = quantity.saturating_mul(offer.price_per_unit);
        let purchase_order = Transaction::with_id(
            format!("po:{}-{}", contract.id(), offer_tx_id),
            TransactionKind::PurchaseOrder(PurchaseOrder {
                product_id: offer.product_id.clone(),
                buyer_id: contract.buyer_id().to_owned(),
                seller_id: offer.seller_id.clone(),
                quantity,
                agreed_price_per_unit: offer.price_per_unit,
                currency: offer.currency.clone(),
                contract_id: Some(contract.id().to_owned()),
            }),
        );
        let execution = Transaction::with_id(
            format!("exec:{}-{}", contract.id(), offer_tx_id),
            TransactionKind::ContractExecution(ContractExecution {
                contract_id: contract.id().to_owned(),
                purchase_order_tx_id: purchase_order.id.clone(),
                buyer_id: contract.buyer_id().to_owned(),
                seller_id: offer.seller_id.clone(),
                product_id: offer.product_id.clone(),
                quantity,
                total_price,
                currency: offer.currency.clone(),
            }),
        );

        Some(PurchasePlan {
            purchase_order,
            execution,
            quantity,
            total_price,
        })
    }

    fn apply_purchase(
        contract: &mut Contract,
        offer: &SupplyOffer,
        plan: PurchasePlan,
        offer_tx_id: &str,
        generated: &mut Vec<Transaction>,
    ) {
        log::info!(
            "Contract {} auto-executed: {} × {} @ {} {} from {} (total {})",
            contract.id(),
            plan.quantity,
            offer.product_id,
            offer.price_per_unit,
            offer.currency,
            offer.seller_id,
            plan.total_price
        );

        contract.quantity_purchased += plan.quantity;
        contract.execution_count += 1;
        if contract.quantity_purchased >= contract.conditions().max_quantity {
            contract.status = ContractStatus::Fulfilled;
            log::info!("Contract {} fulfilled", contract.id());
        }

        log::debug!(
            "Contract {} emitted purchase for offer {}",
            contract.id(),
            offer_tx_id
        );
        generated.push(plan.purchase_order);
        generated.push(plan.execution);
    }

    /// Cancel a contract by ID, preventing further automatic executions.
    ///
    /// # Errors
    ///
    /// Returns `Err(ContractError::NotFound)` if no contract with the given ID exists.
    pub fn cancel_contract(&mut self, id: &str) -> Result<(), ContractError> {
        let contract = self
            .contracts
            .get_mut(id)
            .ok_or_else(|| ContractError::NotFound(id.to_owned()))?;
        contract.status = ContractStatus::Cancelled;
        Ok(())
    }

    /// Pause a contract by ID.
    ///
    /// # Errors
    ///
    /// Returns `Err(ContractError::NotFound)` if no contract with the given ID exists, or
    /// `Err(ContractError::Inactive)` if the contract is not currently active.
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
    ///
    /// # Errors
    ///
    /// Returns `Err(ContractError::NotFound)` if no contract with the given ID exists.
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
        let executor_opt = self.executor.clone();

        for contract in self.contracts.values_mut() {
            if !contract.is_active() {
                continue;
            }

            // An active WASM gate must approve before standard condition matching.
            if let (Some(ref wasm_b64), Some(ref executor)) =
                (&contract.definition.wasm_code_b64, &executor_opt)
            {
                let initial_state = serde_json::to_vec(offer)
                    .map(|offer_json| {
                        let mut state = HashMap::new();
                        state.insert("offer".to_string(), offer_json);
                        state
                    })
                    .map_err(|error| error.to_string());
                let execution_id = format!("wasm:{}:{}", contract.id(), offer_tx_id);
                let gate =
                    ApprovalGate::new(executor.as_ref(), ApprovalGatePolicy::ContractEvaluation);
                match gate.evaluate(&execution_id, wasm_b64, initial_state) {
                    GateDecision::Approved => {}
                    GateDecision::Denied { reason } => {
                        log::warn!(
                            "Contract {}: WASM gate denied offer {}: {reason}",
                            contract.id(),
                            offer_tx_id
                        );
                        continue;
                    }
                }
            }

            if !Self::offer_matches(contract, offer) {
                continue;
            }

            if !contract.conditions().auto_execute {
                log::info!(
                    "Contract {} conditions met for offer {} but auto_execute=false",
                    contract.id(),
                    offer_tx_id
                );
                continue;
            }

            let Some(plan) = Self::prepare_purchase(contract, offer, offer_tx_id) else {
                contract.status = ContractStatus::Fulfilled;
                continue;
            };
            Self::apply_purchase(contract, offer, plan, offer_tx_id, &mut generated);
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

    /// Rebuild a contract engine's full runtime state by replaying the given
    /// chain of blocks.
    ///
    /// Two-pass algorithm:
    /// 1. Register every [`ContractCreation`] transaction as a new contract.
    /// 2. Replay every [`ContractExecution`] transaction to update
    ///    `quantity_purchased`, `execution_count`, and `status`.
    ///
    /// This is called after a chain-sync event so that recovered nodes have
    /// correct contract state without re-running the original offer evaluations.
    #[must_use]
    pub fn rebuild_from_chain(chain: &[Block]) -> Self {
        let mut engine = Self::new();
        // Pass 1 — register all contracts.
        for block in chain {
            for tx in &block.transactions {
                if let TransactionKind::ContractCreation(ref def) = tx.kind {
                    engine.load_from_ledger(def.clone());
                }
            }
        }
        // Pass 2 — replay executions to restore runtime state.
        for block in chain {
            for tx in &block.transactions {
                if let TransactionKind::ContractExecution(ref exec) = tx.kind {
                    if let Some(contract) = engine.contracts.get_mut(&exec.contract_id) {
                        contract.quantity_purchased += exec.quantity;
                        contract.execution_count += 1;
                        if contract.quantity_purchased
                            >= contract.definition.conditions.max_quantity
                        {
                            contract.status = ContractStatus::Fulfilled;
                        }
                    }
                }
            }
        }
        engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{
        Block, PurchaseConditions, SmartContractDef, SupplyOffer, TransactionKind,
    };

    /// Build a contract definition with prices expressed in minor currency units
    /// (e.g. cents: 1500 = $15.00).
    // Keep this test builder aligned with the contract fields it exercises.
    #[allow(clippy::too_many_arguments)]
    fn make_contract(
        id: &str,
        buyer: &str,
        product: &str,
        max_price: u64,
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
            wasm_code_b64: None,
        }
    }

    /// Build a supply offer with `price` expressed in minor currency units.
    fn make_offer(seller: &str, product: &str, qty: u64, price: u64, lead: u32) -> SupplyOffer {
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

    /// Mine a minimal block for use in chain-replay tests.
    fn fake_block(index: u64, prev_hash: &str, txns: Vec<Transaction>) -> Block {
        let mut b = Block::new(index, txns, prev_hash.to_owned());
        b.mine(1);
        b
    }

    // -------------------------------------------------------------------------
    // Basic registration / retrieval
    // -------------------------------------------------------------------------

    #[test]
    fn test_register_and_retrieve_contract() {
        let mut engine = ContractEngine::new();
        let def = make_contract("c1", "buyer-1", "SKU-001", 1500, 10, 10, 100, true);
        engine.register_contract(def).unwrap();
        let c = engine.get_contract("c1").unwrap();
        assert_eq!(c.id(), "c1");
        assert_eq!(c.buyer_id(), "buyer-1");
    }

    #[test]
    fn test_duplicate_registration_rejected() {
        let mut engine = ContractEngine::new();
        let def = make_contract("c1", "buyer-1", "SKU-001", 1500, 10, 10, 100, true);
        engine.register_contract(def.clone()).unwrap();
        assert!(engine.register_contract(def).is_err());
    }

    // -------------------------------------------------------------------------
    // Offer evaluation — happy paths
    // -------------------------------------------------------------------------

    #[test]
    fn test_matching_offer_auto_executes() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract(
                "c1", "buyer-1", "SKU-001", 1500, 10, 10, 100, true,
            ))
            .unwrap();
        // Offer at 1200 cents ($12.00) — within the 1500-cent ($15.00) cap.
        let offer = make_offer("seller-1", "SKU-001", 50, 1200, 7);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert_eq!(txs.len(), 2); // PurchaseOrder + ContractExecution
        if let TransactionKind::PurchaseOrder(ref po) = txs[0].kind {
            assert_eq!(po.buyer_id, "buyer-1");
            assert_eq!(po.seller_id, "seller-1");
            assert_eq!(po.quantity, 50);
            assert_eq!(po.agreed_price_per_unit, 1200u64);
            assert_eq!(po.contract_id, Some("c1".into()));
        } else {
            panic!("expected PurchaseOrder");
        }
    }

    #[test]
    fn test_matching_offer_preserves_quantity_cap_and_transaction_order() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract(
                "c1", "buyer-1", "SKU-001", 1500, 10, 10, 30, true,
            ))
            .unwrap();

        let txs = engine.evaluate_supply_offer(
            &make_offer("seller-1", "SKU-001", 100, 1200, 7),
            "offer-tx-1",
        );
        assert_eq!(txs.len(), 2);

        let TransactionKind::PurchaseOrder(order) = &txs[0].kind else {
            panic!("expected PurchaseOrder first");
        };
        assert_eq!(order.quantity, 30);

        let TransactionKind::ContractExecution(execution) = &txs[1].kind else {
            panic!("expected ContractExecution second");
        };
        assert_eq!(execution.purchase_order_tx_id, txs[0].id);
        assert_eq!(execution.quantity, 30);
        assert_eq!(
            engine.get_contract("c1").unwrap().status,
            ContractStatus::Fulfilled
        );
    }

    // -------------------------------------------------------------------------
    // Offer evaluation — rejection paths
    // -------------------------------------------------------------------------

    #[test]
    fn test_offer_price_too_high_not_executed() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract(
                "c1", "buyer-1", "SKU-001", 1000, 10, 10, 100, true,
            ))
            .unwrap();
        // Offer at 1500 cents ($15.00) — exceeds the 1000-cent ($10.00) cap.
        let offer = make_offer("seller-1", "SKU-001", 50, 1500, 7);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_offer_lead_time_too_long_not_executed() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract(
                "c1", "buyer-1", "SKU-001", 1500, 5, 10, 100, true,
            ))
            .unwrap();
        // Lead time 10 days — exceeds the 5-day max.
        let offer = make_offer("seller-1", "SKU-001", 50, 1000, 10);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert!(txs.is_empty());
    }

    // -------------------------------------------------------------------------
    // Fulfilment & lifecycle
    // -------------------------------------------------------------------------

    #[test]
    fn test_contract_fulfilled_after_max_quantity() {
        let mut engine = ContractEngine::new();
        // max_quantity = 30
        engine
            .register_contract(make_contract(
                "c1", "buyer-1", "SKU-001", 1500, 10, 10, 30, true,
            ))
            .unwrap();
        let offer = make_offer("seller-1", "SKU-001", 100, 1000, 5);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert_eq!(txs.len(), 2);
        let c = engine.get_contract("c1").unwrap();
        assert_eq!(c.status, ContractStatus::Fulfilled);
        // Second evaluation should produce nothing (contract is fulfilled).
        let txs2 = engine.evaluate_supply_offer(&offer, "offer-tx-2");
        assert!(txs2.is_empty());
    }

    #[test]
    fn test_cancel_contract_prevents_execution() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract(
                "c1", "buyer-1", "SKU-001", 1500, 10, 10, 100, true,
            ))
            .unwrap();
        engine.cancel_contract("c1").unwrap();
        let offer = make_offer("seller-1", "SKU-001", 50, 1000, 5);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_auto_execute_false_generates_no_transactions() {
        let mut engine = ContractEngine::new();
        engine
            .register_contract(make_contract(
                "c1", "buyer-1", "SKU-001", 1500, 10, 10, 100, false,
            ))
            .unwrap();
        let offer = make_offer("seller-1", "SKU-001", 50, 1000, 5);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_preferred_seller_filter() {
        let mut engine = ContractEngine::new();
        let mut def = make_contract("c1", "buyer-1", "SKU-001", 1500, 10, 10, 100, true);
        def.conditions.preferred_seller_id = Some("preferred-seller".into());
        engine.register_contract(def).unwrap();

        // Wrong seller — should not match.
        let offer_wrong = make_offer("other-seller", "SKU-001", 50, 1000, 5);
        assert!(engine
            .evaluate_supply_offer(&offer_wrong, "tx-1")
            .is_empty());

        // Correct seller — should match.
        let offer_right = make_offer("preferred-seller", "SKU-001", 50, 1000, 5);
        assert_eq!(engine.evaluate_supply_offer(&offer_right, "tx-2").len(), 2);
    }

    // -------------------------------------------------------------------------
    // Chain replay
    // -------------------------------------------------------------------------

    #[test]
    fn test_rebuild_from_chain_restores_state() {
        // Build a contract definition (max 100 units, cap $15.00 = 1500 cents).
        let def = make_contract("c1", "buyer-1", "SKU-001", 1500, 10, 10, 100, true);

        // Genesis block: contains the ContractCreation transaction.
        let creation_tx = Transaction::with_id(
            "creation:c1".to_owned(),
            TransactionKind::ContractCreation(def.clone()),
        );
        let genesis_block = fake_block(0, "0", vec![creation_tx]);

        // Use a live engine to generate a matching ContractExecution transaction
        // for 50 units at 1200 cents ($12.00) — within the cap.
        let mut engine = ContractEngine::new();
        engine.register_contract(def).unwrap();
        let offer = make_offer("seller-1", "SKU-001", 50, 1200, 7);
        let txs = engine.evaluate_supply_offer(&offer, "offer-tx-1");
        assert_eq!(txs.len(), 2);

        // Pull out the ContractExecution tx and put it in block 1.
        let exec_tx = txs
            .into_iter()
            .find(|tx| matches!(tx.kind, TransactionKind::ContractExecution(_)))
            .unwrap();
        let block1 = fake_block(1, "0", vec![exec_tx]);

        // Rebuild engine from the two-block chain.
        let rebuilt = ContractEngine::rebuild_from_chain(&[genesis_block, block1]);

        let contract = rebuilt.get_contract("c1").unwrap();
        assert!(
            contract.quantity_purchased > 0,
            "quantity_purchased must be restored from the ContractExecution replay"
        );
        // 50 units purchased < max_quantity 100 → still Active, not Fulfilled.
        assert_eq!(
            contract.status,
            ContractStatus::Active,
            "contract should remain Active after a partial execution replay"
        );
    }
}

#[cfg(test)]
mod wasm_tests {
    use super::*;

    use base64::prelude::*;
    use glasschain_core::{PurchaseConditions, SmartContractDef, SupplyOffer};
    use glasschain_vm::WasmExecutionProvider;
    use std::sync::Arc;

    fn approving_wasm_b64() -> String {
        let wat = r#"
(module
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "approve")
  (data (i32.const 7) "1")
  (func (export "execute")
    (call $set_state (i32.const 0) (i32.const 7) (i32.const 7) (i32.const 1))
  )
)
"#;
        let wasm = wat::parse_str(wat).expect("WAT compile");
        BASE64_STANDARD.encode(&wasm)
    }

    fn denying_wasm_b64() -> String {
        let wat = r#"
(module
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "approve")
  (data (i32.const 7) "0")
  (func (export "execute")
    (call $set_state (i32.const 0) (i32.const 7) (i32.const 7) (i32.const 1))
  )
)
"#;
        let wasm = wat::parse_str(wat).expect("WAT compile");
        BASE64_STANDARD.encode(&wasm)
    }

    fn exhausting_wasm_b64() -> String {
        let wat = r#"
(module
  (func (export "execute")
    (loop $forever
      (br $forever)
    )
  )
)
"#;
        let wasm = wat::parse_str(wat).expect("WAT compile");
        BASE64_STANDARD.encode(&wasm)
    }

    fn offer() -> SupplyOffer {
        SupplyOffer {
            product_id: "SKU-001".into(),
            product_name: "Widget".into(),
            seller_id: "seller-1".into(),
            quantity_available: 50,
            price_per_unit: 1500,
            lead_time_days: 7,
            currency: "USD".into(),
        }
    }

    fn contract_def(wasm_b64: Option<String>) -> SmartContractDef {
        SmartContractDef {
            contract_id: "wasm-test".into(),
            buyer_id: "buyer-1".into(),
            product_id: "SKU-001".into(),
            conditions: PurchaseConditions {
                max_price_per_unit: 2000,
                min_quantity: 1,
                max_quantity: 100,
                max_lead_time_days: 14,
                preferred_seller_id: None,
                currency: "USD".into(),
                auto_execute: true,
            },
            wasm_code_b64: wasm_b64,
        }
    }

    #[test]
    fn test_wasm_gate_approved() {
        let executor = Arc::new(WasmExecutionProvider::new().expect("wasmtime"));
        let mut engine = ContractEngine::new();
        engine.set_executor(executor);
        engine
            .register_contract(contract_def(Some(approving_wasm_b64())))
            .unwrap();
        let txs = engine.evaluate_supply_offer(&offer(), "offer-1");
        assert_eq!(txs.len(), 2);
    }

    #[test]
    fn test_wasm_gate_denied() {
        let executor = Arc::new(WasmExecutionProvider::new().expect("wasmtime"));
        let mut engine = ContractEngine::new();
        engine.set_executor(executor);
        engine
            .register_contract(contract_def(Some(denying_wasm_b64())))
            .unwrap();
        let txs = engine.evaluate_supply_offer(&offer(), "offer-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_exhausted_wasm_gate_denies_offer() {
        let executor = Arc::new(WasmExecutionProvider::new().expect("wasmtime"));
        let mut engine = ContractEngine::new();
        engine.set_executor(executor);
        engine
            .register_contract(contract_def(Some(exhausting_wasm_b64())))
            .unwrap();
        let txs = engine.evaluate_supply_offer(&offer(), "offer-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_invalid_wasm_gate_denies_offer() {
        let executor = Arc::new(WasmExecutionProvider::new().expect("wasmtime"));
        let mut engine = ContractEngine::new();
        engine.set_executor(executor);
        engine
            .register_contract(contract_def(Some("not-base64".into())))
            .unwrap();
        let txs = engine.evaluate_supply_offer(&offer(), "offer-1");
        assert!(txs.is_empty());
    }

    #[test]
    fn test_no_executor_no_wasm_uses_rust_matching() {
        // Without an executor, even a contract with wasm_code_b64 falls through
        // to standard Rust condition matching.
        let mut engine = ContractEngine::new();
        engine
            .register_contract(contract_def(Some(approving_wasm_b64())))
            .unwrap();
        let txs = engine.evaluate_supply_offer(&offer(), "offer-1");
        // No executor → WASM gate skipped → Rust matching → auto_execute=true → 2 txs
        assert_eq!(txs.len(), 2);
    }
}
