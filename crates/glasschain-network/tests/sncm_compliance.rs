//! Anvisa SNCM Compliance Validation Suite
//!
//! This test suite simulates a complete pharmaceutical supply-chain shipment
//! from **Manufacturer → Distributor → Pharmacy**, verifying that `GlassChain`
//! correctly records every custody transfer and computes Metadata Trust Scores
//! in accordance with Brazilian traceability law **RDC 157/2017**.
//!
//! ## Regulatory Reference
//! - **RDC 157/2017** (Anvisa): Establishes the traceability requirements for
//!   medicines in the Brazilian market (Sistema Nacional de Controle de
//!   Medicamentos – SNCM).
//! - Each unit must carry: GTIN, batch number, expiry date, and individual
//!   serial number.
//! - Custody transfers (Manufacture, Dispatch, Receipt) must be recorded in
//!   a tamper-evident ledger.
//!
//! ## Simulated Scenario
//! ```text
//!   ┌──────────────┐  dispatch   ┌─────────────────┐  dispatch   ┌──────────────┐
//!   │ Manufacturer │ ──────────► │   Distributor   │ ──────────► │   Pharmacy   │
//!   │  (Fabricante)│             │  (Distribuidora)│             │  (Farmácia)  │
//!   └──────────────┘             └─────────────────┘             └──────────────┘
//!         │                             │                               │
//!    manufacture                    receive /                       receive
//!    event on ledger              re-dispatch events              event on ledger
//! ```

use glasschain_core::{
    MetadataTrustScore, TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
    TRUST_SCORE_STANDARD_THRESHOLD,
};
use glasschain_network::{Node, NodeEvent};
use std::time::Duration;
use tokio::time::timeout;

// ── Helper constructors ────────────────────────────────────────────────────

/// A fully compliant pharmaceutical product (all SNCM fields present).
fn dipirona_asset(custodian_id: &str, serial_suffix: &str) -> TraceableAsset {
    TraceableAsset {
        gtin: Some("07891234100016".into()), // fictional GTIN-14
        batch_number: Some("LOTE-2025-A001".into()),
        expiry_date: Some("2027-06-30".into()),
        serial_number: Some(format!("SN-BR-{serial_suffix}")),
        anvisa_registration: Some("MS 1.1234.0042.001-7".into()),
        manufacturer_id: Some("12.345.678/0001-99".into()), // fictional CNPJ
        product_name: "Dipirona Sódica 500mg".into(),
        custodian_id: custodian_id.into(),
        country_of_origin: Some("BR".into()),
        storage_temp_celsius: Some("15-30".into()),
        quantity: 1000,
    }
}

/// An asset with missing serial number (partial SNCM compliance).
fn incomplete_asset(custodian_id: &str) -> TraceableAsset {
    TraceableAsset {
        gtin: Some("07891234100016".into()),
        batch_number: Some("LOTE-2025-B001".into()),
        expiry_date: Some("2027-06-30".into()),
        serial_number: None, // ← non-compliant: no serial
        anvisa_registration: None,
        manufacturer_id: None,
        product_name: "Generic Drug".into(),
        custodian_id: custodian_id.into(),
        country_of_origin: None,
        storage_temp_celsius: None,
        quantity: 100,
    }
}

fn asset_tx(asset: TraceableAsset, event_type: &str, originator: &str) -> Transaction {
    Transaction::new(TransactionKind::AssetRegistration(
        TraceableAssetRegistration {
            asset,
            event_type: event_type.into(),
            originator_id: originator.into(),
            purchase_order_ref: None,
        },
    ))
}

fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

// ── Unit-level trust score tests ───────────────────────────────────────────

#[test]
fn sncm_full_compliance_scores_100() {
    let asset = dipirona_asset("fabricante-xyz", "00000001");
    let score = MetadataTrustScore::compute(&asset);
    assert_eq!(score.score, 100);
    assert!(score.is_standard);
    assert!(score.missing_core_fields.is_empty());
}

#[test]
fn sncm_missing_serial_is_flagged() {
    let asset = incomplete_asset("fabricante-xyz");
    let score = MetadataTrustScore::compute(&asset);
    assert!(score.missing_core_fields.iter().any(|f| f == "serial_number"));
    assert!(score.score < TRUST_SCORE_STANDARD_THRESHOLD);
    assert!(!score.is_standard);
}

#[test]
fn sncm_missing_gtin_is_flagged() {
    let mut asset = dipirona_asset("fab", "001");
    asset.gtin = None;
    let score = MetadataTrustScore::compute(&asset);
    assert!(score.missing_core_fields.iter().any(|f| f == "gtin"));
}

#[test]
fn sncm_missing_batch_is_flagged() {
    let mut asset = dipirona_asset("fab", "001");
    asset.batch_number = None;
    let score = MetadataTrustScore::compute(&asset);
    assert!(score.missing_core_fields.iter().any(|f| f == "batch_number"));
}

#[test]
fn sncm_missing_expiry_is_flagged() {
    let mut asset = dipirona_asset("fab", "001");
    asset.expiry_date = None;
    let score = MetadataTrustScore::compute(&asset);
    assert!(score.missing_core_fields.iter().any(|f| f == "expiry_date"));
}

#[test]
fn sncm_fee_nudge_non_standard_pays_full_gas() {
    let score = MetadataTrustScore::compute(&incomplete_asset("test"));
    assert!((score.fee_multiplier() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn sncm_fee_nudge_standard_pays_half_gas() {
    let score = MetadataTrustScore::compute(&dipirona_asset("fab", "001"));
    assert!((score.fee_multiplier() - 0.5).abs() < f64::EPSILON);
}

/// All four core SNCM fields present but no bonus fields → score = 80 (at
/// the standard threshold exactly).
#[test]
fn sncm_four_core_fields_exactly_at_threshold() {
    let mut asset = dipirona_asset("fab", "001");
    asset.anvisa_registration = None;
    asset.manufacturer_id = None;
    let score = MetadataTrustScore::compute(&asset);
    assert_eq!(score.score, 80);
    assert!(score.is_standard);
}

// ── Integration: full supply-chain scenario on a live node ─────────────────

/// Simulate the full Manufacturer → Distributor → Pharmacy custody chain.
///
/// Each step posts an `AssetRegistration` transaction; after mining, we verify
/// that all three events are recorded on-chain and are tamper-evident.
#[tokio::test]
async fn sncm_full_supply_chain_recorded_on_ledger() {
    let addr = free_addr();
    let node = Node::new("sncm-test-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    // Step 1: Manufacturer registers the batch.
    let manufacture_tx = asset_tx(
        dipirona_asset("fabricante-abc", "00000001"),
        "manufacture",
        "fabricante-abc",
    );
    node.submit_transaction(manufacture_tx.clone())
        .await
        .unwrap();

    // Step 2: Manufacturer dispatches to distributor.
    let dispatch_tx = asset_tx(
        dipirona_asset("distribuidora-centro", "00000001"),
        "dispatch",
        "fabricante-abc",
    );
    node.submit_transaction(dispatch_tx.clone()).await.unwrap();

    // Step 3: Distributor receives.
    let receive_distributor_tx = asset_tx(
        dipirona_asset("distribuidora-centro", "00000001"),
        "receive",
        "distribuidora-centro",
    );
    node.submit_transaction(receive_distributor_tx.clone())
        .await
        .unwrap();

    // Step 4: Distributor dispatches to pharmacy.
    let dispatch_pharmacy_tx = asset_tx(
        dipirona_asset("farmacia-sul", "00000001"),
        "dispatch",
        "distribuidora-centro",
    );
    node.submit_transaction(dispatch_pharmacy_tx.clone())
        .await
        .unwrap();

    // Step 5: Pharmacy receives.
    let receive_pharmacy_tx = asset_tx(
        dipirona_asset("farmacia-sul", "00000001"),
        "receive",
        "farmacia-sul",
    );
    node.submit_transaction(receive_pharmacy_tx.clone())
        .await
        .unwrap();

    // Mine all transactions into a block.
    node.mine().await.unwrap();

    let ledger = node.ledger_snapshot().await;
    assert!(ledger.validate_chain().is_ok(), "ledger chain is invalid");

    // Verify all 5 custody events are in block 1.
    let block1 = &ledger.chain[1];
    assert_eq!(
        block1.transactions.len(),
        5,
        "expected 5 supply-chain transactions in block 1"
    );

    // Verify each transaction is an AssetRegistration.
    let event_types: Vec<String> = block1
        .transactions
        .iter()
        .filter_map(|tx| {
            if let TransactionKind::AssetRegistration(ref reg) = tx.kind {
                Some(reg.event_type.clone())
            } else {
                None
            }
        })
        .collect();

    assert!(event_types.contains(&"manufacture".to_string()));
    assert_eq!(
        event_types
            .iter()
            .filter(|e| e.as_str() == "dispatch")
            .count(),
        2
    );
    assert_eq!(
        event_types
            .iter()
            .filter(|e| e.as_str() == "receive")
            .count(),
        2
    );
}

/// Verify that a non-compliant asset transaction IS accepted on-chain (the
/// nudge approach does not reject — it just scores low) but the trust score
/// reflects the deficiency.
#[tokio::test]
async fn sncm_non_compliant_asset_accepted_but_low_trust() {
    let addr = free_addr();
    let node = Node::new("sncm-low-trust-node", &addr, 1);
    node.start(vec![]).await.unwrap();

    let non_compliant_tx = asset_tx(incomplete_asset("unknown-supplier"), "manufacture", "unknown-supplier");

    // The ledger should accept the transaction (nudge, not hard failure).
    node.submit_transaction(non_compliant_tx).await.unwrap();
    node.mine().await.unwrap();

    let ledger = node.ledger_snapshot().await;
    assert!(ledger.validate_chain().is_ok());

    // Confirm the low trust score for the embedded asset.
    let block1 = &ledger.chain[1];
    assert_eq!(block1.transactions.len(), 1);
    if let TransactionKind::AssetRegistration(ref reg) = block1.transactions[0].kind {
        let score = MetadataTrustScore::compute(&reg.asset);
        assert!(!score.is_standard, "non-compliant asset should not be standard");
        assert!(
            score.missing_core_fields.iter().any(|f| f == "serial_number"),
            "serial_number should be flagged"
        );
    } else {
        panic!("expected AssetRegistration transaction");
    }
}

/// Verify the `TransactionAccepted` event fires for an SNCM asset registration.
#[tokio::test]
async fn sncm_asset_registration_emits_transaction_accepted_event() {
    let addr = free_addr();
    let node = Node::new("sncm-event-node", &addr, 1);
    let mut events = node.subscribe();
    node.start(vec![]).await.unwrap();

    let tx = asset_tx(
        dipirona_asset("fabricante-abc", "00000042"),
        "manufacture",
        "fabricante-abc",
    );
    node.submit_transaction(tx).await.unwrap();

    let evt = timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("timeout")
        .expect("channel closed");

    assert!(
        matches!(evt, NodeEvent::TransactionAccepted(_)),
        "expected TransactionAccepted"
    );
}

/// Multi-node: Manufacturer node mines an SNCM block; Pharmacy node syncs it.
#[tokio::test]
async fn sncm_chain_syncs_between_manufacturer_and_pharmacy_nodes() {
    // Manufacturer node mines the custody event.
    let addr_mfr = free_addr();
    let node_mfr = Node::new("node-fabricante", &addr_mfr, 1);
    node_mfr.start(vec![]).await.unwrap();

    node_mfr
        .submit_transaction(asset_tx(
            dipirona_asset("fabricante-abc", "00000001"),
            "manufacture",
            "fabricante-abc",
        ))
        .await
        .unwrap();
    node_mfr.mine().await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Pharmacy node connects and syncs.
    let addr_pharmacy = free_addr();
    let node_pharmacy = Node::new("node-farmacia", &addr_pharmacy, 1);
    node_pharmacy
        .start(vec![addr_mfr.clone()])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(600)).await;

    let ledger = node_pharmacy.ledger_snapshot().await;
    assert!(
        ledger.chain.len() >= 2,
        "pharmacy node did not sync manufacturer's block"
    );
    assert!(ledger.validate_chain().is_ok());

    // Verify the synced block contains the manufacture event.
    let has_manufacture = ledger.chain[1..].iter().any(|b| {
        b.transactions.iter().any(|tx| {
            matches!(&tx.kind, TransactionKind::AssetRegistration(reg)
                if reg.event_type == "manufacture")
        })
    });
    assert!(
        has_manufacture,
        "synced ledger should contain manufacture event"
    );
}
