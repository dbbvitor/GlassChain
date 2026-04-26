use glasschain_core::{PurchaseConditions, SmartContractDef};
use serde::{Deserialize, Serialize};

/// Runtime status of a smart contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ContractStatus {
    /// Contract is active and will be evaluated against incoming supply offers.
    Active,
    /// Contract has been paused by the buyer; no automatic execution occurs.
    Paused,
    /// Contract was fully executed (max_quantity purchased) and is closed.
    Fulfilled,
    /// Contract was explicitly cancelled by the buyer.
    Cancelled,
}

impl std::fmt::Display for ContractStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContractStatus::Active => write!(f, "Active"),
            ContractStatus::Paused => write!(f, "Paused"),
            ContractStatus::Fulfilled => write!(f, "Fulfilled"),
            ContractStatus::Cancelled => write!(f, "Cancelled"),
        }
    }
}

/// A live smart contract managed by the contract engine.
///
/// Wraps a [`SmartContractDef`] with runtime state such as the total
/// quantity already purchased and the current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contract {
    /// The on-ledger definition supplied by the buyer.
    pub definition: SmartContractDef,
    /// Current lifecycle status.
    pub status: ContractStatus,
    /// Cumulative quantity ordered via automatic executions of this contract.
    pub quantity_purchased: u64,
    /// Number of times this contract has been automatically executed.
    pub execution_count: u32,
}

impl Contract {
    /// Create a new active contract from a ledger-committed definition.
    pub fn new(definition: SmartContractDef) -> Self {
        Self {
            definition,
            status: ContractStatus::Active,
            quantity_purchased: 0,
            execution_count: 0,
        }
    }

    /// Return `true` when the contract is accepting automatic executions.
    pub fn is_active(&self) -> bool {
        self.status == ContractStatus::Active
    }

    /// Return a reference to the contract's purchase conditions.
    pub fn conditions(&self) -> &PurchaseConditions {
        &self.definition.conditions
    }

    /// Return the contract's unique identifier.
    pub fn id(&self) -> &str {
        &self.definition.contract_id
    }

    /// Return the buyer's identifier.
    pub fn buyer_id(&self) -> &str {
        &self.definition.buyer_id
    }

    /// Return the targeted product identifier.
    pub fn product_id(&self) -> &str {
        &self.definition.product_id
    }
}
