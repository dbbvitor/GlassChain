/// Integration tests for the `GlassChain` network node.
///
/// These tests spin up real Tokio tasks and real TCP connections on localhost.
use glasschain_contracts::InventoryTrigger;
use glasschain_core::{
    CoreError, ExecutionProvider, InventoryUpdate, SupplyOffer, Transaction, TransactionKind,
};
use glasschain_network::{Node, NodeEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

fn inventory_tx(owner: &str) -> Transaction {
    Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-TEST".into(),
        owner_id: owner.into(),
        quantity_delta: 10,
        reason: "integration test".into(),
    }))
}

fn supply_offer_tx(seller: &str, product: &str, price: u64, lead: u32) -> Transaction {
    Transaction::new(TransactionKind::SupplyOffer(SupplyOffer {
        product_id: product.into(),
        product_name: "Widget".into(),
        seller_id: seller.into(),
        quantity_available: 100,
        price_per_unit: price,
        lead_time_days: lead,
        currency: "USD".into(),
    }))
}

struct ApprovingExecutionProvider;

impl ExecutionProvider for ApprovingExecutionProvider {
    fn execute(
        &self,
        _contract_id: &str,
        _payload: &[u8],
        _limits: glasschain_core::ExecutionLimits,
    ) -> Result<Vec<(String, Vec<u8>)>, CoreError> {
        Ok(vec![("approve".into(), b"1".to_vec())])
    }

    fn name(&self) -> &'static str {
        "test-approval"
    }
}

/// Helper: pick an ephemeral port on localhost that is very likely free.
fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

#[tokio::test]
async fn test_node_starts_and_mines_block() {
    let addr = free_addr();
    let node = Node::new("test-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    node.submit_transaction(inventory_tx("owner-1"))
        .await
        .unwrap();
    node.mine().await.unwrap();

    let ledger = node.ledger_snapshot().await;
    assert_eq!(ledger.chain.len(), 2); // genesis + mined block
    assert!(ledger.validate_chain().is_ok());
    assert_eq!(ledger.chain[1].transactions.len(), 1);
}

#[tokio::test]
async fn test_transaction_accepted_event_fires() {
    let addr = free_addr();
    let node = Node::new("evt-node", &addr, 1);
    let mut events = node.subscribe();
    node.start(vec![]).await.unwrap();

    node.submit_transaction(inventory_tx("owner-1"))
        .await
        .unwrap();

    let evt = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout waiting for event")
        .expect("channel closed");

    assert!(matches!(evt, NodeEvent::TransactionAccepted(_)));
}

#[tokio::test]
async fn test_block_mined_event_fires() {
    let addr = free_addr();
    let node = Node::new("mine-node", &addr, 1);
    let mut events = node.subscribe();
    node.start(vec![]).await.unwrap();

    node.submit_transaction(inventory_tx("owner-1"))
        .await
        .unwrap();
    // Consume TransactionAccepted event first.
    let _ = timeout(Duration::from_secs(1), events.recv()).await;

    node.mine().await.unwrap();

    let evt = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout waiting for BlockMined event")
        .expect("channel closed");

    assert!(matches!(evt, NodeEvent::BlockMined { .. }));
}

#[tokio::test]
async fn test_smart_contract_auto_execution_on_supply_offer() {
    use glasschain_core::{PurchaseConditions, SmartContractDef, TransactionKind};

    let addr = free_addr();
    let node = Node::new("contract-node", &addr, 1);
    let mut events = node.subscribe();
    node.start(vec![]).await.unwrap();

    // Create a smart contract.
    let contract_tx = Transaction::new(TransactionKind::ContractCreation(SmartContractDef {
        contract_id: "c-auto-1".into(),
        buyer_id: "buyer-acme".into(),
        product_id: "SKU-AUTO".into(),
        conditions: PurchaseConditions {
            max_price_per_unit: 2000,
            min_quantity: 1,
            max_quantity: 50,
            max_lead_time_days: 14,
            preferred_seller_id: None,
            currency: "USD".into(),
            auto_execute: true,
        },
        wasm_code_b64: None,
    }));
    node.submit_transaction(contract_tx).await.unwrap();

    // Drain the ContractCreation accepted event.
    let _ = timeout(Duration::from_millis(200), events.recv()).await;

    // Post a matching supply offer.
    let offer_tx = supply_offer_tx("seller-z", "SKU-AUTO", 1500, 7);
    node.submit_transaction(offer_tx).await.unwrap();

    // Collect events with a 3-second window, looking for ContractExecuted.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(500), events.recv()).await {
            Ok(Ok(NodeEvent::ContractExecuted { contract_id, .. })) => {
                if contract_id == "c-auto-1" {
                    found = true;
                    break;
                }
            }
            Ok(Ok(_)) => {} // skip other events
            _ => break,
        }
    }
    assert!(found, "ContractExecuted event was not emitted");
}

#[tokio::test]
async fn test_registered_execution_provider_reaches_both_automation_paths() {
    let addr = free_addr();
    let node = Node::new("automation-node", &addr, 1);
    node.start(vec![]).await.unwrap();
    node.set_execution_provider(Arc::new(ApprovingExecutionProvider))
        .await;

    node.register_inventory_trigger(InventoryTrigger {
        trigger_id: "trigger-registered".into(),
        product_id: "SKU-AUTO".into(),
        owner_id: "owner-1".into(),
        reorder_threshold: 100,
        reorder_quantity: 25,
        seller_id: "seller-1".into(),
        price_per_unit: 1000,
        currency: "USD".into(),
        active: true,
        wasm_code_b64: Some("dGVzdA==".into()),
    })
    .await;

    node.submit_transaction(Transaction::new(TransactionKind::ContractCreation(
        glasschain_core::SmartContractDef {
            contract_id: "c-registered".into(),
            buyer_id: "buyer-1".into(),
            product_id: "SKU-AUTO".into(),
            conditions: glasschain_core::PurchaseConditions {
                max_price_per_unit: 2000,
                min_quantity: 1,
                max_quantity: 50,
                max_lead_time_days: 14,
                preferred_seller_id: None,
                currency: "USD".into(),
                auto_execute: true,
            },
            wasm_code_b64: Some("dGVzdA==".into()),
        },
    )))
    .await
    .unwrap();
    node.submit_transaction(supply_offer_tx("seller-1", "SKU-AUTO", 1500, 7))
        .await
        .unwrap();

    let offer_pending = node.ledger_snapshot().await.pending_transactions;
    assert!(offer_pending.iter().any(|tx| {
        matches!(tx.kind, TransactionKind::ContractExecution(ref execution) if execution.contract_id == "c-registered")
    }));

    node.submit_transaction(Transaction::new(TransactionKind::InventoryUpdate(
        InventoryUpdate {
            product_id: "SKU-AUTO".into(),
            owner_id: "owner-1".into(),
            quantity_delta: -1,
            reason: "below threshold".into(),
        },
    )))
    .await
    .unwrap();
    node.mine().await.unwrap();

    let after_inventory = node.ledger_snapshot().await.pending_transactions;
    assert!(after_inventory.iter().any(|tx| {
        matches!(tx.kind, TransactionKind::PurchaseOrder(ref order) if order.product_id == "SKU-AUTO")
    }));
}

#[tokio::test]
async fn test_two_nodes_sync_chain() {
    // Node A starts and mines a block.
    let addr_a = free_addr();
    let node_a = Node::new("node-a", &addr_a, 1);
    node_a.start(vec![]).await.unwrap();
    node_a
        .submit_transaction(inventory_tx("owner-a"))
        .await
        .unwrap();
    node_a.mine().await.unwrap();

    // Give node A a moment to finish mining.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Node B connects to node A as a seed peer.
    let addr_b = free_addr();
    let node_b = Node::new("node-b", &addr_b, 1);
    node_b.start(vec![addr_a.clone()]).await.unwrap();

    // Allow time for handshake and chain synchronisation.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Node B should have adopted node A's chain (length 2: genesis + block 1).
    let ledger_b = node_b.ledger_snapshot().await;
    assert!(
        ledger_b.chain.len() >= 2,
        "Node B chain length {} < 2 after sync",
        ledger_b.chain.len()
    );
}
