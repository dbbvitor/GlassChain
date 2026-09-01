//! Node-level purchase-to-settlement scenario (ticket #43): the full
//! RFQ → Quote → PO → Acceptance → Shipment → Receipt → Dispute → Settlement
//! chain, plus certification and audit flows, end-to-end across two
//! organizations on two connected nodes.
//!
//! The test is the flow host: it drives each party's `FlowRunner` with
//! committed records (fed back from the nodes' chains) and business wake-ups,
//! and executes emitted records durably through the real commit path
//! (`submit_transaction` → `mine` → peer broadcast). Signature attachment is
//! the stand-in for the endorsement layer (presence-only at v1 admission).

use glasschain_core::{
    providers::in_memory::InMemoryStorageProvider, CanonicalRecord, RecordSignature,
    StorageProvider, Transaction, TransactionKind,
};
use glasschain_network::Node;
use glasschain_workflows::{
    audit_flow, buyer_flow, certification_flow, seller_flow, Action, AttestationConfig,
    AttestationFlowState, Event, FlowRunner, FlowTriage, PurchaseFlowConfig, PurchaseFlowState,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAKER: &str = "org-maker";
const BUYER: &str = "org-buyer";

/// The one deal both parties' flows coordinate on.
fn deal_config(org: &str, counterparty: &str) -> PurchaseFlowConfig {
    PurchaseFlowConfig {
        org: org.to_owned(),
        counterparty: counterparty.to_owned(),
        product_id: "SKU-001".to_owned(),
        quantity: 100,
        currency: "BRL".to_owned(),
        lot_ref: "lot-1".to_owned(),
        rfq_id: "rfq-1".to_owned(),
        negotiated_at: 1_700_000_000,
        delivery_on: "2026-09-01".to_owned(),
    }
}

fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

fn storage() -> Arc<dyn StorageProvider> {
    Arc::new(InMemoryStorageProvider::new())
}

/// A product + anchored lot pair committed by the maker (the custody anchor
/// every downstream record references).
fn catalog_records() -> Vec<CanonicalRecord> {
    let mut product_payload = BTreeMap::new();
    product_payload.insert("product_id".to_owned(), Value::String("SKU-001".into()));
    product_payload.insert("gtin".to_owned(), Value::String("07891234100016".into()));
    product_payload.insert(
        "product_name".to_owned(),
        Value::String("Amoxicillin 500mg".into()),
    );
    let product = CanonicalRecord::new(1_700_000_000, "product", product_payload, MAKER);

    let mut lot_payload = BTreeMap::new();
    lot_payload.insert("lot_id".to_owned(), Value::String("LOT-1".into()));
    lot_payload.insert("product_id".to_owned(), Value::String("SKU-001".into()));
    lot_payload.insert("batch_number".to_owned(), Value::String("B-2026".into()));
    let mut lot = CanonicalRecord::new(1_700_000_000, "lot", lot_payload, MAKER);
    "lot-1".clone_into(&mut lot.record_id);
    lot.commitment = lot.commitment().ok();
    vec![product, lot]
}

/// Everything the scenario's phases share: two connected nodes, both parties'
/// flows, and their per-party checkpoint storages.
struct DealContext {
    maker: Node,
    buyer: Node,
    maker_storage: Arc<dyn StorageProvider>,
    buyer_storage: Arc<dyn StorageProvider>,
    maker_triage: FlowTriage,
    buyer_triage: FlowTriage,
    buyer_flow: FlowRunner<PurchaseFlowState>,
    seller_flow: FlowRunner<PurchaseFlowState>,
    buyer_initial: PurchaseFlowState,
    seller_initial: PurchaseFlowState,
}

impl DealContext {
    async fn new() -> Self {
        let maker_addr = free_addr();
        let maker = Node::new("maker-node", &maker_addr, 1);
        maker.start(vec![]).await.unwrap();
        let buyer = Node::new("buyer-node", free_addr(), 1);
        buyer.start(vec![maker_addr]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let config = deal_config(BUYER, MAKER);
        Self {
            maker,
            buyer,
            maker_storage: storage(),
            buyer_storage: storage(),
            maker_triage: FlowTriage::new(),
            buyer_triage: FlowTriage::new(),
            buyer_flow: buyer_flow(&config),
            seller_flow: seller_flow(&deal_config(MAKER, BUYER)),
            buyer_initial: PurchaseFlowState::RfqIssued {
                rfq_id: "rfq-1".to_owned(),
            },
            seller_initial: PurchaseFlowState::AwaitingPurchaseOrder,
        }
    }

    /// Poll until `peer`'s chain is at least as long as `node`'s (block
    /// propagation), or panic after 5s — never hang.
    async fn wait_for_sync(&self, node: &Node, peer: &Node) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let local = node.ledger_snapshot().await.chain.len();
            let remote = peer.ledger_snapshot().await.chain.len();
            if remote >= local {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "peer chain {remote} never reached {local}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Execute a flow-emitted record durably: attach the stand-in signature
    /// (the endorsement layer's job in production), submit through `node`,
    /// mine, and wait for the record to reach `peer` too.
    async fn commit_record(
        &self,
        node: &Node,
        peer: &Node,
        mut record: CanonicalRecord,
        issuer: &str,
    ) {
        record.signatures.push(RecordSignature {
            signer: issuer.to_owned(),
            signature_bytes: vec![0x42],
        });
        node.submit_transaction(Transaction::with_id(
            record.record_id.clone(),
            TransactionKind::CanonicalRecord(record),
        ))
        .await
        .expect("record admitted");
        node.mine().await.expect("block committed");
        self.wait_for_sync(node, peer).await;
    }

    /// All canonical records committed on a node's chain, in block order.
    async fn committed_records(&self, node: &Node) -> Vec<CanonicalRecord> {
        node.ledger_snapshot()
            .await
            .chain
            .iter()
            .flat_map(|block| &block.transactions)
            .filter_map(|tx| match &tx.kind {
                TransactionKind::CanonicalRecord(record) => Some(record.clone()),
                _ => None,
            })
            .collect()
    }

    async fn find_record(&self, node: &Node, record_id: &str) -> CanonicalRecord {
        self.committed_records(node)
            .await
            .into_iter()
            .find(|r| r.record_id == record_id)
            .unwrap_or_else(|| panic!("{record_id} must be committed and propagated"))
    }

    /// The maker anchors the catalog: product + immutable lot.
    async fn anchor_catalog(&self) {
        for record in catalog_records() {
            let mut signed = record;
            signed.signatures.push(RecordSignature {
                signer: MAKER.to_owned(),
                signature_bytes: vec![0x42],
            });
            self.maker
                .submit_transaction(Transaction::with_id(
                    signed.record_id.clone(),
                    TransactionKind::CanonicalRecord(signed),
                ))
                .await
                .unwrap();
        }
        self.maker.mine().await.unwrap();
        self.wait_for_sync(&self.maker, &self.buyer).await;
    }

    /// RFQ → Quote → PO (buyer side), with the interruption: the PO emission
    /// is delivered twice and acked once — the ledger dedupes. Returns the
    /// emitted `purchase_order` record.
    async fn buyer_po_phase(&self) -> CanonicalRecord {
        let outcome = self
            .buyer_flow
            .handle(
                &self.buyer_storage,
                &self.buyer_triage,
                "purchase:rfq-1",
                &self.buyer_initial,
                &Event::Woken("quote-accepted:q-1".into()),
            )
            .unwrap()
            .expect("quote transition applies");
        assert!(matches!(
            outcome.state,
            PurchaseFlowState::QuoteAccepted { .. }
        ));

        let first = self
            .buyer_flow
            .handle(
                &self.buyer_storage,
                &self.buyer_triage,
                "purchase:rfq-1",
                &self.buyer_initial,
                &Event::Woken("commit-po".into()),
            )
            .unwrap()
            .expect("po transition applies");
        let [Action::EmitRecord(po)] = &first.actions[..] else {
            panic!("expected the PO emission, got {:?}", first.actions);
        };
        // The runtime "crashes" before ack; resume re-delivers the same action.
        let resumed = self
            .buyer_flow
            .handle(
                &self.buyer_storage,
                &self.buyer_triage,
                "purchase:rfq-1",
                &self.buyer_initial,
                &Event::Resumed("resume after crash".into()),
            )
            .unwrap()
            .expect("pending work re-delivered");
        assert_eq!(resumed.actions, first.actions, "re-delivery is identical");
        // Execute once (the replayed emission would be byte-identical), ack once.
        self.commit_record(&self.buyer, &self.maker, po.clone(), BUYER)
            .await;
        self.buyer_flow
            .ack(&self.buyer_storage, &self.buyer_triage, "purchase:rfq-1", 1)
            .unwrap();
        po.clone()
    }

    /// Acceptance → Shipment (seller side): consume the committed PO, emit the
    /// custody edge's first half.
    async fn seller_ship_phase(&self) -> CanonicalRecord {
        let po_record = self.find_record(&self.maker, "po:rfq-1").await;
        let outcome = self
            .seller_flow
            .handle(
                &self.maker_storage,
                &self.maker_triage,
                "purchase:rfq-1",
                &self.seller_initial,
                &Event::RecordCommitted(po_record),
            )
            .unwrap()
            .expect("acceptance transition applies");
        assert_eq!(
            outcome.state,
            PurchaseFlowState::PoAccepted {
                po_ref: "po:rfq-1".to_owned(),
            }
        );

        let outcome = self
            .seller_flow
            .handle(
                &self.maker_storage,
                &self.maker_triage,
                "purchase:rfq-1",
                &self.seller_initial,
                &Event::Woken("ship".into()),
            )
            .unwrap()
            .expect("ship transition applies");
        let [Action::EmitRecord(shipment)] = &outcome.actions[..] else {
            panic!("expected the shipment emission, got {:?}", outcome.actions);
        };
        assert_eq!(
            outcome.state,
            PurchaseFlowState::AwaitingReceipt {
                po_ref: "po:rfq-1".to_owned(),
                shipment_ref: "shipment:po:rfq-1:lot-1".to_owned(),
            }
        );
        self.commit_record(&self.maker, &self.buyer, shipment.clone(), MAKER)
            .await;
        self.seller_flow
            .ack(&self.maker_storage, &self.maker_triage, "purchase:rfq-1", 1)
            .unwrap();
        shipment.clone()
    }

    /// Receipt (buyer side): consume the shipment, emit the custody edge's
    /// second half.
    async fn receipt_phase(&self) {
        let shipment_record = self
            .find_record(&self.buyer, "shipment:po:rfq-1:lot-1")
            .await;
        let outcome = self
            .buyer_flow
            .handle(
                &self.buyer_storage,
                &self.buyer_triage,
                "purchase:rfq-1",
                &self.buyer_initial,
                &Event::RecordCommitted(shipment_record),
            )
            .unwrap()
            .expect("receipt transition applies");
        let [Action::EmitRecord(receipt)] = &outcome.actions[..] else {
            panic!("expected the receipt emission, got {:?}", outcome.actions);
        };
        self.commit_record(&self.buyer, &self.maker, receipt.clone(), BUYER)
            .await;
        self.buyer_flow
            .ack(&self.buyer_storage, &self.buyer_triage, "purchase:rfq-1", 1)
            .unwrap();
    }

    /// The seller consumes the receipt; both parties dispute and settle.
    async fn dispute_settlement_phase(&self) {
        let receipt_record = self
            .find_record(&self.maker, "receipt:shipment:po:rfq-1:lot-1")
            .await;
        let outcome = self
            .seller_flow
            .handle(
                &self.maker_storage,
                &self.maker_triage,
                "purchase:rfq-1",
                &self.seller_initial,
                &Event::RecordCommitted(receipt_record),
            )
            .unwrap()
            .expect("seller delivery transition applies");
        assert!(matches!(outcome.state, PurchaseFlowState::Delivered { .. }));

        for (label, flow, triage, storage, initial) in [
            (
                "buyer",
                &self.buyer_flow,
                &self.buyer_triage,
                &self.buyer_storage,
                &self.buyer_initial,
            ),
            (
                "seller",
                &self.seller_flow,
                &self.maker_triage,
                &self.maker_storage,
                &self.seller_initial,
            ),
        ] {
            let outcome = flow
                .handle(
                    storage,
                    triage,
                    "purchase:rfq-1",
                    initial,
                    &Event::Woken("dispute:packaging-damaged".into()),
                )
                .unwrap()
                .expect("dispute transition applies");
            assert!(
                matches!(outcome.state, PurchaseFlowState::Disputed { .. }),
                "{label}"
            );
            let outcome = flow
                .handle(
                    storage,
                    triage,
                    "purchase:rfq-1",
                    initial,
                    &Event::Woken("settle".into()),
                )
                .unwrap()
                .expect("settle transition applies");
            assert_eq!(
                outcome.state,
                PurchaseFlowState::Settled {
                    po_ref: "po:rfq-1".to_owned(),
                },
                "{label}"
            );
            assert!(outcome.completed, "{label}: settlement is terminal");
        }
    }

    /// Certification and audit flows emit records referencing the immutable
    /// lot, committed through the maker node.
    async fn attestation_phase(&self) {
        let lot_record = self.find_record(&self.maker, "lot-1").await;
        let attestations = [
            (
                "cert:lot-1",
                certification_flow(AttestationConfig {
                    family: "quality_certification",
                    issuer: MAKER.to_owned(),
                    scope: "cold-chain".to_owned(),
                    valid_from: "2026-09-01".to_owned(),
                    valid_to: "2027-09-01".to_owned(),
                    issued_at: 1_700_000_100,
                }),
            ),
            (
                "audit:lot-1",
                audit_flow(AttestationConfig {
                    family: "audit_attestation",
                    issuer: MAKER.to_owned(),
                    scope: "gmp".to_owned(),
                    valid_from: "2026-09-01".to_owned(),
                    valid_to: "2027-09-01".to_owned(),
                    issued_at: 1_700_000_100,
                }),
            ),
        ];
        for (flow_id, flow) in attestations {
            let initial = AttestationFlowState::AwaitingLot;
            let outcome = flow
                .handle(
                    &self.maker_storage,
                    &self.maker_triage,
                    flow_id,
                    &initial,
                    &Event::RecordCommitted(lot_record.clone()),
                )
                .unwrap()
                .expect("anchor transition applies");
            assert!(matches!(
                outcome.state,
                AttestationFlowState::LotAnchored { .. }
            ));
            let outcome = flow
                .handle(
                    &self.maker_storage,
                    &self.maker_triage,
                    flow_id,
                    &initial,
                    &Event::Woken("attest".into()),
                )
                .unwrap()
                .expect("attest transition applies");
            let [Action::EmitRecord(attestation)] = &outcome.actions[..] else {
                panic!(
                    "expected an attestation emission, got {:?}",
                    outcome.actions
                );
            };
            self.commit_record(&self.maker, &self.buyer, attestation.clone(), MAKER)
                .await;
            flow.ack(&self.maker_storage, &self.maker_triage, flow_id, 1)
                .unwrap();
        }
    }

    /// Assert the committed records and custody edges on both chains: every
    /// record present and propagated, no duplicates, custody maker → buyer,
    /// attestations anchored and lot-referencing.
    async fn assert_outcomes(&self) {
        for (label, node) in [("maker", &self.maker), ("buyer", &self.buyer)] {
            let records = self.committed_records(node).await;
            let ids: Vec<&str> = records.iter().map(|r| r.record_id.as_str()).collect();
            for expected in [
                "lot-1",
                "po:rfq-1",
                "shipment:po:rfq-1:lot-1",
                "receipt:shipment:po:rfq-1:lot-1",
                "quality_certification:lot-1",
                "audit_attestation:lot-1",
            ] {
                assert!(
                    ids.contains(&expected),
                    "{label}: missing {expected}, committed records: {ids:?}"
                );
            }
            let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
            assert_eq!(
                unique.len(),
                ids.len(),
                "{label}: no record may commit twice"
            );

            // Custody edge: the lot moved maker → buyer; the buyer signed the
            // receipt.
            let shipment = records
                .iter()
                .find(|r| r.record_id == "shipment:po:rfq-1:lot-1")
                .unwrap();
            assert_eq!(
                shipment.payload.get("lot_ref"),
                Some(&Value::String("lot-1".into()))
            );
            assert_eq!(
                shipment.payload.get("from_org"),
                Some(&Value::String(MAKER.into()))
            );
            assert_eq!(
                shipment.payload.get("to_org"),
                Some(&Value::String(BUYER.into()))
            );
            let receipt = records
                .iter()
                .find(|r| r.record_id == "receipt:shipment:po:rfq-1:lot-1")
                .unwrap();
            assert_eq!(
                receipt.payload.get("receiver_id"),
                Some(&Value::String(BUYER.into()))
            );

            // Attestations reference the immutable lot record and carry their
            // canonical anchor (recomputed, not just echoed).
            for family in ["quality_certification", "audit_attestation"] {
                let attestation = records.iter().find(|r| r.schema_id == family).unwrap();
                assert_eq!(
                    attestation.payload.get("lot_ref"),
                    Some(&Value::String("lot-1".into()))
                );
                assert_eq!(
                    attestation.commitment,
                    Some(attestation.commitment().unwrap()),
                    "{label}: {family} carries its canonical anchor"
                );
            }
        }
    }
}

#[tokio::test]
async fn purchase_to_settlement_end_to_end_across_two_orgs() {
    let ctx = DealContext::new().await;
    ctx.anchor_catalog().await;
    ctx.buyer_po_phase().await;
    ctx.seller_ship_phase().await;
    ctx.receipt_phase().await;
    ctx.dispute_settlement_phase().await;
    ctx.attestation_phase().await;
    ctx.assert_outcomes().await;
}
