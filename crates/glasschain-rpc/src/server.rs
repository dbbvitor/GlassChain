//! Tonic gRPC server implementations for `LedgerService` and `NodeService`.

use crate::auth::{MspAuthInterceptor, TrustedKeyRegistry};
use crate::proto::glasschain_v1::{
    identity_service_server::{IdentityService, IdentityServiceServer},
    ledger_service_server::{LedgerService, LedgerServiceServer},
    node_service_server::{NodeService, NodeServiceServer},
    CustodyEventProto, ExchangeCertificateRequest, ExchangeCertificateResponse, GetBlockRequest,
    GetBlockResponse, GetChainStatusRequest, GetChainStatusResponse, GetNodeStatusRequest,
    GetNodeStatusResponse, GetPeersRequest, GetPeersResponse, GetVerifiableLineageRequest,
    GetVerifiableLineageResponse, MineBlockRequest, MineBlockResponse, QueryAssetHistoryRequest,
    QueryAssetHistoryResponse, StreamBlocksRequest, StreamBlocksResponse, SubmitTransactionRequest,
    SubmitTransactionResponse, SubscribeToEventsRequest, SubscribeToEventsResponse,
    TransactionProto, VerifyEndorsementRequest, VerifyEndorsementResponse,
};
use glasschain_core::{TraceableAssetRegistration, Transaction, TransactionKind};
use glasschain_identity::SignedTransaction;
use glasschain_network::{Node, NodeEvent};
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn build_transaction_protos(block: &glasschain_core::Block) -> Vec<TransactionProto> {
    block
        .transactions
        .iter()
        .map(|tx| {
            let kind = match &tx.kind {
                TransactionKind::SupplyOffer(_) => "SupplyOffer",
                TransactionKind::PurchaseOrder(_) => "PurchaseOrder",
                TransactionKind::ContractCreation(_) => "ContractCreation",
                TransactionKind::ContractExecution(_) => "ContractExecution",
                TransactionKind::InventoryUpdate(_) => "InventoryUpdate",
                TransactionKind::AssetRegistration(_) => "AssetRegistration",
                TransactionKind::CanonicalRecord(_) => "CanonicalRecord",
            };
            TransactionProto {
                id: tx.id.clone(),
                timestamp: tx.timestamp,
                kind: kind.to_owned(),
                payload_json: serde_json::to_string(tx).unwrap_or_default(),
            }
        })
        .collect()
}

/// Convert a block to a [`GetBlockResponse`] (used by the `GetBlock` RPC).
fn block_to_get_response(block: &glasschain_core::Block) -> GetBlockResponse {
    GetBlockResponse {
        index: block.index,
        hash: block.hash.clone(),
        previous_hash: block.previous_hash.clone(),
        timestamp: block.timestamp,
        nonce: block.nonce,
        transactions: build_transaction_protos(block),
    }
}

/// Convert a block to a [`StreamBlocksResponse`] (used by the `StreamBlocks` RPC).
fn block_to_stream_response(block: &glasschain_core::Block) -> StreamBlocksResponse {
    StreamBlocksResponse {
        index: block.index,
        hash: block.hash.clone(),
        previous_hash: block.previous_hash.clone(),
        timestamp: block.timestamp,
        nonce: block.nonce,
        transactions: build_transaction_protos(block),
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Map a [`NodeEvent`] to its subscription response.
fn event_to_response(event: &NodeEvent) -> SubscribeToEventsResponse {
    match event {
        NodeEvent::TransactionAccepted(t) => SubscribeToEventsResponse {
            timestamp: now_unix(),
            event_type: "transaction_accepted".into(),
            payload_json: serde_json::json!({ "transaction_id": t.id }).to_string(),
        },
        NodeEvent::BlockMined { index, hash } => SubscribeToEventsResponse {
            timestamp: now_unix(),
            event_type: "block_mined".into(),
            payload_json: serde_json::json!({
                "block_index": index,
                "block_hash": hash
            })
            .to_string(),
        },
        NodeEvent::BlockReceived { index, hash } => SubscribeToEventsResponse {
            timestamp: now_unix(),
            event_type: "block_received".into(),
            payload_json: serde_json::json!({
                "block_index": index,
                "block_hash": hash
            })
            .to_string(),
        },
        NodeEvent::PeerConnected(addr) => SubscribeToEventsResponse {
            timestamp: now_unix(),
            event_type: "peer_connected".into(),
            payload_json: serde_json::json!({ "address": addr }).to_string(),
        },
        NodeEvent::PeerDisconnected(addr) => SubscribeToEventsResponse {
            timestamp: now_unix(),
            event_type: "peer_disconnected".into(),
            payload_json: serde_json::json!({ "address": addr }).to_string(),
        },
        NodeEvent::ContractExecuted {
            contract_id,
            quantity,
        } => SubscribeToEventsResponse {
            timestamp: now_unix(),
            event_type: "contract_executed".into(),
            payload_json: serde_json::json!({
                "contract_id": contract_id,
                "quantity": quantity
            })
            .to_string(),
        },
        NodeEvent::AutonomousTransactionGenerated {
            trigger_id,
            transaction_id,
        } => SubscribeToEventsResponse {
            timestamp: now_unix(),
            event_type: "autonomous_tx_generated".into(),
            payload_json: serde_json::json!({
                "trigger_id": trigger_id,
                "transaction_id": transaction_id
            })
            .to_string(),
        },
    }
}

// ── Shared server state ───────────────────────────────────────────────────────

/// Shared state for both gRPC services.
///
/// The node reference provides access to the live ledger, peer list, and
/// event stream — the RPC layer no longer manages its own isolated ledger.
#[derive(Clone)]
struct ServerState {
    node: Arc<Node>,
}

// ── LedgerService implementation ──────────────────────────────────────────────

#[tonic::async_trait]
impl LedgerService for ServerState {
    type StreamBlocksStream = ReceiverStream<Result<StreamBlocksResponse, Status>>;
    type SubscribeToEventsStream = ReceiverStream<Result<SubscribeToEventsResponse, Status>>;

    async fn get_block(
        &self,
        request: Request<GetBlockRequest>,
    ) -> Result<Response<GetBlockResponse>, Status> {
        let index = request.into_inner().index;
        let ledger = self.node.shared_ledger();
        let ledger = ledger.lock().await;
        let block = usize::try_from(index)
            .ok()
            .and_then(|index| ledger.chain.get(index));
        block.map_or_else(
            || Err(Status::not_found(format!("block {index} not found"))),
            |block| Ok(Response::new(block_to_get_response(block))),
        )
    }

    /// Stream existing blocks from `start_index` and then push each new block
    /// as it is mined or received from a peer (live streaming).
    async fn stream_blocks(
        &self,
        request: Request<StreamBlocksRequest>,
    ) -> Result<Response<Self::StreamBlocksStream>, Status> {
        let start = usize::try_from(request.into_inner().start_index).unwrap_or(usize::MAX);
        let shared_ledger = self.node.shared_ledger();
        let mut event_rx = self.node.subscribe();

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            // First, send all existing blocks.
            let existing: Vec<StreamBlocksResponse> = {
                let ledger = shared_ledger.lock().await;
                ledger
                    .chain
                    .iter()
                    .skip(start)
                    .map(block_to_stream_response)
                    .collect()
            };
            for b in existing {
                if tx.send(Ok(b)).await.is_err() {
                    return;
                }
            }
            // Then stream new blocks as they are mined or received.
            loop {
                match event_rx.recv().await {
                    Ok(
                        NodeEvent::BlockMined { index, .. }
                        | NodeEvent::BlockReceived { index, .. },
                    ) => {
                        if let Ok(index) = usize::try_from(index) {
                            if index >= start {
                                let block_proto = {
                                    let ledger = shared_ledger.lock().await;
                                    ledger.chain.get(index).map(block_to_stream_response)
                                };
                                if let Some(b) = block_proto {
                                    if tx.send(Ok(b)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        log::warn!("stream_blocks lagged; skipped {skipped} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    /// Submit a transaction.
    ///
    /// If `signed_transaction_json` is non-empty the signature is verified
    /// before the transaction is forwarded to the node; otherwise the unsigned
    /// `transaction_json` path is used (backward-compatible).
    async fn submit_transaction(
        &self,
        request: Request<SubmitTransactionRequest>,
    ) -> Result<Response<SubmitTransactionResponse>, Status> {
        let req = request.into_inner();

        // ── Signed path ────────────────────────────────────────────────────
        if !req.signed_transaction_json.is_empty() {
            let signed: SignedTransaction = match serde_json::from_str(&req.signed_transaction_json)
            {
                Ok(s) => s,
                Err(e) => {
                    return Ok(Response::new(SubmitTransactionResponse {
                        accepted: false,
                        transaction_id: String::new(),
                        error: format!("invalid signed_transaction_json: {e}"),
                    }));
                }
            };
            if let Err(e) = signed.verify() {
                return Ok(Response::new(SubmitTransactionResponse {
                    accepted: false,
                    transaction_id: String::new(),
                    error: format!("signature verification failed: {e}"),
                }));
            }
            let tx_id = signed.transaction.id.clone();
            return match self.node.submit_transaction(signed.transaction).await {
                Ok(()) => Ok(Response::new(SubmitTransactionResponse {
                    accepted: true,
                    transaction_id: tx_id,
                    error: String::new(),
                })),
                Err(e) => Ok(Response::new(SubmitTransactionResponse {
                    accepted: false,
                    transaction_id: tx_id,
                    error: e.to_string(),
                })),
            };
        }

        // ── Unsigned path ──────────────────────────────────────────────────
        let tx: Transaction = match serde_json::from_str(&req.transaction_json) {
            Ok(t) => t,
            Err(e) => {
                return Ok(Response::new(SubmitTransactionResponse {
                    accepted: false,
                    transaction_id: String::new(),
                    error: e.to_string(),
                }));
            }
        };
        let tx_id = tx.id.clone();
        match self.node.submit_transaction(tx).await {
            Ok(()) => Ok(Response::new(SubmitTransactionResponse {
                accepted: true,
                transaction_id: tx_id,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(SubmitTransactionResponse {
                accepted: false,
                transaction_id: tx_id,
                error: e.to_string(),
            })),
        }
    }

    async fn get_chain_status(
        &self,
        _request: Request<GetChainStatusRequest>,
    ) -> Result<Response<GetChainStatusResponse>, Status> {
        let ledger = self.node.shared_ledger();
        let ledger = ledger.lock().await;
        Ok(Response::new(GetChainStatusResponse {
            chain_length: ledger.chain.len() as u64,
            tip_hash: ledger
                .chain
                .last()
                .map(|b| b.hash.clone())
                .unwrap_or_default(),
            pending_transactions: ledger.pending_transactions.len() as u64,
        }))
    }

    async fn query_asset_history(
        &self,
        request: Request<QueryAssetHistoryRequest>,
    ) -> Result<Response<QueryAssetHistoryResponse>, Status> {
        let req = request.into_inner();
        let ledger = self.node.shared_ledger();
        let transactions = {
            let ledger = ledger.lock().await;
            ledger
                .chain
                .iter()
                .flat_map(|b| b.transactions.iter())
                .filter_map(|tx| {
                    if let TransactionKind::AssetRegistration(TraceableAssetRegistration {
                        asset,
                        ..
                    }) = &tx.kind
                    {
                        let gtin_match =
                            req.gtin.is_empty() || asset.gtin.as_deref() == Some(req.gtin.as_str());
                        let serial_match = req.serial_number.is_empty()
                            || asset.serial_number.as_deref() == Some(req.serial_number.as_str());
                        if gtin_match && serial_match {
                            return Some(TransactionProto {
                                id: tx.id.clone(),
                                timestamp: tx.timestamp,
                                kind: "AssetRegistration".to_owned(),
                                payload_json: serde_json::to_string(tx).unwrap_or_default(),
                            });
                        }
                    }
                    None
                })
                .collect()
        };
        Ok(Response::new(QueryAssetHistoryResponse { transactions }))
    }

    /// Subscribe to live node events translated into gRPC `SubscribeToEventsResponse` messages.
    async fn subscribe_to_events(
        &self,
        _request: Request<SubscribeToEventsRequest>,
    ) -> Result<Response<Self::SubscribeToEventsStream>, Status> {
        let mut node_rx = self.node.subscribe();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                let evt = match node_rx.recv().await {
                    Ok(evt) => evt,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        log::warn!("subscribe_to_events lagged; skipped {skipped} events");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let resp = event_to_response(&evt);
                if tx.send(Ok(resp)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_verifiable_lineage(
        &self,
        request: Request<GetVerifiableLineageRequest>,
    ) -> Result<Response<GetVerifiableLineageResponse>, Status> {
        let asset_id = request.into_inner().asset_id;
        if asset_id.is_empty() {
            return Err(Status::invalid_argument("asset_id must not be empty"));
        }
        // Phase 5: Full implementation requires wiring glasschain-indexer's
        // ProvenanceIndex and AnalyticalFlattener to the ServerState.
        // For now, return a partial response from chain state.
        let ledger = self.node.shared_ledger();
        let ledger = ledger.lock().await;
        let mut custody_chain = Vec::new();
        let mut record_count = 0u32;
        for block in &ledger.chain {
            for tx in &block.transactions {
                if let TransactionKind::AssetRegistration(reg) = &tx.kind {
                    // Reconstruct the canonical composite key from the stored
                    // asset fields and compare with strict equality.  Substring
                    // matching would let a short GTIN like "0789" accidentally
                    // match a query for "07891234567890".
                    let canonical_id = match (&reg.asset.gtin, &reg.asset.serial_number) {
                        (Some(g), Some(s)) => format!("GTIN:{g}:SN:{s}"),
                        (Some(g), None) => format!("GTIN:{g}"),
                        (None, Some(s)) => format!("SN:{s}"),
                        (None, None) => String::new(),
                    };
                    let matches = !canonical_id.is_empty() && canonical_id == asset_id;
                    if matches {
                        custody_chain.push(CustodyEventProto {
                            asset_id: asset_id.clone(),
                            event_type: reg.event_type.clone(),
                            custodian_id: reg.asset.custodian_id.clone(),
                            transaction_id: tx.id.clone(),
                            block_index: block.index,
                            timestamp: tx.timestamp,
                        });
                        record_count += 1;
                    }
                }
            }
        }
        drop(ledger);
        Ok(Response::new(GetVerifiableLineageResponse {
            asset_id,
            custody_chain,
            is_complete: record_count > 0,
            trust_score_avg: 0.0, // requires indexer wiring
            total_records: record_count,
        }))
    }
}

// ── NodeService implementation ────────────────────────────────────────────────

#[tonic::async_trait]
impl NodeService for ServerState {
    async fn get_node_status(
        &self,
        _request: Request<GetNodeStatusRequest>,
    ) -> Result<Response<GetNodeStatusResponse>, Status> {
        let ledger = self.node.shared_ledger();
        let chain_length = ledger.lock().await.chain.len() as u64;
        let peer_count = self.node.known_peers().await.len() as u64;
        Ok(Response::new(GetNodeStatusResponse {
            node_id: self.node.node_id.clone(),
            listen_addr: self.node.listen_addr().to_owned(),
            version: "glasschain/1".into(),
            chain_length,
            peer_count,
        }))
    }

    async fn get_peers(
        &self,
        _request: Request<GetPeersRequest>,
    ) -> Result<Response<GetPeersResponse>, Status> {
        let peer_addresses = self.node.known_peers().await;
        Ok(Response::new(GetPeersResponse { peer_addresses }))
    }

    /// Mine a block via the node's `PoW` implementation.
    ///
    /// This RPC uses the node's synchronous mining API, which waits for block
    /// production to complete before returning the result to the caller.
    async fn mine_block(
        &self,
        _request: Request<MineBlockRequest>,
    ) -> Result<Response<MineBlockResponse>, Status> {
        match self.node.mine().await {
            Ok(()) => {
                let ledger = self.node.shared_ledger();
                let ledger = ledger.lock().await;
                ledger.chain.last().map_or_else(
                    || {
                        Ok(Response::new(MineBlockResponse {
                            success: false,
                            block_index: 0,
                            block_hash: String::new(),
                            error: "chain empty after mining".into(),
                        }))
                    },
                    |block| {
                        Ok(Response::new(MineBlockResponse {
                            success: true,
                            block_index: block.index,
                            block_hash: block.hash.clone(),
                            error: String::new(),
                        }))
                    },
                )
            }
            Err(e) => Ok(Response::new(MineBlockResponse {
                success: false,
                block_index: 0,
                block_hash: String::new(),
                error: e.to_string(),
            })),
        }
    }
}

// ── IdentityService implementation ───────────────────────────────────────────

#[tonic::async_trait]
impl IdentityService for ServerState {
    /// Exchange an organization Root CA certificate.
    ///
    /// This allows an external organization to register its Root CA with this
    /// node, enabling future signature verification against the org's PKI.
    async fn exchange_certificate(
        &self,
        request: Request<ExchangeCertificateRequest>,
    ) -> Result<Response<ExchangeCertificateResponse>, Status> {
        let req = request.into_inner();
        if req.org_name.is_empty() || req.root_ca_cert_pem.is_empty() {
            return Err(Status::invalid_argument(
                "org_name and root_ca_cert_pem are required",
            ));
        }
        log::info!(
            "Certificate exchange: org={} node={}",
            req.org_name,
            req.node_id
        );
        // Phase 2: Full integration requires storing the cert in a trust store
        // and returning this node's own certificate.  For now, acknowledge receipt.
        Ok(Response::new(ExchangeCertificateResponse {
            accepted: true,
            node_cert_pem: String::new(), // populated once identity integration is complete
            error: String::new(),
        }))
    }

    /// Verify an endorsement proposal against the configured policy.
    ///
    /// Accepts a JSON-serialised [`EndorsementProposal`] and returns whether
    /// the required number of valid endorsements have been collected.
    async fn verify_endorsement(
        &self,
        request: Request<VerifyEndorsementRequest>,
    ) -> Result<Response<VerifyEndorsementResponse>, Status> {
        let proposal_json = request.into_inner().proposal_json;
        if proposal_json.is_empty() {
            return Err(Status::invalid_argument("proposal_json must not be empty"));
        }
        // Phase 2: Full integration would deserialize the EndorsementProposal
        // and run EndorsementEngine::evaluate().
        // Return a well-documented stub that explains the expected contract.
        log::debug!(
            "verify_endorsement called (proposal_json len={})",
            proposal_json.len()
        );
        Ok(Response::new(VerifyEndorsementResponse {
            approved: false,
            proposal_id: String::new(),
            endorser_count: 0,
            rejection_reason: "endorsement engine not yet wired to RPC layer".into(),
        }))
    }
}

// ── Server builder ────────────────────────────────────────────────────────────

/// Combined gRPC server exposing `LedgerService`, `NodeService`, and
/// `IdentityService`.
///
/// The server wraps a live [`Node`] so that all RPC calls operate on the same
/// ledger state as the P2P network layer.
///
/// Optionally, an [`MspAuthInterceptor`] can be attached (via
/// [`with_auth`](Self::with_auth)) to enforce per-request ed25519 authentication
/// on every inbound RPC.
pub struct GlasschainServer {
    node: Arc<Node>,
    /// Optional MSP authentication interceptor.
    auth: Option<MspAuthInterceptor>,
}

impl GlasschainServer {
    /// Create a new server backed by the given node, with no authentication.
    #[must_use]
    pub const fn new(node: Arc<Node>) -> Self {
        Self { node, auth: None }
    }

    /// Create a server with MSP authentication.
    ///
    /// When `require_auth` is `true`, every inbound RPC must carry the three
    /// `x-glasschain-*` metadata headers and pass ed25519 signature verification
    /// against the supplied `registry`.  When `false`, headers are validated if
    /// present but their absence is allowed (backward-compatible mode).
    #[must_use]
    pub const fn with_auth(
        node: Arc<Node>,
        registry: TrustedKeyRegistry,
        require_auth: bool,
    ) -> Self {
        let interceptor = if require_auth {
            MspAuthInterceptor::new_strict(registry)
        } else {
            MspAuthInterceptor::new(registry)
        };
        Self {
            node,
            auth: Some(interceptor),
        }
    }

    /// Start the gRPC server and listen on `addr`.
    ///
    /// When an [`MspAuthInterceptor`] was configured via [`with_auth`](Self::with_auth),
    /// it is attached to every service.  Otherwise the services run without
    /// authentication middleware.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying tonic transport layer fails to bind to
    /// `addr` or encounters a fatal error while serving.
    ///
    /// # Example
    /// ```rust,no_run
    /// use glasschain_network::Node;
    /// use glasschain_rpc::server::GlasschainServer;
    /// use std::sync::Arc;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let node = Arc::new(Node::new("node-1", "0.0.0.0:8000", 2));
    ///     GlasschainServer::new(node)
    ///         .serve("[::1]:50051".parse().unwrap())
    ///         .await
    ///         .unwrap();
    /// }
    /// ```
    pub async fn serve(self, addr: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        let state = ServerState { node: self.node };

        log::info!("GlassChain gRPC server listening on {addr}");

        if let Some(auth) = self.auth {
            Server::builder()
                .add_service(LedgerServiceServer::with_interceptor(
                    state.clone(),
                    auth.clone(),
                ))
                .add_service(NodeServiceServer::with_interceptor(
                    state.clone(),
                    auth.clone(),
                ))
                .add_service(IdentityServiceServer::with_interceptor(state, auth))
                .serve(addr)
                .await?;
        } else {
            Server::builder()
                .add_service(LedgerServiceServer::new(state.clone()))
                .add_service(NodeServiceServer::new(state.clone()))
                .add_service(IdentityServiceServer::new(state))
                .serve(addr)
                .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::event_to_response;
    use glasschain_core::{InventoryUpdate, Transaction, TransactionKind};
    use glasschain_network::NodeEvent;

    /// Assert the `event_type` and payload JSON of a mapped event (timestamp
    /// is wall-clock and intentionally not asserted).
    fn assert_maps(event: &NodeEvent, event_type: &str, expected_payload: &serde_json::Value) {
        let resp = event_to_response(event);
        assert_eq!(resp.event_type, event_type);
        let actual: serde_json::Value = serde_json::from_str(&resp.payload_json).unwrap();
        assert_eq!(&actual, expected_payload);
    }

    #[test]
    fn test_event_mapping_all_variants() {
        let tx = Transaction::with_id(
            "tx-1".to_owned(),
            TransactionKind::InventoryUpdate(InventoryUpdate {
                product_id: "SKU-001".into(),
                owner_id: "owner-1".into(),
                quantity_delta: 10,
                reason: "test".into(),
            }),
        );
        assert_maps(
            &NodeEvent::TransactionAccepted(tx),
            "transaction_accepted",
            &serde_json::json!({ "transaction_id": "tx-1" }),
        );
        assert_maps(
            &NodeEvent::BlockMined {
                index: 3,
                hash: "abc".into(),
            },
            "block_mined",
            &serde_json::json!({ "block_index": 3, "block_hash": "abc" }),
        );
        assert_maps(
            &NodeEvent::BlockReceived {
                index: 4,
                hash: "def".into(),
            },
            "block_received",
            &serde_json::json!({ "block_index": 4, "block_hash": "def" }),
        );
        assert_maps(
            &NodeEvent::PeerConnected("10.0.0.1:8000".into()),
            "peer_connected",
            &serde_json::json!({ "address": "10.0.0.1:8000" }),
        );
        assert_maps(
            &NodeEvent::PeerDisconnected("10.0.0.1:8000".into()),
            "peer_disconnected",
            &serde_json::json!({ "address": "10.0.0.1:8000" }),
        );
        assert_maps(
            &NodeEvent::ContractExecuted {
                contract_id: "c1".into(),
                quantity: 50,
            },
            "contract_executed",
            &serde_json::json!({ "contract_id": "c1", "quantity": 50 }),
        );
        assert_maps(
            &NodeEvent::AutonomousTransactionGenerated {
                trigger_id: "trig-1".into(),
                transaction_id: "tx-2".into(),
            },
            "autonomous_tx_generated",
            &serde_json::json!({ "trigger_id": "trig-1", "transaction_id": "tx-2" }),
        );
    }
}
