//! WebAssembly contract runtime for `GlassChain` (Phase 4).
//!
//! This crate implements the [`ExecutionProvider`] trait using **Wasmtime**,
//! a fast, safe, standards-compliant WebAssembly runtime from the Bytecode
//! Alliance. It enforces independent instruction-fuel and host-operation gas
//! budgets.
//!
//! ## Design
//!
//! Smart contracts are compiled WebAssembly modules that export a single
//! `execute` function:
//!
//! ```wat
//! (module
//!   (import "env" "get_state" (func $get_state (param i32 i32) (result i32)))
//!   (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
//!   (export "execute" (func $main))
//!   ...
//! )
//! ```
//!
//! The runtime provides host functions (`get_state`, `set_state`) that the
//! contract uses to read and write the World State.  All mutations are
//! collected and returned to the caller for atomic commitment.
//!
//! ## Gas metering
//!
//! Wasmtime's "fuel" feature deducts one unit of fuel per WASM instruction.
//! The caller provides [`ExecutionLimits`] with separate fuel and operation-gas
//! budgets. Host state reads and writes use [`gas::GasCounter`]; exhausting
//! either budget returns [`CoreError::GasExhausted`].
//!
//! ## Safety
//!
//! Each contract execution runs in a fully sandboxed Wasmtime instance.  A
//! misbehaving contract cannot access the host's memory, filesystem, or
//! network.

pub mod gas;
pub mod wasm;

pub use wasm::WasmExecutionProvider;
