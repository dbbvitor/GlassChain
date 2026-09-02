//! Performance benchmarks for the `GlassChain` `WatcherService` (Phase 4 ECA engine).
//!
//! Run with:
//! ```text
//! cargo bench -p glasschain-workflows
//! ```
//!
//! Target: handle 1,000+ autonomous inventory triggers per second.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use glasschain_core::InventoryUpdate;
use glasschain_workflows::watcher::{InventoryTrigger, WatcherService};
use std::hint::black_box;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_trigger(id: &str, product: &str, owner: &str, threshold: i64) -> InventoryTrigger {
    InventoryTrigger {
        trigger_id: id.into(),
        product_id: product.into(),
        owner_id: owner.into(),
        reorder_threshold: threshold,
        reorder_quantity: 100,
        seller_id: "supplier-bench".into(),
        price_per_unit: 1000,
        currency: "BRL".into(),
        active: true,
        ..Default::default() // covers wasm_code_b64: None
    }
}

fn inv_update(product: &str, owner: &str, delta: i64) -> InventoryUpdate {
    InventoryUpdate {
        product_id: product.into(),
        owner_id: owner.into(),
        quantity_delta: delta,
        reason: "bench".into(),
    }
}

// ── Benchmarks ────────────────────────────────────────────────────────────────

/// Baseline: a single trigger fires on one inventory update.
fn bench_single_trigger_fire(c: &mut Criterion) {
    c.bench_function("watcher_single_trigger_fire", |b| {
        b.iter_batched(
            || {
                let mut svc = WatcherService::new();
                svc.add_trigger(make_trigger("t1", "P-001", "owner", 0));
                svc
            },
            |mut svc| {
                let orders = svc.on_inventory_update(black_box(&inv_update("P-001", "owner", -1)));
                black_box(orders);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

/// 10 triggers share the same product; a single large drawdown fires all of them.
fn bench_10_triggers_one_product(c: &mut Criterion) {
    c.bench_function("watcher_10_triggers_one_product", |b| {
        b.iter_batched(
            || {
                let mut svc = WatcherService::new();
                for i in 0..10_i64 {
                    svc.add_trigger(make_trigger(&format!("t{i}"), "P-001", "owner", i * 10));
                }
                svc
            },
            |mut svc| {
                // Drop inventory well below all thresholds
                let orders =
                    svc.on_inventory_update(black_box(&inv_update("P-001", "owner", -200)));
                black_box(orders);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

/// Throughput goal: 1 000 independent triggers, one per product, all fire.
fn bench_1000_triggers_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("watcher_throughput");
    group.throughput(Throughput::Elements(1_000));

    group.bench_function("1000_independent_triggers", |b| {
        b.iter_batched(
            || {
                let mut svc = WatcherService::new();
                for i in 0u64..1_000 {
                    svc.add_trigger(make_trigger(
                        &format!("t{i}"),
                        &format!("PROD-{i:04}"),
                        "warehouse",
                        5,
                    ));
                }
                svc
            },
            |mut svc| {
                // Each product drops below its threshold → all 1 000 triggers fire
                for i in 0u64..1_000 {
                    let product = format!("PROD-{i:04}");
                    let orders =
                        svc.on_inventory_update(black_box(&inv_update(&product, "warehouse", -10)));
                    black_box(orders);
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Hot path: 10 000 updates that intentionally stay *above* the threshold so
/// no trigger ever fires.  Measures the pure ECA evaluation overhead.
fn bench_high_frequency_updates_no_fire(c: &mut Criterion) {
    let mut group = c.benchmark_group("watcher_no_fire");
    group.throughput(Throughput::Elements(10_000));

    group.bench_function("10000_updates_above_threshold", |b| {
        b.iter_batched(
            || {
                let mut svc = WatcherService::new();
                // Threshold is -1 000; positive increments will never reach it
                svc.add_trigger(make_trigger("t1", "P-001", "owner", -1_000));
                svc
            },
            |mut svc| {
                for _ in 0..10_000 {
                    let orders =
                        svc.on_inventory_update(black_box(&inv_update("P-001", "owner", 1)));
                    black_box(orders);
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Serialization: JSON-encode the full runtime state after 1 000 products have
/// accumulated inventory.  Exercises [`WatcherService::serialize_state`].
fn bench_serialize_state(c: &mut Criterion) {
    c.bench_function("watcher_serialize_state_1000_products", |b| {
        b.iter_batched(
            || {
                let mut svc = WatcherService::new();
                for i in 0u64..1_000 {
                    svc.on_inventory_update(&inv_update(&format!("P-{i:04}"), "owner", 100));
                }
                svc
            },
            |svc| {
                let bytes = svc.serialize_state().unwrap();
                black_box(bytes);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

// ── Registration ──────────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_single_trigger_fire,
    bench_10_triggers_one_product,
    bench_1000_triggers_throughput,
    bench_high_frequency_updates_no_fire,
    bench_serialize_state,
);
criterion_main!(benches);
