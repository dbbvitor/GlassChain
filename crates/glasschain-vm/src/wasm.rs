//! Wasmtime-backed [`ExecutionProvider`] implementation.

use glasschain_core::{CoreError, ExecutionProvider};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wasmtime::{Config, Engine, Linker, Module, Store};

/// Shared host state accessible to WASM contracts during execution.
///
/// The contract reads and writes key-value pairs through host functions.
/// All mutations are collected in `mutations` and returned after the contract
/// returns, allowing the node to commit them atomically.
struct HostState {
    /// Read-only snapshot of the World State (passed in before execution).
    world_state: HashMap<String, Vec<u8>>,
    /// Key-value mutations produced by this execution.
    mutations: Vec<(String, Vec<u8>)>,
    /// Key and value exchange buffers (backing the host-function linear memory).
    key_buf: Vec<u8>,
    val_buf: Vec<u8>,
}

/// Wasmtime-based [`ExecutionProvider`] with deterministic gas metering.
///
/// Contracts are compiled WASM modules.  Each execution gets a fresh
/// [`Store`] with a configured fuel budget (= `gas_limit`).
pub struct WasmExecutionProvider {
    engine: Engine,
}

impl WasmExecutionProvider {
    /// Create a new provider.  The engine is configured with:
    /// - **fuel consumption enabled** — each WASM instruction costs 1 unit of
    ///   fuel, capping contract execution at `gas_limit` instructions.
    /// - **epoch interruption disabled** — fuel is the only interrupt mechanism.
    pub fn new() -> Result<Self, CoreError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine =
            Engine::new(&config).map_err(|e| CoreError::Storage(format!("wasmtime engine: {e}")))?;
        Ok(Self { engine })
    }
}

impl Default for WasmExecutionProvider {
    fn default() -> Self {
        Self::new().expect("wasmtime engine creation failed")
    }
}

impl ExecutionProvider for WasmExecutionProvider {
    /// Execute a WASM contract module with the given gas limit.
    ///
    /// `payload` must be valid WASM binary (`.wasm`) or WebAssembly text
    /// format (`.wat`) bytes.
    ///
    /// The contract module **must** export a function named `execute` with
    /// signature `() -> ()`.  It may import:
    /// - `env::set_state(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32)`
    ///   — write a key-value pair to the World State.
    fn execute(
        &self,
        contract_id: &str,
        payload: &[u8],
        gas_limit: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
        let module = Module::new(&self.engine, payload)
            .map_err(|e| CoreError::Storage(format!("wasm compile [{contract_id}]: {e}")))?;

        let host_state = Arc::new(Mutex::new(HostState {
            world_state: HashMap::new(),
            mutations: Vec::new(),
            key_buf: Vec::new(),
            val_buf: Vec::new(),
        }));

        let mut linker: Linker<Arc<Mutex<HostState>>> = Linker::new(&self.engine);

        // Host function: set_state(key_ptr, key_len, val_ptr, val_len)
        {
            linker
                .func_wrap(
                    "env",
                    "set_state",
                    |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                     key_ptr: i32,
                     key_len: i32,
                     val_ptr: i32,
                     val_len: i32| {
                        let mem = match caller.get_export("memory") {
                            Some(wasmtime::Extern::Memory(m)) => m,
                            _ => return,
                        };
                        let data = mem.data(&caller);
                        let kp = key_ptr as usize;
                        let kl = key_len as usize;
                        let vp = val_ptr as usize;
                        let vl = val_len as usize;
                        if kp + kl > data.len() || vp + vl > data.len() {
                            return;
                        }
                        let key = String::from_utf8_lossy(&data[kp..kp + kl]).to_string();
                        let val = data[vp..vp + vl].to_vec();
                        caller.data().lock().unwrap().mutations.push((key, val));
                    },
                )
                .map_err(|e| CoreError::Storage(format!("linker set_state: {e}")))?;
        }

        // Host function: get_state_len(key_ptr, key_len) -> i32 (value length, -1 if not found)
        {
            let hs_clone = Arc::clone(&host_state);
            linker
                .func_wrap(
                    "env",
                    "get_state_len",
                    move |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                          key_ptr: i32,
                          key_len: i32|
                          -> i32 {
                        let mem = match caller.get_export("memory") {
                            Some(wasmtime::Extern::Memory(m)) => m,
                            _ => return -1,
                        };
                        let data = mem.data(&caller);
                        let kp = key_ptr as usize;
                        let kl = key_len as usize;
                        if kp + kl > data.len() {
                            return -1;
                        }
                        let key = String::from_utf8_lossy(&data[kp..kp + kl]).to_string();
                        match hs_clone.lock().unwrap().world_state.get(&key) {
                            Some(v) => v.len() as i32,
                            None => -1,
                        }
                    },
                )
                .map_err(|e| CoreError::Storage(format!("linker get_state_len: {e}")))?;
        }

        let mut store = Store::new(&self.engine, Arc::clone(&host_state));
        store
            .set_fuel(gas_limit)
            .map_err(|e| CoreError::Storage(format!("set_fuel: {e}")))?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| CoreError::Storage(format!("wasm instantiate [{contract_id}]: {e}")))?;

        let execute_fn = instance
            .get_typed_func::<(), ()>(&mut store, "execute")
            .map_err(|e| {
                CoreError::Storage(format!("wasm export 'execute' [{contract_id}]: {e}"))
            })?;

        match execute_fn.call(&mut store, ()) {
            Ok(_) => {}
            Err(e) => {
                // Check if this is a fuel-exhaustion trap via wasmtime's
                // Trap enum (most reliable) or by checking remaining fuel.
                let is_fuel_exhaustion = e
                    .downcast_ref::<wasmtime::Trap>()
                    .map(|t| *t == wasmtime::Trap::OutOfFuel)
                    .unwrap_or(false)
                    || store.get_fuel().unwrap_or(1) == 0;
                if is_fuel_exhaustion {
                    let used = gas_limit
                        .saturating_sub(store.get_fuel().unwrap_or(0));
                    return Err(CoreError::GasExhausted {
                        used,
                        limit: gas_limit,
                    });
                }
                return Err(CoreError::Storage(format!(
                    "wasm execution [{contract_id}]: {e}"
                )));
            }
        }

        let mutations = host_state.lock().unwrap().mutations.clone();
        log::debug!(
            "WasmExecutionProvider: contract={contract_id} mutations={} fuel_remaining={}",
            mutations.len(),
            store.get_fuel().unwrap_or(0)
        );
        Ok(mutations)
    }

    fn name(&self) -> &str {
        "wasmtime"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_core::ExecutionProvider;

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

    #[test]
    fn test_hello_world_contract() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(hello_world_wat());
        let mutations = provider.execute("hello-contract", &wasm, 10_000).unwrap();
        assert_eq!(mutations.len(), 1);
        assert_eq!(mutations[0].0, "hello");
        assert_eq!(mutations[0].1, b"world");
    }

    #[test]
    fn test_gas_exhaustion() {
        let provider = WasmExecutionProvider::new().unwrap();
        let wasm = compile_wat(infinite_loop_wat());
        let result = provider.execute("loop-contract", &wasm, 100);
        assert!(
            matches!(result, Err(CoreError::GasExhausted { .. })),
            "expected GasExhausted, got {:?}",
            result
        );
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
        let mutations = provider.execute("noop-contract", &wasm, 10_000).unwrap();
        assert!(mutations.is_empty());
    }
}
