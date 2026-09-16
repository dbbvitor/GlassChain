// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Failure paths of the pre-TLS certificate exchange: a client that hangs up
//! mid-exchange, advertises an invalid certificate length, truncates the
//! certificate, or completes the exchange but fails the TLS handshake — all
//! must leave the node's accept loop alive and unharmed.

use glasschain_network::Node;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::CertificateDer;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[path = "common/ports.rs"]
mod ports;

use ports::free_addr;

async fn node_with_cert() -> (Node, Vec<u8>, String) {
    let mut org = glasschain_identity::Organization::new("PharmaCorp").unwrap();
    let identity = org.issue_identity("prelude-node").unwrap().clone();
    let cert_pem = identity.certificate_pem.clone().unwrap();
    let cert_der = CertificateDer::from_pem_slice(cert_pem.as_bytes()).unwrap();

    let addr = free_addr();
    let node = Node::new_with_identity("prelude-node", &addr, 1, std::sync::Arc::new(identity));
    node.start(vec![]).await.unwrap();
    (node, cert_der.as_ref().to_vec(), addr)
}

/// A client that disconnects before sending its certificate length must not
/// disturb the node: the accept loop logs and moves on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_disconnect_before_cert_exchange_leaves_node_alive() {
    let (_node, _node_cert, addr) = node_with_cert().await;

    // Connect and drop without sending anything: the node's read of the
    // peer-certificate length hits EOF.
    let stream = TcpStream::connect(&addr).await.unwrap();
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The node still accepts healthy connections.
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    stream.write_all(&(2u32.to_be_bytes())).await.unwrap();
    stream.write_all(&[1u8, 2]).await.unwrap();
    let mut len_buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut len_buf))
        .await
        .expect("timeout")
        .expect("node read the truncated cert and still serves the exchange");
}

/// An advertised certificate length of zero (or beyond the 64 KiB cap) is
/// rejected before any certificate bytes are read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_peer_certificate_lengths_are_refused() {
    let (_node, _node_cert, addr) = node_with_cert().await;

    // Zero length.
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    stream.write_all(&0u32.to_be_bytes()).await.unwrap();
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut buf))
        .await
        .expect("timeout")
        .expect_err("the node refuses a 0-length certificate and closes");

    // Length beyond the 64 KiB cap.
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    stream
        .write_all(&u32::try_from(64 * 1024 + 1).unwrap().to_be_bytes())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut buf))
        .await
        .expect("timeout")
        .expect_err("the node refuses the oversized length and closes");
}

/// A truncated certificate body hits EOF mid-read; the node drops the
/// connection without a handshake.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncated_certificate_read_is_dropped() {
    let (_node, node_cert, addr) = node_with_cert().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    stream.write_all(&64u32.to_be_bytes()).await.unwrap();
    stream.write_all(&[1u8; 8]).await.unwrap(); // far short of 64 bytes

    // The node never answers: it is stuck waiting for the remaining bytes
    // until the client disconnects.
    tokio::time::sleep(Duration::from_millis(150)).await;
    drop(stream);

    // Healthy connections still work after the aborted exchange.
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let len = u32::try_from(node_cert.len()).unwrap();
    stream.write_all(&len.to_be_bytes()).await.unwrap();
    stream.write_all(&node_cert).await.unwrap();
    let mut len_buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut len_buf))
        .await
        .expect("timeout")
        .expect("node serves a healthy exchange");
}

/// A well-formed exchange followed by bytes that cannot complete the TLS
/// handshake is dropped by the TLS acceptor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn garbage_tls_handshake_is_refused_by_the_acceptor() {
    let (_node, node_cert, addr) = node_with_cert().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    // Complete the certificate exchange honestly…
    let len = u32::try_from(node_cert.len()).unwrap();
    stream.write_all(&len.to_be_bytes()).await.unwrap();
    stream.write_all(&node_cert).await.unwrap();
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let node_len = u32::from_be_bytes(len_buf) as usize;
    let mut node_cert_read = vec![0u8; node_len];
    stream.read_exact(&mut node_cert_read).await.unwrap();
    assert!(
        !node_cert_read.is_empty(),
        "exchange returned the node cert"
    );

    // …then send TLS garbage instead of a ClientHello.
    stream
        .write_all(&[0x16, 0x03, 0x99, 0x00, 0x05, 1, 2, 3, 4, 5])
        .await
        .unwrap();

    // The node drops the session; nothing usable comes back.
    let mut buf = [0u8; 16];
    let _ = tokio::time::timeout(Duration::from_millis(500), stream.read(&mut buf)).await;

    // The node is still alive for healthy peers.
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    stream.write_all(&len.to_be_bytes()).await.unwrap();
    stream.write_all(&node_cert).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut len_buf))
        .await
        .expect("timeout")
        .expect("node alive after garbage TLS");
}

// ── Dial-side prelude: the node connecting to a fake peer ────────────────────

/// A TCP listener that accepts and answers the certificate prelude in a
/// deliberately broken way.
enum FakePeer {
    /// Accepts and drops the stream: the node's read of the peer-certificate
    /// length hits EOF.
    SilentDrop,
    /// Answers a zero-length certificate advertisement.
    ZeroLength,
    /// Advertises a length and truncates the certificate body.
    Truncated,
}

fn spawn_fake_peer(kind: FakePeer) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt as _;
        use tokio::io::AsyncWriteExt as _;
        let (std_stream, _) = listener.accept().unwrap();
        let stream = tokio::net::TcpStream::from_std(std_stream).unwrap();
        let mut stream = tokio::io::BufReader::new(stream);
        // Read the node's advertised certificate (length + body) so the node
        // proceeds to reading ours.
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            return;
        }
        let node_len = u32::from_be_bytes(len_buf) as usize;
        let mut cert = vec![0u8; node_len];
        if stream.read_exact(&mut cert).await.is_err() {
            return;
        }
        match kind {
            FakePeer::SilentDrop => drop(stream),
            FakePeer::ZeroLength => {
                let _ = stream.write_all(&0u32.to_be_bytes()).await;
            }
            FakePeer::Truncated => {
                let _ = stream.write_all(&16u32.to_be_bytes()).await;
                let _ = stream.write_all(&[9u8; 3]).await;
                drop(stream);
            }
        }
        // Hold the task open briefly so the socket lingers.
        tokio::time::sleep(Duration::from_millis(300)).await;
    });
    addr
}

async fn dial_node_with_seed(seed: &str) -> Node {
    let addr = free_addr();
    let node = Node::new("dial-node", &addr, 1);
    node.start(vec![seed.to_owned()]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    node
}

/// A dial target that accepts TCP and then vanishes fails the pre-TLS
/// exchange without crashing the node.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dial_to_silent_peer_fails_cleanly() {
    let seed = spawn_fake_peer(FakePeer::SilentDrop);
    let _node = dial_node_with_seed(&seed).await;
}

/// A zero-length peer certificate advertisement is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dial_rejects_zero_length_peer_certificate() {
    let seed = spawn_fake_peer(FakePeer::ZeroLength);
    let _node = dial_node_with_seed(&seed).await;
}

/// A truncated peer certificate body is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dial_rejects_truncated_peer_certificate() {
    let seed = spawn_fake_peer(FakePeer::Truncated);
    let _node = dial_node_with_seed(&seed).await;
}
