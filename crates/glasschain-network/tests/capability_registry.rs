/// Node-level scenarios for the capability registry (ticket #36): future-height
/// activation commits and flips the effective set exactly at the declared
/// height, old blocks keep their meaning, same-block transitions are rejected,
/// replay derives the same history, wire-version mismatches disconnect, and
/// peers lacking an active capability become read-only observers.
use glasschain_core::crypto::sha256;
use glasschain_core::{
    capability_hash, CapabilityActivation, CapabilityAdvertisement, CapabilityHistory,
    InventoryUpdate, RecordSignature, Transaction, TransactionKind,
};
use glasschain_network::{Message, NetworkError, Node, PeerReader, PeerWriter, PROTOCOL_VERSION};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::RootCertStore;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

const CLIENT_CERT_A: &[u8] = b"capability-test-client-cert-A-0123456789";

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
    let server_name = ServerName::try_from("glasschain-node").unwrap();
    let tls = connector.connect(server_name, stream).await.unwrap();
    let (r, w) = tokio::io::split(tls);
    (
        PeerReader::new(r, node_addr.to_owned()),
        PeerWriter::new(w, node_addr.to_owned()),
        node_cert,
    )
}

async fn read_node_hello(reader: &mut PeerReader) {
    let msg = timeout(Duration::from_secs(2), reader.receive())
        .await
        .expect("timeout waiting for node Hello")
        .expect("node Hello missing");
    assert!(matches!(msg, Message::Hello { .. }));
}

async fn send_hello(
    writer: &mut PeerWriter,
    node_id: &str,
    listen_addr: &str,
    fingerprint: &str,
    version: &str,
    capabilities: Vec<CapabilityAdvertisement>,
) {
    let msg = Message::Hello {
        node_id: node_id.to_owned(),
        tls_cert_fingerprint: fingerprint.to_owned(),
        chain_length: 1,
        version: version.to_owned(),
        capabilities,
        org: "org-test".to_owned(),
        certificate_pem: None,
        listen_addr: listen_addr.to_owned(),
    };
    writer.send(&msg).await.unwrap();
}

fn activation_tx(id: &str, height: u64) -> Transaction {
    Transaction::with_id(
        format!("cap:{id}:{height}"),
        TransactionKind::CapabilityActivation(CapabilityActivation {
            capability_id: id.into(),
            version: 1,
            hash: capability_hash(id, 1),
            activation_height: height,
            signatures: vec![RecordSignature {
                signer: "org-issuer".into(),
                signature_bytes: vec![0x42],
            }],
        }),
    )
}

// ── Scenarios ─────────────────────────────────────────────────────────────────

/// A future-height activation commits, and the effective set flips exactly at
/// the declared height — never midway through its own block, and old blocks
/// keep their meaning (the whole chain still validates).
#[tokio::test]
async fn test_future_height_activation_flips_effective_set() {
    let addr = free_addr();
    let node = Node::new("cap-activate", &addr, 1);
    node.start(vec![]).await.unwrap();

    // Tip is genesis (index 0); activation lands in block 1 and declares
    // height 4 — strictly future of its own block.
    node.submit_transaction(activation_tx("bft_consensus", 4))
        .await
        .expect("future activation must be admitted");
    for _ in 0..4 {
        node.mine().await.expect("mine");
    }

    let ledger = node.ledger_snapshot().await;
    assert!(ledger.validate_chain().is_ok(), "chain stays valid");
    let history = CapabilityHistory::build_from_blocks(&ledger.chain).expect("valid history");
    assert!(
        !history.effective_set(3).is_active("bft_consensus"),
        "activation must not take effect before its declared height"
    );
    assert!(history.effective_set(4).is_active("bft_consensus"));
    assert!(history.effective_set(10).is_active("bft_consensus"));
    assert!(
        history.effective_set(3).is_active("state_commitment"),
        "genesis capabilities stay active for old heights"
    );
}

/// An activation that would take effect in the block carrying it (or earlier)
/// is rejected at admission.
#[tokio::test]
async fn test_same_block_activation_rejected_at_admission() {
    let addr = free_addr();
    let node = Node::new("cap-same-block", &addr, 1);
    node.start(vec![]).await.unwrap();

    // Tip is genesis; the next block is index 1, so declaring height 1 is not
    // strictly future.
    let error = node
        .submit_transaction(activation_tx("pdc", 1))
        .await
        .expect_err("same-block activation must be rejected");
    assert!(error.to_string().contains("future"), "{error}");

    let ledger = node.ledger_snapshot().await;
    assert!(ledger.pending_transactions.is_empty(), "no partial state");
}

/// A syncing node derives the same capability history from committed blocks.
#[tokio::test]
async fn test_replay_derives_same_history_after_sync() {
    let addr_a = free_addr();
    let node_a = Node::new("cap-a", &addr_a, 1);
    node_a.start(vec![]).await.unwrap();
    node_a
        .submit_transaction(activation_tx("endorsement", 3))
        .await
        .expect("admit");
    for _ in 0..3 {
        node_a.mine().await.expect("mine");
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let addr_b = free_addr();
    let node_b = Node::new("cap-b", &addr_b, 1);
    node_b.start(vec![addr_a.clone()]).await.expect("start b");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let ledger_b = node_b.ledger_snapshot().await;
    let history_b = CapabilityHistory::build_from_blocks(&ledger_b.chain).expect("replay");
    assert!(
        history_b.effective_set(2).is_active("state_commitment")
            && !history_b.effective_set(2).is_active("endorsement")
    );
    assert!(history_b.effective_set(3).is_active("endorsement"));
}

/// A peer that advertises no capabilities is downgraded to a read-only
/// observer: its transactions are ignored and never reach the pending pool.
#[tokio::test]
async fn test_unsupported_peer_becomes_read_only() {
    let addr = free_addr();
    let node = Node::new("cap-ro-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (mut reader, mut writer, _) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader).await;
    send_hello(
        &mut writer,
        "old-peer",
        "127.0.0.1:1",
        &sha256(CLIENT_CERT_A),
        PROTOCOL_VERSION,
        vec![], // supports no capabilities
    )
    .await;

    // The read-only peer proposes a valid transaction; the node must ignore it.
    let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-RO".into(),
        owner_id: "old-peer".into(),
        quantity_delta: 10,
        reason: "read-only proposal".into(),
    }));
    writer.send(&Message::Transaction(tx)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let ledger = node.ledger_snapshot().await;
    assert!(
        ledger.pending_transactions.is_empty(),
        "read-only observers may not propose writes"
    );
}

/// A peer advertising an active capability at the wrong version cannot
/// support it and is a read-only observer.
#[tokio::test]
async fn test_wrong_version_advertisement_is_read_only() {
    let addr = free_addr();
    let node = Node::new("cap-wrong-version", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (mut reader, mut writer, _) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader).await;
    send_hello(
        &mut writer,
        "wrong-version-peer",
        "127.0.0.1:3",
        &sha256(CLIENT_CERT_A),
        PROTOCOL_VERSION,
        vec![CapabilityAdvertisement {
            id: "state_commitment".into(),
            version: 2, // not the active version
        }],
    )
    .await;

    let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-WV".into(),
        owner_id: "wrong-version-peer".into(),
        quantity_delta: 1,
        reason: "wrong-version proposal".into(),
    }));
    writer.send(&Message::Transaction(tx)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let ledger = node.ledger_snapshot().await;
    assert!(
        ledger.pending_transactions.is_empty(),
        "a wrong-version advertisement must not grant write rights"
    );
}

/// Read-only status is re-evaluated against the tip: a peer that supports the
/// genesis set loses write rights once a capability it lacks activates.
#[tokio::test]
async fn test_read_only_reevaluated_after_activation() {
    let addr = free_addr();
    let node = Node::new("cap-reeval", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (mut reader, mut writer, _) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader).await;
    // The peer supports exactly the genesis-active capabilities.
    send_hello(
        &mut writer,
        "genesis-peer",
        "127.0.0.1:4",
        &sha256(CLIENT_CERT_A),
        PROTOCOL_VERSION,
        vec![
            CapabilityAdvertisement {
                id: "canonical_schema_v1".into(),
                version: 1,
            },
            CapabilityAdvertisement {
                id: "state_commitment".into(),
                version: 1,
            },
        ],
    )
    .await;

    // Before the activation the peer has full write rights.
    let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-EARLY".into(),
        owner_id: "genesis-peer".into(),
        quantity_delta: 1,
        reason: "pre-activation proposal".into(),
    }));
    writer.send(&Message::Transaction(tx)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    {
        let ledger = node.ledger_snapshot().await;
        assert_eq!(
            ledger.pending_transactions.len(),
            1,
            "pre-activation write lands"
        );
    }
    node.mine().await.expect("mine the early tx");

    // Activate `pdc` (which the peer does not advertise) at height 3.
    node.submit_transaction(activation_tx("pdc", 3))
        .await
        .expect("future activation admitted");
    node.mine().await.expect("block 2 carries the activation");
    node.mine()
        .await
        .expect("block 3 reaches the declared height");

    // The same peer now lacks an active capability → read-only.
    let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-LATE".into(),
        owner_id: "genesis-peer".into(),
        quantity_delta: 1,
        reason: "post-activation proposal".into(),
    }));
    writer.send(&Message::Transaction(tx)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let ledger = node.ledger_snapshot().await;
    assert!(
        ledger.pending_transactions.is_empty(),
        "a peer lacking a newly active capability must become read-only"
    );
}

/// A peer with an incompatible wire-encoding version is disconnected.
#[tokio::test]
async fn test_protocol_version_mismatch_disconnects() {
    let addr = free_addr();
    let node = Node::new("cap-version-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    let (mut reader, mut writer, _) = connect_raw(&addr, CLIENT_CERT_A).await;
    read_node_hello(&mut reader).await;
    send_hello(
        &mut writer,
        "old-protocol-peer",
        "127.0.0.1:2",
        &sha256(CLIENT_CERT_A),
        "glasschain/0",
        vec![],
    )
    .await;

    let result = timeout(Duration::from_secs(2), reader.receive())
        .await
        .expect("timeout waiting for version-mismatch disconnect")
        .expect_err("incompatible version must disconnect");
    assert!(matches!(result, NetworkError::PeerDisconnected(_)));
}
