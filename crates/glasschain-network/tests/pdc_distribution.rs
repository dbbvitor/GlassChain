//! Node-level private-payload distribution scenarios (ticket #47, ADR-003):
//! offline catch-up via pull reconciliation, retention/purge, and
//! certificate-verified delivery.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use glasschain_core::{
    crypto::sha256, CapabilityActivation, RecordSignature, Transaction, TransactionKind, WriteOp,
    WriteVisibility,
};
use glasschain_identity::{CertChainVerifier, Channel, ChannelConfig, Organization};
use glasschain_network::Node;
use std::sync::Arc;
use std::time::Duration;

const WRITER: &str = "org-writer";
const MEMBER_PEER: &str = "org-member";
const COLLECTION: &str = "pricing";
const PRIVATE_VALUE: &[u8] = &[0xDE, 0xAD, 0xBE, 0xEF];

fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

fn pricing_collection(retention_secs: u64) -> Channel {
    Channel::new(ChannelConfig {
        name: COLLECTION.to_owned(),
        member_ids: vec![WRITER.to_owned(), MEMBER_PEER.to_owned()],
        description: "Private pricing collection".to_owned(),
        endorsement_policy: None,
        retention_secs,
    })
}

fn activation_tx(height: u64) -> Transaction {
    Transaction::with_id(
        format!("cap:pdc:{height}"),
        TransactionKind::CapabilityActivation(CapabilityActivation {
            capability_id: "pdc".into(),
            version: 1,
            hash: glasschain_core::capability_hash("pdc", 1),
            activation_height: height,
            signatures: vec![RecordSignature {
                signer: "org-gov".into(),
                signature_bytes: vec![0x42],
            }],
        }),
    )
}

/// A guest that writes one PDC-scoped value at runtime (never a data segment).
const fn pdc_contract_wat() -> &'static str {
    r#"
(module
  (import "env" "persist_state" (func $persist (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "supply")
  (data (i32.const 10) "inventory")
  (data (i32.const 20) "price")
  (data (i32.const 40) "pricing")
  (func (export "execute")
    (i32.store (i32.const 30) (i32.const 0xEFBEADDE))
    (drop (call $persist
      (i32.const 0) (i32.const 6)
      (i32.const 10) (i32.const 9)
      (i32.const 20) (i32.const 5)
      (i32.const 30) (i32.const 4)
      (i32.const 0) (i32.const 1)
      (i32.const 40) (i32.const 7)))
  )
)
"#
}

fn pdc_contract_wasm() -> Vec<u8> {
    wat::parse_str(pdc_contract_wat()).expect("WAT compiles")
}

fn contract_creation_tx(contract_id: &str) -> Transaction {
    Transaction::new(TransactionKind::ContractCreation(
        glasschain_core::SmartContractDef {
            contract_id: contract_id.to_owned(),
            buyer_id: "buyer-1".into(),
            product_id: "SKU-1".into(),
            conditions: glasschain_core::PurchaseConditions {
                max_price_per_unit: 2000,
                min_quantity: 1,
                max_quantity: 50,
                max_lead_time_days: 14,
                preferred_seller_id: None,
                currency: "BRL".into(),
                auto_execute: false,
            },
            wasm_code_b64: Some(BASE64_STANDARD.encode(pdc_contract_wasm())),
        },
    ))
}

fn contract_execution_tx(contract_id: &str) -> Transaction {
    Transaction::new(TransactionKind::ContractExecution(
        glasschain_core::ContractExecution {
            contract_id: contract_id.to_owned(),
            purchase_order_tx_id: "po-1".into(),
            buyer_id: "buyer-1".into(),
            seller_id: "seller-1".into(),
            product_id: "SKU-1".into(),
            quantity: 10,
            currency: "BRL".into(),
            total_price: 15_000,
        },
    ))
}

/// Poll until `condition` holds, or panic after `secs` — never hang.
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

async fn wait_for_sync(node: &Node, peer: &Node) {
    poll_until("peer chain sync", 5, || async {
        peer.ledger_snapshot().await.chain.len() >= node.ledger_snapshot().await.chain.len()
    })
    .await;
}

/// A member that joins **after** dissemination catches up: the writer
/// disseminates while no member peer is connected, the member then syncs the
/// chain and pulls the missing payload via reconciliation.
#[tokio::test]
async fn offline_member_catches_up_via_reconciliation() {
    let writer_addr = free_addr();
    let writer = Node::new(WRITER, &writer_addr, 1);
    writer.start(vec![]).await.unwrap();
    writer.set_collections(vec![pricing_collection(3600)]).await;
    writer
        .set_execution_provider(Arc::new(
            glasschain_vm::WasmExecutionProvider::new().unwrap(),
        ))
        .await;

    // Activate `pdc`, commit the PDC write: no member peer is connected yet,
    // so the payload is held by the writer alone.
    writer.submit_transaction(activation_tx(2)).await.unwrap();
    writer.mine().await.unwrap();
    writer
        .submit_transaction(contract_creation_tx("pdc-catchup"))
        .await
        .unwrap();
    writer
        .submit_transaction(contract_execution_tx("pdc-catchup"))
        .await
        .unwrap();
    writer.mine().await.unwrap();

    let commitment = sha256(PRIVATE_VALUE);
    assert_eq!(
        writer.transient_payload(COLLECTION, &commitment).await,
        Some(PRIVATE_VALUE.to_vec()),
        "the writer holds its own payload"
    );

    // The member joins after the fact and syncs the chain.
    let member = Node::new(MEMBER_PEER, free_addr(), 1);
    member.start(vec![writer_addr]).await.unwrap();
    member.set_collections(vec![pricing_collection(3600)]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    wait_for_sync(&writer, &member).await;
    assert_eq!(
        member.transient_payload(COLLECTION, &commitment).await,
        None,
        "the member has not received the payload yet"
    );

    // Pull reconciliation: the member requests exactly what it is missing.
    let requested = member
        .reconcile_private_payloads(COLLECTION)
        .await
        .expect("reconcile");
    assert_eq!(requested, 1, "one missing payload requested");

    poll_until("member received the reconciled payload", 3, || async {
        member.transient_payload(COLLECTION, &commitment).await == Some(PRIVATE_VALUE.to_vec())
    })
    .await;

    // A non-member reconciles nothing.
    let outsider = Node::new("org-outsider", free_addr(), 1);
    outsider.start(vec![free_addr()]).await.unwrap();
    outsider
        .set_collections(vec![pricing_collection(3600)])
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        outsider
            .reconcile_private_payloads(COLLECTION)
            .await
            .unwrap(),
        0,
        "a non-member must not request private payloads"
    );
}

/// Purge removes expired payloads; the chain's commitments persist forever —
/// the committed block stays valid and still names the commitment.
#[tokio::test]
async fn purge_removes_payloads_commitments_persist() {
    let writer = Node::new(WRITER, free_addr(), 1);
    writer.start(vec![]).await.unwrap();
    writer.set_collections(vec![pricing_collection(1)]).await;
    writer
        .set_execution_provider(Arc::new(
            glasschain_vm::WasmExecutionProvider::new().unwrap(),
        ))
        .await;

    writer.submit_transaction(activation_tx(2)).await.unwrap();
    writer.mine().await.unwrap();
    writer
        .submit_transaction(contract_creation_tx("pdc-purge"))
        .await
        .unwrap();
    writer
        .submit_transaction(contract_execution_tx("pdc-purge"))
        .await
        .unwrap();
    writer.mine().await.unwrap();

    let commitment = sha256(PRIVATE_VALUE);
    assert_eq!(
        writer.transient_payload(COLLECTION, &commitment).await,
        Some(PRIVATE_VALUE.to_vec()),
        "the payload is held before retention expires"
    );

    // Record the committed commitment, then let the 1s retention expire.
    let chain = writer.ledger_snapshot().await.chain;
    let committed = chain
        .iter()
        .flat_map(|block| &block.write_set)
        .find(|write| matches!(write.visibility, WriteVisibility::Pdc(_)))
        .cloned()
        .expect("the PDC write is committed");
    let WriteOp::Set(committed_value) = &committed.op else {
        panic!("expected a set");
    };
    assert_eq!(committed_value.as_slice(), commitment.as_bytes());

    tokio::time::sleep(Duration::from_millis(1200)).await;
    let purged = writer
        .purge_expired_private_payloads()
        .await
        .expect("purge");
    assert_eq!(purged, 1, "the expired payload is purged");
    assert_eq!(
        writer.transient_payload(COLLECTION, &commitment).await,
        None,
        "the payload is gone after purge"
    );

    // The commitment persists: the committed block is untouched and still
    // valid, so a late auditor can prove existence and consistency.
    let chain_after = writer.ledger_snapshot().await.chain;
    let still_committed = chain_after
        .iter()
        .flat_map(|block| &block.write_set)
        .find(|write| matches!(write.visibility, WriteVisibility::Pdc(_)))
        .cloned()
        .expect("the commitment persists after purge");
    assert_eq!(
        still_committed.op, committed.op,
        "the chain's commitment is unaltered by the purge"
    );
    assert!(chain_after.iter().all(glasschain_core::Block::is_valid));
}

/// Certificate-verified delivery: with a `cert_verifier` configured, the
/// payload path trusts only certificate-verified orgs (the TLS cert's subject
/// CN), not the self-asserted `Hello` org.
#[tokio::test]
async fn identity_verified_payload_delivery() {
    let mut org = Organization::new("PharmaCorp").unwrap();
    let writer_identity = org.issue_identity(WRITER).unwrap().clone();
    let member_identity = org.issue_identity(MEMBER_PEER).unwrap().clone();

    let writer_addr = free_addr();
    let writer = Node::new_with_identity(WRITER, &writer_addr, 1, Arc::new(writer_identity));
    writer.start(vec![]).await.unwrap();
    writer.set_collections(vec![pricing_collection(3600)]).await;
    writer
        .set_execution_provider(Arc::new(
            glasschain_vm::WasmExecutionProvider::new().unwrap(),
        ))
        .await;

    let member = Node::new_with_identity(MEMBER_PEER, free_addr(), 1, Arc::new(member_identity));
    // The member verifies private-payload senders against the org Root CA —
    // configured before start so the first handshake is already verified.
    member
        .set_cert_verifier(CertChainVerifier::from_org(&org).unwrap())
        .await;
    member.set_collections(vec![pricing_collection(3600)]).await;
    member.start(vec![writer_addr]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Activate `pdc` and commit the PDC write: the writer's org is
    // certificate-verified (its TLS cert CN is its node id, issued by the org
    // CA), so the member accepts the payload.
    writer.submit_transaction(activation_tx(2)).await.unwrap();
    writer.mine().await.unwrap();
    writer
        .submit_transaction(contract_creation_tx("pdc-verified"))
        .await
        .unwrap();
    writer
        .submit_transaction(contract_execution_tx("pdc-verified"))
        .await
        .unwrap();
    writer.mine().await.unwrap();

    let commitment = sha256(PRIVATE_VALUE);
    poll_until("member received the verified payload", 3, || async {
        member.transient_payload(COLLECTION, &commitment).await == Some(PRIVATE_VALUE.to_vec())
    })
    .await;

    // A member running verification rejects a payload from an unverified
    // sender: the outsider's org is self-asserted (no org cert), so even if a
    // compromised member leaked the bytes, the verified member would not
    // store them.
    let outsider = Node::new("org-outsider", free_addr(), 1);
    outsider.start(vec![]).await.unwrap();
    outsider
        .set_collections(vec![pricing_collection(3600)])
        .await;
    outsider
        .submit_private_payload(COLLECTION, b"smuggled".to_vec())
        .await
        .expect_err("a non-member org cannot submit payloads");
}

/// Federation trust store (ADR-011, ticket #57): a member whose verifier is
/// configured with its own org Root CA **plus** the writer org's Root CA as a
/// federation anchor accepts a cross-org payload. A member holding only its
/// own org anchor withholds the same payload — fail closed, not fail open.
#[tokio::test]
async fn federation_trust_store_enables_cross_org_payload_delivery() {
    let _ = env_logger::try_init();
    let mut writer_org = Organization::new(WRITER).unwrap();
    let writer_identity = writer_org.issue_identity(WRITER).unwrap().clone();
    let mut member_org = Organization::new(MEMBER_PEER).unwrap();
    let member_identity = member_org.issue_identity(MEMBER_PEER).unwrap().clone();

    let writer_addr = free_addr();
    let writer = Node::new_with_identity(WRITER, &writer_addr, 1, Arc::new(writer_identity));
    writer.start(vec![]).await.unwrap();
    writer.set_collections(vec![pricing_collection(3600)]).await;
    writer
        .set_execution_provider(Arc::new(
            glasschain_vm::WasmExecutionProvider::new().unwrap(),
        ))
        .await;
    writer.submit_transaction(activation_tx(2)).await.unwrap();
    writer.mine().await.unwrap();
    writer
        .submit_transaction(contract_creation_tx("pdc-federation"))
        .await
        .unwrap();
    writer
        .submit_transaction(contract_execution_tx("pdc-federation"))
        .await
        .unwrap();
    writer.mine().await.unwrap();
    let commitment = sha256(PRIVATE_VALUE);

    // Member trusting the writer's org Root CA via a federation anchor:
    // the cross-org payload is delivered.
    let member = Node::new_with_identity(MEMBER_PEER, free_addr(), 1, Arc::new(member_identity));
    let mut verifier = CertChainVerifier::from_org(&member_org).unwrap();
    verifier
        .add_federation_root_pem(WRITER, &writer_org.root_ca_cert_pem)
        .unwrap();
    member.set_cert_verifier(verifier).await;
    member.set_collections(vec![pricing_collection(3600)]).await;
    member.start(vec![writer_addr.clone()]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    wait_for_sync(&writer, &member).await;
    let _ = member.reconcile_private_payloads(COLLECTION).await;
    poll_until("member received the cross-org payload", 3, || async {
        member.transient_payload(COLLECTION, &commitment).await == Some(PRIVATE_VALUE.to_vec())
    })
    .await;

    // Control: a member verifying only its own org withholds the same
    // payload — the writer's org is outside its trust store.
    let solo = Node::new_with_identity(
        MEMBER_PEER,
        free_addr(),
        1,
        Arc::new(member_org.issue_identity(MEMBER_PEER).unwrap().clone()),
    );
    solo.set_cert_verifier(CertChainVerifier::from_org(&member_org).unwrap())
        .await;
    solo.set_collections(vec![pricing_collection(3600)]).await;
    solo.start(vec![writer_addr]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    wait_for_sync(&writer, &solo).await;
    let _ = solo.reconcile_private_payloads(COLLECTION).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        solo.transient_payload(COLLECTION, &commitment).await,
        None,
        "a member without the writer org's anchor must not store the cross-org payload"
    );
}
