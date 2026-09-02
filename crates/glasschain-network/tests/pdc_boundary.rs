//! Node-level private-data-collection boundary scenarios (ticket #46, ADR-003).
//!
//! Three nodes: a writer (mines), a member peer, and an outsider — all share
//! the same collection configuration, but only the writer and member peer are
//! members. Covered here:
//!
//! * **Member write + commit**: a PDC-scoped VM write commits with only the
//!   collection reference and the value commitment in the block; the raw
//!   payload travels point-to-point to the member peer's transient store.
//! * **Non-member verification**: the outsider's chain carries the identical
//!   commitment and no payload bytes anywhere.
//! * **Admission**: `submit_private_payload` fails for a non-member and for an
//!   inactive `pdc` capability; PDC-scoped writes are dropped whole while the
//!   capability is inactive.
//!
//! The transport-level leakage rejection (a payload pushed directly to a
//! non-member) and the `/3` wire-version gate are covered in
//! `protocol_security.rs`, which owns the raw-TLS harness.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use glasschain_core::{
    crypto::sha256, CapabilityActivation, PersistentWrite, RecordSignature, Transaction,
    TransactionKind, WriteOp, WriteVisibility,
};
use glasschain_identity::{Channel, ChannelConfig};
use glasschain_network::Node;
use std::time::Duration;

const WRITER: &str = "org-writer";
const MEMBER_PEER: &str = "org-member";
const OUTSIDER: &str = "org-outsider";
const COLLECTION: &str = "pricing";
/// The private payload bytes, written at runtime by the guest (never a data
/// segment, so the committed contract bytes cannot carry them).
const PRIVATE_VALUE: &[u8] = &[0xDE, 0xAD, 0xBE, 0xEF];

fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

fn pricing_collection() -> Channel {
    Channel::new(ChannelConfig {
        name: COLLECTION.to_owned(),
        member_ids: vec![WRITER.to_owned(), MEMBER_PEER.to_owned()],
        description: "Private pricing collection".to_owned(),
        endorsement_policy: None,
        retention_secs: 3600,
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

/// A guest that writes one PDC-scoped value into collection `pricing`,
/// computing the value bytes at runtime (an `i32.store`, not a data segment).
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
    ;; The private value bytes (DE AD BE EF) are written at runtime.
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

/// The committed PDC write from a block's write set, if any.
fn pdc_write(block_index: usize, chain: &[glasschain_core::Block]) -> Option<PersistentWrite> {
    chain.get(block_index).and_then(|block| {
        block
            .write_set
            .iter()
            .find(|write| matches!(write.visibility, WriteVisibility::Pdc(_)))
            .cloned()
    })
}

/// Poll until `peer`'s chain is at least as long as `node`'s, or panic.
async fn wait_for_sync(node: &Node, peer: &Node) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if peer.ledger_snapshot().await.chain.len() >= node.ledger_snapshot().await.chain.len() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "peer chain never caught up"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn pdc_member_write_and_commit_with_nonmember_verification() {
    // ── Three nodes, one collection, two members ────────────────────────
    let writer_addr = free_addr();
    let writer = Node::new(WRITER, &writer_addr, 1);
    writer.start(vec![]).await.unwrap();
    let member_peer = Node::new(MEMBER_PEER, free_addr(), 1);
    member_peer.start(vec![writer_addr.clone()]).await.unwrap();
    let outsider = Node::new(OUTSIDER, free_addr(), 1);
    outsider.start(vec![writer_addr]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    writer
        .set_execution_provider(std::sync::Arc::new(
            glasschain_vm::WasmExecutionProvider::new().unwrap(),
        ))
        .await;
    for node in [&writer, &member_peer, &outsider] {
        node.set_collections(vec![pricing_collection()]).await;
    }

    // ── Activate the `pdc` capability at height 2 ───────────────────────
    writer.submit_transaction(activation_tx(2)).await.unwrap();
    writer.mine().await.unwrap();
    wait_for_sync(&writer, &member_peer).await;
    wait_for_sync(&writer, &outsider).await;

    // ── Member write + commit: the PDC write commits as a commitment ────
    writer
        .submit_transaction(contract_creation_tx("pdc-writer"))
        .await
        .unwrap();
    writer
        .submit_transaction(contract_execution_tx("pdc-writer"))
        .await
        .unwrap();
    writer.mine().await.unwrap();
    wait_for_sync(&writer, &member_peer).await;
    wait_for_sync(&writer, &outsider).await;

    let commitment = sha256(PRIVATE_VALUE);
    let writer_chain = writer.ledger_snapshot().await.chain;
    let committed_write =
        pdc_write(writer_chain.len() - 1, &writer_chain).expect("a PDC write committed");
    assert_eq!(
        committed_write.visibility,
        WriteVisibility::Pdc(COLLECTION.to_owned()),
        "the block carries the collection reference"
    );
    let WriteOp::Set(committed) = &committed_write.op else {
        panic!("expected a set");
    };
    assert_eq!(
        committed.as_slice(),
        commitment.as_bytes(),
        "the block carries only the value commitment"
    );

    // The member peer holds the raw payload in its transient store; the
    // writer holds its own. Delivery is asynchronous — poll with a deadline.
    for member in [&writer, &member_peer] {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if member.transient_payload(COLLECTION, &commitment).await
                == Some(PRIVATE_VALUE.to_vec())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "a member node never held the payload in its transient store"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    // The outsider holds nothing.
    assert_eq!(
        outsider.transient_payload(COLLECTION, &commitment).await,
        None,
        "a non-member must never receive the payload"
    );

    // ── Non-member verification: same commitment, no payload anywhere ───
    let outsider_chain = outsider.ledger_snapshot().await.chain;
    let outsider_write = pdc_write(outsider_chain.len() - 1, &outsider_chain)
        .expect("the outsider's chain carries the committed PDC write");
    assert_eq!(outsider_write.visibility, committed_write.visibility);
    assert_eq!(
        outsider_write.op, committed_write.op,
        "commitments are identical"
    );
    let chain_json = serde_json::to_string(&outsider_chain).unwrap();
    let payload_json = serde_json::to_string(PRIVATE_VALUE).unwrap();
    assert!(
        !chain_json.contains(&payload_json),
        "raw payload bytes leaked into the outsider's chain"
    );
    assert!(
        !chain_json.contains(&BASE64_STANDARD.encode(PRIVATE_VALUE)),
        "base64 payload leaked into the outsider's chain"
    );
}

#[tokio::test]
async fn pdc_writes_require_the_active_capability() {
    let writer = Node::new("cap-writer", free_addr(), 1);
    writer.start(vec![]).await.unwrap();
    writer.set_collections(vec![pricing_collection()]).await;
    writer
        .set_execution_provider(std::sync::Arc::new(
            glasschain_vm::WasmExecutionProvider::new().unwrap(),
        ))
        .await;

    // No `pdc` activation: the contract executes, but its PDC write must be
    // dropped whole (the error surfaces; nothing commits, nothing is held).
    writer
        .submit_transaction(contract_creation_tx("pdc-cap"))
        .await
        .unwrap();
    writer
        .submit_transaction(contract_execution_tx("pdc-cap"))
        .await
        .unwrap();
    let result = writer.mine().await;
    assert!(
        result.is_err(),
        "a PDC write without the capability must fail"
    );

    let chain = writer.ledger_snapshot().await.chain;
    assert!(
        chain.iter().all(|block| block
            .write_set
            .iter()
            .all(|write| !matches!(write.visibility, WriteVisibility::Pdc(_)))),
        "no PDC write may commit while the capability is inactive"
    );
    assert_eq!(
        writer
            .transient_payload(COLLECTION, &sha256(PRIVATE_VALUE))
            .await,
        None,
        "no payload may be held for a rejected candidate"
    );
}

#[tokio::test]
async fn non_member_miner_never_holds_private_cleartext() {
    // A node that is NOT a collection member but mines a relayed PDC
    // execution commits the public commitment and must not hold the payload
    // in its transient store (ADR-003 boundary c; the review's H2 finding).
    let miner = Node::new(OUTSIDER, free_addr(), 1);
    miner.start(vec![]).await.unwrap();
    miner.set_collections(vec![pricing_collection()]).await;
    miner
        .set_execution_provider(std::sync::Arc::new(
            glasschain_vm::WasmExecutionProvider::new().unwrap(),
        ))
        .await;
    miner.submit_transaction(activation_tx(2)).await.unwrap();
    miner.mine().await.unwrap();
    miner
        .submit_transaction(contract_creation_tx("pdc-relayed"))
        .await
        .unwrap();
    miner
        .submit_transaction(contract_execution_tx("pdc-relayed"))
        .await
        .unwrap();
    miner.mine().await.unwrap();

    let chain = miner.ledger_snapshot().await.chain;
    let committed =
        pdc_write(chain.len() - 1, &chain).expect("the PDC write commits (capability active)");
    let WriteOp::Set(committed_value) = &committed.op else {
        panic!("expected a set");
    };
    assert_eq!(
        committed_value.as_slice(),
        sha256(PRIVATE_VALUE).as_bytes(),
        "only the commitment is global"
    );
    assert_eq!(
        miner
            .transient_payload(COLLECTION, &sha256(PRIVATE_VALUE))
            .await,
        None,
        "a non-member miner must never hold private cleartext"
    );
}

#[tokio::test]
async fn private_payload_admission_gates() {
    let writer = Node::new(WRITER, free_addr(), 1);
    writer.start(vec![]).await.unwrap();
    writer.set_collections(vec![pricing_collection()]).await;

    // Capability gate: payloads need `pdc` active.
    let err = writer
        .submit_private_payload(COLLECTION, PRIVATE_VALUE.to_vec())
        .await
        .expect_err("payload submission without the capability must fail");
    assert!(err.to_string().contains("'pdc' capability"), "{err}");

    // Activate the capability; the member can now submit.
    writer.submit_transaction(activation_tx(2)).await.unwrap();
    writer.mine().await.unwrap();
    writer
        .submit_private_payload(COLLECTION, PRIVATE_VALUE.to_vec())
        .await
        .expect("a member org may submit a private payload");
    assert_eq!(
        writer
            .transient_payload(COLLECTION, &sha256(PRIVATE_VALUE))
            .await,
        Some(PRIVATE_VALUE.to_vec())
    );

    // Membership gate: a non-member org cannot submit at all.
    let outsider = Node::new(OUTSIDER, free_addr(), 1);
    outsider.start(vec![]).await.unwrap();
    outsider.set_collections(vec![pricing_collection()]).await;
    let err = outsider
        .submit_private_payload(COLLECTION, PRIVATE_VALUE.to_vec())
        .await
        .expect_err("non-member submission must fail");
    assert!(err.to_string().contains("not a member"), "{err}");
}
