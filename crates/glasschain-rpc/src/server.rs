//! Tonic gRPC server implementations for `LedgerService` and `NodeService`.

use crate::proto::glasschain_v1::{
    ledger_service_server::{LedgerService, LedgerServiceServer},
    node_service_server::{NodeService, NodeServiceServer},
    GetBlockRequest, GetBlockResponse, GetChainStatusRequest, GetChainStatusResponse,
    GetNodeStatusRequest, GetNodeStatusResponse, GetPeersRequest, GetPeersResponse,
    MineBlockRequest, MineBlockResponse, QueryAssetHistoryRequest, QueryAssetHistoryResponse,
    StreamBlocksRequest, StreamBlocksResponse, SubmitTransactionRequest, SubmitTransactionResponse,
    SubscribeToEventsRequest, SubscribeToEventsResponse, TransactionProto,
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
        match ledger.chain.get(index as usize) {
            Some(block) => Ok(Response::new(block_to_get_response(block))),
            None => Err(Status::not_found(format!("block {index} not found"))),
        }
    }

    /// Stream existing blocks from `start_index` and then push each new block
    /// as it is mined or received from a peer (live streaming).
    async fn stream_blocks(
        &self,
        request: Request<StreamBlocksRequest>,
    ) -> Result<Response<Self::StreamBlocksStream>, Status> {
        let start = request.into_inner().start_index as usize;
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
                    Ok(NodeEvent::BlockMined { index, .. } | NodeEvent::BlockReceived { index, ..
}) => {
                        if (index as usize) >= start {
                            let block_proto = {
                                let ledger = shared_ledger.lock().await;
                                ledger
                                    .chain
                                    .get(index as usize)
                                    .map(block_to_stream_response)
                            };
                            if let Some(b) = block_proto {
                                if tx.send(Ok(b)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        log::warn!("stream_blocks lagged; skipped {skipped} events");
                        continue;
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
                Ok(()) => {
                    Ok(Response::new(SubmitTransactionResponse {
                        accepted: true,
                        transaction_id: tx_id,
                        error: String::new(),
                    }))
                }
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
            Ok(()) => {
                Ok(Response::new(SubmitTransactionResponse {
                    accepted: true,
                    transaction_id: tx_id,
                    error: String::new(),
                }))
            }
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
        let ledger = ledger.lock().await;
        let transactions = ledger
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
            .collect();
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
                let resp = match &evt {
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
                };
                if tx.send(Ok(resp)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
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
    /// The CPU-heavy `PoW` loop runs outside any mutex (the node's `mine()`
    /// method already uses the `prepare_mining` / `commit_mined_block` split),
    /// so this RPC call does not block other requests.
    async fn mine_block(
        &self,
        _request: Request<MineBlockRequest>,
    ) -> Result<Response<MineBlockResponse>, Status> {
        match self.node.mine().await {
            Ok(()) => {
                let ledger = self.node.shared_ledger();
                let ledger = ledger.lock().await;
                if let Some(block) = ledger.chain.last() {
                    Ok(Response::new(MineBlockResponse {
                        success: true,
                        block_index: block.index,
                        block_hash: block.hash.clone(),
                        error: String::new(),
                    }))
                } else {
                    Ok(Response::new(MineBlockResponse {
                        success: false,
                        block_index: 0,
                        block_hash: String::new(),
                        error: "chain empty after mining".into(),
                    }))
                }
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

// ── Server builder ────────────────────────────────────────────────────────────

/// Combined gRPC server exposing both `LedgerService` and `NodeService`.
///
/// The server wraps a live [`Node`] so that all RPC calls operate on the same
/// ledger state as the P2P network layer.
pub struct GlasschainServer {
    node: Arc<Node>,
}

impl GlasschainServer {
    /// Create a new server backed by the given node.
    #[must_use] 
    pub const fn new(node: Arc<Node>) -> Self {
        Self { node }
    }

    /// Start the gRPC server and listen on `addr`.
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

        Server::builder()
            .add_service(LedgerServiceServer::new(state.clone()))
            .add_service(NodeServiceServer::new(state))
            .serve(addr)
            .await?;

        Ok(())
    }
}
