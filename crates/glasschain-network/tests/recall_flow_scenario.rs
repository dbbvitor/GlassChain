//! Node-level recall scenario (ticket #44): recall, quarantine, and dispute
//! as first-class flows over three organizations (manufacturer, distributor,
//! pharmacy) on three connected nodes.
//!
//! Replaces the legacy `test_recall_simulation_manufacturer_to_pharmacy`
//! single-node `AssetRegistration` simulation: the lot's custody chain is
//! committed as canonical records, the manufacturer's recall flow issues and
//! activates the recall, and both downstream custodians observe the **public**
//! recall record — neither is a counterparty to it — responding with
//! quarantine and dispute records respectively. The dispute reason never
//! enters the global chain (payload whitelist, ADR-010 §1).

use glasschain_core::{
    providers::in_memory::InMemoryStorageProvider, CanonicalRecord, RecordSignature,
    StorageProvider, Transaction, TransactionKind,
};
use glasschain_network::Node;
use glasschain_workflows::{
    dispute_flow, quarantine_flow, recall_flow, Action, Event, FlowRunner, FlowTriage,
    RecallConfig, RecallFlowState, RecallResponseConfig, RecallResponseState,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAKER: &str = "org-maker";
const DISTRIBUTOR: &str = "org-distributor";
const PHARMACY: &str = "org-pharmacy";
const LOT: &str = "lot-1";
const DISPUTE_REASON: &str = "batch-not-in-our-stock";

/// Everything the scenario's phases share: three connected nodes, the
/// issuer's recall flow, and both custodians' response flows.
struct RecallContext {
    maker: Node,
    distributor: Node,
    pharmacy: Node,
    storages: Vec<Arc<dyn StorageProvider>>,
    triages: Vec<FlowTriage>,
    recall_flow: FlowRunner<RecallFlowState>,
    quarantine_flow: FlowRunner<RecallResponseState>,
    dispute_flow: FlowRunner<RecallResponseState>,
    recall_initial: RecallFlowState,
    response_initial: RecallResponseState,
}

impl RecallContext {
    /// Three fully-connected nodes; every node learns both peers via the
    /// handshake, so every mined block reaches all three chains.
    async fn new() -> Self {
        let maker_addr = free_addr();
        let distributor_addr = free_addr();
        let pharmacy_addr = free_addr();
        let maker = Node::new("maker-node", &maker_addr, 1);
        maker.start(vec![]).await.unwrap();
        let distributor = Node::new("distributor-node", &distributor_addr, 1);
        distributor.start(vec![maker_addr.clone()]).await.unwrap();
        let pharmacy = Node::new("pharmacy-node", &pharmacy_addr, 1);
        pharmacy
            .start(vec![maker_addr, distributor_addr])
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        Self {
            maker,
            distributor,
            pharmacy,
            storages: vec![storage(), storage(), storage()],
            triages: vec![FlowTriage::new(), FlowTriage::new(), FlowTriage::new()],
            recall_flow: recall_flow(RecallConfig {
                issuer: MAKER.to_owned(),
                lot_ref: LOT.to_owned(),
                issued_at: 1_700_000_200,
            }),
            quarantine_flow: quarantine_flow(RecallResponseConfig {
                org: DISTRIBUTOR.to_owned(),
                lot_ref: LOT.to_owned(),
                responded_at: 1_700_000_300,
            }),
            dispute_flow: dispute_flow(RecallResponseConfig {
                org: PHARMACY.to_owned(),
                lot_ref: LOT.to_owned(),
                responded_at: 1_700_000_300,
            }),
            recall_initial: RecallFlowState::AwaitingLot,
            response_initial: RecallResponseState::WatchingLot {
                lot_ref: LOT.to_owned(),
            },
        }
    }

    /// Poll until every node's chain is at least `min_len` blocks, or panic
    /// after 5s — never hang.
    async fn wait_for_propagation(&self, min_len: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        for (label, node) in self.nodes() {
            loop {
                if node.ledger_snapshot().await.chain.len() >= min_len {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{label} chain never reached {min_len} blocks"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }

    const fn nodes(&self) -> [(&'static str, &Node); 3] {
        [
            ("maker", &self.maker),
            ("distributor", &self.distributor),
            ("pharmacy", &self.pharmacy),
        ]
    }

    /// Execute a flow-emitted record durably through `node` and wait for it to
    /// reach the other two chains.
    async fn commit_record(&self, node: &Node, mut record: CanonicalRecord, issuer: &str) {
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
        let min_len = node.ledger_snapshot().await.chain.len();
        self.wait_for_propagation(min_len).await;
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

    /// The manufacturer anchors the catalog: product + immutable lot.
    async fn anchor_catalog(&self) {
        for record in catalog_records() {
            self.commit_record(&self.maker, record, MAKER).await;
        }
    }

    /// The lot's custody chain: manufacturer → distributor → pharmacy, one
    /// shipment + `delivery_receipt` per hop (the public trail the recall rides).
    async fn custody_chain(&self) {
        for (from, to, shipper, receiver) in [
            (MAKER, DISTRIBUTOR, &self.maker, &self.distributor),
            (DISTRIBUTOR, PHARMACY, &self.distributor, &self.pharmacy),
        ] {
            let mut shipment_payload = BTreeMap::new();
            shipment_payload.insert("lot_ref".to_owned(), Value::String(LOT.to_owned()));
            shipment_payload.insert("from_org".to_owned(), Value::String(from.to_owned()));
            shipment_payload.insert("to_org".to_owned(), Value::String(to.to_owned()));
            let mut shipment =
                CanonicalRecord::new(1_700_000_100, "shipment", shipment_payload, from);
            shipment.record_id = format!("shipment:{from}:{to}");
            let shipment_ref = shipment.record_id.clone();
            self.commit_record(shipper, shipment, from).await;

            let mut receipt_payload = BTreeMap::new();
            receipt_payload.insert(
                "shipment_ref".to_owned(),
                Value::String(shipment_ref.clone()),
            );
            receipt_payload.insert("receiver_id".to_owned(), Value::String(to.to_owned()));
            receipt_payload.insert("received_at".to_owned(), Value::String("2026-09-01".into()));
            let mut receipt =
                CanonicalRecord::new(1_700_000_100, "delivery_receipt", receipt_payload, to);
            receipt.record_id = format!("receipt:{shipment_ref}");
            self.commit_record(receiver, receipt, to).await;
        }
    }

    /// The manufacturer's recall lifecycle: issue (with an interrupted
    /// emission re-delivered from the checkpoint) and activate.
    async fn recall_lifecycle_phase(&self) {
        let lot_record = self
            .committed_records(&self.maker)
            .await
            .into_iter()
            .find(|record| record.record_id == LOT)
            .expect("lot committed");
        let (maker_storage, maker_triage) = (&self.storages[0], &self.triages[0]);
        let outcome = self
            .recall_flow
            .handle(
                maker_storage,
                maker_triage,
                "recall:lot-1",
                &self.recall_initial,
                &Event::RecordCommitted(lot_record),
            )
            .unwrap()
            .expect("anchor transition applies");
        assert!(matches!(outcome.state, RecallFlowState::LotAnchored { .. }));

        // Issue — the runtime "crashes" before ack; resume re-delivers the
        // same emission from the checkpoint (AC4).
        let first = self
            .recall_flow
            .handle(
                maker_storage,
                maker_triage,
                "recall:lot-1",
                &self.recall_initial,
                &Event::Woken("recall:contamination-suspected".into()),
            )
            .unwrap()
            .expect("issue transition applies");
        let [Action::EmitRecord(issued)] = &first.actions[..] else {
            panic!("expected the recall emission, got {:?}", first.actions);
        };
        let resumed = self
            .recall_flow
            .handle(
                maker_storage,
                maker_triage,
                "recall:lot-1",
                &self.recall_initial,
                &Event::Resumed("resume after crash".into()),
            )
            .unwrap()
            .expect("pending work re-delivered");
        assert_eq!(resumed.actions, first.actions, "re-delivery is identical");
        self.commit_record(&self.maker, issued.clone(), MAKER).await;
        self.recall_flow
            .ack(maker_storage, maker_triage, "recall:lot-1", 1)
            .unwrap();

        // Activate: a NEW append-only record with status "active".
        let outcome = self
            .recall_flow
            .handle(
                maker_storage,
                maker_triage,
                "recall:lot-1",
                &self.recall_initial,
                &Event::Woken("activate".into()),
            )
            .unwrap()
            .expect("activate transition applies");
        let [Action::EmitRecord(active)] = &outcome.actions[..] else {
            panic!(
                "expected the activation emission, got {:?}",
                outcome.actions
            );
        };
        self.commit_record(&self.maker, active.clone(), MAKER).await;
        self.recall_flow
            .ack(maker_storage, maker_triage, "recall:lot-1", 1)
            .unwrap();
    }

    /// Both custodians observe the public recall on their synced chains and
    /// respond: the distributor quarantines, the pharmacy disputes.
    async fn response_phase(&self) {
        for (label, flow, node, party, wake, issuer) in [
            (
                "distributor",
                &self.quarantine_flow,
                &self.distributor,
                1,
                "quarantine",
                DISTRIBUTOR,
            ),
            (
                "pharmacy",
                &self.dispute_flow,
                &self.pharmacy,
                2,
                &format!("dispute:{DISPUTE_REASON}"),
                PHARMACY,
            ),
        ] {
            let recall_record = self
                .committed_records(node)
                .await
                .into_iter()
                .find(|record| record.record_id == format!("recall:{LOT}"))
                .unwrap_or_else(|| panic!("{label} never observed the public recall"));
            let storage = &self.storages[party];
            let triage = &self.triages[party];
            let outcome = flow
                .handle(
                    storage,
                    triage,
                    "response:lot-1",
                    &self.response_initial,
                    &Event::RecordCommitted(recall_record),
                )
                .unwrap()
                .unwrap_or_else(|| panic!("{label} observation transition applies"));
            assert!(
                matches!(outcome.state, RecallResponseState::RecallObserved { .. }),
                "{label}"
            );
            let outcome = flow
                .handle(
                    storage,
                    triage,
                    "response:lot-1",
                    &self.response_initial,
                    &Event::Woken(wake.to_owned()),
                )
                .unwrap()
                .unwrap_or_else(|| panic!("{label} response transition applies"));
            let [Action::EmitRecord(response)] = &outcome.actions[..] else {
                panic!("{label}: expected the response emission");
            };
            self.commit_record(node, response.clone(), issuer).await;
            flow.ack(storage, triage, "response:lot-1", 1).unwrap();
        }
    }

    /// Assert the public recall trail on all three chains: every record
    /// present and propagated, no duplicates, no commercial records, and the
    /// dispute reason never on-chain.
    async fn assert_trail(&self) {
        for (label, node) in self.nodes() {
            let records = self.committed_records(node).await;
            let ids: Vec<&str> = records
                .iter()
                .map(|record| record.record_id.as_str())
                .collect();
            for expected in [
                LOT,
                "shipment:org-maker:org-distributor",
                "receipt:shipment:org-maker:org-distributor",
                "shipment:org-distributor:org-pharmacy",
                "receipt:shipment:org-distributor:org-pharmacy",
                "recall:lot-1",
                "recall:lot-1:active",
                "transformation:lot-1:quarantine",
                "transformation:lot-1:disputed",
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

            // Commercial terms never enter the global chain: no purchase
            // records at all, and the dispute reason appears in no payload.
            assert!(
                records
                    .iter()
                    .all(|record| record.schema_id != "purchase_order"),
                "{label}: commercial records must not exist"
            );
            for record in &records {
                let payload_json = serde_json::to_string(&record.payload).unwrap();
                assert!(
                    !payload_json.contains(DISPUTE_REASON),
                    "{label}: dispute reason leaked in {}: {payload_json}",
                    record.record_id
                );
            }

            // The recall trail references the immutable lot.
            let recall = records
                .iter()
                .find(|record| record.record_id == "recall:lot-1")
                .unwrap();
            assert_eq!(
                recall.payload.get("lot_ref"),
                Some(&Value::String(LOT.to_owned()))
            );
            assert_eq!(
                recall.payload.get("status").and_then(Value::as_str),
                Some("issued")
            );
            let quarantined = records
                .iter()
                .find(|record| record.record_id == "transformation:lot-1:quarantine")
                .unwrap();
            assert_eq!(
                quarantined.payload.get("lot_ref"),
                Some(&Value::String(LOT.to_owned()))
            );
        }
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

/// A product + anchored lot pair committed by the manufacturer (the immutable
/// anchor every recall record references).
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
    LOT.clone_into(&mut lot.record_id);
    lot.commitment = lot.commitment().ok();
    vec![product, lot]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recall_quarantine_dispute_flow_across_three_orgs() {
    let ctx = RecallContext::new().await;
    ctx.anchor_catalog().await;
    ctx.custody_chain().await;
    ctx.recall_lifecycle_phase().await;
    ctx.response_phase().await;
    ctx.assert_trail().await;
}
