//! TCP-level fault injection via an in-process proxy layer (#70, D7).
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
//!
//! The WAN profile scenarios below (`real_tcp_wan_*`) are the D7 latency /
//! jitter / bandwidth extension: seedable one-way shaping per relay
//! direction (`tests/common/proxy.rs`), switching profiles mid-scenario,
//! and time-without-quorum measured separately from recovery. They are
//! **real TCP wall-clock tests** — generous margins, labeled separately
//! from deterministic simulated-network runs.

#[path = "common/proxy.rs"]
mod proxy;

use proxy::{TcpProxy, WanProfile};

use glasschain_core::{InventoryUpdate, Transaction, TransactionKind};
use glasschain_network::Node;
use std::time::Duration;

/// Allocate a unique loopback port for this test process.
///
/// Probing `bind(":0")` and dropping the listener races with sibling tests in
/// the same binary: the kernel can hand the same just-freed ephemeral port to
/// two probes before either node binds it (`AddrInUse` on CI). Ports are
/// reserved from a per-process band below the OS ephemeral range (which
/// starts at 32768 on Linux, 49152 on macOS/Windows), with a bind probe to
/// skip ports held by anything else.
fn free_addr() -> String {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let band = u16::try_from(std::process::id() % 32).expect("pid mod 32 fits u16");
    loop {
        let offset = NEXT.fetch_add(1, Ordering::Relaxed) % 300;
        let port = 22_000 + band * 300 + offset;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return format!("127.0.0.1:{port}");
        }
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

    a.set_advertise_addr(proxy_a.front_addr());
    b.set_advertise_addr(proxy_b.front_addr());

    a.start(vec![proxy_b.front_addr().to_owned()])
        .await
        .unwrap();
    b.start(vec![proxy_a.front_addr().to_owned()])
        .await
        .unwrap();

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
    a.connect_peer(proxy_b.front_addr());
    b.connect_peer(proxy_a.front_addr());

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

/// Four nodes meshed through per-node proxies (each node advertises its
/// proxy front), then shaped per profile.
///
/// The mesh handshake runs **unshaped**: shaping per TLS record compounds
/// latency across a handshake and, at ≥~120 ms/chunk, stalls mesh formation
/// (~4 × 5 s reconnect cycles — the measured D7 finding). The profile is
/// applied with `set_profile` once every node knows its three peers, so the
/// scenario tests WAN delay on **established** links, not the handshake.
async fn four_node_mesh(profiles: &[WanProfile]) -> (Vec<Node>, Vec<TcpProxy>) {
    let mut nodes: Vec<Node> = (0..4)
        .map(|i| Node::new(format!("wan-{i}"), free_addr(), 1))
        .collect();
    let mut proxies = Vec::new();
    for node in &nodes {
        let proxy = TcpProxy::spawn_with_profile(node.listen_addr(), WanProfile::none()).await;
        proxies.push(proxy);
    }
    for (i, node) in nodes.iter_mut().enumerate() {
        let front = proxies[i].front_addr().to_owned();
        node.set_advertise_addr(&front);
    }
    nodes[0].start(vec![]).await.unwrap();
    for (i, node) in nodes.iter().enumerate().skip(1) {
        let peers: Vec<String> = (0..4)
            .filter(|j| *j != i)
            .map(|j| proxies[j].front_addr().to_owned())
            .collect();
        node.start(peers).await.unwrap();
    }
    // Mesh up: every node knows its three peers (Hello-completed, not just
    // dialed).
    poll_until("all nodes see their three peers", 20, || async {
        let mut all = true;
        for node in &nodes {
            if node.known_peers().await.len() < 3 {
                all = false;
            }
        }
        all
    })
    .await;
    // Shape the established links for the scenario.
    for (proxy, profile) in proxies.iter().zip(profiles) {
        proxy.set_profile(*profile).await;
    }
    (nodes, proxies)
}

/// All four nodes agree on the same tip: no conflicting finalization.
async fn tips_agree(nodes: &[Node]) -> bool {
    let first = tip(&nodes[0]).await;
    for node in nodes {
        if tip(node).await != first {
            return false;
        }
    }
    true
}

async fn tip(node: &Node) -> (u64, String) {
    let chain = node.ledger_snapshot().await.chain;
    (
        u64::try_from(chain.len()).expect("test chain fits u64"),
        chain.last().expect("non-empty").hash.clone(),
    )
}

/// Whether `node` has reached `expected`.
async fn tip_reached(node: &Node, expected: (u64, String)) -> bool {
    tip(node).await == expected
}

/// D7 scenario 1 — no-fault baseline through the proxy overlay: blocks mined
/// on one node propagate to all four and every node agrees on the tip.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_tcp_wan_baseline_converges_with_no_faults() {
    let (nodes, _) = four_node_mesh(&[WanProfile::none(); 4]).await;

    for i in 0..3 {
        nodes[0]
            .submit_transaction(inv_tx(&format!("baseline-{i}"), 1))
            .await
            .unwrap();
        nodes[0].mine().await.unwrap();
    }
    let expected = tip(&nodes[0]).await;
    poll_until("all four nodes reached the mined tip", 8, || async {
        let snapshot = expected.clone();
        let mut all = true;
        for n in &nodes {
            if !tip_reached(n, snapshot.clone()).await {
                all = false;
            }
        }
        all
    })
    .await;
    assert!(
        tips_agree(&nodes).await,
        "no-fault run must not produce conflicting tips"
    );
}

/// D7 scenario 2 — asymmetric WAN delay (one direction fast, the other slow):
/// convergence still completes and tips agree.
///
/// Applied after the mesh is up (`four_node_mesh`), so the 200 ms profile
/// shapes established links only — the handshake stall at ≥~120 ms/chunk does
/// not apply here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_tcp_wan_asymmetric_delay_still_converges() {
    let asymmetric = WanProfile {
        latency_ms: 200,
        jitter_ms: 80,
        bandwidth_bps: 0,
    };
    // Only the first node's relay is shaped: asymmetric by construction.
    let profiles = [
        asymmetric,
        WanProfile::none(),
        WanProfile::none(),
        WanProfile::none(),
    ];
    let (nodes, _) = four_node_mesh(&profiles).await;

    for i in 0..2 {
        nodes[0]
            .submit_transaction(inv_tx(&format!("wan-delay-{i}"), 1))
            .await
            .unwrap();
        nodes[0].mine().await.unwrap();
    }
    let expected = tip(&nodes[0]).await;
    poll_until(
        "all nodes converged through the shaped link",
        20,
        || async {
            let snapshot = expected.clone();
            let mut all = true;
            for n in &nodes {
                if !tip_reached(n, snapshot.clone()).await {
                    all = false;
                }
            }
            all
        },
    )
    .await;
    assert!(
        tips_agree(&nodes).await,
        "asymmetric delay must not produce conflicting tips"
    );
}

/// D7 scenario 3 — partition while blocks are mined, then repair: re-convergence
/// with no conflicting finalization; time without quorum measured separately
/// from the recovery window and printed (not asserted).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_tcp_wan_partition_repair_converges_without_conflict() {
    let (nodes, proxies) = four_node_mesh(&[WanProfile::none(); 4]).await;

    nodes[0]
        .submit_transaction(inv_tx("pre-partition", 1))
        .await
        .unwrap();
    nodes[0].mine().await.unwrap();
    let synced = tip(&nodes[0]).await;
    poll_until("mesh synced the first block", 8, || async {
        let snapshot = synced.clone();
        let mut all = true;
        for n in &nodes {
            if !tip_reached(n, snapshot.clone()).await {
                all = false;
            }
        }
        all
    })
    .await;

    let quorum_lost_at = std::time::Instant::now();
    for proxy in &proxies {
        proxy.partition().await;
    }
    for i in 0..2 {
        nodes[0]
            .submit_transaction(inv_tx(&format!("during-{i}"), 1))
            .await
            .unwrap();
        nodes[0].mine().await.unwrap();
    }
    let ahead = tip(&nodes[0]).await;
    // Hold the partition past the built-in reconnect window: the proxies are
    // the only route (advertised addresses), so nothing can bypass them.
    tokio::time::sleep(Duration::from_secs(6)).await;
    for node in &nodes[1..] {
        assert_eq!(
            tip(node).await,
            synced,
            "a partitioned node must not receive blocks mined after the severance"
        );
    }
    assert!(ahead > synced, "the mining side advanced while partitioned");

    for proxy in &proxies {
        proxy.repair();
    }
    // Reconnect via the advertised (proxy) addresses.
    for node in nodes.iter().skip(1) {
        node.connect_peer(proxies[0].front_addr());
    }
    poll_until(
        "re-convergence after repair (recovery measured from repair)",
        15,
        || async {
            let snapshot = ahead.clone();
            let mut all = true;
            for n in &nodes {
                if !tip_reached(n, snapshot.clone()).await {
                    all = false;
                }
            }
            all
        },
    )
    .await;
    let quorum_restored_ms = quorum_lost_at.elapsed();
    println!("real_tcp_wan time-without-quorum (partition to convergence): {quorum_restored_ms:?}");
    assert!(
        tips_agree(&nodes).await,
        "partition + repair must not leave conflicting finalization"
    );
}
