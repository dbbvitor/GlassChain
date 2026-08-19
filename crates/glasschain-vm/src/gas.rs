//! Deterministic gas accounting for contract execution.
//!
//! `GlassChain` uses Wasmtime's **fuel** feature for WASM instruction metering
//! and [`GasCounter`] for an independent host-operation budget. [`GasCosts`]
//! defines the operation charges used for invocation, state reads, and state
//! writes. The call-depth guard remains available for future recursive contract
//! calls but is not wired into the current execution path.
//!
//! ## Typical Usage
//!
//! ```text
//! let mut counter = GasCounter::new(100_000);
//! counter.charge(counter.costs().base_execution)?;   // charge base invocation
//! counter.push_call()?;                            // enter sub-call
//! counter.charge_state_read(128)?;                 // read 128-byte state
//! counter.pop_call();                              // exit sub-call
//! let report = counter.to_report(true);            // execution succeeded
//! ```

use serde::{Deserialize, Serialize};

// ── GasReport ────────────────────────────────────────────────────────────────

/// Standalone gas usage report for callers that use [`GasCounter`] directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasReport {
    /// Gas units consumed during this execution.
    pub gas_used: u64,
    /// Gas limit that was in effect for this execution.
    pub gas_limit: u64,
    /// Whether the contract terminated normally (`true`) or was interrupted
    /// by gas exhaustion or another error (`false`).
    pub completed_normally: bool,
}

impl GasReport {
    /// Create a new [`GasReport`] from raw values.
    #[must_use]
    pub const fn new(gas_used: u64, gas_limit: u64, completed_normally: bool) -> Self {
        Self {
            gas_used,
            gas_limit,
            completed_normally,
        }
    }

    /// Return the fraction of gas consumed, in the range `[0.0, 1.0]`.
    ///
    /// Returns `0.0` when `gas_limit` is zero to avoid a division-by-zero.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn utilisation(&self) -> f64 {
        if self.gas_limit == 0 {
            return 0.0;
        }
        (self.gas_used as f64) / (self.gas_limit as f64)
    }
}

// ── GasCosts ─────────────────────────────────────────────────────────────────

/// Per-operation gas cost table.
///
/// Construct with [`GasCosts::default_costs`] for standard mainnet parameters,
/// or build a custom table for testing and alternative network configurations.
///
/// The cost model charges a **flat fee** per operation type plus an additional
/// **per-byte** component for state accesses, incentivising contracts to
/// minimise storage footprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasCosts {
    /// Base cost charged for any contract invocation (before any state access).
    pub base_execution: u64,
    /// Flat cost per `get_state` call, before the per-byte component.
    pub state_read: u64,
    /// Flat cost per `set_state` call, before the per-byte component.
    pub state_write: u64,
    /// Additional gas charged per byte **read** from state storage.
    pub per_byte_read: u64,
    /// Additional gas charged per byte **written** to state storage.
    pub per_byte_write: u64,
    /// Maximum allowed call-stack depth for future recursive contract calls.
    ///
    /// `push_call` returns an error when `call_depth` would exceed this value.
    /// The current Wasmtime provider does not invoke it because contracts cannot
    /// recursively call other contracts.
    pub max_call_depth: u32,
}

impl GasCosts {
    /// Return the default mainnet cost table.
    ///
    /// | Parameter        | Value |
    /// |:-----------------|------:|
    /// | `base_execution` | 1 000 |
    /// | `state_read`     |    50 |
    /// | `state_write`    |   200 |
    /// | `per_byte_read`  |     1 |
    /// | `per_byte_write` |     2 |
    /// | `max_call_depth` |     8 |
    #[must_use]
    pub const fn default_costs() -> Self {
        Self {
            base_execution: 1_000,
            state_read: 50,
            state_write: 200,
            per_byte_read: 1,
            per_byte_write: 2,
            max_call_depth: 8,
        }
    }

    /// Total gas cost for a state-read operation on `byte_count` bytes.
    ///
    /// Formula: `state_read + per_byte_read × byte_count`.
    #[must_use]
    pub const fn total_state_read_cost(&self, byte_count: u64) -> u64 {
        self.state_read + self.per_byte_read * byte_count
    }

    /// Total gas cost for a state-write operation on `byte_count` bytes.
    ///
    /// Formula: `state_write + per_byte_write × byte_count`.
    #[must_use]
    pub const fn total_state_write_cost(&self, byte_count: u64) -> u64 {
        self.state_write + self.per_byte_write * byte_count
    }
}

// ── GasCounter ───────────────────────────────────────────────────────────────

/// Stateful gas counter for a single contract execution context.
///
/// `GasCounter` tracks:
/// - Total gas consumed (`used`) against a hard `limit`.
/// - The number of discrete state-read and state-write operations.
/// - The current call-stack depth, when a caller explicitly uses the optional
///   [`push_call`][Self::push_call] guard for recursive execution.
///
/// # Example
///
/// ```text
/// let mut g = GasCounter::new(50_000);
/// g.charge(g.costs().base_execution).expect("enough gas for base");
/// g.charge_state_write(64).expect("enough gas for write");
/// let report = g.to_report(true);
/// assert_eq!(report.gas_used, 1_000 + 200 + 2 * 64);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasCounter {
    /// Maximum gas units available for this execution.
    pub limit: u64,
    /// Gas units consumed so far.
    pub used: u64,
    /// Current call-stack depth (incremented by [`push_call`][Self::push_call],
    /// decremented by [`pop_call`][Self::pop_call]).
    pub call_depth: u32,
    /// Cost table governing this execution context.
    ///
    /// The field is private to prevent callers from mutating gas prices
    /// mid-execution.  Use the [`GasCounter::costs`] accessor for read-only
    /// access, and [`GasCounter::new_with_costs`] to supply a custom schedule
    /// at construction time.
    costs: GasCosts,
    /// Total number of state-read operations charged so far.
    pub state_reads: u64,
    /// Total number of state-write operations charged so far.
    pub state_writes: u64,
}

impl GasCounter {
    /// Create a new counter using the default mainnet [`GasCosts`].
    #[must_use]
    pub const fn new(limit: u64) -> Self {
        Self {
            limit,
            used: 0,
            call_depth: 0,
            costs: GasCosts::default_costs(),
            state_reads: 0,
            state_writes: 0,
        }
    }

    /// Create a new counter with a custom [`GasCosts`] table.
    ///
    /// Useful for testing, alternative networks, or fee-schedule upgrades
    /// without touching the mainnet defaults.
    #[must_use]
    pub const fn new_with_costs(limit: u64, costs: GasCosts) -> Self {
        Self {
            limit,
            used: 0,
            call_depth: 0,
            costs,
            state_reads: 0,
            state_writes: 0,
        }
    }

    /// Read-only access to the cost table governing this execution context.
    ///
    /// The cost table is sealed after construction to prevent callers from
    /// mutating gas prices mid-execution.  Supply a custom schedule at build
    /// time via [`GasCounter::new_with_costs`].
    #[must_use]
    pub const fn costs(&self) -> &GasCosts {
        &self.costs
    }

    /// Deduct `amount` gas units from the budget.
    ///
    /// # Errors
    ///
    /// Returns `Err("gas exhausted")` if `used` would exceed `limit` after
    /// the charge.  The counter is still updated even on error, so callers
    /// **must** halt execution upon receiving this error.
    pub fn charge(&mut self, amount: u64) -> Result<(), String> {
        self.used += amount;
        if self.used > self.limit {
            Err("gas exhausted".to_string())
        } else {
            Ok(())
        }
    }

    /// Charge for a state-read operation that reads `byte_count` bytes.
    ///
    /// Internally calls [`charge`][Self::charge] with the value returned by
    /// [`GasCosts::total_state_read_cost`], then increments `state_reads` on
    /// success.
    ///
    /// # Errors
    ///
    /// Propagates `Err("gas exhausted")` from [`charge`][Self::charge].
    pub fn charge_state_read(&mut self, byte_count: u64) -> Result<(), String> {
        let cost = self.costs.total_state_read_cost(byte_count);
        self.charge(cost)?;
        self.state_reads += 1;
        Ok(())
    }

    /// Charge for a state-write operation that writes `byte_count` bytes.
    ///
    /// Internally calls [`charge`][Self::charge] with the value returned by
    /// [`GasCosts::total_state_write_cost`], then increments `state_writes` on
    /// success.
    ///
    /// # Errors
    ///
    /// Propagates `Err("gas exhausted")` from [`charge`][Self::charge].
    pub fn charge_state_write(&mut self, byte_count: u64) -> Result<(), String> {
        let cost = self.costs.total_state_write_cost(byte_count);
        self.charge(cost)?;
        self.state_writes += 1;
        Ok(())
    }

    /// Increment the call-stack depth, enforcing the anti-reentrancy limit.
    ///
    /// # Errors
    ///
    /// Returns `Err("max call depth exceeded: potential reentrancy")` if
    /// `call_depth` would exceed [`GasCosts::max_call_depth`] after the
    /// increment.
    pub fn push_call(&mut self) -> Result<(), String> {
        self.call_depth += 1;
        if self.call_depth > self.costs.max_call_depth {
            Err("max call depth exceeded: potential reentrancy".to_string())
        } else {
            Ok(())
        }
    }

    /// Decrement the call-stack depth, saturating at zero.
    ///
    /// Always succeeds; it is a logic error to call `pop_call` more times
    /// than [`push_call`][Self::push_call], but the counter simply clamps at
    /// zero rather than panicking.
    pub const fn pop_call(&mut self) {
        self.call_depth = self.call_depth.saturating_sub(1);
    }

    /// Gas units remaining before the limit is reached.
    ///
    /// Uses saturating subtraction, so this never wraps below zero even when
    /// `used` has exceeded `limit` (which can happen when `charge` returns an
    /// error but the caller continues executing).
    #[must_use]
    pub const fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }

    /// Build a [`GasReport`] from the current counter state.
    ///
    /// `completed_normally` should be `true` when the contract reached a clean
    /// exit point, and `false` when execution was halted (e.g., gas exhausted,
    /// reentrancy guard triggered, or a trap).
    #[must_use]
    pub const fn to_report(&self, completed_normally: bool) -> GasReport {
        GasReport::new(self.used, self.limit, completed_normally)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Backward-compatible GasReport tests ──────────────────────────────────

    #[test]
    fn test_gas_report_utilisation() {
        let r = GasReport::new(500, 1_000, true);
        assert!((r.utilisation() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_gas_report_zero_limit() {
        let r = GasReport::new(0, 0, true);
        assert!((r.utilisation() - 0.0).abs() < f64::EPSILON);
    }

    // ── GasCosts ──────────────────────────────────────────────────────────────

    #[test]
    fn test_default_costs_values() {
        let c = GasCosts::default_costs();
        assert_eq!(c.base_execution, 1_000, "base_execution");
        assert_eq!(c.state_read, 50, "state_read");
        assert_eq!(c.state_write, 200, "state_write");
        assert_eq!(c.per_byte_read, 1, "per_byte_read");
        assert_eq!(c.per_byte_write, 2, "per_byte_write");
        assert_eq!(c.max_call_depth, 8, "max_call_depth");
    }

    #[test]
    fn test_gas_costs_custom() {
        let custom = GasCosts {
            base_execution: 500,
            state_read: 25,
            state_write: 100,
            per_byte_read: 2,
            per_byte_write: 4,
            max_call_depth: 4,
        };
        let mut counter = GasCounter::new_with_costs(10_000, custom);

        // total_state_read_cost:  25 + 2 * 10 = 45
        assert!(counter.charge_state_read(10).is_ok());
        assert_eq!(counter.used, 45, "read cost with custom table");

        // total_state_write_cost: 100 + 4 * 10 = 140
        assert!(counter.charge_state_write(10).is_ok());
        assert_eq!(counter.used, 185, "cumulative cost after write");

        // max_call_depth = 4 → 5th push must fail
        for _ in 0..4 {
            counter.push_call().expect("within custom depth limit");
        }
        assert!(
            counter.push_call().is_err(),
            "5th push_call must fail with custom max_call_depth=4"
        );
    }

    // ── GasCounter — basic charging ───────────────────────────────────────────

    #[test]
    fn test_counter_charge_within_limit() {
        let mut counter = GasCounter::new(1_000);
        assert!(counter.charge(500).is_ok(), "500 within limit of 1 000");
        assert_eq!(counter.used, 500);
    }

    #[test]
    fn test_counter_charge_exhausts() {
        let mut counter = GasCounter::new(1_000);
        let result = counter.charge(1_001);
        assert!(
            result.is_err(),
            "charging 1 001 must exhaust limit of 1 000"
        );
        assert_eq!(result.unwrap_err(), "gas exhausted");
    }

    // ── GasCounter — state access costs ──────────────────────────────────────

    #[test]
    fn test_counter_state_read_cost() {
        let mut counter = GasCounter::new(10_000);
        // state_read(50) + per_byte_read(1) * 100 = 150
        assert!(counter.charge_state_read(100).is_ok());
        assert_eq!(counter.used, 150, "state read: 50 + 1*100 = 150");
        assert_eq!(counter.state_reads, 1, "one read op recorded");
    }

    #[test]
    fn test_counter_state_write_cost() {
        let mut counter = GasCounter::new(10_000);
        // state_write(200) + per_byte_write(2) * 50 = 300
        assert!(counter.charge_state_write(50).is_ok());
        assert_eq!(counter.used, 300, "state write: 200 + 2*50 = 300");
        assert_eq!(counter.state_writes, 1, "one write op recorded");
    }

    // ── GasCounter — reentrancy guard ─────────────────────────────────────────

    #[test]
    fn test_counter_reentrancy_guard() {
        let mut counter = GasCounter::new(1_000_000);
        // max_call_depth = 8 — eight pushes must succeed
        for i in 0..8 {
            assert!(
                counter.push_call().is_ok(),
                "push {i} should be within depth limit"
            );
        }
        // ninth push must fail
        let result = counter.push_call();
        assert!(
            result.is_err(),
            "9th push_call must exceed max_call_depth=8"
        );
        assert_eq!(
            result.unwrap_err(),
            "max call depth exceeded: potential reentrancy"
        );
    }

    // ── GasCounter — report & remaining ──────────────────────────────────────

    #[test]
    fn test_counter_to_report() {
        let mut counter = GasCounter::new(1_000);
        counter.charge(400).unwrap();
        let report = counter.to_report(true);
        assert_eq!(report.gas_used, 400);
        assert_eq!(report.gas_limit, 1_000);
        assert!(report.completed_normally);
    }

    #[test]
    fn test_counter_remaining() {
        let mut counter = GasCounter::new(1_000);
        counter.charge(300).unwrap();
        assert_eq!(counter.remaining(), 700, "1 000 - 300 = 700 remaining");
    }
}
