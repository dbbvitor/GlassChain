//! Durable TOFU pins (#88): the peer registry survives restarts, a
//! transport-fingerprint change requires a signed rotation by the pinned
//! identity key, and a corrupt persisted pin fails closed.
//!
//! The node's transport certificate is regenerated on every construction, so
//! restarting a peer with the same identity is the natural rotation case: the
//! fingerprint changes, the identity key does not.

use glasschain_core::providers::in_memory::InMemoryStorageProvider;
use glasschain_core::StorageProvider;
use glasschain_identity::{CertChainVerifier, Organization};
use glasschain_network::Node;
use std::sync::Arc;
use std::time::Duration;

#[path = "common/ports.rs"]
mod ports;

use ports::free_addr;

/// A fail-closed verifier (ADR-013): the org's CRL rides along with its root.
fn verifier_with_crl(org: &Organization) -> CertChainVerifier {
    let mut verifier = CertChainVerifier::from_org(org).unwrap();
    verifier.add_crl_pem(&org.crl_pem().unwrap()).unwrap();
    verifier
}

async fn poll_for(desc: &str, secs: u64, mut condition: impl AsyncFnMut() -> bool) {
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

/// The persisted pin exists after restart, and only a proof by the pinned key
/// can move it: an impostor with the same node id and org but a different key
/// is refused, while the peer's own restart (new transport certificate, same
/// identity key) rotates the pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tofu_pins_survive_restart_and_rotate_only_with_the_pinned_key() {
    let _ = env_logger::try_init();
    let mut org = Organization::new("PharmaCorp").unwrap();
    let a_identity = org.issue_identity("node-a").unwrap().clone();
    let b_identity = org.issue_identity("node-b").unwrap().clone();
    // Same node id and org, different key: the impersonation attempt.
    let impostor_identity = org.issue_identity("node-b").unwrap().clone();
    let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());

    // ── First contact: A pins B ─────────────────────────────────────────────
    let a = Node::new_with_storage_and_identity(
        "node-a",
        free_addr(),
        1,
        Arc::clone(&storage),
        Arc::new(a_identity.clone()),
    );
    a.set_cert_verifier(verifier_with_crl(&org)).await;
    a.start(vec![]).await.unwrap();

    let b_addr = free_addr();
    let b = Node::new_with_identity("node-b", &b_addr, 1, Arc::new(b_identity.clone()));
    b.set_cert_verifier(verifier_with_crl(&org)).await;
    b.start(vec![a.listen_addr().to_owned()]).await.unwrap();
    poll_for("A pinned B", 5, || async {
        a.known_peers().await.iter().any(|p| p == &b_addr)
    })
    .await;

    // ── Restart: a new node over the same storage ───────────────────────────
    let restarted = Node::new_with_storage_and_identity(
        "node-a",
        free_addr(),
        1,
        Arc::clone(&storage),
        Arc::new(a_identity),
    );
    restarted.set_cert_verifier(verifier_with_crl(&org)).await;
    restarted.start(vec![]).await.unwrap();

    // The impostor claims B's advertised address with a different key:
    // the persisted pin refuses the unsigned rotation (without persistence it
    // would be accepted as first contact).
    let mut impostor =
        Node::new_with_identity("node-b", free_addr(), 1, Arc::new(impostor_identity));
    impostor.set_advertise_addr(&b_addr);
    impostor.set_cert_verifier(verifier_with_crl(&org)).await;
    impostor
        .start(vec![restarted.listen_addr().to_owned()])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(
        !restarted.known_peers().await.iter().any(|p| p == &b_addr),
        "a different key must not rotate a persisted pin"
    );

    // B restarts with the same identity (a fresh transport certificate): the
    // fingerprint changes, but the pinned key signs the rotation.
    let mut b2 = Node::new_with_identity("node-b", free_addr(), 1, Arc::new(b_identity));
    b2.set_advertise_addr(&b_addr);
    b2.set_cert_verifier(verifier_with_crl(&org)).await;
    b2.start(vec![restarted.listen_addr().to_owned()])
        .await
        .unwrap();
    poll_for("restarted node accepted the signed rotation", 5, || async {
        restarted.known_peers().await.iter().any(|p| p == &b_addr)
    })
    .await;
}

/// A corrupt persisted pin poisons its address: the peer is refused (fail
/// closed) instead of being silently re-pinned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_persisted_pin_refuses_the_address() {
    let _ = env_logger::try_init();
    let mut org = Organization::new("PharmaCorp").unwrap();
    let node_identity = org.issue_identity("node-a").unwrap().clone();
    let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
    storage
        .put_state("tofu:peer:127.0.0.1:9999", b"not a pin")
        .unwrap();

    let node = Node::new_with_storage_and_identity(
        "node-a",
        free_addr(),
        1,
        Arc::clone(&storage),
        Arc::new(node_identity),
    );
    node.set_cert_verifier(verifier_with_crl(&org)).await;
    node.start(vec![]).await.unwrap();

    let mut peer = Node::new("corrupt-peer", free_addr(), 1);
    peer.set_advertise_addr("127.0.0.1:9999");
    peer.start(vec![node.listen_addr().to_owned()])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(
        !node
            .known_peers()
            .await
            .iter()
            .any(|p| p == "127.0.0.1:9999"),
        "a corrupt persisted pin must refuse the peer"
    );
}
