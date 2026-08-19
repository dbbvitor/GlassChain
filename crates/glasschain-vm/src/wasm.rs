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

    // The host import registrations are kept together so their ABI stays visible.
    #[allow(clippy::too_many_lines)]
    fn build_linker(
        &self,
        host_state: &Arc<Mutex<HostState>>,
    ) -> Result<Linker<Arc<Mutex<HostState>>>, CoreError> {
        let mut linker: Linker<Arc<Mutex<HostState>>> = Linker::new(&self.engine);

        linker
            .func_wrap(
                "env",
                "set_state",
                |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                 key_ptr: i32,
                 key_len: i32,
                 val_ptr: i32,
                 val_len: i32| {
                    let Some(wasmtime::Extern::Memory(mem)) = caller.get_export("memory") else {
                        return;
                    };
                    let data = mem.data(&caller);
                    let kp = usize::try_from(key_ptr).unwrap_or(usize::MAX);
                    let kl = usize::try_from(key_len).unwrap_or(usize::MAX);
                    let vp = usize::try_from(val_ptr).unwrap_or(usize::MAX);
                    let vl = usize::try_from(val_len).unwrap_or(usize::MAX);
                    if kp.checked_add(kl).is_none_or(|end| end > data.len())
                        || vp.checked_add(vl).is_none_or(|end| end > data.len())
                    {
                        return;
                    }
                    let key = String::from_utf8_lossy(&data[kp..kp + kl]).to_string();
                    let val = data[vp..vp + vl].to_vec();
                    caller.data().lock().unwrap().mutations.push((key, val));
                },
            )
            .map_err(|e| CoreError::Execution(format!("linker set_state: {e}")))?;

        {
            let hs_clone = Arc::clone(host_state);
            linker
                .func_wrap(
                    "env",
                    "get_state_len",
                    move |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                          key_ptr: i32,
                          key_len: i32|
                          -> i32 {
                        let Some(wasmtime::Extern::Memory(mem)) = caller.get_export("memory")
                        else {
                            return -1;
                        };
                        let data = mem.data(&caller);
                        let kp = usize::try_from(key_ptr).unwrap_or(usize::MAX);
                        let kl = usize::try_from(key_len).unwrap_or(usize::MAX);
                        if kp.checked_add(kl).is_none_or(|end| end > data.len()) {
                            return -1;
                        }
                        let key = String::from_utf8_lossy(&data[kp..kp + kl]).to_string();
                        hs_clone
                            .lock()
                            .unwrap()
                            .world_state
                            .get(&key)
                            .map_or(-1, |v| i32::try_from(v.len()).unwrap_or(-1))
                    },
                )
                .map_err(|e| CoreError::Execution(format!("linker get_state_len: {e}")))?;
        }

        {
            let hs_clone_get = Arc::clone(host_state);
            linker
                .func_wrap(
                    "env",
                    "get_state",
                    move |mut caller: wasmtime::Caller<'_, Arc<Mutex<HostState>>>,
                          key_ptr: i32,
                          key_len: i32,
                          val_ptr: i32,
                          val_buf_len: i32|
                          -> i32 {
                        let Some(wasmtime::Extern::Memory(mem)) = caller.get_export("memory")
                        else {
                            return -1;
                        };

                        let kp = usize::try_from(key_ptr).unwrap_or(usize::MAX);
                        let kl = usize::try_from(key_len).unwrap_or(usize::MAX);
                        let vp = usize::try_from(val_ptr).unwrap_or(usize::MAX);
                        let vbl = usize::try_from(val_buf_len).unwrap_or(usize::MAX);

                        let key = {
                            let data = mem.data(&caller);
                            if kp.checked_add(kl).is_none_or(|end| end > data.len()) {
                                return -1;
                            }
                            String::from_utf8_lossy(&data[kp..kp + kl]).to_string()
                        };

                        let value = {
                            let hs = hs_clone_get.lock().unwrap();
                            match hs.world_state.get(&key) {
                                Some(v) => v.clone(),
                                None => return -1,
                            }
                        };

                        if value.len() > vbl {
                            return -2;
                        }

                        let data_mut = mem.data_mut(&mut caller);
                        if vp
                            .checked_add(value.len())
                            .is_none_or(|end| end > data_mut.len())
                        {
                            return -1;
                        }
                        data_mut[vp..vp + value.len()].copy_from_slice(&value);
                        i32::try_from(value.len()).unwrap_or(-1)
                    },
                )
                .map_err(|e| CoreError::Execution(format!("linker get_state: {e}")))?;
        }

        Ok(linker)
    }

    fn execute_internal(
        &self,
        contract_id: &str,
        payload: &[u8],
        initial_state: HashMap<String, Vec<u8>>,
        gas_limit: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
        let module = Module::new(&self.engine, payload)
            .map_err(|e| CoreError::Execution(format!("wasm compile [{contract_id}]: {e}")))?;

        let host_state = Arc::new(Mutex::new(HostState {
            world_state: initial_state,
            mutations: Vec::new(),
        }));
        let linker = self.build_linker(&host_state)?;

        let mut store = Store::new(&self.engine, Arc::clone(&host_state));
        store
            .set_fuel(gas_limit)
            .map_err(|e| CoreError::Execution(format!("set_fuel: {e}")))?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| CoreError::Execution(format!("wasm instantiate [{contract_id}]: {e}")))?;

        let execute_fn = instance
            .get_typed_func::<(), ()>(&mut store, "execute")
            .map_err(|e| {
                CoreError::Execution(format!("wasm export 'execute' [{contract_id}]: {e}"))
            })?;

        match execute_fn.call(&mut store, ()) {
            Ok(()) => {}
            Err(e) => {
                let is_fuel_exhaustion = e
                    .downcast_ref::<wasmtime::Trap>()
                    .is_some_and(|t| *t == wasmtime::Trap::OutOfFuel)
                    || store.get_fuel().unwrap_or(1) == 0;
                if is_fuel_exhaustion {
                    let used = gas_limit.saturating_sub(store.get_fuel().unwrap_or(0));
                    return Err(CoreError::GasExhausted {
                        used,
                        limit: gas_limit,
                    });
                }
                return Err(CoreError::Execution(format!(
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
    /// - `env::get_state_len(key_ptr: i32, key_len: i32) -> i32`
    ///   — return the byte length of a stored value, or `-1` if the key is absent.
    /// - `env::get_state(key_ptr: i32, key_len: i32, val_ptr: i32, val_buf_len: i32) -> i32`
    ///   — copy a stored value into the contract's linear memory; returns the
    ///   number of bytes written, `-1` if the key is missing or if the key
    ///   pointer is out of bounds, or `-2` if the value is larger than the
    ///   supplied buffer.
    #[allow(clippy::too_many_lines)]
    fn execute(
        &self,
        contract_id: &str,
        payload: &[u8],
        gas_limit: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
        self.execute_internal(contract_id, payload, HashMap::new(), gas_limit)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_with_state(
        &self,
        contract_id: &str,
        payload: &[u8],
        initial_state: std::collections::HashMap<String, Vec<u8>>,
        gas_limit: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
        self.execute_internal(contract_id, payload, initial_state, gas_limit)
    }

    fn name(&self) -> &'static str {
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
            "expected GasExhausted, got {result:?}"
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
        let result = provider.execute("get-state-test", &wasm, 10_000);
        assert!(result.is_ok(), "expected Ok(mutations), got {result:?}");
        // No mutations were written, so the mutations list must be empty.
        assert!(result.unwrap().is_empty());
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

        let mutations = provider
            .execute_with_state("state-test", &wasm, initial, 50_000)
            .unwrap();

        assert_eq!(mutations.len(), 1);
        assert_eq!(mutations[0].0, "pong_len");
        assert_eq!(mutations[0].1, b"found");
    }
}
