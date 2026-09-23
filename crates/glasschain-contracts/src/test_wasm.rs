// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
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

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(b64: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("fixture must be valid base64")
    }

    /// Both fixtures must be real, distinct WASM modules: an empty or garbage
    /// string must not pass for a module that the VM can execute.
    #[test]
    fn fixtures_are_distinct_valid_wasm_modules() {
        let approving = decode(&approving_wasm_b64());
        let denying = decode(&denying_wasm_b64());
        assert_eq!(&approving[..4], b"\0asm");
        assert_eq!(&denying[..4], b"\0asm");
        assert_ne!(approving, denying);
    }
}
