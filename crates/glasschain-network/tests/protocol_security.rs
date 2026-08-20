//! Integration tests for the protocol/security branches of `process_message`
//! in `src/node.rs` that the high-level node-pair tests do not exercise.
//!
//! Rather than pairing two `Node`s, these tests drive a **raw TLS client** at
//! the wire protocol so a single branch can be hit exactly.  The wire protocol
//! is:
//! 1. a 4-byte big-endian length prefix followed by the peer's TLS certificate
//!    DER (an application-layer fingerprint exchange *before* the TLS
//!    handshake), then
//! 2. a TLS handshake (server issues self-signed `glasschain-node` cert,
//!    no client auth), then
//! 3. length-prefixed JSON `Message`s.
//!
//! Covered branches:
//! * Hello rejection: self-connection, advertised-vs-observed fingerprint
//!   mismatch, and TOFU identity drift.
//! * Block: unauthenticated guard, future-timestamp rejection, too-far-ahead →
//!   `RequestChain`, and stale/invalid → not appended.
//! * Transaction: unauthenticated peer guard.
//! * `RequestPeers` → `Peers` reply, `Peers` dedupe + re-dial, and `Goodbye`
//!   graceful handling.

use glasschain_core::crypto::sha256;
use glasschain_core::{Block, InventoryUpdate, Transaction, TransactionKind};
use glasschain_network::{
    Message, NetworkError, Node, NodeEvent, PeerReader, PeerWriter, PROTOCOL_VERSION,
};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::RootCertStore;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Pick an ephemeral loopback port that is very likely free.
fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

/// Certificate bytes presented by the raw test client in the pre-TLS exchange.
/// The node treats these as opaque and only fingerprints them, so fixed dummy
/// buffers suffice; tests pick different buffers when they need different
/// observed fingerprints.
const CLIENT_CERT_A: &[u8] = b"test-client-certificate-A-0123456789abcdef";
const CLIENT_CERT_B: &[u8] = b"test-client-certificate-B-0123456789abcdef";

/// Pre-TLS certificate exchange: write our DER bytes, then read the peer's.
async fn exchange_certs(stream: &mut TcpStream, our_cert: &[u8]) -> Vec<u8> {
    let len = u32::try_from(our_cert.len()).expect("cert fits u32");
    stream.write_all(&len.to_be_bytes()).await.unwrap();
    stream.write_all(our_cert).await.unwrap();
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let peer_len = u32::from_be_bytes(len_buf) as usize;
    let mut peer_cert = vec![0u8; peer_len];
    stream.read_exact(&mut peer_cert).await.unwrap();
    peer_cert
}

/// Connect a raw TLS client to `node_addr`, complete the pre-TLS certificate
/// exchange and TLS handshake, and return the framed reader/writer plus the
/// node's certificate DER (so tests can compute the node's own fingerprint).
async fn connect_raw(node_addr: &str, our_cert: &[u8]) -> (PeerReader, PeerWriter, Vec<u8>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut stream = TcpStream::connect(node_addr).await.unwrap();
    let node_cert = exchange_certs(&mut stream, our_cert).await;

    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(node_cert.clone())).unwrap();
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(cfg));
    let server_name = ServerName::try_from("glasschain-node")
        .expect("valid server name")
        .to_owned();
    let tls_stream = connector.connect(server_name, stream).await.unwrap();
    let (r, w) = tokio::io::split(tls_stream);
    (
        PeerReader::new(r, node_addr.to_owned()),
        PeerWriter::new(w, node_addr.to_owned()),
        node_cert,
    )
}

/// Send a `Hello` advertising the given TLS certificate fingerprint.
async fn send_hello(writer: &mut PeerWriter, node_id: &str, listen_addr: &str, fingerprint: &str) {
    let msg = Message::Hello {
        node_id: node_id.to_owned(),
        tls_cert_fingerprint: fingerprint.to_owned(),
        chain_length: 1,
        version: PROTOCOL_VERSION.to_owned(),
        listen_addr: listen_addr.to_owned(),
    };
    writer.send(&msg).await.unwrap();
}

/// Read and discard the `Hello` the node sends immediately on connect.
async fn read_node_hello(reader: &mut PeerReader) {
    let msg = timeout(Duration::from_secs(2), reader.receive())
        .await
        .expect("timeout waiting for node Hello")
        .expect("node Hello missing");
    assert!(matches!(msg, Message::Hello { .. }));
}

/// Connect and complete an honest Hello handshake: present `our_cert` and
/// advertise its observed fingerprint.
async fn complete_hello(
    node_addr: &str,
    our_cert: &[u8],
    node_id: &str,
    listen_addr: &str,
) -> (PeerReader, PeerWriter) {
    let (mut reader, mut writer, _node_cert) = connect_raw(node_addr, our_cert).await;
    read_node_hello(&mut reader).await;
    send_hello(&mut writer, node_id, listen_addr, &sha256(our_cert)).await;
    (reader, writer)
}

/// Wait until `addr` appears in the node's known-peers set; returns the set.
async fn wait_for_known_peer(node: &Node, addr: &str) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let peers = node.known_peers().await;
        if peers.iter().any(|p| p == addr) {
            return peers;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "peer {addr:?} was never recorded as known"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ── Process-wide log capture (for log-assert branches) ────────────────────────

static LOG_RECORDS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static LOGGER: CaptureLogger = CaptureLogger;

struct CaptureLogger;

impl log::Log for CaptureLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if let Some(records) = LOG_RECORDS.get() {
            records.lock().unwrap().push(record.args().to_string());
        }
    }
    fn flush(&self) {}
}

/// Install the process-wide log capture (idempotent; shared by all tests).
fn init_log_capture() {
    LOG_RECORDS.get_or_init(|| Mutex::new(Vec::new()));
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

/// Wait until some captured log record contains `substring`.
async fn wait_for_log(substring: &str) {
    init_log_capture();
    let records = LOG_RECORDS.get().expect("log capture initialised");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let hit = records
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains(substring));
        if hit {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "log record containing {substring:?} was never observed"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ── Hello rejection branches ───────────────────────────────────────────────────

/// (1a) A Hello advertising this node's own TLS fingerprint is a
/// self-connection and must be disconnected.
#[tokio::test]
async fn hello_self_connection_is_disconnected() {
    let addr = free_addr();
    let node = Node::new("self-test-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (mut reader, mut writer, node_cert) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader).await;

    // Advertise the node's own certificate fingerprint → self-connection.
    let node_fp = sha256(&node_cert);
    send_hello(&mut writer, "evil-self", "127.0.0.1:1", &node_fp).await;

    let result = timeout(Duration::from_secs(2), reader.receive())
        .await
        .expect("timeout waiting for self-connection disconnect")
        .expect_err("self-connection must trigger a disconnect");
    assert!(matches!(result, NetworkError::PeerDisconnected(_)));
}

/// (1b) A Hello whose advertised fingerprint does not match the observed
/// session certificate fingerprint must be disconnected.
#[tokio::test]
async fn hello_fingerprint_mismatch_is_disconnected() {
    let addr = free_addr();
    let node = Node::new("fp-test-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (mut reader, mut writer, _node_cert) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader).await;

    // Present CLIENT_CERT_A (observed = sha256(A)) but advertise a different
    // fingerprint → session-level verification must reject the peer.
    let bogus_fp = "0".repeat(64);
    send_hello(&mut writer, "imposter", "127.0.0.1:1", &bogus_fp).await;

    let result = timeout(Duration::from_secs(2), reader.receive())
        .await
        .expect("timeout waiting for fingerprint-mismatch disconnect")
        .expect_err("fingerprint mismatch must trigger a disconnect");
    assert!(matches!(result, NetworkError::PeerDisconnected(_)));
}

/// (1c) TOFU: a reconnect claiming the same listen address + `node_id` but a
/// changed certificate fingerprint must be rejected by the peer registry.
#[tokio::test]
async fn hello_tofu_rejects_changed_identity_on_same_listen_addr() {
    let addr = free_addr();
    let node = Node::new("tofu-test-node", &addr, 1);
    node.start(vec![]).await.unwrap();
    let mut events = node.subscribe();

    let listen_addr = "127.0.0.1:31000";

    // First contact: register the identity for `listen_addr` (cert A).
    let (mut reader1, mut writer1, _) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader1).await;
    send_hello(
        &mut writer1,
        "tofu-peer",
        listen_addr,
        &sha256(CLIENT_CERT_A),
    )
    .await;

    // Wait until node A has recorded the connection (PeerConnected event),
    // then drop the first session.
    let evt = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout waiting for first PeerConnected")
        .expect("event channel closed");
    assert!(matches!(evt, NodeEvent::PeerConnected(_)));
    drop(reader1);
    drop(writer1);

    // Second contact: same listen address + node_id, but a different cert
    // (changed observed fingerprint) → TOFU registry rejects the peer.
    let (mut reader2, mut writer2, _) = connect_raw(&addr, CLIENT_CERT_B).await;
    read_node_hello(&mut reader2).await;
    send_hello(
        &mut writer2,
        "tofu-peer",
        listen_addr,
        &sha256(CLIENT_CERT_B),
    )
    .await;

    let result = timeout(Duration::from_secs(2), reader2.receive())
        .await
        .expect("timeout waiting for TOFU-rejection disconnect")
        .expect_err("TOFU identity drift must trigger a disconnect");
    assert!(matches!(result, NetworkError::PeerDisconnected(_)));
}

// ── Block handler branches ────────────────────────────────────────────────────

/// (2a) A block from a peer that never completed Hello is ignored and the
/// connection stays usable.
#[tokio::test]
async fn block_from_unauthenticated_peer_is_ignored() {
    let addr = free_addr();
    let node = Node::new("unauth-block-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    // Connect + TLS but never send a Hello → current_stable_addr stays None.
    let (mut reader, mut writer, _node_cert) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader).await;

    // A *valid* block (chained to genesis, correct PoW) so that, if the auth
    // guard were missing, it would be appended and the test would fail.
    let genesis = node.ledger_snapshot().await.chain[0].clone();
    let mut block = Block::new(1, vec![], genesis.hash.clone());
    block.mine(1);
    writer.send(&Message::Block(block)).await.unwrap();

    // The guard returns without disconnecting: probe with RequestChain and
    // confirm the chain was not changed.
    writer.send(&Message::RequestChain).await.unwrap();
    let reply = timeout(Duration::from_secs(2), reader.receive())
        .await
        .expect("timeout waiting for chain reply")
        .expect("chain reply missing");
    match reply {
        Message::Chain(chain) => {
            assert_eq!(chain.len(), 1, "unauthenticated block must not be appended");
        }
        other => panic!("expected Chain reply, got {other:?}"),
    }
}

/// (2b) A block with a timestamp more than 2 hours in the future is rejected
/// before any chain logic runs.
#[tokio::test]
async fn block_with_future_timestamp_is_rejected() {
    init_log_capture();
    let addr = free_addr();
    let node = Node::new("future-block-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (_reader, mut writer) =
        complete_hello(&addr, CLIENT_CERT_A, "honest-client", "127.0.0.1:1").await;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut block = Block::new(1, vec![], "0".into());
    block.timestamp = now + 10_000; // ~2.8h in the future (> 2h budget)
    writer.send(&Message::Block(block)).await.unwrap();

    // The specific rejection log confirms the branch fired.
    wait_for_log("seconds in the future").await;
    assert_eq!(node.ledger_snapshot().await.chain.len(), 1);
}

/// (2c) A block whose index is ahead of our tip triggers a `RequestChain`.
#[tokio::test]
async fn block_too_far_ahead_requests_chain() {
    let addr = free_addr();
    let node = Node::new("ahead-block-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (mut reader, mut writer) =
        complete_hello(&addr, CLIENT_CERT_A, "honest-client", "127.0.0.1:1").await;

    let block = Block::new(5, vec![], "0".into()); // tip is index 0 → 5 is ahead
    writer.send(&Message::Block(block)).await.unwrap();

    let reply = timeout(Duration::from_secs(2), reader.receive())
        .await
        .expect("timeout waiting for RequestChain")
        .expect("reply missing");
    assert!(
        matches!(reply, Message::RequestChain),
        "expected RequestChain, got {reply:?}"
    );
    assert_eq!(node.ledger_snapshot().await.chain.len(), 1);
}

/// (2d) A block at the expected index that does not chain to the previous
/// block is treated as stale/invalid and not appended.
#[tokio::test]
async fn invalid_stale_block_is_not_appended() {
    init_log_capture();
    let addr = free_addr();
    let node = Node::new("stale-block-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (mut reader, mut writer) =
        complete_hello(&addr, CLIENT_CERT_A, "honest-client", "127.0.0.1:1").await;

    // Index matches the expected tip (1) but previous_hash does not chain to
    // the genesis block and its PoW never satisfies the target.
    let block = Block::new(1, vec![], "0".into());
    writer.send(&Message::Block(block)).await.unwrap();

    wait_for_log("Received invalid or stale block from").await;
    assert_eq!(node.ledger_snapshot().await.chain.len(), 1);

    // Equal-index blocks do not trigger a RequestChain.
    assert!(
        timeout(Duration::from_millis(400), reader.receive())
            .await
            .is_err(),
        "expected no message for an invalid/stale block"
    );
}

// ── Transaction handler branch ────────────────────────────────────────────────

/// (3) A transaction from a peer that never completed Hello is ignored.
#[tokio::test]
async fn transaction_from_unauthenticated_peer_is_ignored() {
    let addr = free_addr();
    let node = Node::new("unauth-tx-node", &addr, 1);
    node.start(vec![]).await.unwrap();
    let mut events = node.subscribe();

    let (mut reader, mut writer, _node_cert) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader).await;

    let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-UNAUTH".into(),
        owner_id: "rogue".into(),
        quantity_delta: 10,
        reason: "unauth probe".into(),
    }));
    writer.send(&Message::Transaction(tx)).await.unwrap();

    // Nothing added and no acceptance event.
    assert!(node.ledger_snapshot().await.pending_transactions.is_empty());
    assert!(
        timeout(Duration::from_millis(300), events.recv())
            .await
            .is_err(),
        "unauthenticated transaction must not be accepted"
    );

    // The connection stays usable (guard does not disconnect).
    writer.send(&Message::RequestChain).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(2), reader.receive())
            .await
            .expect("timeout waiting for chain reply")
            .expect("chain reply missing"),
        Message::Chain(_)
    ));
}

// ── RequestPeers / Peers / Goodbye handlers ───────────────────────────────────

/// (4) A `RequestPeers` elicits a `Peers` reply containing known peers but not the
/// requester itself.
#[tokio::test]
async fn request_peers_returns_known_peers_excluding_requester() {
    let addr_a = free_addr();
    let node_a = Node::new("peers-a", &addr_a, 1);
    node_a.start(vec![]).await.unwrap();

    // A second node joins so node A has a known peer other than the requester.
    let addr_b = free_addr();
    let node_b = Node::new("peers-b", &addr_b, 1);
    node_b.start(vec![addr_a.clone()]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let our_listen = "127.0.0.1:99"; // distinct from addr_b
    let (mut reader, mut writer) =
        complete_hello(&addr_a, CLIENT_CERT_A, "peers-requester", our_listen).await;

    writer.send(&Message::RequestPeers).await.unwrap();
    let reply = timeout(Duration::from_secs(2), reader.receive())
        .await
        .expect("timeout waiting for Peers reply")
        .expect("reply missing");
    match reply {
        Message::Peers(peers) => {
            assert_eq!(peers, vec![addr_b], "expected only the other known peer");
        }
        other => panic!("expected Peers reply, got {other:?}"),
    }
}

/// (5) A `Peers` message is deduplicated and each new address is re-dialed.
#[tokio::test]
async fn peers_message_dedupes_and_redials() {
    let addr = free_addr();
    let node = Node::new("peers-msg-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    // A listener we control; the node should dial each deduped new address.
    let dial_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dial_addr = dial_listener.local_addr().unwrap().to_string();
    let (dialed_tx, mut dialed_rx) = tokio::sync::mpsc::channel(1);
    let listener_task = tokio::spawn(async move {
        if let Ok(Ok((_stream, _peer))) =
            timeout(Duration::from_secs(3), dial_listener.accept()).await
        {
            let _ = dialed_tx.send(()).await;
        }
    });

    let dead_addr = free_addr(); // nothing listens here; this dial simply fails

    let (_reader, mut writer) =
        complete_hello(&addr, CLIENT_CERT_A, "peers-sender", "127.0.0.1:98").await;
    // Advertise dial_addr twice (dedupe) and one unreachable address.
    writer
        .send(&Message::Peers(vec![
            dial_addr.clone(),
            dial_addr.clone(),
            dead_addr.clone(),
        ]))
        .await
        .unwrap();

    // Dedupe: each advertised address is recorded exactly once.
    let peers = wait_for_known_peer(&node, &dial_addr).await;
    assert_eq!(peers.iter().filter(|p| *p == &dial_addr).count(), 1);
    assert_eq!(peers.iter().filter(|p| *p == &dead_addr).count(), 1);

    // Re-dial: the node connects to the newly advertised listener.
    timeout(Duration::from_secs(3), dialed_rx.recv())
        .await
        .expect("timeout waiting for re-dial")
        .expect("re-dial signal missing");
    assert!(listener_task.await.is_ok());
}

/// (6) A Goodbye is acknowledged gracefully (logged) without the node
/// terminating the session; teardown happens when the peer closes.
#[tokio::test]
async fn goodbye_is_handled_gracefully() {
    init_log_capture();
    let addr = free_addr();
    let node = Node::new("goodbye-node", &addr, 1);
    node.start(vec![]).await.unwrap();
    let mut events = node.subscribe();

    let reason = "integration-test-shutdown";
    let (mut reader, mut writer) =
        complete_hello(&addr, CLIENT_CERT_A, "goodbye-client", "127.0.0.1:97").await;
    writer
        .send(&Message::Goodbye {
            reason: reason.to_owned(),
        })
        .await
        .unwrap();

    // The branch logs the goodbye and keeps the connection open: the session
    // is still usable for a subsequent request.
    wait_for_log(&format!("says goodbye: {reason}")).await;
    writer.send(&Message::RequestChain).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(2), reader.receive())
            .await
            .expect("timeout waiting for chain reply")
            .expect("chain reply missing"),
        Message::Chain(_)
    ));

    // The client then closes, and the node reports a graceful disconnect.
    drop(reader);
    drop(writer);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut got_disconnect = false;
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(500), events.recv()).await {
            Ok(Ok(NodeEvent::PeerDisconnected(_))) => {
                got_disconnect = true;
                break;
            }
            Ok(Ok(_)) => {} // skip earlier events (e.g. PeerConnected)
            _ => break,
        }
    }
    assert!(got_disconnect, "PeerDisconnected event was not emitted");
}
