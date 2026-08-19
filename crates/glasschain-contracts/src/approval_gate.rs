use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use glasschain_core::{ExecutionLimits, ExecutionProvider};
use std::collections::HashMap;
use std::fmt;

/// Named execution budgets for the two automation paths that use approval gates.
#[derive(Debug, Clone, Copy)]
pub enum ApprovalGatePolicy {
    ContractEvaluation,
    InventoryTrigger,
}

impl ApprovalGatePolicy {
    const fn limits(self) -> ExecutionLimits {
        match self {
            Self::ContractEvaluation => ExecutionLimits::new(50_000, 50_000),
            Self::InventoryTrigger => ExecutionLimits::new(100_000, 100_000),
        }
    }
}

/// The result of evaluating an active approval gate.
#[derive(Debug)]
pub enum GateDecision {
    Approved,
    Denied { reason: GateDenial },
}

/// Why an active approval gate denied an automated purchase.
#[derive(Debug)]
pub enum GateDenial {
    InvalidPayload(String),
    StatePreparation(String),
    Execution(String),
    MissingApproval,
}

impl fmt::Display for GateDenial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPayload(error) => write!(formatter, "invalid payload: {error}"),
            Self::StatePreparation(error) => {
                write!(formatter, "state preparation failed: {error}")
            }
            Self::Execution(error) => write!(formatter, "execution failed: {error}"),
            Self::MissingApproval => formatter.write_str("approval mutation missing"),
        }
    }
}

/// Concrete approval-gate module shared by contract and inventory automation.
pub struct ApprovalGate<'a> {
    executor: &'a dyn ExecutionProvider,
    policy: ApprovalGatePolicy,
}

impl<'a> ApprovalGate<'a> {
    pub const fn new(executor: &'a dyn ExecutionProvider, policy: ApprovalGatePolicy) -> Self {
        Self { executor, policy }
    }

    pub fn evaluate(
        &self,
        execution_id: &str,
        wasm_code_b64: &str,
        initial_state: Result<HashMap<String, Vec<u8>>, String>,
    ) -> GateDecision {
        let wasm_bytes = match BASE64_STANDARD.decode(wasm_code_b64) {
            Ok(bytes) => bytes,
            Err(error) => {
                return GateDecision::Denied {
                    reason: GateDenial::InvalidPayload(error.to_string()),
                };
            }
        };

        let initial_state = match initial_state {
            Ok(state) => state,
            Err(error) => {
                return GateDecision::Denied {
                    reason: GateDenial::StatePreparation(error),
                };
            }
        };

        let mutations = match self.executor.execute_with_state(
            execution_id,
            &wasm_bytes,
            initial_state,
            self.policy.limits(),
        ) {
            Ok(mutations) => mutations,
            Err(error) => {
                return GateDecision::Denied {
                    reason: GateDenial::Execution(error.to_string()),
                };
            }
        };

        if mutations
            .iter()
            .any(|(key, value)| key == "approve" && value.as_slice() == b"1")
        {
            GateDecision::Approved
        } else {
            GateDecision::Denied {
                reason: GateDenial::MissingApproval,
            }
        }
    }
}
