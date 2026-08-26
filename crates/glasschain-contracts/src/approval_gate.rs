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

        let result = match self.executor.execute_with_state(
            execution_id,
            &wasm_bytes,
            initial_state,
            self.policy.limits(),
        ) {
            Ok(result) => result,
            Err(error) => {
                return GateDecision::Denied {
                    reason: GateDenial::Execution(error.to_string()),
                };
            }
        };

        // Approval gates consume **ephemeral** output only (ADR-007 decision 1):
        // a guest contract cannot approve by requesting a persistent write, and
        // an approval evaluation never persists anything.
        if result
            .ephemeral
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

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{CoreError, ExecutionLimits, ExecutionProvider, ExecutionResult};

    /// Canned executor: returns a fixed ephemeral result for every call.
    struct StubExecutor {
        mutations: Result<Vec<(String, Vec<u8>)>, String>,
    }

    impl ExecutionProvider for StubExecutor {
        fn execute(
            &self,
            _contract_id: &str,
            _payload: &[u8],
            _limits: ExecutionLimits,
        ) -> Result<ExecutionResult, CoreError> {
            self.mutations
                .clone()
                .map(ExecutionResult::from)
                .map_err(CoreError::Execution)
        }

        fn name(&self) -> &'static str {
            "stub"
        }
    }

    fn gate(executor: &StubExecutor) -> ApprovalGate<'_> {
        ApprovalGate::new(executor, ApprovalGatePolicy::ContractEvaluation)
    }

    fn evaluate(
        executor: &StubExecutor,
        wasm_code_b64: &str,
        initial_state: Result<HashMap<String, Vec<u8>>, String>,
    ) -> GateDecision {
        gate(executor).evaluate("execution-1", wasm_code_b64, initial_state)
    }

    #[test]
    fn approves_when_approve_mutation_is_one() {
        let executor = StubExecutor {
            mutations: Ok(vec![("approve".into(), b"1".to_vec())]),
        };
        let decision = evaluate(
            &executor,
            &BASE64_STANDARD.encode(b"wasm"),
            Ok(HashMap::new()),
        );
        assert!(matches!(decision, GateDecision::Approved));
    }

    #[test]
    fn denies_when_approve_mutation_missing() {
        let executor = StubExecutor {
            mutations: Ok(vec![("other".into(), b"1".to_vec())]),
        };
        let decision = evaluate(
            &executor,
            &BASE64_STANDARD.encode(b"wasm"),
            Ok(HashMap::new()),
        );
        assert!(matches!(
            decision,
            GateDecision::Denied {
                reason: GateDenial::MissingApproval
            }
        ));
    }

    #[test]
    fn denies_invalid_base64_payload() {
        let executor = StubExecutor {
            mutations: Ok(Vec::new()),
        };
        let decision = evaluate(&executor, "not-base64", Ok(HashMap::new()));
        assert!(matches!(
            decision,
            GateDecision::Denied {
                reason: GateDenial::InvalidPayload(_)
            }
        ));
    }

    #[test]
    fn denies_state_preparation_failure() {
        let executor = StubExecutor {
            mutations: Ok(Vec::new()),
        };
        let decision = evaluate(
            &executor,
            &BASE64_STANDARD.encode(b"wasm"),
            Err("offer serialization failed".into()),
        );
        assert!(matches!(
            decision,
            GateDecision::Denied {
                reason: GateDenial::StatePreparation(_)
            }
        ));
    }

    #[test]
    fn denies_execution_failure() {
        let executor = StubExecutor {
            mutations: Err("gas exhausted".into()),
        };
        let decision = evaluate(
            &executor,
            &BASE64_STANDARD.encode(b"wasm"),
            Ok(HashMap::new()),
        );
        assert!(matches!(
            decision,
            GateDecision::Denied {
                reason: GateDenial::Execution(_)
            }
        ));
    }

    #[test]
    fn gate_denial_display_formats_all_variants() {
        assert_eq!(
            GateDenial::InvalidPayload("bad".into()).to_string(),
            "invalid payload: bad"
        );
        assert_eq!(
            GateDenial::StatePreparation("sp".into()).to_string(),
            "state preparation failed: sp"
        );
        assert_eq!(
            GateDenial::Execution("ex".into()).to_string(),
            "execution failed: ex"
        );
        assert_eq!(
            GateDenial::MissingApproval.to_string(),
            "approval mutation missing"
        );
    }
}
