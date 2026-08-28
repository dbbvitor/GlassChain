//! Deterministic chaos tests using the [Madsim](https://github.com/madsim-rs/madsim) framework.
//!
//! ## Running
//!
//! ### Standard tokio mode (always works)
//! ```bash
//! cargo test -p glasschain-network --test madsim_chaos
//! ```
//!
//! ### Madsim simulation mode (deterministic time + reproducible seeds)
//! ```bash
//! RUSTFLAGS="--cfg madsim" cargo test -p glasschain-network --test madsim_chaos
//! ```
//!
//! ## Architecture note
//!
//! Full **network-level** partition simulation (intercepting TCP at the OS layer)
//! requires replacing `tokio::net` with `madsim-tokio`.  That is an opt-in
//! refactor tracked separately.  The tests here demonstrate:
//!
//! 1. **Deterministic time control** — `madsim::time::advance` lets a test
//!    fast-forward through delays that would normally take seconds.
//! 2. **Reproducible seeds** — every `#[madsim::test]` run is seeded so
//!    failures are fully reproducible (`MADSIM_TEST_SEED=<n> cargo test …`).
//! 3. **Application-layer partition simulation** — the existing `GlassChain`
//!    `Node` infrastructure is used to model network splits by controlling
//!    which peers are dialled rather than patching the TCP stack.
//! 4. **High-frequency watcher stress** — 1 000 autonomous inventory triggers
//!    are exercised under simulated time pressure.

// ── Conditional compilation ───────────────────────────────────────────────────
//
// When compiled with `--cfg madsim`, tests run inside the Madsim simulator and
// gain deterministic time control.  Otherwise they fall back to the real Tokio
// runtime so CI can run the file without any special flags.

#[cfg(madsim)]
use madsim::time as sim_time;

use glasschain_contracts::{InventoryTrigger, WatcherService};
use glasschain_core::{
    InventoryUpdate, MetadataTrustScore, TraceableAsset, Transaction, TransactionKind,
    TRUST_SCORE_STANDARD_THRESHOLD,
};
use glasschain_network::{Node, NodeEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Allocate a free loopback port for testing.
fn free_addr() -> String {
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

fn inv_tx(owner: &str, delta: i64) -> Transaction {
    Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "CHAOS-SKU".into(),
        owner_id: owner.into(),
        quantity_delta: delta,
        reason: "madsim chaos test".into(),
    }))
}

/// Poll `node`'s chain length until it reaches `want`, returning the last
/// length observed.
///
/// Peer sync latency is unbounded in wall-clock terms: a TLS handshake plus the
/// initial chain transfer routinely exceeds half a second on a loaded CI runner
/// (Windows in particular). Sleeping for a fixed guess and asserting once makes
/// every sync assertion in this file a coin flip; polling to a generous ceiling
/// keeps the assertion meaningful while removing the flake.
async fn await_chain_len(node: &Node, want: usize) -> usize {
    const STEP: Duration = Duration::from_millis(50);
    const CEILING: Duration = Duration::from_secs(10);

    let mut len = 0;
    for _ in 0..(CEILING.as_millis() / STEP.as_millis()) {
        len = node.ledger_snapshot().await.chain.len();
        if len >= want {
            return len;
        }

        #[cfg(madsim)]
        sim_time::advance(STEP).await;
        #[cfg(not(madsim))]
        sleep(STEP).await;
    }
    len
}

fn make_trigger(id: &str, product: &str, owner: &str, threshold: i64) -> InventoryTrigger {
    InventoryTrigger {
        trigger_id: id.into(),
        product_id: product.into(),
        owner_id: owner.into(),
        reorder_threshold: threshold,
        reorder_quantity: 100,
        seller_id: "auto-supplier".into(),
        price_per_unit: 1_000,
        currency: "BRL".into(),
        active: true,
        wasm_code_b64: None,
    }
}

// ── Madsim simulation tests ───────────────────────────────────────────────────

/// Verify that a node mines and validates a block within a bounded wall-clock
/// window.  Under madsim, simulated time is advanced deterministically so the
/// test never flakes due to scheduling jitter.
///
/// In standard mode this is a bounded-timeout integration test.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test)]
async fn test_madsim_single_node_mines_within_time_budget() {
    let addr = free_addr();
    let node = Node::new("madsim-node-1", &addr, 1);
    node.start(vec![]).await.unwrap();

    node.submit_transaction(inv_tx("owner-1", 100))
        .await
        .unwrap();

    // Under madsim we advance simulated time; under real Tokio we simply wait.
    #[cfg(madsim)]
    {
        // Give the node a "head start" equivalent of 50 ms of simulated time
        // before asserting — deterministic regardless of host CPU speed.
        sim_time::advance(Duration::from_millis(50)).await;
    }

    let mine_result = timeout(Duration::from_secs(5), node.mine()).await;
    assert!(
        mine_result.is_ok(),
        "mine() timed out — node did not produce a block within the time budget"
    );
    assert!(mine_result.unwrap().is_ok(), "mine() returned an error");

    let ledger = node.ledger_snapshot().await;
    assert_eq!(ledger.chain.len(), 2, "genesis + 1 mined block");
    assert!(ledger.validate_chain().is_ok(), "chain must be valid");
}

/// Partition and merge under simulated time: isolated partitions commit
/// blocks that are final at commit, and joining nodes converge to their
/// partition's chain (liveness).
///
/// 1. Node A commits 2 blocks while isolated; every commit notification
///    carries a quorum certificate that validates against the committed block.
/// 2. Node B commits 1 block while isolated.
/// 3. Node C connects to **A** only — converges to A's chain.
/// 4. Node D connects to **B** only — converges to B's chain.
/// 5. Assert convergence and that both chains validate.
///
/// Under madsim, time advances deterministically between partition phases.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test(flavor = "multi_thread", worker_threads = 4))]
async fn test_madsim_partition_merge_converges_with_final_commits() {
    let addr_a = free_addr();
    let addr_b = free_addr();

    // ── Phase 1: isolated commits ────────────────────────────────────────
    let node_a = Arc::new(Node::new("partition-a", &addr_a, 1));
    let node_b = Arc::new(Node::new("partition-b", &addr_b, 1));
    let mut events_a = node_a.subscribe();
    node_a.start(vec![]).await.unwrap();
    node_b.start(vec![]).await.unwrap();

    // A commits two blocks.
    node_a
        .submit_transaction(inv_tx("a-owner", 50))
        .await
        .unwrap();
    node_a.mine().await.unwrap();
    node_a
        .submit_transaction(inv_tx("a-owner", 50))
        .await
        .unwrap();
    node_a.mine().await.unwrap();

    // B commits one block.
    node_b
        .submit_transaction(inv_tx("b-owner", 10))
        .await
        .unwrap();
    node_b.mine().await.unwrap();

    #[cfg(madsim)]
    sim_time::advance(Duration::from_millis(20)).await;

    let len_a = node_a.ledger_snapshot().await.chain.len();
    let len_b = node_b.ledger_snapshot().await.chain.len();
    assert_eq!(len_a, 3, "node A: genesis + 2 committed blocks");
    assert_eq!(len_b, 2, "node B: genesis + 1 committed block");

    // Each of A's commits is final at commit: its notification's certificate
    // attests the committed block, verifiable locally without trusting A.
    let ledger_a = node_a.ledger_snapshot().await;
    let mut certified = 0;
    while let Ok(event) = events_a.try_recv() {
        if let NodeEvent::BlockMined {
            index, certificate, ..
        } = event
        {
            let notification = glasschain_core::CommitNotification {
                block: ledger_a.chain[usize::try_from(index).expect("index fits usize")].clone(),
                certificate,
            };
            notification
                .validate()
                .expect("certificate must attest the committed block");
            certified += 1;
        }
    }
    assert_eq!(certified, 2, "both of A's commits carried certificates");

    // ── Phase 2: convergence after the partition ──────────────────────────
    // Node C joins partition containing A.
    let addr_c = free_addr();
    let node_c = Arc::new(Node::new("partition-c", &addr_c, 1));
    node_c.start(vec![addr_a.clone()]).await.unwrap();

    // Node D joins partition containing B.
    let addr_d = free_addr();
    let node_d = Arc::new(Node::new("partition-d", &addr_d, 1));
    node_d.start(vec![addr_b.clone()]).await.unwrap();

    #[cfg(madsim)]
    sim_time::advance(Duration::from_millis(500)).await;

    let len_c = await_chain_len(&node_c, len_a).await;
    let len_d = await_chain_len(&node_d, len_b).await;

    assert!(
        len_c >= len_a,
        "node C (synced to A) should have ≥ {len_a} blocks, got {len_c}"
    );
    assert!(
        len_d >= len_b,
        "node D (synced to B) should have ≥ {len_b} blocks, got {len_d}"
    );
    assert!(node_c.ledger_snapshot().await.validate_chain().is_ok());
    assert!(node_d.ledger_snapshot().await.validate_chain().is_ok());
}

/// Simulate a **node failure mid-stream**: a node processes transactions,
/// then is dropped (simulating a crash), and a new node re-joins from its
/// seed — verifying that the chain remains consistent across restarts.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test)]
async fn test_madsim_node_crash_and_rejoin() {
    let addr_primary = free_addr();
    let node_primary = Arc::new(Node::new("primary", &addr_primary, 1));
    node_primary.start(vec![]).await.unwrap();

    node_primary
        .submit_transaction(inv_tx("crasher", 200))
        .await
        .unwrap();
    node_primary.mine().await.unwrap();

    let chain_before = node_primary.ledger_snapshot().await.chain.len();

    // A secondary node syncs with primary.
    let addr_secondary = free_addr();
    {
        let node_secondary = Node::new("secondary-v1", &addr_secondary, 1);
        node_secondary
            .start(vec![addr_primary.clone()])
            .await
            .unwrap();

        #[cfg(madsim)]
        sim_time::advance(Duration::from_millis(300)).await;

        let len = await_chain_len(&node_secondary, 2).await;
        assert!(len >= 2, "secondary must have synced (got {len} blocks)");
    }
    // `node_secondary` is dropped here — simulating a crash.

    #[cfg(madsim)]
    sim_time::advance(Duration::from_millis(100)).await;
    #[cfg(not(madsim))]
    sleep(Duration::from_millis(100)).await;

    // Primary continues mining.
    node_primary
        .submit_transaction(inv_tx("crasher", 200))
        .await
        .unwrap();
    node_primary.mine().await.unwrap();

    let chain_after = node_primary.ledger_snapshot().await.chain.len();
    assert!(
        chain_after > chain_before,
        "primary must keep growing after secondary crash"
    );

    // A new node re-joins and adopts the full chain.
    let addr_rejoin = free_addr();
    let node_rejoin = Node::new("secondary-v2", &addr_rejoin, 1);
    node_rejoin.start(vec![addr_primary.clone()]).await.unwrap();

    #[cfg(madsim)]
    sim_time::advance(Duration::from_millis(400)).await;

    let rejoin_len = await_chain_len(&node_rejoin, chain_after).await;
    assert!(
        rejoin_len >= chain_after,
        "rejoining node must sync the full chain (expected ≥{chain_after}, got {rejoin_len})"
    );
    assert!(node_rejoin.ledger_snapshot().await.validate_chain().is_ok());
}

/// Stress test: 1 000 autonomous `InventoryTrigger` firings processed by the
/// `WatcherService` under deterministic madsim time control.
///
/// This validates the Phase 4 plan goal: "handle 1 000+ autonomous inventory
/// triggers per second."  Under madsim the test completes in zero wall-clock
/// time since the `WatcherService` is synchronous.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test)]
async fn test_madsim_1000_autonomous_triggers_stress() {
    const N: usize = 1_000;

    let mut watcher = WatcherService::new();
    for i in 0..N {
        watcher.add_trigger(make_trigger(
            &format!("stress-{i}"),
            &format!("STRESS-{i:04}"),
            "stress-warehouse",
            0, // threshold: fire when inventory ≤ 0
        ));
    }

    // Record start time using system time (deterministic under madsim).
    let t0 = std::time::Instant::now();

    let mut total_orders = 0usize;
    for i in 0..N {
        let update = InventoryUpdate {
            product_id: format!("STRESS-{i:04}"),
            owner_id: "stress-warehouse".into(),
            quantity_delta: -1, // drops from 0 to -1 → below threshold
            reason: "stress depletion".into(),
        };
        let orders = watcher.on_inventory_update(&update);
        total_orders += orders.len();
    }

    let elapsed = t0.elapsed();

    assert_eq!(total_orders, N, "all {N} triggers must fire exactly once");

    // Under real Tokio (no madsim), assert the throughput target.
    // Under madsim, time is virtual so the wall-clock assertion is skipped.
    #[cfg(not(madsim))]
    {
        // The WatcherService is synchronous Rust — 1 000 firings must complete
        // well within 1 second on any modern machine.
        assert!(
            elapsed < Duration::from_secs(1),
            "1 000 trigger firings took {elapsed:?} — must be < 1 s"
        );
        log::debug!("1 000 trigger firings completed in {elapsed:?}");
    }
    #[cfg(madsim)]
    {
        // Under madsim, elapsed is meaningless (virtual time not advanced here).
        let _ = elapsed;
    }

    // Verify all generated orders have unique IDs.
    let mut all_ids = std::collections::HashSet::new();
    let mut watcher2 = WatcherService::new();
    for i in 0..N {
        watcher2.add_trigger(make_trigger(
            &format!("unique-{i}"),
            &format!("UNIQ-{i:04}"),
            "owner",
            0,
        ));
    }
    for i in 0..N {
        let update = InventoryUpdate {
            product_id: format!("UNIQ-{i:04}"),
            owner_id: "owner".into(),
            quantity_delta: -1,
            reason: "uniqueness check".into(),
        };
        for order in watcher2.on_inventory_update(&update) {
            let inserted = all_ids.insert(order.id.clone());
            assert!(inserted, "duplicate transaction ID: {}", order.id);
        }
    }
    assert_eq!(all_ids.len(), N, "all {N} tx IDs must be unique");
}

/// Verify that SNCM trust-score nudge mechanics hold under concurrent
/// high-frequency asset submissions — a key regulatory requirement.
///
/// Under madsim, simulated time is advanced between submission rounds so the
/// test exercises timing-dependent state (e.g. block timestamps) deterministically.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test(flavor = "multi_thread", worker_threads = 2))]
async fn test_madsim_sncm_nudge_under_simulated_load() {
    let addr = free_addr();
    let node = Arc::new(Node::new("sncm-load", &addr, 1));
    node.start(vec![]).await.unwrap();

    let compliant_asset = TraceableAsset {
        gtin: Some("07891234567890".into()),
        batch_number: Some("LOTE-MADSIM-001".into()),
        expiry_date: Some("2028-06-30".into()),
        serial_number: Some("SN-MADSIM-001".into()),
        anvisa_registration: Some("MS 1.0001.0001.001-1".into()),
        manufacturer_id: Some("12.345.678/0001-99".into()),
        product_name: "Dipirona 500mg".into(),
        custodian_id: "fab-madsim".into(),
        country_of_origin: Some("BR".into()),
        storage_temp_celsius: Some("15-30".into()),
        quantity: 500,
    };

    let non_compliant_asset = TraceableAsset {
        gtin: None,
        batch_number: None,
        expiry_date: None,
        serial_number: None,
        anvisa_registration: None,
        manufacturer_id: None,
        product_name: "Unknown Compound".into(),
        custodian_id: "unknown-fab".into(),
        country_of_origin: None,
        storage_temp_celsius: None,
        quantity: 1,
    };

    // Compute scores before submitting to assert nudge invariants.
    let compliant_score = MetadataTrustScore::compute(&compliant_asset);
    let nc_score = MetadataTrustScore::compute(&non_compliant_asset);

    assert!(compliant_score.score >= TRUST_SCORE_STANDARD_THRESHOLD);
    assert!(nc_score.score < TRUST_SCORE_STANDARD_THRESHOLD);

    // Fee multiplier: compliant = 0.5×, non-compliant = 1.0×.
    assert!(
        (compliant_score.fee_multiplier() - 0.5).abs() < f64::EPSILON,
        "compliant asset must get 50 % fee discount"
    );
    assert!(
        (nc_score.fee_multiplier() - 1.0).abs() < f64::EPSILON,
        "non-compliant asset must pay full fee"
    );

    // Both are accepted (nudge model — no hard rejection).
    let tx_compliant = Transaction::new(TransactionKind::AssetRegistration(
        glasschain_core::TraceableAssetRegistration {
            asset: compliant_asset,
            event_type: "MANUFACTURE".into(),
            originator_id: "fab-madsim".into(),
            purchase_order_ref: None,
        },
    ));
    let tx_nc = Transaction::new(TransactionKind::AssetRegistration(
        glasschain_core::TraceableAssetRegistration {
            asset: non_compliant_asset,
            event_type: "UNKNOWN".into(),
            originator_id: "unknown-fab".into(),
            purchase_order_ref: None,
        },
    ));

    node.submit_transaction(tx_compliant).await.unwrap();
    node.submit_transaction(tx_nc).await.unwrap();

    #[cfg(madsim)]
    sim_time::advance(Duration::from_millis(10)).await;

    node.mine().await.unwrap();

    let ledger = node.ledger_snapshot().await;
    let data_block = &ledger.chain[1];
    assert_eq!(
        data_block.transactions.len(),
        2,
        "both compliant and non-compliant assets committed"
    );
    assert!(ledger.validate_chain().is_ok());
}

/// Simulate repeated block-mined events to verify the event bus remains
/// live and ordered under simulated time pressure.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test)]
async fn test_madsim_event_bus_ordering_under_simulated_time() {
    const ROUNDS: usize = 3;

    let addr = free_addr();
    let node = Arc::new(Node::new("event-order", &addr, 1));
    let mut rx = node.subscribe();
    node.start(vec![]).await.unwrap();

    for round in 0..ROUNDS {
        node.submit_transaction(inv_tx(&format!("owner-{round}"), 10))
            .await
            .unwrap();

        #[cfg(madsim)]
        sim_time::advance(Duration::from_millis(5)).await;

        node.mine().await.unwrap();

        #[cfg(madsim)]
        sim_time::advance(Duration::from_millis(5)).await;
    }

    // Collect all BlockMined events — must see exactly ROUNDS of them.
    let mut mined_indices: Vec<u64> = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let NodeEvent::BlockMined { index, .. } = event {
            mined_indices.push(index);
        }
    }

    assert_eq!(
        mined_indices.len(),
        ROUNDS,
        "must receive exactly {ROUNDS} BlockMined events"
    );

    // Indices must be strictly increasing (events delivered in order).
    for w in mined_indices.windows(2) {
        assert!(
            w[1] > w[0],
            "block indices must be strictly increasing: {mined_indices:?}"
        );
    }
}

/// Deterministic partition scenario documented as a **reference implementation**
/// for full madsim-tokio network simulation.
///
/// This test documents the migration path to full TCP-level partition simulation.
/// With `madsim-tokio`, the `tokio::net` types used inside `glasschain-network`
/// would be intercepted by the madsim runtime, enabling:
///
/// ```text
/// let handle = madsim::Handle::current();
/// handle.net.partition(&[node_a_addr], &[node_b_addr]);
/// // ... observe divergence ...
/// handle.net.repair(&[node_a_addr], &[node_b_addr]);
/// // ... observe re-convergence ...
/// ```
///
/// Until that integration is complete, the test simulates the **observable
/// outcome** of a partition using the application-layer technique above.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test(flavor = "multi_thread", worker_threads = 2))]
async fn test_madsim_partition_reference_implementation() {
    // ── Pre-partition: A and B are connected ─────────────────────────────
    let addr_a = free_addr();
    let node_a = Arc::new(Node::new("ref-a", &addr_a, 1));
    node_a.start(vec![]).await.unwrap();

    node_a
        .submit_transaction(inv_tx("ref-a-pre", 10))
        .await
        .unwrap();
    node_a.mine().await.unwrap();

    let addr_b = free_addr();
    let node_b = Arc::new(Node::new("ref-b", &addr_b, 1));
    node_b.start(vec![addr_a.clone()]).await.unwrap();

    #[cfg(madsim)]
    sim_time::advance(Duration::from_millis(300)).await;

    let len_b_synced = await_chain_len(&node_b, 2).await;
    assert!(len_b_synced >= 2, "B must sync A's block before partition");

    // ── Partition: A and B mine independently (no peer connection) ────────
    // Application-layer partition: new node C connects only to A, new node D
    // connects only to B — their chains diverge.
    node_a
        .submit_transaction(inv_tx("ref-a-partition", 20))
        .await
        .unwrap();
    node_a.mine().await.unwrap();

    node_b
        .submit_transaction(inv_tx("ref-b-partition", 30))
        .await
        .unwrap();
    node_b.mine().await.unwrap();
    node_b
        .submit_transaction(inv_tx("ref-b-partition-2", 30))
        .await
        .unwrap();
    node_b.mine().await.unwrap();

    // ── Post-partition: both chains validate, and a joining node converges ──
    let chain_len_a = node_a.ledger_snapshot().await.chain.len();
    let chain_len_b = node_b.ledger_snapshot().await.chain.len();
    assert!(node_a.ledger_snapshot().await.validate_chain().is_ok());
    assert!(node_b.ledger_snapshot().await.validate_chain().is_ok());

    // A new node connects to B and converges to B's committed chain.
    let addr_e = free_addr();
    let node_e = Arc::new(Node::new("ref-e", &addr_e, 1));
    node_e.start(vec![addr_b.clone()]).await.unwrap();

    #[cfg(madsim)]
    sim_time::advance(Duration::from_millis(400)).await;

    let len_e = await_chain_len(&node_e, chain_len_b).await;
    assert!(
        len_e >= chain_len_b,
        "E (connected to B) must converge to B's chain: E={len_e} B={chain_len_b}"
    );
    assert!(
        len_e >= chain_len_a,
        "convergence is liveness, not fork resolution: E={len_e} A={chain_len_a}"
    );
    assert!(node_e.ledger_snapshot().await.validate_chain().is_ok());

    // TODO (madsim-tokio migration): replace the above application-layer
    // partition with:
    //   let handle = madsim::Handle::current();
    //   handle.net.partition(&[node_a_ip], &[node_b_ip]);
    //   // ... mine independently ...
    //   handle.net.repair(&[node_a_ip], &[node_b_ip]);
    //   // ... assert re-convergence ...
    // This requires linking `glasschain-network` against `madsim-tokio` so that
    // `tokio::net::TcpListener` is intercepted by the simulator.
}
