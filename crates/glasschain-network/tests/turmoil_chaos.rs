//! Deterministic partition and repair over [turmoil](https://github.com/tokio-rs/turmoil)'s
//! simulated network (wayfinder #154, #166). The node's sockets come from the
//! `turmoil-sim` seam (`crate::net`), so the scenario needs no real ports and
//! no wall-clock waits: turmoil drives virtual time and the partition.
//!
//! Run with:
//! `cargo test -p glasschain-network --test turmoil_chaos --features turmoil-sim`

#![cfg(feature = "turmoil-sim")]

use glasschain_core::{InventoryUpdate, Transaction, TransactionKind};
use glasschain_network::Node;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::sleep;

const PORT_A: u16 = 9_001;
const PORT_B: u16 = 9_002;

type Nodes = Arc<Mutex<HashMap<&'static str, Arc<Node>>>>;

/// Commands a host accepts so that every node operation — and therefore every
/// simulated socket write — runs inside the host that owns the node.
enum Command {
    /// Submit a transaction and mine it into a block.
    Mine,
    /// Dial a peer through the node's reconnect path.
    Redial(String),
}

fn inv_tx(owner: &str) -> Transaction {
    Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "TURMOIL-SKU".into(),
        owner_id: owner.into(),
        quantity_delta: 1,
        reason: "turmoil partition scenario".into(),
    }))
}

fn node_of(nodes: &Nodes, id: &str) -> Arc<Node> {
    nodes
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .expect("host registered its node before the scenario starts")
}

/// Poll `node` until its chain reaches `want` blocks.
async fn await_chain_len(node: &Node, want: usize) {
    for _ in 0..600 {
        if node.ledger_snapshot().await.chain.len() >= want {
            return;
        }
        sleep(Duration::from_millis(50)).await;
    }
    let len = node.ledger_snapshot().await.chain.len();
    panic!("chain did not reach {want} blocks (stalled at {len})");
}

/// A partition stops block propagation; repairing it plus one reconnect lets
/// the isolated node catch up to the chain it missed.
#[test]
fn partition_heals_and_the_isolated_node_catches_up() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(600))
        .build();

    let nodes: Nodes = Arc::new(Mutex::new(HashMap::new()));
    let completed = Arc::new(AtomicBool::new(false));
    let (tx_a, rx_a) = tokio::sync::mpsc::unbounded_channel();
    let (tx_b, rx_b) = tokio::sync::mpsc::unbounded_channel();
    let rx_a = Arc::new(Mutex::new(Some(rx_a)));
    let rx_b = Arc::new(Mutex::new(Some(rx_b)));

    let host_nodes = Arc::clone(&nodes);
    sim.host("node-a", move || {
        let nodes = Arc::clone(&host_nodes);
        let mut rx = rx_a
            .lock()
            .unwrap()
            .take()
            .expect("node-a host is registered once");
        async move {
            let node = Arc::new(Node::new("node-a", format!("0.0.0.0:{PORT_A}"), 1));
            node.start(Vec::new()).await?;
            nodes.lock().unwrap().insert("node-a", Arc::clone(&node));
            while let Some(command) = rx.recv().await {
                if matches!(command, Command::Mine) {
                    node.submit_transaction(inv_tx("a-owner")).await?;
                    node.mine().await?;
                }
            }
            Ok(())
        }
    });

    let host_nodes = Arc::clone(&nodes);
    sim.host("node-b", move || {
        let nodes = Arc::clone(&host_nodes);
        let mut rx = rx_b
            .lock()
            .unwrap()
            .take()
            .expect("node-b host is registered once");
        async move {
            let peer_a = format!("{}:{PORT_A}", turmoil::lookup("node-a"));
            let node = Arc::new(Node::new("node-b", format!("0.0.0.0:{PORT_B}"), 1));
            node.start(vec![peer_a]).await?;
            nodes.lock().unwrap().insert("node-b", Arc::clone(&node));
            while let Some(command) = rx.recv().await {
                if let Command::Redial(peer) = command {
                    node.connect_peer(&peer);
                }
            }
            Ok(())
        }
    });

    let client_nodes = Arc::clone(&nodes);
    let client_completed = Arc::clone(&completed);
    sim.client("test", async move {
        let node_a = node_of(&client_nodes, "node-a");
        let node_b = node_of(&client_nodes, "node-b");
        let peer_a = format!("{}:{PORT_A}", turmoil::lookup("node-a"));

        // Baseline: B dials A and converges on genesis plus A's first block.
        await_chain_len(&node_a, 1).await;
        await_chain_len(&node_b, 1).await;
        tx_a.send(Command::Mine).unwrap();
        await_chain_len(&node_a, 2).await;
        await_chain_len(&node_b, 2).await;

        // Partition: A extends its chain, B must not see it.
        turmoil::partition("node-a", "node-b");
        tx_a.send(Command::Mine).unwrap();
        await_chain_len(&node_a, 3).await;
        sleep(Duration::from_secs(2)).await;
        assert_eq!(
            node_b.ledger_snapshot().await.chain.len(),
            2,
            "B must not receive blocks while partitioned"
        );

        // Repair: one reconnect is enough for B to catch up to A's chain.
        turmoil::repair("node-a", "node-b");
        tx_b.send(Command::Redial(peer_a)).unwrap();
        await_chain_len(&node_b, 3).await;

        let chain_a = node_a.ledger_snapshot().await;
        let chain_b = node_b.ledger_snapshot().await;
        assert_eq!(chain_a.chain.len(), 3);
        assert_eq!(
            chain_a.chain.last().unwrap().hash,
            chain_b.chain.last().unwrap().hash,
            "both nodes converge on the same tip"
        );
        client_completed.store(true, Ordering::SeqCst);
        Ok(())
    });

    sim.run()
        .expect("simulation completes without a host panic");
    assert!(
        completed.load(Ordering::SeqCst),
        "the scenario must finish inside the simulation window"
    );
}
