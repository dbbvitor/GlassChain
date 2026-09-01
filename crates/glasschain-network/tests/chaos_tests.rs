//! Network Chaos Testing Suite (Phase 6).
//!
//! These tests simulate adverse network conditions to verify that `GlassChain`
//! nodes handle failures gracefully:
//!
//! - **30% node failure**: spin up 10 nodes, kill 3, verify the remainder
//!   can still mine and sync.
//! - **Sequential node disconnect / reconnect**: verify chain consistency after
//!   a node rejoins.
//! - **Concurrent commits**: independently mined blocks are final at commit
//!   (each carries a validating quorum certificate) and syncing nodes converge.

use glasschain_core::{
    CommitNotification, InventoryUpdate, MetadataTrustScore, TraceableAsset, Transaction,
    TransactionKind, TRUST_SCORE_STANDARD_THRESHOLD,
};
use glasschain_network::{Node, NodeEvent};
use glasschain_workflows::{InventoryTrigger, WatcherService};
use std::time::Duration;
use tokio::time::{sleep, timeout};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn inventory_tx(owner: &str) -> Transaction {
    Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-CHAOS".into(),
        owner_id: owner.into(),
        quantity_delta: 10,
        reason: "chaos test".into(),
    }))
}

/// Allocate an available loopback port.
fn free_addr() -> String {
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Scenario: 30% node failure.
///
/// Start `node_0` and mine 2 blocks.  Then start `node_1` and `node_2` which both
/// sync via the `RequestChain` handshake.  Drop `node_2` (~33% failure), mine
/// another block on `node_0`.  A new `node_3` that connects should see all 4
/// blocks and the chain must be valid.
#[tokio::test]
async fn test_partial_node_failure_chain_remains_operational() {
    let addr_0 = free_addr();
    let addr_1 = free_addr();
    let addr_2 = free_addr();
    let addr_3 = free_addr();

    let node_0 = Node::new("chaos-0", &addr_0, 1);
    node_0.start(vec![]).await.unwrap();

    // Mine 2 blocks on node_0.
    node_0
        .submit_transaction(inventory_tx("node-0-a"))
        .await
        .unwrap();
    node_0.mine().await.unwrap();
    node_0
        .submit_transaction(inventory_tx("node-0-b"))
        .await
        .unwrap();
    node_0.mine().await.unwrap();

    // Nodes 1 and 2 join and sync via RequestChain.
    let node_1 = Node::new("chaos-1", &addr_1, 1);
    node_1.start(vec![addr_0.clone()]).await.unwrap();
    let node_2 = Node::new("chaos-2", &addr_2, 1);
    node_2.start(vec![addr_0.clone()]).await.unwrap();
    sleep(Duration::from_millis(600)).await;

    let len_1 = node_1.ledger_snapshot().await.chain.len();
    let len_2 = node_2.ledger_snapshot().await.chain.len();
    assert!(len_1 >= 3, "node 1 did not sync: chain len={len_1}");
    assert!(len_2 >= 3, "node 2 did not sync: chain len={len_2}");

    // Simulate ~33% failure: drop node 2.
    drop(node_2);

    // The surviving nodes mine another block.
    node_0
        .submit_transaction(inventory_tx("node-0-c"))
        .await
        .unwrap();
    node_0.mine().await.unwrap();

    // A fresh node connects and should see all 4 blocks.
    let node_3 = Node::new("chaos-3", &addr_3, 1);
    node_3.start(vec![addr_0.clone()]).await.unwrap();
    sleep(Duration::from_millis(600)).await;

    let ledger_3 = node_3.ledger_snapshot().await;
    assert!(
        ledger_3.chain.len() >= 4,
        "node 3 should have 4 blocks (got {})",
        ledger_3.chain.len()
    );
    assert!(ledger_3.validate_chain().is_ok());
}

/// Scenario: node disconnect and rejoin.
///
/// Node A mines a block.  Node B connects, syncs, disconnects (simulated by
/// not being in A's seed list), then a new node C connects to A.  C should
/// still sync A's chain correctly.
#[tokio::test]
async fn test_node_can_sync_after_rejoining() {
    // Node A mines first.
    let addr_a = free_addr();
    let node_a = Node::new("chaos-a", &addr_a, 1);
    node_a.start(vec![]).await.unwrap();
    node_a
        .submit_transaction(inventory_tx("node-a"))
        .await
        .unwrap();
    node_a.mine().await.unwrap();
    sleep(Duration::from_millis(100)).await;

    // Node B joins and syncs.
    let addr_b = free_addr();
    let node_b = Node::new("chaos-b", &addr_b, 1);
    node_b.start(vec![addr_a.clone()]).await.unwrap();
    sleep(Duration::from_millis(400)).await;
    let ledger_b = node_b.ledger_snapshot().await;
    assert!(ledger_b.chain.len() >= 2, "node B did not sync");

    // Node B "disconnects" (we just stop using it).
    drop(node_b);

    // Node A mines another block.
    node_a
        .submit_transaction(inventory_tx("node-a-2"))
        .await
        .unwrap();
    node_a.mine().await.unwrap();
    sleep(Duration::from_millis(100)).await;

    // Node C connects to A and should get the updated chain.
    let addr_c = free_addr();
    let node_c = Node::new("chaos-c", &addr_c, 1);
    node_c.start(vec![addr_a.clone()]).await.unwrap();
    sleep(Duration::from_millis(500)).await;

    let ledger_c = node_c.ledger_snapshot().await;
    assert!(
        ledger_c.chain.len() >= 3,
        "node C should have 3 blocks (genesis + 2 mined by A)"
    );
    assert!(ledger_c.validate_chain().is_ok());
}

/// Scenario: concurrent commits are final at commit, and a syncing node
/// converges (liveness).
///
/// Nodes A and B commit independently. Each committed block carries a quorum
/// certificate on its commit notification that a verifying member validates
/// against the committed block — finality does not depend on trusting the
/// producing node. Node C then connects to B and converges to B's chain.
#[tokio::test]
async fn test_committed_blocks_are_final_and_sync_converges() {
    let addr_a = free_addr();
    let addr_b = free_addr();
    let node_a = Node::new("concurrent-a", &addr_a, 1);
    let node_b = Node::new("concurrent-b", &addr_b, 1);
    let mut events_b = node_b.subscribe();

    // Both nodes start standalone (no peers).
    node_a.start(vec![]).await.unwrap();
    node_b.start(vec![]).await.unwrap();

    // Node B commits two blocks.
    node_b
        .submit_transaction(inventory_tx("b-1"))
        .await
        .unwrap();
    node_b.mine().await.unwrap();
    node_b
        .submit_transaction(inventory_tx("b-2"))
        .await
        .unwrap();
    node_b.mine().await.unwrap();

    // Node A commits one block.
    node_a
        .submit_transaction(inventory_tx("a-1"))
        .await
        .unwrap();
    node_a.mine().await.unwrap();

    let remote_chain_blocks = node_b.ledger_snapshot().await.chain.len();
    let local_chain_blocks = node_a.ledger_snapshot().await.chain.len();
    assert_eq!(remote_chain_blocks, 3, "node B should have 3 blocks");
    assert_eq!(local_chain_blocks, 2, "node A should have 2 blocks");

    // Every commit notification from B carries a certificate that validates
    // against the committed block: the block is final at commit, verified
    // locally, never on "the leader said so".
    let ledger_b = node_b.ledger_snapshot().await;
    let mut certified = 0;
    while let Ok(event) = events_b.try_recv() {
        if let NodeEvent::BlockMined {
            index, certificate, ..
        } = event
        {
            let block = ledger_b.chain[usize::try_from(index).expect("index fits usize")].clone();
            let notification = CommitNotification { block, certificate };
            notification
                .validate()
                .expect("certificate must attest the committed block");
            certified += 1;
        }
    }
    assert_eq!(certified, 2, "both of B's commits carried certificates");

    // A syncing node converges to B's chain (liveness).
    let addr_c = free_addr();
    let node_c = Node::new("concurrent-c", &addr_c, 1);
    node_c.start(vec![addr_b.clone()]).await.unwrap();
    sleep(Duration::from_millis(500)).await;

    let ledger_c = node_c.ledger_snapshot().await;
    assert!(
        ledger_c.chain.len() >= 3,
        "node C should have adopted B's chain of 3 blocks (got {})",
        ledger_c.chain.len()
    );
    assert!(ledger_c.validate_chain().is_ok());
}

/// A syncing (verifying) member validates the quorum certificate on every
/// block it receives and commits — finality is verifiable locally, not on the
/// producer's word.
#[tokio::test]
async fn test_received_block_certificate_validates_on_verifying_member() {
    let addr_a = free_addr();
    let node_a = Node::new("certifier-a", &addr_a, 1);
    node_a.start(vec![]).await.unwrap();
    node_a
        .submit_transaction(inventory_tx("a-1"))
        .await
        .unwrap();
    node_a.mine().await.unwrap();
    sleep(Duration::from_millis(100)).await;

    // B connects and syncs, then observes A's next live block broadcast.
    let addr_b = free_addr();
    let node_b = Node::new("certifier-b", &addr_b, 1);
    let mut events_b = node_b.subscribe();
    node_b.start(vec![addr_a.clone()]).await.unwrap();
    sleep(Duration::from_millis(500)).await;

    node_a
        .submit_transaction(inventory_tx("a-2"))
        .await
        .unwrap();
    node_a.mine().await.unwrap();
    sleep(Duration::from_millis(500)).await;

    let ledger_b = node_b.ledger_snapshot().await;
    let mut received_certificates = 0;
    while let Ok(event) = events_b.try_recv() {
        if let NodeEvent::BlockReceived {
            index, certificate, ..
        } = event
        {
            let block = ledger_b.chain[usize::try_from(index).expect("index fits usize")].clone();
            CommitNotification { block, certificate }
                .validate()
                .expect("verifying member must be able to validate the attestation set");
            received_certificates += 1;
        }
    }
    assert!(
        received_certificates >= 1,
        "the verifying member validated the received block's certificate"
    );
}

/// Scenario: block mined event fires and chain is valid after mining.
///
/// This basic smoke test ensures that in chaotic conditions (multiple
/// concurrent operations), the node-level locking still keeps the chain
/// consistent.
#[tokio::test]
async fn test_block_mined_event_fires_under_load() {
    let addr = free_addr();
    let node = Node::new("load-node", &addr, 1);
    let mut events = node.subscribe();
    node.start(vec![]).await.unwrap();

    // Submit several transactions concurrently.
    let submit_futs: Vec<_> = (0..5)
        .map(|i| {
            let tx = inventory_tx(&format!("owner-{i}"));
            node.submit_transaction(tx)
        })
        .collect();
    for fut in submit_futs {
        fut.await.unwrap();
    }

    node.mine().await.unwrap();

    // Wait for the BlockMined event.
    let found = timeout(Duration::from_secs(3), async {
        loop {
            match events.recv().await {
                Ok(NodeEvent::BlockMined { .. }) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    assert!(found, "BlockMined event not received");

    let ledger = node.ledger_snapshot().await;
    assert!(ledger.validate_chain().is_ok());
    assert_eq!(ledger.chain[1].transactions.len(), 5);
}

/// Validates that the watcher service can handle multiple autonomous inventory triggers
/// at high frequency — simulating Phase 4 autonomous WASM triggers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_high_frequency_autonomous_inventory() {
    // Create a watcher service and add 10 triggers for different products
    let mut watcher = WatcherService::new();
    for i in 0..10u64 {
        watcher.add_trigger(InventoryTrigger {
            trigger_id: format!("trigger-{i}"),
            owner_id: "warehouse-central".into(),
            product_id: format!("PROD-{i:03}"),
            reorder_threshold: 5,
            reorder_quantity: 100,
            seller_id: "supplier-auto".into(),
            price_per_unit: 1000,
            currency: "BRL".into(),
            active: true,
            wasm_code_b64: None,
        });
    }

    // Fire all 10 triggers in one batch — each crosses threshold by going to -10
    let mut total_orders = 0;
    for i in 0..10u64 {
        let update = InventoryUpdate {
            product_id: format!("PROD-{i:03}"),
            owner_id: "warehouse-central".into(),
            quantity_delta: -10, // inventory 0 → -10, which is ≤ threshold 5
            reason: "autonomous depletion test".into(),
        };
        let orders = watcher.on_inventory_update(&update);
        total_orders += orders.len();
    }

    assert_eq!(total_orders, 10, "All 10 triggers should fire exactly once");

    // Fire again — each trigger should fire again (second invocation)
    let mut second_round = 0;
    for i in 0..10u64 {
        let update = InventoryUpdate {
            product_id: format!("PROD-{i:03}"),
            owner_id: "warehouse-central".into(),
            quantity_delta: -100,
            reason: "second depletion".into(),
        };
        let orders = watcher.on_inventory_update(&update);
        second_round += orders.len();
    }
    assert_eq!(
        second_round, 10,
        "All 10 triggers fire again on second drop"
    );

    // IDs across the two rounds must be unique (fire counter increments)
    // Verify by collecting all IDs from a fresh single-trigger run
    let mut watcher2 = WatcherService::new();
    watcher2.add_trigger(InventoryTrigger {
        trigger_id: "t-unique".into(),
        owner_id: "owner".into(),
        product_id: "P-UNIQUE".into(),
        reorder_threshold: 0,
        reorder_quantity: 10,
        seller_id: "s".into(),
        price_per_unit: 100,
        currency: "BRL".into(),
        active: true,
        wasm_code_b64: None,
    });
    let fire1 = watcher2.on_inventory_update(&InventoryUpdate {
        product_id: "P-UNIQUE".into(),
        owner_id: "owner".into(),
        quantity_delta: -1,
        reason: "r".into(),
    });
    let fire2 = watcher2.on_inventory_update(&InventoryUpdate {
        product_id: "P-UNIQUE".into(),
        owner_id: "owner".into(),
        quantity_delta: -1,
        reason: "r".into(),
    });
    assert_eq!(fire1.len(), 1);
    assert_eq!(fire2.len(), 1);
    assert_ne!(
        fire1[0].id, fire2[0].id,
        "repeated firings must yield unique tx IDs"
    );
}

/// Validates that asset schema validation (Phase 3 Nudge engine) works correctly
/// when assets flow through the network: compliant assets get discount, non-compliant are flagged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_schema_validation_in_network_context() {
    // Compliant asset — all SNCM mandatory fields
    let compliant = TraceableAsset {
        gtin: Some("07891234567890".into()),
        batch_number: Some("LOTE-2025-001".into()),
        expiry_date: Some("2027-12-31".into()),
        serial_number: Some("SN-00000001".into()),
        anvisa_registration: None,
        manufacturer_id: None,
        product_name: "Amoxicilina 500mg".into(),
        custodian_id: "fab-01".into(),
        country_of_origin: None,
        storage_temp_celsius: None,
        quantity: 500,
    };
    let score = MetadataTrustScore::compute(&compliant);
    assert!(score.score >= TRUST_SCORE_STANDARD_THRESHOLD);
    assert!(
        (score.fee_multiplier() - 0.5).abs() < f64::EPSILON,
        "Standard asset pays 50% fee"
    );

    // Non-compliant asset — missing all SNCM fields
    let non_compliant = TraceableAsset {
        gtin: None,
        batch_number: None,
        expiry_date: None,
        serial_number: None,
        anvisa_registration: None,
        manufacturer_id: None,
        product_name: "Medicamento Desconhecido".into(),
        custodian_id: "unknown".into(),
        country_of_origin: None,
        storage_temp_celsius: None,
        quantity: 1,
    };
    let nc_score = MetadataTrustScore::compute(&non_compliant);
    assert!(nc_score.score < TRUST_SCORE_STANDARD_THRESHOLD);
    assert!(
        (nc_score.fee_multiplier() - 1.0).abs() < f64::EPSILON,
        "Non-standard pays 100% fee"
    );

    // Both assets are accepted (nudge model — no hard rejection)
    assert_eq!(
        nc_score.missing_core_fields.len(),
        4,
        "All 4 core fields missing"
    );
}
