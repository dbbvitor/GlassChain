//! Shared test fixtures: WASM gate modules used by engine and watcher tests.
//!
//! Both automation paths approve a purchase only when the executed module
//! writes `approve = "1"`, so the approving/denying modules live here once
//! instead of being copy-pasted per test module.

use base64::Engine as _;

/// A base64-encoded WASM module that writes `approve = "1"`.
#[must_use]
pub fn approving_wasm_b64() -> String {
    compile(
        r#"
(module
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "approve")
  (data (i32.const 7) "1")
  (func (export "execute")
    (call $set_state (i32.const 0) (i32.const 7) (i32.const 7) (i32.const 1))
  )
)
"#,
    )
}

/// A base64-encoded WASM module that writes `approve = "0"` — denying.
#[must_use]
pub fn denying_wasm_b64() -> String {
    compile(
        r#"
(module
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "approve")
  (data (i32.const 7) "0")
  (func (export "execute")
    (call $set_state (i32.const 0) (i32.const 7) (i32.const 7) (i32.const 1))
  )
)
"#,
    )
}

fn compile(wat: &str) -> String {
    let wasm = wat::parse_str(wat).expect("fixture WAT must compile");
    base64::engine::general_purpose::STANDARD.encode(&wasm)
}
