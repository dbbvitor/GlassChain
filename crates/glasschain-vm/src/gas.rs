//! Deterministic gas metering.
//!
//! `GlassChain` uses Wasmtime's **fuel** feature for gas metering.  Each WASM
//! instruction consumes one unit of fuel.  This module provides helper types
//! for reporting and tracking gas usage.

use serde::{Deserialize, Serialize};

/// Gas usage report returned after contract execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasReport {
    /// Gas units consumed during this execution.
    pub gas_used: u64,
    /// Gas limit that was in effect for this execution.
    pub gas_limit: u64,
    /// Whether the contract terminated normally (`true`) or was interrupted by
    /// gas exhaustion (`false`).
    pub completed_normally: bool,
}

impl GasReport {
    /// Create a new gas report.
    #[must_use]
    pub const fn new(gas_used: u64, gas_limit: u64, completed_normally: bool) -> Self {
        Self {
            gas_used,
            gas_limit,
            completed_normally,
        }
    }

    /// Return the fraction of gas consumed, in the range `[0.0, 1.0]`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn utilisation(&self) -> f64 {
        if self.gas_limit == 0 {
            return 0.0;
        }
        (self.gas_used as f64) / (self.gas_limit as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_report_utilisation() {
        let r = GasReport::new(500, 1000, true);
        assert!((r.utilisation() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_gas_report_zero_limit() {
        let r = GasReport::new(0, 0, true);
        assert!((r.utilisation() - 0.0).abs() < f64::EPSILON);
    }
}
