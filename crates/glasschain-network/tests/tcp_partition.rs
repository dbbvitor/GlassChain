//! TCP-level fault injection via an in-process proxy layer (#70).
//!
//! Instead of patching the async runtime (the madsim-tokio route, blocked on
//! fork support for tokio 1.53 — see the issue), nodes run over **real
//! loopback TCP** with a lightweight in-process proxy between each ordered
//! pair: the dialer connects to the proxy port, the proxy relays raw byte
//! streams to the target node, and `partition` kills the established relay
//! tasks and refuses new connections until `repair`.
//!
//! That exercises the exact paths an application-layer partition cannot: an
//! **established** TLS session torn down mid-flight, TLS handshake failure
//! over a dead path, TOFU re-verification of the returning peer, and
//! re-convergence after repair — with unmodified tokio and the production
//! network stack.
//!
//! Both peers sit behind proxies and advertise their proxy port
//! (`Node::set_advertise_addr`): reconnects dial the advertised address, so
//! with a proxy on only one side the built-in 5-second reconnect would bypass
//! the partition over the direct route.

use glasschain_core::{InventoryUpdate, Transaction, TransactionKind};
use glasschain_network::Node;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    sync::Mutex,
    task::JoinHandle,
};

fn free_addr() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

/// A bidirectional relay between one dialer-side port and one target address.
/// `partition` aborts the relays (severing the established sockets) and
/// refuses new connections; `repair` allows them again.
struct TcpProxy {
    /// Front port — the address dialers use.
    front_addr: String,
    enabled: Arc<AtomicBool>,
    relays: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl TcpProxy {
    /// Proxy a freshly bound front port to `target`.
    async fn spawn(target: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let front_addr = listener.local_addr().unwrap().to_string();
        let enabled = Arc::new(AtomicBool::new(true));
        let relays: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();

        let acceptor_enabled = Arc::clone(&enabled);
        let acceptor_relays = Arc::clone(&relays);
        let target = target.to_owned();
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    break;
                };
                if !acceptor_enabled.load(Ordering::SeqCst) {
                    // Partition: refuse the connection outright.
                    drop(client);
                    continue;
                }
                let Ok(upstream) = TcpStream::connect(&target).await else {
                    drop(client);
                    continue;
                };
                acceptor_relays.lock().await.push(tokio::spawn(async move {
                    // Errors (including the abort-induced cancellation below)
                    // just end the relay; both sockets drop and the TCP stacks
                    // on both nodes see the disconnect.
                    let mut client = client;
                    let mut upstream = upstream;
                    let _ = copy_bidirectional(&mut client, &mut upstream).await;
                }));
            }
        });

        Self {
            front_addr,
            enabled,
            relays,
        }
    }

    /// Sever every established relay and refuse new connections.
    async fn partition(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        let mut relays = self.relays.lock().await;
        for handle in relays.drain(..) {
            handle.abort();
        }
    }

    /// Allow new connections again.
    fn repair(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }
}

fn inv_tx(id: &str, delta: i64) -> Transaction {
    let mut tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "PART-SKU".into(),
        owner_id: "owner".into(),
        quantity_delta: delta,
        reason: "tcp partition test".into(),
    }));
    id.clone_into(&mut tx.id);
    tx
}

/// Poll `condition` or panic after `secs`.
async fn poll_until(desc: &str, secs: u64, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if condition().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "condition never held: {desc}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn chain_len(node: &Node) -> usize {
    node.ledger_snapshot().await.chain.len()
}

/// A partition severs the **established** TLS session: blocks mined during the
/// partition do not propagate, and after repair the peers reconnect (TOFU
/// re-verifies the returning peer against its pinned fingerprint) and
/// re-converge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn established_session_survives_partition_and_reconverges_after_repair() {
    // Each node sits behind its own proxy and advertises the proxy port, so
    // every dial and built-in reconnect routes through the proxies.
    let mut a = Node::new("partition-a", free_addr(), 1);
    let mut b = Node::new("partition-b", free_addr(), 1);

    let proxy_a = TcpProxy::spawn(a.listen_addr()).await;
    let proxy_b = TcpProxy::spawn(b.listen_addr()).await;

    a.set_advertise_addr(&proxy_a.front_addr);
    b.set_advertise_addr(&proxy_b.front_addr);

    a.start(vec![proxy_b.front_addr.clone()]).await.unwrap();
    b.start(vec![proxy_a.front_addr.clone()]).await.unwrap();

    // Establish: B syncs a block mined on A through the proxy.
    a.submit_transaction(inv_tx("pre-partition", 1))
        .await
        .unwrap();
    a.mine().await.unwrap();
    let height = chain_len(&a).await;
    poll_until("B synced the pre-partition block", 5, || async {
        chain_len(&b).await >= height
    })
    .await;

    // ── Partition: the established sessions die mid-flight ─────────────────
    proxy_a.partition().await;
    proxy_b.partition().await;

    // Blocks mined during the partition do NOT propagate. Wait past the
    // built-in reconnect delay (5 s): if the reconnect could bypass the
    // proxies via the direct bind addresses, B would re-sync within this
    // window and the height assertion below would fail.
    a.submit_transaction(inv_tx("during-partition", 2))
        .await
        .unwrap();
    a.mine().await.unwrap();
    let partitioned_height = chain_len(&a).await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        chain_len(&b).await,
        height,
        "the partitioned peer must not receive blocks mined after the severance"
    );

    // ── Repair: proxies accept again; peers reconnect and re-verify TOFU ───
    proxy_a.repair();
    proxy_b.repair();
    a.connect_peer(&proxy_b.front_addr);
    b.connect_peer(&proxy_a.front_addr);

    // Re-convergence: the block mined during the partition reaches B.
    poll_until("B re-converged after repair", 8, || async {
        chain_len(&b).await >= partitioned_height
    })
    .await;

    // The repaired peers stay connected: a further block propagates.
    a.submit_transaction(inv_tx("post-repair", 3))
        .await
        .unwrap();
    a.mine().await.unwrap();
    let final_height = chain_len(&a).await;
    poll_until("post-repair block propagates", 5, || async {
        chain_len(&b).await >= final_height
    })
    .await;
}
