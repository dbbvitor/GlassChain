//! D3 admission-cost benchmark ([source-comment debt plan], performance Step 3
//! prerequisite).
//!
//! Measures the cost shape of [`Ledger::add_transaction`]: the per-admission
//! capability-history rebuild over the committed chain (the `ponytail:` marker
//! at `ledger.rs::add_transaction`) and the committed + pending idempotency
//! scan, as both grow with history and pool size.
//!
//! Run with:
//! ```text
//! cargo bench -p glasschain-core --bench ledger_admission
//! ```
//!
//! Baseline numbers for the optimization decision; no optimization ships with
//! this harness.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use glasschain_core::{CanonicalRecord, Ledger, RecordSignature, Transaction, TransactionKind};
use std::collections::BTreeMap;
use std::hint::black_box;

/// A validation-passing, signed `party_identity` canonical record.
fn signed_record() -> CanonicalRecord {
    let payload: BTreeMap<String, serde_json::Value> = serde_json::from_value(serde_json::json!({
        "org_id": "cooperative-x",
        "legal_name": "Cooperative X",
    }))
    .expect("bench payload");
    let mut record = CanonicalRecord::new(0, "party_identity", payload, "org-issuer");
    record.signatures.push(RecordSignature {
        algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
        signer: record.issuer.clone(),
        signature_bytes: vec![0x42; 8],
    });
    record
}

/// Admit `n` record transactions through the real admission path, mining one
/// block per record — the history the next admission must rebuild and scan.
fn ledger_with_history(n: usize) -> Ledger {
    let mut ledger = Ledger::new(1);
    for i in 0..n {
        let mut record = signed_record();
        record.record_id = format!("history-record-{i}");
        ledger
            .add_transaction(Transaction::new(TransactionKind::CanonicalRecord(record)))
            .expect("bench history record must admit");
        ledger.mine_pending_transactions().expect("bench mining");
    }
    ledger
}

/// One admission burst against a fixed history; the pending pool is cleared
/// per burst so the measured scan stays history-dominated.
const BURST: usize = 64;
const POOL_CAP: usize = 1_024;

fn bench_history_size(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    label: &str,
    history: usize,
    burst: usize,
    duplicates: bool,
) {
    let mut ledger = ledger_with_history(history);
    let duplicate_id = "history-record-0".to_owned();
    let mut counter = 0usize;
    group.bench_function(label, |b| {
        b.iter(|| {
            for _ in 0..burst {
                counter += 1;
                let mut record = signed_record();
                let id = if duplicates {
                    duplicate_id.clone()
                } else {
                    format!("bench-record-{counter}")
                };
                record.record_id = id;
                let result = ledger
                    .add_transaction(Transaction::new(TransactionKind::CanonicalRecord(record)))
                    .is_ok();
                black_box(result);
            }
            ledger.pending_transactions.clear();
        });
    });
}

fn admission_history_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("ledger_admission/records");
    group.throughput(Throughput::Elements(BURST as u64));
    for history in [100, 1_000, 10_000] {
        bench_history_size(
            &mut group,
            format!("fresh/history_{history}").as_str(),
            history,
            BURST,
            false,
        );
        bench_history_size(
            &mut group,
            format!("duplicate/history_{history}").as_str(),
            history,
            BURST,
            true,
        );
    }
    group.finish();
}

fn pending_pool_scan(c: &mut Criterion) {
    // Pool growth: same history, an accumulating pending pool — the second
    // scan arm of the idempotency check. The pool grows across bursts up to a
    // cap, then restarts, so each burst's scan includes the pool cost.
    let mut group = c.benchmark_group("ledger_admission/pool");
    group.throughput(Throughput::Elements(BURST as u64));
    let mut ledger = ledger_with_history(1_000);
    let mut counter = 0usize;
    group.bench_function("growing_pending/history_1000", |b| {
        b.iter(|| {
            if ledger.pending_transactions.len() >= POOL_CAP {
                ledger.pending_transactions.clear();
            }
            for _ in 0..BURST {
                counter += 1;
                let mut record = signed_record();
                record.record_id = format!("bench-pool-{counter}");
                let result = ledger
                    .add_transaction(Transaction::new(TransactionKind::CanonicalRecord(record)))
                    .is_ok();
                black_box(result);
            }
        });
    });
}

fn criterion_config() -> Criterion {
    Criterion::default().sample_size(20)
}

criterion_group!(
    name = ledger_admission;
    config = criterion_config();
    targets = admission_history_scan, pending_pool_scan
);
criterion_main!(ledger_admission);
