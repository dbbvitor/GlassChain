//! Deterministic contract layer for `GlassChain` (ticket #49's packaging
//! split): verification-only, deterministic contract code — the contract
//! registry, condition matching, and the WASM approval gate.
//!
//! ## The deterministic-contract invariant
//!
//! Everything in this crate is a pure function of its inputs: no wall clock,
//! no randomness, no network, no persistence. Given the same contracts and the
//! same committed state, evaluation and emission are byte-identical — which is
//! what makes replay and cross-node agreement safe. I/O-driven automation
//! (event watchers, flow orchestration) lives in `glasschain-workflows`.

pub mod approval_gate;
pub mod contract;
pub mod engine;
pub mod error;
// Fixture WASM for tests across the contract/workflow boundary (ticket
// #49): compiled at runtime from the in-crate WAT sources, hence the main
// wat dep.
pub mod test_wasm;

pub use approval_gate::{ApprovalGate, ApprovalGatePolicy, GateDecision};
pub use contract::{Contract, ContractStatus};
pub use engine::ContractEngine;
pub use error::ContractError;
