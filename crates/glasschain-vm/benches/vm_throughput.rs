//! Performance benchmarks for the `GlassChain` WASM execution engine.
//!
//! Run with:
//! ```text
//! cargo bench -p glasschain-vm
//! ```
//!
//! The benchmarks target the plan goal of handling **1,000+ autonomous
//! inventory triggers per second**.  Each benchmark measures a different
//! cost centre in the execution pipeline.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use glasschain_core::ExecutionProvider;
use glasschain_vm::WasmExecutionProvider;
use std::collections::HashMap;
use wat::parse_str;

// ── WAT fixtures ─────────────────────────────────────────────────────────────

/// Minimal WASM: sets approve=1 and returns immediately.
const APPROVE_WAT: &str = r#"(module
  (import "env" "set_state" (func $set (param i32 i32 i32 i32)))
  (export "execute" (func $run))
  (export "memory" (memory 0))
  (memory 1)
  (data (i32.const 0) "approve")
  (data (i32.const 16) "1")
  (func $run
    i32.const 0 i32.const 7
    i32.const 16 i32.const 1
    call $set
  )
)"#;

/// State-intensive WASM: reads and writes state multiple times.
const STATE_INTENSIVE_WAT: &str = r#"(module
  (import "env" "set_state" (func $set (param i32 i32 i32 i32)))
  (export "execute" (func $run))
  (export "memory" (memory 0))
  (memory 1)
  (data (i32.const 0) "k1") (data (i32.const 8) "v1")
  (data (i32.const 16) "k2") (data (i32.const 24) "v2")
  (data (i32.const 32) "k3") (data (i32.const 40) "v3")
  (func $run
    i32.const 0 i32.const 2 i32.const 8 i32.const 2 call $set
    i32.const 16 i32.const 2 i32.const 24 i32.const 2 call $set
    i32.const 32 i32.const 2 i32.const 40 i32.const 2 call $set
  )
)"#;

/// Compute-intensive WASM: tight loop that uses most of the gas budget.
const COMPUTE_LOOP_WAT: &str = r#"(module
  (export "execute" (func $run))
  (export "memory" (memory 0))
  (memory 1)
  (func $run (local $i i32)
    i32.const 0
    local.set $i
    block $break
      loop $loop
        local.get $i
        i32.const 500
        i32.ge_u
        br_if $break
        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $loop
      end
    end
  )
)"#;

// ── Benchmarks ───────────────────────────────────────────────────────────────

fn bench_single_noop_execution(c: &mut Criterion) {
    let executor = WasmExecutionProvider::new().expect("vm init failed");
    let wasm = parse_str(APPROVE_WAT).expect("WAT compile failed");

    c.bench_function("wasm_single_approve_execution", |b| {
        b.iter(|| {
            let result = executor.execute(
                black_box("approve-contract"),
                black_box(&wasm),
                black_box(100_000),
            );
            black_box(result.unwrap());
        });
    });
}

fn bench_state_write_execution(c: &mut Criterion) {
    let executor = WasmExecutionProvider::new().expect("vm init failed");
    let wasm = parse_str(STATE_INTENSIVE_WAT).expect("WAT compile failed");

    c.bench_function("wasm_3state_write_execution", |b| {
        b.iter(|| {
            let result = executor.execute(
                black_box("state-contract"),
                black_box(&wasm),
                black_box(100_000),
            );
            black_box(result.unwrap());
        });
    });
}

fn bench_compute_loop_execution(c: &mut Criterion) {
    let executor = WasmExecutionProvider::new().expect("vm init failed");
    let wasm = parse_str(COMPUTE_LOOP_WAT).expect("WAT compile failed");

    c.bench_function("wasm_compute_loop_500iters", |b| {
        b.iter(|| {
            let result = executor.execute(
                black_box("compute-contract"),
                black_box(&wasm),
                black_box(1_000_000),
            );
            black_box(result.unwrap());
        });
    });
}

fn bench_throughput_1000_invocations(c: &mut Criterion) {
    let executor = WasmExecutionProvider::new().expect("vm init failed");
    let wasm = parse_str(APPROVE_WAT).expect("WAT compile failed");

    let mut group = c.benchmark_group("wasm_throughput");
    group.throughput(Throughput::Elements(1_000));

    group.bench_function("1000_sequential_invocations", |b| {
        b.iter(|| {
            for i in 0u64..1_000 {
                let contract_id = format!("contract-{i}");
                let result = executor.execute(
                    black_box(contract_id.as_str()),
                    black_box(&wasm),
                    black_box(50_000),
                );
                black_box(result.unwrap());
            }
        });
    });

    group.finish();
}

fn bench_execute_with_state(c: &mut Criterion) {
    let executor = WasmExecutionProvider::new().expect("vm init failed");
    let wasm = parse_str(APPROVE_WAT).expect("WAT compile failed");

    let world_state: HashMap<String, Vec<u8>> = [
        ("inventory_level".to_string(), b"-10".to_vec()),
        ("threshold".to_string(), b"5".to_vec()),
        ("product_id".to_string(), b"PROD-001".to_vec()),
    ]
    .into_iter()
    .collect();

    c.bench_function("wasm_execute_with_state_context", |b| {
        b.iter(|| {
            let result = executor.execute_with_state(
                black_box("stateful-contract"),
                black_box(&wasm),
                black_box(world_state.clone()),
                black_box(100_000),
            );
            black_box(result.unwrap());
        });
    });
}

// ── Registration ─────────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_single_noop_execution,
    bench_state_write_execution,
    bench_compute_loop_execution,
    bench_throughput_1000_invocations,
    bench_execute_with_state,
);
criterion_main!(benches);
