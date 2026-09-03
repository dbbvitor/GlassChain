/// Node-level scenarios for canonical schema v1 (ticket #34): valid records
/// commit, invalid records are rejected at admission, unknown namespaces and
/// private cleartext never reach the chain, legacy asset-shaped inputs hit the
/// explicit migration boundary, and canonical records sync between nodes.
use glasschain_core::{
    CanonicalRecord, RecordSignature, Registry, Transaction, TransactionKind, SCHEMA_VERSION_V1,
};
use glasschain_network::Node;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

const HEX64: &str = "abababababababababababababababababababababababababababababababab";

/// Pick an ephemeral port on localhost that is very likely free.
fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

fn payload_map(fields: Value) -> BTreeMap<String, Value> {
    serde_json::from_value(fields).expect("payload object")
}

/// A signed, anchored canonical record for `schema_id` (anchored where the
/// family requires it; state commitments get one signature per counterparty).
fn record(schema_id: &str) -> CanonicalRecord {
    let payload = match schema_id {
        "party_identity" => {
            payload_map(json!({"org_id": "cooperative-x", "legal_name": "Cooperative X"}))
        }
        "product" => payload_map(
            json!({"product_id": "SKU-1", "gtin": "07891234100016", "product_name": "Drug A"}),
        ),
        "lot" => payload_map(
            json!({"lot_id": "lot-1", "product_id": "SKU-1", "batch_number": "BATCH-001"}),
        ),
        "shipment" => {
            payload_map(json!({"lot_ref": HEX64, "from_org": "maker-1", "to_org": "dist-1"}))
        }
        "inventory_threshold" => payload_map(json!({
            "trigger_id": "trig-1",
            "product_id": "SKU-1",
            "owner_id": "buyer-1",
            "reorder_threshold": 100,
        })),
        "purchase_order" => payload_map(json!({
            "product_id": "SKU-1",
            "buyer_id": "buyer-1",
            "seller_id": "seller-1",
            "quantity": 50,
            "currency": "USD",
        })),
        "transit_event" => payload_map(json!({
            "shipment_ref": "ship-1",
            "event_type": "departure",
            "location": "BR-SP",
        })),
        "delivery_receipt" => payload_map(json!({
            "shipment_ref": "ship-1",
            "receiver_id": "pharmacy-1",
            "received_at": "2026-01-15",
        })),
        "inventory_transformation" => payload_map(json!({
            "lot_ref": HEX64,
            "transformation_type": "split",
        })),
        "recall" => payload_map(json!({
            "lot_ref": HEX64,
            "reason": "contamination",
            "status": "issued",
            "issued_by": "maker-1",
        })),
        "quality_certification" | "audit_attestation" => payload_map(json!({
            "lot_ref": HEX64,
            "issuer": "certifier-1",
            "scope": "GMP",
            "valid_from": "2026-01-01",
            "valid_to": "2027-01-01",
            "status": "valid",
            "evidence_manifest": {"manifest_commitment": HEX64},
        })),
        "state_commitment" => payload_map(json!({"merkle_root": HEX64,
            "counterparties": ["org-a", "org-b"]})),
        other => panic!("unknown family {other}"),
    };
    let mut record = CanonicalRecord::new(0, schema_id, payload, "org-issuer");
    record.signatures.push(RecordSignature {
        algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
        signer: "org-issuer".into(),
        signature_bytes: vec![0x42; 8],
    });
    if schema_id == "state_commitment" {
        // v1 requires one opaque signature per named counterparty.
        record.signatures.push(RecordSignature {
            algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
            signer: "org-a".into(),
            signature_bytes: vec![0x43; 8],
        });
    }
    let anchored = Registry::v1()
        .lookup_schema(schema_id, SCHEMA_VERSION_V1)
        .is_some_and(|entry| entry.descriptor.anchored);
    if anchored {
        record.commitment = Some(record.commitment().expect("commitment"));
    }
    record
}

fn canonical_tx(record: CanonicalRecord) -> Transaction {
    Transaction::with_id(
        format!("canonical:{}", record.record_id),
        TransactionKind::CanonicalRecord(record),
    )
}

async fn start_node(id: &str) -> Node {
    let addr = free_addr();
    let node = Node::new(id, &addr, 1);
    node.start(vec![]).await.expect("node starts");
    node
}

#[tokio::test]
async fn test_all_thirteen_families_commit_at_node_level() {
    let node = start_node("canon-commit").await;
    for schema_id in [
        "party_identity",
        "product",
        "lot",
        "inventory_threshold",
        "purchase_order",
        "shipment",
        "transit_event",
        "delivery_receipt",
        "inventory_transformation",
        "recall",
        "quality_certification",
        "audit_attestation",
        "state_commitment",
    ] {
        node.submit_transaction(canonical_tx(record(schema_id)))
            .await
            .expect("valid record must be admitted");
    }
    node.mine().await.expect("mine");

    let ledger = node.ledger_snapshot().await;
    assert!(ledger.validate_chain().is_ok());
    let mut committed: Vec<&str> = ledger
        .chain
        .iter()
        .flat_map(|b| b.transactions.iter())
        .filter_map(|tx| match &tx.kind {
            TransactionKind::CanonicalRecord(record) => Some(record.schema_id.as_str()),
            _ => None,
        })
        .collect();
    committed.sort_unstable();
    assert_eq!(committed.len(), 13, "all thirteen families commit");
}

/// Number of canonical transactions committed on `ledger`.
fn committed_canonical_count(ledger: &glasschain_core::Ledger) -> usize {
    ledger
        .chain
        .iter()
        .flat_map(|b| b.transactions.iter())
        .filter(|tx| matches!(tx.kind, TransactionKind::CanonicalRecord(_)))
        .count()
}

#[tokio::test]
async fn test_invalid_record_rejected_at_admission_without_partial_state() {
    let node = start_node("canon-reject").await;
    let mut bad = record("lot");
    bad.payload.remove("batch_number");
    let error = node
        .submit_transaction(canonical_tx(bad))
        .await
        .expect_err("missing required field must be rejected at admission");
    assert!(error.to_string().contains("batch_number"), "{error}");

    let ledger = node.ledger_snapshot().await;
    assert!(ledger.pending_transactions.is_empty(), "no partial state");
    node.mine().await.expect("mine");
    let ledger = node.ledger_snapshot().await;
    assert_eq!(
        committed_canonical_count(&ledger),
        0,
        "nothing invalid ever commits"
    );
}

#[tokio::test]
async fn test_unknown_namespace_rejected_at_admission() {
    let node = start_node("canon-ns").await;
    let mut record = record("party_identity");
    record.extensions.insert(
        "urn:partner:unknown".into(),
        glasschain_core::ExtensionValue {
            schema_version: 1,
            value: BTreeMap::from([("partner_key".into(), json!("value"))]),
        },
    );
    let error = node
        .submit_transaction(canonical_tx(record))
        .await
        .expect_err("unknown namespace must be rejected");
    assert!(error.to_string().contains("unknown extension"), "{error}");
}

#[tokio::test]
async fn test_private_cleartext_rejected_at_admission() {
    let node = start_node("canon-privacy").await;

    // Raw pricing under an unregistered namespace.
    let mut price_record = record("party_identity");
    price_record.extensions.insert(
        "pricing".into(),
        glasschain_core::ExtensionValue {
            schema_version: 1,
            value: BTreeMap::from([("unit_price".into(), json!(2990))]),
        },
    );
    assert!(
        node.submit_transaction(canonical_tx(price_record))
            .await
            .is_err(),
        "raw private values must be rejected"
    );

    // Raw pricing smuggled onto a public shipment payload.
    let mut smuggled = record("shipment");
    smuggled.payload.insert("unit_price".into(), json!(2990));
    assert!(
        node.submit_transaction(canonical_tx(smuggled))
            .await
            .is_err(),
        "cleartext payload fields must be rejected"
    );

    // And nothing reached the chain.
    node.mine().await.expect("mine");
    let ledger = node.ledger_snapshot().await;
    assert_eq!(
        committed_canonical_count(&ledger),
        0,
        "no private cleartext ever commits"
    );
}

#[tokio::test]
async fn test_legacy_asset_shaped_record_rejected_with_migration_path() {
    let node = start_node("canon-legacy").await;
    let mut record = record("inventory_threshold");
    record
        .payload
        .insert("gtin".into(), json!("07891234100016"));
    record
        .payload
        .insert("batch_number".into(), json!("BATCH-001"));
    record
        .payload
        .insert("expiry_date".into(), json!("2027-12-31"));
    let error = node
        .submit_transaction(canonical_tx(record))
        .await
        .expect_err("TraceableAsset-shaped input must be rejected");
    assert!(
        error.to_string().contains("migrate"),
        "error must name the migration path: {error}"
    );
}

#[tokio::test]
async fn test_canonical_record_syncs_between_nodes() {
    // Node A admits and commits a canonical record.
    let addr_a = free_addr();
    let node_a = Node::new("canon-a", &addr_a, 1);
    node_a.start(vec![]).await.expect("start a");
    node_a
        .submit_transaction(canonical_tx(record("quality_certification")))
        .await
        .expect("admit");
    node_a.mine().await.expect("mine");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Node B connects as a seed peer and syncs the committed record.
    let addr_b = free_addr();
    let node_b = Node::new("canon-b", &addr_b, 1);
    node_b.start(vec![addr_a.clone()]).await.expect("start b");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let ledger_b = node_b.ledger_snapshot().await;
    assert!(ledger_b.chain.len() >= 2, "node B must sync the block");
    let synced: Vec<&str> = ledger_b
        .chain
        .iter()
        .flat_map(|b| b.transactions.iter())
        .filter_map(|tx| match &tx.kind {
            TransactionKind::CanonicalRecord(record) => Some(record.schema_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(synced, vec!["quality_certification"]);
    assert!(
        ledger_b.validate_chain().is_ok(),
        "synced chain must validate"
    );
}
