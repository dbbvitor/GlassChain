//! Read-path memory and ingestion-cost scenario (performance plan §5).
//!
//! Measures the retained read-path projections — `InMemoryIndexer`,
//! `AnalyticalFlattener`, `ProvenanceIndex` — under growing registration
//! histories, plus ingestion latency, query latency and rebuild time.
//! Ignored by default; run in release:
//!
//! ```text
//! cargo test -p glasschain-indexer --release --test read_path_memory -- --ignored --nocapture
//! ```

use glasschain_core::asset::TraceableAsset;
use glasschain_core::{Block, TraceableAssetRegistration, Transaction, TransactionKind};
use glasschain_indexer::{AnalyticalFlattener, InMemoryIndexer, IndexerProvider, ProvenanceIndex};
use std::collections::BTreeMap;

/// Explicit memory budget: ingestion stops when retained RSS growth exceeds
/// it (perf plan §5 — "stopping at an explicit memory budget").
const MEMORY_BUDGET_MIB: u64 = 512;

const REGISTRATIONS_PER_BLOCK: usize = 8;

fn registration_tx(index: usize) -> Transaction {
    let asset = TraceableAsset {
        gtin: Some(format!("GTIN-{:013}", 789_123_410_000usize + index)),
        batch_number: Some(format!("BATCH-{index:06}")),
        expiry_date: Some("2027-01-01".to_owned()),
        serial_number: Some(format!("SN-{index:010}")),
        anvisa_registration: Some("MS 1234567.890123".to_owned()),
        manufacturer_id: Some("CNPJ-42".to_owned()),
        product_name: "Drug A".to_owned(),
        custodian_id: "maker-1".to_owned(),
        country_of_origin: Some("BR".to_owned()),
        storage_temp_celsius: Some("2-8".to_owned()),
        quantity: 100,
    };
    Transaction::new(TransactionKind::AssetRegistration(
        TraceableAssetRegistration {
            asset,
            event_type: "manufacture".to_owned(),
            originator_id: "maker-1".to_owned(),
            purchase_order_ref: None,
        },
    ))
}

/// `n` registration transactions across blocks of 8, unchained (the read
/// projections do not verify chaining).
fn synthetic_chain(n: usize) -> Vec<Block> {
    let mut blocks = Vec::with_capacity(n.div_ceil(REGISTRATIONS_PER_BLOCK));
    let mut i = 0usize;
    while i < n {
        let take = (n - i).min(REGISTRATIONS_PER_BLOCK);
        let txs: Vec<Transaction> = (0..take)
            .map(|offset| {
                let mut tx = registration_tx(i + offset);
                tx.id = format!("reg-tx-{}", i + offset);
                tx
            })
            .collect();
        let block = Block::new(
            u64::try_from(blocks.len() + 1).expect("test heights fit u64"),
            txs,
            format!("prev-{}", blocks.len()),
        );
        blocks.push(block);
        i += take;
    }
    blocks
}

/// Read VmRSS/VmHWM (kiB) from /proc/self/status. Linux only; returns
/// `(rss_kib, hwm_kib)` or `None` elsewhere.
fn rss_snapshot() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let mut rss = None;
    let mut hwm = None;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss = rest.split_whitespace().next()?.parse().ok();
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            hwm = rest.split_whitespace().next()?.parse().ok();
        }
    }
    Some((rss?, hwm?))
}

const fn kib_to_mib(kib: u64) -> u64 {
    kib / 1024
}

/// One scenario: ingest `n` registrations through the three projections,
/// measure retained memory, ingestion latency, query latency and rebuild time.
fn scenario(n: usize) {
    let (rss0, hwm0) = rss_snapshot().expect("linux-only measurement");
    let chain = synthetic_chain(n);
    let indexer = InMemoryIndexer::new();
    let mut flattener = AnalyticalFlattener::new();
    let mut provenance = ProvenanceIndex::new();

    let mut ingest_ms: Vec<u128> = Vec::with_capacity(chain.len());
    let mut payload_bytes_total = 0usize;
    let start = std::time::Instant::now();
    for block in &chain {
        let block_start = std::time::Instant::now();
        indexer
            .index_block(block)
            .expect("synthetic blocks must index");
        let indexed_txs = glasschain_indexer::indexed_transactions_of(block)
            .expect("synthetic transactions must serialize");
        let indexed_block = glasschain_indexer::IndexedBlock::from(block);
        flattener.ingest_indexed_block(&indexed_block, &indexed_txs);
        provenance.ingest_block(block);
        payload_bytes_total += indexed_txs
            .iter()
            .map(|tx| tx.payload_json.len())
            .sum::<usize>();
        ingest_ms.push(block_start.elapsed().as_millis());
    }
    let ingest_total = start.elapsed();
    let avg_payload_bytes = payload_bytes_total / n.max(1);

    let (rss1, hwm1) = rss_snapshot().expect("linux-only measurement");
    let growth = kib_to_mib(rss1.saturating_sub(rss0));
    let peak_growth = kib_to_mib(hwm1.saturating_sub(hwm0));

    // Queries (one hit + one full scan each, representative of the handlers).
    let q_start = std::time::Instant::now();
    let gtin = format!("GTIN-{:013}", 789_123_410_000u64 + 3);
    let hits = flattener.records_by_gtin(&gtin);
    let total = flattener.total_quantity_for_gtin(&gtin);
    let chain_events = provenance
        .get_custody_chain(&format!(
            "GTIN:GTIN-{:013}:SN:SN-{:010}",
            789_123_410_000u64 + 3,
            3
        ))
        .len();
    let csv = glasschain_indexer::AnalyticalFlattener::to_csv_row(
        flattener.records().first().expect("rows exist"),
    );
    let query_us = q_start.elapsed().as_micros();

    // Rebuild: replay the whole committed history into a fresh flattener.
    let rebuild_start = std::time::Instant::now();
    let mut rebuilt = AnalyticalFlattener::new();
    for block in &chain {
        let replay_txs = glasschain_indexer::indexed_transactions_of(block)
            .expect("synthetic transactions must serialize");
        rebuilt.ingest_indexed_block(&glasschain_indexer::IndexedBlock::from(block), &replay_txs);
    }
    let replay_time = rebuild_start.elapsed();

    ingest_ms.sort_unstable();
    let p50 = ingest_ms.get(ingest_ms.len() / 2).copied().unwrap_or(0);
    let p95 = ingest_ms
        .get(ingest_ms.len() * 95 / 100)
        .copied()
        .unwrap_or(0);
    let bytes_per_row = avg_payload_bytes;
    let struct_bytes = std::mem::size_of::<glasschain_indexer::FlatAssetRecord>();
    println!("read-path scenario n={n}");
    println!(
        "  retained: flattener rows={}, custody assets={}, indexer blocks={}",
        flattener.records().len(),
        provenance.tracked_assets().len(),
        n.div_ceil(REGISTRATIONS_PER_BLOCK),
    );
    println!(
        "  rss growth={growth} MiB, peak growth={peak_growth} MiB, budget={MEMORY_BUDGET_MIB} MiB"
    );
    println!("  ingestion total={ingest_total:?}, p50={p50} ms/block, p95={p95} ms/block");
    println!(
        "  query hit={} rows, qty={} units, custody={} events, csv_len={len}, latency={query_us} µs",
        hits.len(),
        total,
        chain_events,
        len = csv.len(),
    );
    println!("  rebuild (fresh flattener, same chain)={replay_time:?}");
    println!("  approx: struct/row={struct_bytes} B, avg payload={bytes_per_row} B");
}

#[test]
#[ignore = "memory/latency measurement, not a pass/fail gate"]
fn read_path_memory_scenario() {
    for n in [1_000, 10_000, 100_000] {
        let (rss_before, _) = rss_snapshot().expect("linux-only measurement");
        scenario(n);
        let (rss_after, _) = rss_snapshot().expect("linux-only measurement");
        // Stop at the explicit budget rather than risking the host.
        if kib_to_mib(rss_after.saturating_sub(rss_before)) > MEMORY_BUDGET_MIB {
            println!(
                "read-path scenario stopped after n={n}: budget {MEMORY_BUDGET_MIB} MiB exceeded"
            );
            break;
        }
        // Small allocation churn nudges the allocator between scenarios so
        // the next scenario's growth is not masked by retained pages.
        drop(chain_holder());
    }
}

/// A tiny allocation churn to nudge the allocator between scenarios.
fn chain_holder() -> BTreeMap<u64, Vec<u8>> {
    (0..1_000u64).map(|i| (i, vec![0u8; 1_024])).collect()
}
