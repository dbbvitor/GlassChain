//! Integration tests for the `GlassChain` gRPC server.
//!
//! These tests start a real [`Node`] with in-memory storage (the same way
//! `crates/glasschain-network/tests/node_integration.rs` does), serve a
//! [`GlasschainServer`] on an ephemeral loopback port, and drive its gRPC
//! surface over a real Tonic in-process TCP connection.
use glasschain_core::{
    ContractExecution, InventoryUpdate, PurchaseConditions, PurchaseOrder, SmartContractDef,
    SupplyOffer, TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
};
use glasschain_identity::Identity;
use glasschain_network::Node;
use glasschain_rpc::proto::glasschain_v1::{
    identity_service_client::IdentityServiceClient, ledger_service_client::LedgerServiceClient,
    node_service_client::NodeServiceClient, ExchangeCertificateRequest, GetBlockRequest,
    GetChainStatusRequest, GetNodeStatusRequest, GetPeersRequest, GetVerifiableLineageRequest,
    QueryAssetHistoryRequest, StreamBlocksRequest, SubmitTransactionRequest,
    SubscribeToEventsRequest,
};
use glasschain_rpc::server::GlasschainServer;
use std::sync::Arc;
use std::time::Duration;

const GTIN: &str = "07891234567890";

type LedgerClient = LedgerServiceClient<tonic::transport::Channel>;
type NodeClient = NodeServiceClient<tonic::transport::Channel>;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Allocate an ephemeral loopback port that is very likely free.
fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

/// Start a bare node with a temporary in-memory ledger.
async fn start_node() -> Arc<Node> {
    let node = Arc::new(Node::new("rpc-node", free_addr(), 1));
    node.start(vec![]).await.unwrap();
    node
}

/// Connect to `endpoint` (retrying briefly until the server accepts).
async fn connect(endpoint: &str) -> tonic::transport::Channel {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tonic::transport::Endpoint::from_shared(endpoint.to_owned())
            .unwrap()
            .connect()
            .await
        {
            Ok(ch) => return ch,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("failed to connect to gRPC server at {endpoint}: {e}"),
        }
    }
}

/// Serve a [`GlasschainServer`] on an ephemeral port and wait until it is ready.
async fn start_server(node: Arc<Node>) -> (tonic::transport::Channel, tokio::task::JoinHandle<()>) {
    let addr = free_addr();
    let endpoint = format!("http://{addr}");
    let server = GlasschainServer::new(node);
    let handle = tokio::spawn(async move {
        let _ = server.serve(addr.parse().unwrap()).await;
    });
    let channel = connect(&endpoint).await;
    (channel, handle)
}

fn inventory_tx(owner: &str) -> Transaction {
    Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-TEST".into(),
        owner_id: owner.into(),
        quantity_delta: 10,
        reason: "grpc integration test".into(),
    }))
}

fn asset_reg_tx(
    gtin: Option<&str>,
    serial: Option<&str>,
    event: &str,
    custodian: &str,
) -> Transaction {
    Transaction::new(TransactionKind::AssetRegistration(
        TraceableAssetRegistration {
            asset: TraceableAsset {
                gtin: gtin.map(str::to_owned),
                batch_number: None,
                expiry_date: None,
                serial_number: serial.map(str::to_owned),
                anvisa_registration: None,
                manufacturer_id: None,
                product_name: "Dipirona 500mg".into(),
                custodian_id: custodian.into(),
                country_of_origin: None,
                storage_temp_celsius: None,
                quantity: 1,
            },
            event_type: event.into(),
            originator_id: custodian.into(),
            purchase_order_ref: None,
        },
    ))
}

fn submit_req_json(tx: &Transaction) -> SubmitTransactionRequest {
    SubmitTransactionRequest {
        transaction_json: serde_json::to_string(tx).unwrap(),
        signed_transaction_json: String::new(),
    }
}

async fn submit_tx(ledger: &mut LedgerClient, tx: &Transaction) {
    let resp = ledger
        .submit_transaction(submit_req_json(tx))
        .await
        .unwrap()
        .into_inner();
    assert!(resp.accepted, "tx not accepted: {}", resp.error);
}

// ── Item 1: full gRPC surface over a real connection ─────────────────────────

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn test_grpc_unsigned_submit_chain_and_node_surface() {
    let node = start_node().await;
    let (channel, _handle) = start_server(node.clone()).await;
    let mut ledger = LedgerClient::new(channel.clone());
    let mut node_client = NodeClient::new(channel.clone());
    let mut identity_client = IdentityServiceClient::new(channel);

    // Fresh node status: chain is just genesis, no peers.
    let status = node_client
        .get_node_status(GetNodeStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(status.node_id, "rpc-node");
    assert_eq!(status.chain_length, 1);
    assert_eq!(status.peer_count, 0);

    let peers = node_client
        .get_peers(GetPeersRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(peers.peer_addresses.is_empty());

    // Unsigned submit lands in the pending pool.
    let tx = inventory_tx("owner-1");
    submit_tx(&mut ledger, &tx).await;

    let chain = ledger
        .get_chain_status(GetChainStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(chain.chain_length, 1);
    assert_eq!(chain.pending_transactions, 1);
    assert!(!chain.tip_hash.is_empty());

    // Mine → block 1 committed.
    node.mine().await.unwrap();

    let chain = ledger
        .get_chain_status(GetChainStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(chain.chain_length, 2);
    assert_eq!(chain.pending_transactions, 0);

    // get_block returns the mined block whose tip matches the chain status.
    let block = ledger
        .get_block(GetBlockRequest { index: 1 })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(block.hash, chain.tip_hash);
    assert_eq!(block.transactions.len(), 1);
    assert_eq!(block.transactions[0].kind, "InventoryUpdate");

    // Missing block → NotFound.
    let missing = ledger.get_block(GetBlockRequest { index: 99 }).await;
    assert!(missing.is_err());
    assert_eq!(missing.unwrap_err().code(), tonic::Code::NotFound);

    // Identity service is wired up (acknowledges a certificate exchange).
    let exchange = identity_client
        .exchange_certificate(ExchangeCertificateRequest {
            org_name: "acme".into(),
            root_ca_cert_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----".into(),
            node_id: "rpc-node".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(exchange.accepted);
}

#[tokio::test]
async fn test_grpc_signed_submit_and_signature_verification() {
    let node = start_node().await;
    let (channel, _handle) = start_server(node.clone()).await;
    let mut ledger = LedgerClient::new(channel);

    let identity = Identity::generate("signer-1");
    let tx = inventory_tx("owner-1");
    let mut signed = identity.sign_transaction(tx).unwrap();

    // Valid signature → accepted.
    let resp = ledger
        .submit_transaction(SubmitTransactionRequest {
            transaction_json: String::new(),
            signed_transaction_json: serde_json::to_string(&signed).unwrap(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(resp.accepted);
    assert_eq!(resp.transaction_id, signed.transaction.id);

    // Tampered signature → rejected at the signature-verification gate.
    signed.signature_bytes[0] ^= 0xff;
    let resp = ledger
        .submit_transaction(SubmitTransactionRequest {
            transaction_json: String::new(),
            signed_transaction_json: serde_json::to_string(&signed).unwrap(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!resp.accepted);
    assert!(
        resp.error.contains("signature verification failed"),
        "unexpected error: {}",
        resp.error
    );
}

// ── Item 2: query_asset_history filter logic ─────────────────────────────────

#[tokio::test]
async fn test_query_asset_history_filters() {
    let node = start_node().await;
    let (channel, _handle) = start_server(node.clone()).await;
    let mut ledger = LedgerClient::new(channel.clone());

    let first_registration = asset_reg_tx(Some(GTIN), Some("SN-AAA"), "manufacture", "factory-1");
    let second_registration = asset_reg_tx(Some(GTIN), Some("SN-BBB"), "dispatch", "dist-1");
    let nonmatching_registration = asset_reg_tx(
        Some("99999999999999"),
        Some("SN-CCC"),
        "manufacture",
        "factory-1",
    );
    let serial_only_registration = asset_reg_tx(None, Some("SN-SOLO"), "receive", "pharm-1");
    for tx in [
        &first_registration,
        &second_registration,
        &nonmatching_registration,
        &serial_only_registration,
    ] {
        submit_tx(&mut ledger, tx).await;
    }
    node.mine().await.unwrap();

    // GTIN only → both serials for that GTIN.
    let resp = ledger
        .query_asset_history(QueryAssetHistoryRequest {
            gtin: GTIN.into(),
            serial_number: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.transactions.len(), 2);

    // GTIN + serial → exactly one, and it is an asset registration.
    let resp = ledger
        .query_asset_history(QueryAssetHistoryRequest {
            gtin: GTIN.into(),
            serial_number: "SN-AAA".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.transactions.len(), 1);
    assert_eq!(resp.transactions[0].kind, "AssetRegistration");

    // Non-matching serial under a valid GTIN → empty.
    let resp = ledger
        .query_asset_history(QueryAssetHistoryRequest {
            gtin: GTIN.into(),
            serial_number: "SN-NOPE".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(resp.transactions.is_empty());

    // A GTIN prefix must not cross-match a longer GTIN (boundary-anchored).
    let resp = ledger
        .query_asset_history(QueryAssetHistoryRequest {
            gtin: "0789".into(),
            serial_number: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        resp.transactions.is_empty(),
        "short GTIN prefix must not match GTIN:07891234567890 assets"
    );

    // Serial-only query resolves the standalone `SN:<serial>` canonical key.
    let resp = ledger
        .query_asset_history(QueryAssetHistoryRequest {
            gtin: String::new(),
            serial_number: "SN-SOLO".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.transactions.len(), 1);
    assert_eq!(resp.transactions[0].kind, "AssetRegistration");
}

// ── Item 3: get_verifiable_lineage canonical-key matching ────────────────────

#[tokio::test]
async fn test_verifiable_lineage_canonical_key_matching() {
    let node = start_node().await;
    let (channel, _handle) = start_server(node.clone()).await;
    let mut ledger = LedgerClient::new(channel.clone());

    // Three assets exercising the three non-empty canonical forms.
    let full_asset = asset_reg_tx(Some(GTIN), Some("SN-F"), "manufacture", "factory-1");
    let gtin_only = asset_reg_tx(Some(GTIN), None, "dispatch", "dist-1");
    let serial_only = asset_reg_tx(None, Some("SN-S"), "receive", "pharm-1");
    // A batch-level asset under its own GTIN so its flat-record count stays
    // independent of the GTIN-keyed assertions above.
    let mut batch_asset = asset_reg_tx(Some("99999999999999"), None, "manufacture", "plant-b");
    if let TransactionKind::AssetRegistration(ref mut reg) = batch_asset.kind {
        reg.asset.batch_number = Some("BATCH-X".into());
    }
    for tx in [&full_asset, &gtin_only, &serial_only, &batch_asset] {
        submit_tx(&mut ledger, tx).await;
    }
    node.mine().await.unwrap();

    // GTIN+SN canonical key: the custody chain comes from the provenance
    // index; flat records (and therefore total_records and the trust average)
    // are the flattener's records for the asset's GTIN (both GTIN registrations).
    let resp = ledger
        .get_verifiable_lineage(GetVerifiableLineageRequest {
            asset_id: format!("GTIN:{GTIN}:SN:SN-F"),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.custody_chain.len(), 1);
    assert_eq!(resp.custody_chain[0].event_type, "manufacture");
    assert_eq!(resp.total_records, 2, "flat records are keyed by GTIN");
    assert!(!resp.is_complete, "1 custody event vs 2 flat records");
    assert!(
        resp.trust_score_avg > 0.0,
        "trust average from flat records"
    );

    // GTIN-only key matches the serial-less asset…
    let resp = ledger
        .get_verifiable_lineage(GetVerifiableLineageRequest {
            asset_id: format!("GTIN:{GTIN}"),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.custody_chain.len(), 1);
    assert_eq!(resp.custody_chain[0].custodian_id, "dist-1");
    assert_eq!(resp.total_records, 2);

    // SN-only key matches the serial-only asset (strict equality, no substring
    // cross-matching between different canonical forms).
    let resp = ledger
        .get_verifiable_lineage(GetVerifiableLineageRequest {
            asset_id: "SN:SN-S".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.custody_chain.len(), 1);
    assert_eq!(resp.custody_chain[0].custodian_id, "pharm-1");
    assert_eq!(resp.total_records, 0, "no GTIN → no flat records");

    // BATCH-keyed canonical form: custody from provenance, flat records from
    // the flattener keyed by the batch asset's GTIN.
    let resp = ledger
        .get_verifiable_lineage(GetVerifiableLineageRequest {
            asset_id: "GTIN:99999999999999:BATCH:BATCH-X".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.custody_chain.len(), 1);
    assert_eq!(resp.custody_chain[0].event_type, "manufacture");
    assert_eq!(resp.total_records, 1, "one flat record for the batch GTIN");
    assert!(resp.is_complete, "1 custody event and 1 flat record");
    assert!(resp.trust_score_avg > 0.0);

    // Empty asset_id → invalid_argument.
    let err = ledger
        .get_verifiable_lineage(GetVerifiableLineageRequest {
            asset_id: String::new(),
        })
        .await;
    assert!(err.is_err());
    assert_eq!(err.unwrap_err().code(), tonic::Code::InvalidArgument);
}

// ── Item 4: build_transaction_protos 6-variant match ─────────────────────────

#[tokio::test]
async fn test_build_transaction_protos_all_variants() {
    let node = start_node().await;
    let (channel, _handle) = start_server(node.clone()).await;
    let mut ledger = LedgerClient::new(channel.clone());

    let variants = vec![
        Transaction::new(TransactionKind::SupplyOffer(SupplyOffer {
            product_id: "SKU-1".into(),
            product_name: "Widget".into(),
            seller_id: "seller-1".into(),
            quantity_available: 100,
            price_per_unit: 1000,
            lead_time_days: 7,
            currency: "USD".into(),
        })),
        Transaction::new(TransactionKind::PurchaseOrder(PurchaseOrder {
            product_id: "SKU-1".into(),
            buyer_id: "buyer-1".into(),
            seller_id: "seller-1".into(),
            quantity: 5,
            agreed_price_per_unit: 1000,
            currency: "USD".into(),
            contract_id: None,
        })),
        Transaction::new(TransactionKind::ContractCreation(SmartContractDef {
            contract_id: "c-1".into(),
            buyer_id: "buyer-1".into(),
            product_id: "SKU-1".into(),
            conditions: PurchaseConditions {
                max_price_per_unit: 2000,
                min_quantity: 1,
                max_quantity: 50,
                max_lead_time_days: 14,
                preferred_seller_id: None,
                currency: "USD".into(),
                auto_execute: false,
            },
            wasm_code_b64: None,
        })),
        Transaction::new(TransactionKind::ContractExecution(ContractExecution {
            contract_id: "c-1".into(),
            purchase_order_tx_id: "po-1".into(),
            buyer_id: "buyer-1".into(),
            seller_id: "seller-1".into(),
            product_id: "SKU-1".into(),
            quantity: 5,
            total_price: 5000,
            currency: "USD".into(),
        })),
        Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
            product_id: "SKU-1".into(),
            owner_id: "owner-1".into(),
            quantity_delta: -1,
            reason: "sale".into(),
        })),
        Transaction::new(TransactionKind::AssetRegistration(
            TraceableAssetRegistration {
                asset: TraceableAsset {
                    gtin: Some(GTIN.into()),
                    batch_number: None,
                    expiry_date: None,
                    serial_number: Some("SN-X".into()),
                    anvisa_registration: None,
                    manufacturer_id: None,
                    product_name: "Drug".into(),
                    custodian_id: "cust-1".into(),
                    country_of_origin: None,
                    storage_temp_celsius: None,
                    quantity: 1,
                },
                event_type: "manufacture".into(),
                originator_id: "cust-1".into(),
                purchase_order_ref: None,
            },
        )),
    ];
    for tx in &variants {
        submit_tx(&mut ledger, tx).await;
    }
    node.mine().await.unwrap();

    let block = ledger
        .get_block(GetBlockRequest { index: 1 })
        .await
        .unwrap()
        .into_inner();
    let mut kinds: Vec<&str> = block.transactions.iter().map(|t| t.kind.as_str()).collect();
    kinds.sort_unstable();
    let expected = [
        "AssetRegistration",
        "ContractCreation",
        "ContractExecution",
        "InventoryUpdate",
        "PurchaseOrder",
        "SupplyOffer",
    ];
    assert_eq!(kinds, expected);
    assert_eq!(block.transactions.len(), 6);
}

// ── Item 5: stream_blocks live streaming ─────────────────────────────────────

#[tokio::test]
async fn test_stream_blocks_replays_then_live_streams() {
    let node = start_node().await;
    let (channel, _handle) = start_server(node.clone()).await;
    let mut ledger = LedgerClient::new(channel.clone());

    // Commit block 1 before opening the stream.
    let first = inventory_tx("owner-1");
    submit_tx(&mut ledger, &first).await;
    node.mine().await.unwrap();

    // Stream from index 1: replays block 1, then live-streams block 2.
    let mut stream = ledger
        .stream_blocks(StreamBlocksRequest { start_index: 1 })
        .await
        .unwrap()
        .into_inner();

    let b1 = tokio::time::timeout(Duration::from_secs(3), stream.message())
        .await
        .expect("timeout waiting for replayed block")
        .expect("status error")
        .expect("stream ended early");
    assert_eq!(b1.index, 1);

    // Mine a second block; it should arrive on the live stream.
    let second = inventory_tx("owner-2");
    submit_tx(&mut ledger, &second).await;
    node.mine().await.unwrap();

    let b2 = tokio::time::timeout(Duration::from_secs(3), stream.message())
        .await
        .expect("timeout waiting for live block")
        .expect("status error")
        .expect("stream ended early");
    assert_eq!(b2.index, 2);
}

// ── Item 6: subscribe_to_events mapping ──────────────────────────────────────

#[tokio::test]
async fn test_subscribe_to_events_mapping() {
    let node = start_node().await;
    let (channel, _handle) = start_server(node.clone()).await;
    let mut ledger = LedgerClient::new(channel.clone());

    let mut events = ledger
        .subscribe_to_events(SubscribeToEventsRequest {})
        .await
        .unwrap()
        .into_inner();

    // transaction_accepted on submit.
    let tx = inventory_tx("owner-1");
    submit_tx(&mut ledger, &tx).await;
    let evt = tokio::time::timeout(Duration::from_secs(3), events.message())
        .await
        .expect("timeout waiting for transaction_accepted")
        .expect("status error")
        .expect("stream ended early");
    assert_eq!(evt.event_type, "transaction_accepted");
    assert!(evt.payload_json.contains(&tx.id));

    // block_mined on mine.
    node.mine().await.unwrap();
    let evt = tokio::time::timeout(Duration::from_secs(3), events.message())
        .await
        .expect("timeout waiting for block_mined")
        .expect("status error")
        .expect("stream ended early");
    assert_eq!(evt.event_type, "block_mined");
    assert!(evt.payload_json.contains("\"block_index\":1"));
}
