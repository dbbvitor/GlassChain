//! Wasmtime-backed [`ExecutionProvider`] implementation.

use crate::gas::GasCounter;
use glasschain_core::{
    CoreError, ExecutionLimits, ExecutionProvider, ExecutionResult, GasMeter, PersistentWrite,
    WriteOp, WriteVisibility,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wasmtime::{Config, Engine, Linker, Module, Store};

/// Shared host state accessible to WASM contracts during execution.
///
/// The contract reads and writes key-value pairs through host functions.
/// Ephemeral `set_state` writes land in `mutations`; explicit persistent
/// operations land in `persistent`. Both are returned after the contract
/// returns, allowing the node to commit them atomically.
struct HostState {
    /// Read-only snapshot of the World State (passed in before execution).
    world_state: HashMap<String, Vec<u8>>,
    /// Ephemeral key-value output produced by `set_state` (invocation-local).
    ephemeral: Vec<(String, Vec<u8>)>,
    /// Explicit persistent set/delete operations from `persist_state`.
    writes: Vec<PersistentWrite>,
    /// Independent budget for host state operations.
    operation_gas: GasCounter,
}

/// Wasmtime-based [`ExecutionProvider`] with deterministic dual-budget metering.
///
/// Contracts are compiled WASM modules. Each execution gets a fresh [`Store`]
/// with independent instruction-fuel and host-operation budgets.
pub struct WasmExecutionProvider {
    engine: Engine,
}

impl WasmExecutionProvider {
    /// Create a new provider.  The engine is configured with:
    /// - **fuel consumption enabled** — each WASM instruction costs 1 unit of
    ///   fuel, capped by each execution's `fuel_limit`.
    /// - **epoch interruption disabled** — instruction fuel and host-operation
    ///   traps are the execution limits.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Execution`] if the Wasmtime engine cannot be
    /// initialised with the requested configuration.
    pub fn new() -> Result<Self, CoreError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)
            .map_err(|e| CoreError::Execution(format!("wasmtime engine: {e}")))?;
        Ok(Self { engine })
    }

    fn charge_operation_gas(
        host_state: &Arc<Mutex<HostState>>,
        charge: impl FnOnce(&mut GasCounter) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut state = host_state
            .lock()
            .map_err(|_| "host state lock poisoned".to_string())?;
        charge(&mut state.operation_gas)
    }

    // The host import registrations are kept together so their ABI stays visible.
    #[allow(clippy::too_many_lines)]
    fn build_linker(&self) -> Result<Linker<Arc<Mutex<HostState>>>, CoreError> {
        let mut linker: Linker<Arc<Mutex<HostState>>> = Linker::new(&self.engine);

        linker
            .func_wrap(
                "env",
                "set_state",
                |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                 key_ptr: i32,
                 key_len: i32,
                 val_ptr: i32,
                 val_len: i32|
                 -> wasmtime::Result<()> {
                    let (key, val) = {
                        let Some(wasmtime::Extern::Memory(mem)) = caller.get_export("memory")
                        else {
                            return Ok(());
                        };
                        let data = mem.data(&caller);
                        let Ok(kp) = usize::try_from(key_ptr) else {
                            return Ok(());
                        };
                        let Ok(kl) = usize::try_from(key_len) else {
                            return Ok(());
                        };
                        let Ok(vp) = usize::try_from(val_ptr) else {
                            return Ok(());
                        };
                        let Ok(vl) = usize::try_from(val_len) else {
                            return Ok(());
                        };
                        let Some(kend) = kp.checked_add(kl) else {
                            return Ok(());
                        };
                        let Some(vend) = vp.checked_add(vl) else {
                            return Ok(());
                        };
                        if kend > data.len() || vend > data.len() {
                            return Ok(());
                        }
                        (
                            String::from_utf8_lossy(&data[kp..kend]).to_string(),
                            data[vp..vend].to_vec(),
                        )
                    };

                    Self::charge_operation_gas(caller.data(), |gas| {
                        gas.charge_state_write(val.len() as u64)
                    })
                    .map_err(|e| wasmtime::format_err!("{e}"))?;

                    caller
                        .data()
                        .lock()
                        .map_err(|_| wasmtime::format_err!("host state lock poisoned"))?
                        .ephemeral
                        .push((key, val));
                    Ok(())
                },
            )
            .map_err(|e| CoreError::Execution(format!("linker set_state: {e}")))?;

        linker
            .func_wrap(
                "env",
                "get_state_len",
                |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                 key_ptr: i32,
                 key_len: i32|
                 -> wasmtime::Result<i32> {
                    let key = {
                        let Some(wasmtime::Extern::Memory(mem)) = caller.get_export("memory")
                        else {
                            return Ok(-1);
                        };
                        let data = mem.data(&caller);
                        let Ok(kp) = usize::try_from(key_ptr) else {
                            return Ok(-1);
                        };
                        let Ok(kl) = usize::try_from(key_len) else {
                            return Ok(-1);
                        };
                        let Some(end) = kp.checked_add(kl) else {
                            return Ok(-1);
                        };
                        if end > data.len() {
                            return Ok(-1);
                        }
                        String::from_utf8_lossy(&data[kp..end]).to_string()
                    };

                    Self::charge_operation_gas(caller.data(), |gas| gas.charge_state_read(0))
                        .map_err(|e| wasmtime::format_err!("{e}"))?;
                    let state = caller
                        .data()
                        .lock()
                        .map_err(|_| wasmtime::format_err!("host state lock poisoned"))?;
                    Ok(state
                        .world_state
                        .get(&key)
                        .map_or(-1, |v| i32::try_from(v.len()).unwrap_or(-1)))
                },
            )
            .map_err(|e| CoreError::Execution(format!("linker get_state_len: {e}")))?;

        linker
            .func_wrap(
                "env",
                "get_state",
                |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                 key_ptr: i32,
                 key_len: i32,
                 val_ptr: i32,
                 val_buf_len: i32|
                 -> wasmtime::Result<i32> {
                    let Some(wasmtime::Extern::Memory(mem)) = caller.get_export("memory") else {
                        return Ok(-1);
                    };

                    let Ok(kp) = usize::try_from(key_ptr) else {
                        return Ok(-1);
                    };
                    let Ok(kl) = usize::try_from(key_len) else {
                        return Ok(-1);
                    };
                    let Ok(vp) = usize::try_from(val_ptr) else {
                        return Ok(-1);
                    };
                    let Ok(vbl) = usize::try_from(val_buf_len) else {
                        return Ok(-1);
                    };

                    let key = {
                        let data = mem.data(&caller);
                        let Some(end) = kp.checked_add(kl) else {
                            return Ok(-1);
                        };
                        if end > data.len() {
                            return Ok(-1);
                        }
                        String::from_utf8_lossy(&data[kp..end]).to_string()
                    };

                    let value = {
                        let state = caller
                            .data()
                            .lock()
                            .map_err(|_| wasmtime::format_err!("host state lock poisoned"))?;
                        match state.world_state.get(&key) {
                            Some(v) => v.clone(),
                            None => return Ok(-1),
                        }
                    };

                    Self::charge_operation_gas(caller.data(), |gas| {
                        gas.charge_state_read(value.len() as u64)
                    })
                    .map_err(|e| wasmtime::format_err!("{e}"))?;

                    if value.len() > vbl {
                        return Ok(-2);
                    }

                    let data_mut = mem.data_mut(&mut caller);
                    let Some(end) = vp.checked_add(value.len()) else {
                        return Ok(-1);
                    };
                    if end > data_mut.len() {
                        return Ok(-1);
                    }
                    data_mut[vp..end].copy_from_slice(&value);
                    Ok(i32::try_from(value.len()).unwrap_or(-1))
                },
            )
            .map_err(|e| CoreError::Execution(format!("linker get_state: {e}")))?;

        Self::add_persist_state(&mut linker)?;

        Ok(linker)
    }

    /// Read `len` bytes at `ptr` from the caller's exported linear memory.
    /// Returns `None` on any bounds or conversion problem (host ops treat
    /// malformed pointers as no-ops, matching `set_state`/`get_state`).
    fn read_bytes(
        caller: &mut wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
        ptr: i32,
        len: i32,
    ) -> Option<Vec<u8>> {
        let wasmtime::Extern::Memory(mem) = caller.get_export("memory")? else {
            return None;
        };
        let data = mem.data(caller);
        let start = usize::try_from(ptr).ok()?;
        let length = usize::try_from(len).ok()?;
        let end = start.checked_add(length)?;
        (end <= data.len()).then(|| data[start..end].to_vec())
    }

    /// Read a UTF-8 string from the caller's linear memory.
    ///
    /// Scope identifiers are strict UTF-8, not lossy: collapsing two distinct
    /// byte strings into the same scoped key would silently break the
    /// one-result-per-scope rule, so malformed UTF-8 is a malformed-pointer
    /// error.
    fn read_string(
        caller: &mut wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
        ptr: i32,
        len: i32,
    ) -> Option<String> {
        let bytes = Self::read_bytes(caller, ptr, len)?;
        String::from_utf8(bytes).ok()
    }

    /// Register `env::persist_state` — the explicit persistence host operation
    /// (ADR-007 decision 1). `set_state` remains ephemeral.
    ///
    /// ABI: `persist_state(channel_ptr, channel_len, contract_ptr,
    /// contract_len, key_ptr, key_len, val_ptr, val_len, op, visibility,
    /// pdc_ptr, pdc_len) -> i32`
    /// - `op`: 0 = set (value bytes matter), 1 = delete (value ignored);
    /// - `visibility`: 0 = public, 1 = named PDC (pdc name must be non-empty);
    /// - returns 0 on success, -1 unknown op, -2 unknown visibility, -3 empty
    ///   PDC name, -4 malformed pointers.
    fn add_persist_state(linker: &mut Linker<Arc<Mutex<HostState>>>) -> Result<(), CoreError> {
        linker
            .func_wrap(
                "env",
                "persist_state",
                |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                 channel_ptr: i32,
                 channel_len: i32,
                 contract_ptr: i32,
                 contract_len: i32,
                 key_ptr: i32,
                 key_len: i32,
                 val_ptr: i32,
                 val_len: i32,
                 op: i32,
                 visibility: i32,
                 pdc_ptr: i32,
                 pdc_len: i32|
                 -> wasmtime::Result<i32> {
                    let Some(channel) = Self::read_string(&mut caller, channel_ptr, channel_len)
                    else {
                        return Ok(-4);
                    };
                    let Some(contract) = Self::read_string(&mut caller, contract_ptr, contract_len)
                    else {
                        return Ok(-4);
                    };
                    let Some(key) = Self::read_string(&mut caller, key_ptr, key_len) else {
                        return Ok(-4);
                    };
                    let operation = match op {
                        0 => {
                            let Some(value) = Self::read_bytes(&mut caller, val_ptr, val_len)
                            else {
                                return Ok(-4);
                            };
                            WriteOp::Set(value)
                        }
                        1 => WriteOp::Delete,
                        _ => return Ok(-1),
                    };
                    let write_visibility = match visibility {
                        0 => WriteVisibility::Public,
                        1 => {
                            let Some(pdc) = Self::read_string(&mut caller, pdc_ptr, pdc_len) else {
                                return Ok(-4);
                            };
                            if pdc.is_empty() {
                                return Ok(-3);
                            }
                            WriteVisibility::Pdc(pdc)
                        }
                        _ => return Ok(-2),
                    };

                    let byte_count = match &operation {
                        WriteOp::Set(value) => value.len() as u64,
                        WriteOp::Delete => 0,
                    };
                    Self::charge_operation_gas(caller.data(), |gas| {
                        gas.charge_state_write(byte_count)
                    })
                    .map_err(|e| wasmtime::format_err!("{e}"))?;

                    caller
                        .data()
                        .lock()
                        .map_err(|_| wasmtime::format_err!("host state lock poisoned"))?
                        .writes
                        .push(PersistentWrite {
                            channel,
                            contract,
                            key,
                            op: operation,
                            visibility: write_visibility,
                        });
                    Ok(0)
                },
            )
            .map_err(|e| CoreError::Execution(format!("linker persist_state: {e}")))?;
        Ok(())
    }

    fn execute_internal(
        &self,
        contract_id: &str,
        payload: &[u8],
        initial_state: HashMap<String, Vec<u8>>,
        limits: ExecutionLimits,
    ) -> Result<ExecutionResult, CoreError> {
        let module = Module::new(&self.engine, payload)
            .map_err(|e| CoreError::Execution(format!("wasm compile [{contract_id}]: {e}")))?;

        let host_state = Arc::new(Mutex::new(HostState {
            world_state: initial_state,
            ephemeral: Vec::new(),
            writes: Vec::new(),
            operation_gas: GasCounter::new(limits.operation_gas_limit),
        }));
        let linker = self.build_linker()?;

        let mut store = Store::new(&self.engine, Arc::clone(&host_state));
        // Instantiation is runtime setup, not contract execution. In particular,
        // Wasmtime meters memory initialization and data-segment copies.
        store
            .set_fuel(u64::MAX)
            .map_err(|e| CoreError::Execution(format!("set_fuel (instantiate): {e}")))?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| CoreError::Execution(format!("wasm instantiate [{contract_id}]: {e}")))?;

        store
            .set_fuel(limits.fuel_limit)
            .map_err(|e| CoreError::Execution(format!("set_fuel: {e}")))?;

        let base_cost = host_state
            .lock()
            .map_err(|_| CoreError::Execution("host state lock poisoned".to_string()))?
            .operation_gas
            .costs()
            .base_execution;
        if base_cost > limits.operation_gas_limit {
            return Err(CoreError::GasExhausted {
                meter: GasMeter::Operation,
                used: base_cost,
                limit: limits.operation_gas_limit,
            });
        }
        Self::charge_operation_gas(&host_state, |gas| gas.charge(base_cost))
            .map_err(|e| CoreError::Execution(format!("operation gas setup: {e}")))?;

        let execute_fn = instance
            .get_typed_func::<(), ()>(&mut store, "execute")
            .map_err(|e| {
                CoreError::Execution(format!("wasm export 'execute' [{contract_id}]: {e}"))
            })?;

        match execute_fn.call(&mut store, ()) {
            Ok(()) => {}
            Err(e) => {
                let operation_used = host_state
                    .lock()
                    .map_err(|_| CoreError::Execution("host state lock poisoned".to_string()))?
                    .operation_gas
                    .used;
                if operation_used > limits.operation_gas_limit {
                    return Err(CoreError::GasExhausted {
                        meter: GasMeter::Operation,
                        used: operation_used,
                        limit: limits.operation_gas_limit,
                    });
                }

                let is_fuel_exhaustion = e
                    .downcast_ref::<wasmtime::Trap>()
                    .is_some_and(|t| *t == wasmtime::Trap::OutOfFuel)
                    || store.get_fuel().unwrap_or(1) == 0;
                if is_fuel_exhaustion {
                    let used = limits
                        .fuel_limit
                        .saturating_sub(store.get_fuel().unwrap_or(0));
                    return Err(CoreError::GasExhausted {
                        meter: GasMeter::Fuel,
                        used,
                        limit: limits.fuel_limit,
                    });
                }
                return Err(CoreError::Execution(format!(
                    "wasm execution [{contract_id}]: {e}"
                )));
            }
        }

        let host_state_guard = host_state
            .lock()
            .map_err(|_| CoreError::Execution("host state lock poisoned".to_string()))?;
        let ephemeral = host_state_guard.ephemeral.clone();
        let writes = host_state_guard.writes.clone();
        let operation_used = host_state_guard.operation_gas.used;
        drop(host_state_guard);
        log::debug!(
            "WasmExecutionProvider: contract={contract_id} ephemeral={} writes={} fuel_remaining={} operation_gas_used={operation_used}",
            ephemeral.len(),
            writes.len(),
            store.get_fuel().unwrap_or(0),
        );
        // Canonicalize at the execution seam (ADR-007 decision 2): a scoped key
        // with more than one operation rejects the execution deterministically,
        // and the returned write set is already in canonical order.
        ExecutionResult { ephemeral, writes }
            .canonicalize()
            .map_err(|e| CoreError::Execution(format!("invalid write set [{contract_id}]: {e}")))
    }
}

impl Default for WasmExecutionProvider {
    fn default() -> Self {
        Self::new().expect("wasmtime engine creation failed")
    }
}

impl ExecutionProvider for WasmExecutionProvider {
    /// Execute a WASM contract module with independent fuel and operation limits.
    ///
    /// `payload` must be valid WASM binary (`.wasm`) or WebAssembly text
    /// format (`.wat`) bytes.
    ///
    /// The contract module **must** export a function named `execute` with
    /// signature `() -> ()`.  It may import:
    /// - `env::set_state(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32)`
    ///   — write an **ephemeral** invocation-local key-value pair.
    /// - `env::get_state_len(key_ptr: i32, key_len: i32) -> i32`
    ///   — return the byte length of a stored value, or `-1` if the key is absent.
    /// - `env::get_state(key_ptr: i32, key_len: i32, val_ptr: i32, val_buf_len: i32) -> i32`
    ///   — copy a stored value into the contract's linear memory; returns the
    ///   number of bytes written, `-1` if the key is missing or if the key
    ///   pointer is out of bounds, or `-2` if the value is larger than the
    ///   supplied buffer.
    /// - `env::persist_state(channel_ptr, channel_len, contract_ptr,
    ///   contract_len, key_ptr, key_len, val_ptr, val_len, op, visibility,
    ///   pdc_ptr, pdc_len) -> i32`
    ///   — request an **explicit persistent** set (`op = 0`) or delete
    ///   (`op = 1`) under channel/contract/key scope with public
    ///   (`visibility = 0`) or named-PDC (`visibility = 1`) visibility
    ///   (ADR-007 decision 3). Returns `0` on success; `-1` unknown op,
    ///   `-2` unknown visibility, `-3` empty PDC name, `-4` malformed
    ///   pointers.
    #[allow(clippy::too_many_lines)]
    fn execute(
        &self,
        contract_id: &str,
        payload: &[u8],
        limits: ExecutionLimits,
    ) -> Result<ExecutionResult, CoreError> {
        self.execute_internal(contract_id, payload, HashMap::new(), limits)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_with_state(
        &self,
        contract_id: &str,
        payload: &[u8],
        initial_state: std::collections::HashMap<String, Vec<u8>>,
        limits: ExecutionLimits,
    ) -> Result<ExecutionResult, CoreError> {
        self.execute_internal(contract_id, payload, initial_state, limits)
    }

    fn name(&self) -> &'static str {
        "wasmtime"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::{ExecutionLimits, ExecutionProvider, GasMeter};

    /// Minimal WAT contract that calls `set_state` to write "hello" = "world".
    fn hello_world_wat() -> &'static str {
        r#"
(module
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "hello")
  (data (i32.const 5) "world")
  (func (export "execute")
    (call $set_state
      (i32.const 0) (i32.const 5)
      (i32.const 5) (i32.const 5)
    )
  )
)
"#
    }

    /// Infinite-loop WAT (should exhaust fuel).
    fn infinite_loop_wat() -> &'static str {
        r#"
(module
  (func (export "execute")
    (loop $lp
      (br $lp)
    )
  )
)
"#
    }

    fn compile_wat(wat: &str) -> Vec<u8> {
        wat::parse_str(wat).expect("WAT compile failed")
    }

    fn limits(value: u64) -> ExecutionLimits {
        ExecutionLimits::new(value, value)
    }

    #[test]
    fn test_hello_world_contract() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(hello_world_wat());
        let result = provider
            .execute("hello-contract", &wasm, limits(10_000))
            .unwrap();
        assert_eq!(result.ephemeral.len(), 1);
        assert_eq!(result.ephemeral[0].0, "hello");
        assert_eq!(result.ephemeral[0].1, b"world");
        assert!(result.writes.is_empty());
    }

    #[test]
    fn test_gas_exhaustion() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(infinite_loop_wat());
        let result = provider.execute("loop-contract", &wasm, ExecutionLimits::new(100, 10_000));
        assert!(
            matches!(
                result,
                Err(CoreError::GasExhausted {
                    meter: GasMeter::Fuel,
                    ..
                })
            ),
            "expected GasExhausted, got {result:?}"
        );
    }

    #[test]
    fn test_operation_gas_exhaustion() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(hello_world_wat());
        let result = provider.execute(
            "operation-gas-contract",
            &wasm,
            ExecutionLimits::new(10_000, 1_209),
        );
        assert!(matches!(
            result,
            Err(CoreError::GasExhausted {
                meter: GasMeter::Operation,
                ..
            })
        ));
    }

    /// Garbage bytes that are not a valid WASM module must fail compilation and
    /// surface as a `CoreError::Execution` (never a trap or panic).
    #[test]
    fn test_compile_error_returns_execution_error() {
        let provider = WasmExecutionProvider::new().unwrap();
        let result = provider.execute("not-wasm-contract", b"not-wasm", limits(10_000));
        assert!(
            matches!(result, Err(CoreError::Execution(_))),
            "expected Execution error, got {result:?}"
        );
    }

    /// When the operation-gas limit is below the base execution cost, the early
    /// guard must reject the call with `GasExhausted` / `GasMeter::Operation`.
    #[test]
    fn test_operation_gas_below_base_cost_exhausts() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(r#"(module (func (export "execute")))"#);
        // base_execution costs 1_000; any limit below it must fail upfront.
        let result = provider.execute(
            "base-cost-contract",
            &wasm,
            ExecutionLimits::new(10_000, 999),
        );
        assert!(matches!(
            result,
            Err(CoreError::GasExhausted {
                meter: GasMeter::Operation,
                used: 1_000,
                limit: 999
            })
        ));
    }

    /// A `unreachable` instruction traps — neither fuel nor operation
    /// exhaustion — so it must surface as `CoreError::Execution` rather than
    /// `GasExhausted`.
    #[test]
    fn test_unreachable_trap_returns_execution_error() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (func (export "execute")
    unreachable
  )
)
"#,
        );
        let result = provider.execute("trap-contract", &wasm, limits(10_000));
        assert!(
            matches!(result, Err(CoreError::Execution(_))),
            "expected Execution error, got {result:?}"
        );
    }

    #[test]
    fn test_operation_gas_charges_state_read() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (import "env" "get_state_len" (func $get_state_len (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "ping")
  (func (export "execute")
    (drop (call $get_state_len (i32.const 0) (i32.const 4)))
  )
)
"#,
        );
        let mut initial = HashMap::new();
        initial.insert("ping".to_string(), b"pong".to_vec());
        let result = provider.execute_with_state(
            "operation-read-gas-contract",
            &wasm,
            initial,
            ExecutionLimits::new(10_000, 1_049),
        );
        assert!(matches!(
            result,
            Err(CoreError::GasExhausted {
                meter: GasMeter::Operation,
                ..
            })
        ));
    }

    #[test]
    fn test_provider_name() {
        let provider = WasmExecutionProvider::new().unwrap();
        assert_eq!(provider.name(), "wasmtime");
    }

    #[test]
    fn test_empty_contract_no_mutations() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (func (export "execute"))
)
"#,
        );
        let result = provider
            .execute("noop-contract", &wasm, limits(10_000))
            .unwrap();
        assert!(result.ephemeral.is_empty());
        assert!(result.writes.is_empty());
    }

    #[test]
    fn test_memory_initialization_does_not_consume_contract_gas() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (memory (export "memory") 16)
  (data (i32.const 0) "initialized")
  (func (export "execute"))
)
"#,
        );
        let result = provider.execute(
            "memory-init-contract",
            &wasm,
            ExecutionLimits::new(100, 10_000),
        );
        assert!(
            result.is_ok(),
            "runtime setup must not consume contract gas: {result:?}"
        );
    }

    /// Verify that calling `get_state` for a key that was never written into
    /// `world_state` returns -1 without causing a trap or an `Err` result.
    #[test]
    fn test_get_state_missing_key_returns_neg_one() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (import "env" "get_state" (func $get_state (param i32 i32 i32 i32) (result i32)))
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "missing_key")
  (func (export "execute")
    (local $result i32)
    (local.set $result
      (call $get_state
        (i32.const 0)  (i32.const 11)   ;; key   = "missing_key" (11 bytes)
        (i32.const 100) (i32.const 64)  ;; value buffer at offset 100, capacity 64
      )
    )
    ;; result is -1 but we have no way to assert in WAT itself;
    ;; the important thing is that the call completes without trapping.
    (drop (local.get $result))
  )
)
"#,
        );
        // The contract must execute without returning an Err — get_state returning
        // -1 for a missing key must NOT cause a trap or a Rust-level error.
        let result = provider.execute("get-state-test", &wasm, limits(10_000));
        assert!(result.is_ok(), "expected Ok(result), got {result:?}");
        // No mutations were written, so both output lists must be empty.
        let result = result.unwrap();
        assert!(result.ephemeral.is_empty());
        assert!(result.writes.is_empty());
    }

    /// Verify that calling `get_state` when the key's value is longer than the
    /// caller-supplied buffer returns -2 (buffer too small) without trapping.
    ///
    /// The contract records the low byte of the returned status under the key
    /// "result" so the test can observe the -2 return value (i32 -2 stores as
    /// 0xFE via `i32.store8`).
    #[test]
    fn test_get_state_buffer_too_small_returns_neg_two() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (import "env" "get_state" (func $get_state (param i32 i32 i32 i32) (result i32)))
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "ping")
  (data (i32.const 10) "result")
  (func (export "execute")
    (local $result i32)
    (local.set $result
      (call $get_state
        (i32.const 0)   (i32.const 4)   ;; key = "ping"
        (i32.const 100) (i32.const 2)   ;; 2-byte buffer, value is 4 bytes
      )
    )
    (i32.store8 (i32.const 50) (local.get $result))
    (call $set_state (i32.const 10) (i32.const 6) (i32.const 50) (i32.const 1))
  )
)
"#,
        );
        let mut initial = HashMap::new();
        initial.insert("ping".to_string(), b"pong".to_vec());
        let result = provider
            .execute_with_state("buffer-too-small-test", &wasm, initial, limits(10_000))
            .unwrap();
        assert_eq!(result.ephemeral.len(), 1);
        assert_eq!(result.ephemeral[0].0, "result");
        // -2 as i32; i32.store8 keeps only the low byte 0xFE.
        assert_eq!(result.ephemeral[0].1, vec![0xFE]);
    }

    /// Verify that `execute_with_state` makes the pre-populated world-state
    /// visible to the contract via `get_state` / `get_state_len`.
    #[test]
    fn test_execute_with_state_reads_initial_state() {
        let provider = WasmExecutionProvider::new().unwrap();

        // WAT: read "ping" from world state and write its length as "pong_len".
        // Uses get_state_len to check that "ping" exists, then set_state to
        // record the result.  If get_state_len returns -1 (key absent) the
        // contract writes "pong_len" = "missing".
        let wasm = compile_wat(
            r#"
(module
  (import "env" "set_state"     (func $set_state     (param i32 i32 i32 i32)))
  (import "env" "get_state_len" (func $get_state_len (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  ;; offset 0: key "ping" (4 bytes)
  (data (i32.const 0) "ping")
  ;; offset 10: key "pong_len" (8 bytes)
  (data (i32.const 10) "pong_len")
  ;; offset 20: value "found" (5 bytes)
  (data (i32.const 20) "found")
  ;; offset 30: value "missing" (7 bytes)
  (data (i32.const 30) "missing")
  (func (export "execute")
    (local $len i32)
    (local.set $len (call $get_state_len (i32.const 0) (i32.const 4)))
    (if (i32.ge_s (local.get $len) (i32.const 0))
      (then
        (call $set_state (i32.const 10) (i32.const 8) (i32.const 20) (i32.const 5))
      )
      (else
        (call $set_state (i32.const 10) (i32.const 8) (i32.const 30) (i32.const 7))
      )
    )
  )
)
"#,
        );

        let mut initial = HashMap::new();
        initial.insert("ping".to_string(), b"pong".to_vec());

        let result = provider
            .execute_with_state("state-test", &wasm, initial, limits(50_000))
            .unwrap();

        assert_eq!(result.ephemeral.len(), 1);
        assert_eq!(result.ephemeral[0].0, "pong_len");
        assert_eq!(result.ephemeral[0].1, b"found");
        assert!(result.writes.is_empty());
    }

    /// `set_state` must stay ephemeral (ADR-007 decision 1): an approval
    /// evaluation produces invocation-local output only and persists nothing.
    #[test]
    fn test_set_state_remains_ephemeral() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "approve")
  (data (i32.const 10) "1")
  (data (i32.const 20) "note")
  (data (i32.const 30) "extra-output")
  (func (export "execute")
    (call $set_state (i32.const 0) (i32.const 7) (i32.const 10) (i32.const 1))
    (call $set_state (i32.const 20) (i32.const 4) (i32.const 30) (i32.const 12))
  )
)
"#,
        );
        let result = provider
            .execute("approval-regression", &wasm, limits(50_000))
            .unwrap();
        assert_eq!(result.ephemeral.len(), 2, "approve + note are ephemeral");
        assert!(
            result
                .ephemeral
                .iter()
                .any(|(k, v)| k == "approve" && v == b"1"),
            "approval decision is readable from ephemeral output"
        );
        assert!(
            result.writes.is_empty(),
            "an approval evaluation must persist nothing: {:?}",
            result.writes
        );
        assert!(result.canonicalize().is_ok());
    }

    /// The explicit `persist_state` host operation carries channel, contract,
    /// key, set/delete, and public/PDC visibility (ADR-007 decision 3).
    #[test]
    fn test_persist_state_produces_scoped_writes() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (import "env" "persist_state" (func $persist (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "supply")      ;; channel
  (data (i32.const 10) "inventory")  ;; contract
  (data (i32.const 20) "threshold")  ;; key
  (data (i32.const 30) "42")         ;; value
  (data (i32.const 40) "price")      ;; key
  (data (i32.const 50) "9")          ;; value
  (data (i32.const 60) "pricing")    ;; pdc name
  (data (i32.const 70) "stale")      ;; key
  (func (export "execute")
    ;; set "threshold" = "42", public
    (drop (call $persist
      (i32.const 0) (i32.const 6)
      (i32.const 10) (i32.const 9)
      (i32.const 20) (i32.const 9)
      (i32.const 30) (i32.const 2)
      (i32.const 0) (i32.const 0)
      (i32.const 0) (i32.const 0)))
    ;; set "price" = "9", PDC "pricing"
    (drop (call $persist
      (i32.const 0) (i32.const 6)
      (i32.const 10) (i32.const 9)
      (i32.const 40) (i32.const 5)
      (i32.const 50) (i32.const 1)
      (i32.const 0) (i32.const 1)
      (i32.const 60) (i32.const 7)))
    ;; delete "stale", public
    (drop (call $persist
      (i32.const 0) (i32.const 6)
      (i32.const 10) (i32.const 9)
      (i32.const 70) (i32.const 5)
      (i32.const 0) (i32.const 0)
      (i32.const 1) (i32.const 0)
      (i32.const 0) (i32.const 0)))
  )
)
"#,
        );
        let result = provider
            .execute("persist-state-contract", &wasm, limits(50_000))
            .unwrap();
        assert!(
            result.ephemeral.is_empty(),
            "persist_state is not ephemeral"
        );
        // Canonical order (sorted by channel, contract, key): price, stale, threshold.
        assert_eq!(result.writes.len(), 3);

        let price = &result.writes[0];
        assert_eq!(price.channel, "supply");
        assert_eq!(price.contract, "inventory");
        assert_eq!(price.key, "price");
        assert_eq!(price.op, WriteOp::Set(b"9".to_vec()));
        assert_eq!(price.visibility, WriteVisibility::Pdc("pricing".into()));

        let stale = &result.writes[1];
        assert_eq!(stale.key, "stale");
        assert_eq!(stale.op, WriteOp::Delete);
        assert_eq!(stale.visibility, WriteVisibility::Public);

        let threshold = &result.writes[2];
        assert_eq!(threshold.key, "threshold");
        assert_eq!(threshold.op, WriteOp::Set(b"42".to_vec()));
        assert_eq!(threshold.visibility, WriteVisibility::Public);
    }

    /// Two operations on the same (channel, contract, key) scope are rejected
    /// by canonicalization rather than resolved by provider-specific ordering
    /// (ADR-007 decision 2).
    #[test]
    fn test_duplicate_scoped_write_rejected() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (import "env" "persist_state" (func $persist (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "supply")
  (data (i32.const 10) "inventory")
  (data (i32.const 20) "threshold")
  (data (i32.const 30) "a")
  (data (i32.const 40) "b")
  (func (export "execute")
    (drop (call $persist
      (i32.const 0) (i32.const 6)
      (i32.const 10) (i32.const 9)
      (i32.const 20) (i32.const 9)
      (i32.const 30) (i32.const 1)
      (i32.const 0) (i32.const 0)
      (i32.const 0) (i32.const 0)))
    (drop (call $persist
      (i32.const 0) (i32.const 6)
      (i32.const 10) (i32.const 9)
      (i32.const 20) (i32.const 9)
      (i32.const 40) (i32.const 1)
      (i32.const 0) (i32.const 0)
      (i32.const 0) (i32.const 0)))
  )
)
"#,
        );
        let error = provider
            .execute("duplicate-write-contract", &wasm, limits(50_000))
            .expect_err("duplicate scoped key must reject the execution");
        assert!(error.to_string().contains("more than one"), "{error}");
    }

    /// Unknown op and visibility codes surface as -1 / -2 return values.
    #[test]
    fn test_persist_state_rejects_unknown_codes() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(
            r#"
(module
  (import "env" "persist_state" (func $persist (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "supply")
  (data (i32.const 10) "inventory")
  (data (i32.const 20) "threshold")
  (data (i32.const 30) "a")
  (data (i32.const 40) "op-status")
  (data (i32.const 60) "vis-status")
  (func (export "execute")
    ;; unknown op = 5 → -1 (0xFF)
    (i32.store8 (i32.const 80)
      (call $persist
        (i32.const 0) (i32.const 6)
        (i32.const 10) (i32.const 9)
        (i32.const 20) (i32.const 9)
        (i32.const 30) (i32.const 1)
        (i32.const 5) (i32.const 0)
        (i32.const 0) (i32.const 0)))
    (call $set_state (i32.const 40) (i32.const 9) (i32.const 80) (i32.const 1))
    ;; unknown visibility = 7 → -2 (0xFE)
    (i32.store8 (i32.const 90)
      (call $persist
        (i32.const 0) (i32.const 6)
        (i32.const 10) (i32.const 9)
        (i32.const 20) (i32.const 9)
        (i32.const 30) (i32.const 1)
        (i32.const 0) (i32.const 7)
        (i32.const 0) (i32.const 0)))
    (call $set_state (i32.const 60) (i32.const 10) (i32.const 90) (i32.const 1))
  )
)
"#,
        );
        let result = provider
            .execute("persist-codes-contract", &wasm, limits(50_000))
            .unwrap();
        let status = |key: &str| {
            result
                .ephemeral
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .expect("status was recorded")
        };
        assert_eq!(status("op-status"), vec![0xFF], "-1 for unknown op");
        assert_eq!(
            status("vis-status"),
            vec![0xFE],
            "-2 for unknown visibility"
        );
        assert!(
            result.writes.is_empty(),
            "rejected operations must not produce writes"
        );
    }
}
