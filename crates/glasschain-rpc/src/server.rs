//! Tonic gRPC server implementations for `LedgerService` and `NodeService`.

use crate::proto::glasschain::{
    ledger_service_server::{LedgerService, LedgerServiceServer},
    node_service_server::{NodeService, NodeServiceServer},
    BlockResponse, ChainStatusRequest, ChainStatusResponse, GetBlockRequest, MineBlockRequest,
    MineBlockResponse, NodeStatusRequest, NodeStatusResponse, PeersRequest, PeersResponse,
    StreamBlocksRequest, SubmitTransactionRequest, SubmitTransactionResponse, TransactionProto,
};
use glasschain_core::{Ledger, Transaction, TransactionKind, DEFAULT_DIFFICULTY};
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{transport::Server, Request, Response, Status};

// ── Helpers ─────────────────────────────────────────────────────────────────

fn block_to_proto(block: &glasschain_core::Block) -> BlockResponse {
    let transactions = block
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
        .collect();

    BlockResponse {
        index: block.index,
        hash: block.hash.clone(),
        previous_hash: block.previous_hash.clone(),
        timestamp: block.timestamp,
        nonce: block.nonce,
        transactions,
    }
}

// ── Shared server state ──────────────────────────────────────────────────────

#[derive(Clone)]
struct ServerState {
    ledger: Arc<Mutex<Ledger>>,
    node_id: String,
    listen_addr: String,
    difficulty: usize,
}

// ── LedgerService implementation ─────────────────────────────────────────────

#[tonic::async_trait]
impl LedgerService for ServerState {
    type StreamBlocksStream = tokio_stream::wrappers::ReceiverStream<Result<BlockResponse, Status>>;

    async fn get_block(
        &self,
        request: Request<GetBlockRequest>,
    ) -> Result<Response<BlockResponse>, Status> {
        let index = request.into_inner().index;
        let ledger = self.ledger.lock().await;
        match ledger.chain.get(index as usize) {
            Some(block) => Ok(Response::new(block_to_proto(block))),
            None => Err(Status::not_found(format!("block {index} not found"))),
        }
    }

    async fn stream_blocks(
        &self,
        request: Request<StreamBlocksRequest>,
    ) -> Result<Response<Self::StreamBlocksStream>, Status> {
        let start = request.into_inner().start_index as usize;
        let ledger = self.ledger.lock().await;
        let blocks: Vec<BlockResponse> = ledger
            .chain
            .iter()
            .skip(start)
            .map(block_to_proto)
            .collect();
        drop(ledger);

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            for b in blocks {
                if tx.send(Ok(b)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
    }

    async fn submit_transaction(
        &self,
        request: Request<SubmitTransactionRequest>,
    ) -> Result<Response<SubmitTransactionResponse>, Status> {
        let req = request.into_inner();
        let tx: Transaction = match serde_json::from_str(&req.transaction_json) {
            Ok(t) => t,
            Err(e) => {
                return Ok(Response::new(SubmitTransactionResponse {
                    accepted: false,
                    transaction_id: String::new(),
                    error: e.to_string(),
                }))
            }
        };
        let tx_id = tx.id.clone();
        let mut ledger = self.ledger.lock().await;
        match ledger.add_transaction(tx) {
            Ok(_) => Ok(Response::new(SubmitTransactionResponse {
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
        _request: Request<ChainStatusRequest>,
    ) -> Result<Response<ChainStatusResponse>, Status> {
        let ledger = self.ledger.lock().await;
        Ok(Response::new(ChainStatusResponse {
            chain_length: ledger.chain.len() as u64,
            tip_hash: ledger
                .chain
                .last()
                .map(|b| b.hash.clone())
                .unwrap_or_default(),
            pending_transactions: ledger.pending_transactions.len() as u64,
        }))
    }
}

// ── NodeService implementation ────────────────────────────────────────────────

#[tonic::async_trait]
impl NodeService for ServerState {
    async fn get_node_status(
        &self,
        _request: Request<NodeStatusRequest>,
    ) -> Result<Response<NodeStatusResponse>, Status> {
        let ledger = self.ledger.lock().await;
        Ok(Response::new(NodeStatusResponse {
            node_id: self.node_id.clone(),
            listen_addr: self.listen_addr.clone(),
            version: "glasschain/1".into(),
            chain_length: ledger.chain.len() as u64,
            peer_count: 0,
        }))
    }

    async fn get_peers(
        &self,
        _request: Request<PeersRequest>,
    ) -> Result<Response<PeersResponse>, Status> {
        Ok(Response::new(PeersResponse {
            peer_addresses: vec![],
        }))
    }

    async fn mine_block(
        &self,
        _request: Request<MineBlockRequest>,
    ) -> Result<Response<MineBlockResponse>, Status> {
        let mut ledger = self.ledger.lock().await;
        match ledger.mine_pending_transactions() {
            Ok(block) => {
                let index = block.index;
                let hash = block.hash.clone();
                Ok(Response::new(MineBlockResponse {
                    success: true,
                    block_index: index,
                    block_hash: hash,
                    error: String::new(),
                }))
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
pub struct GlasschainServer {
    difficulty: usize,
    node_id: String,
}

impl GlasschainServer {
    /// Create a new server with the given PoW difficulty.
    pub fn new(difficulty: usize) -> Self {
        Self {
            difficulty,
            node_id: "glasschain-rpc-node".into(),
        }
    }

    /// Override the node ID reported in `NodeService::GetNodeStatus`.
    pub fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = node_id.into();
        self
    }

    /// Start the gRPC server and listen on `addr`.
    ///
    /// # Example
    /// ```rust,no_run
    /// use glasschain_rpc::server::GlasschainServer;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     GlasschainServer::new(2)
    ///         .serve("[::1]:50051".parse().unwrap())
    ///         .await
    ///         .unwrap();
    /// }
    /// ```
    pub async fn serve(self, addr: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        let state = ServerState {
            ledger: Arc::new(Mutex::new(Ledger::new(self.difficulty))),
            node_id: self.node_id,
            listen_addr: addr.to_string(),
            difficulty: self.difficulty,
        };

        log::info!("GlassChain gRPC server listening on {addr}");

        Server::builder()
            .add_service(LedgerServiceServer::new(state.clone()))
            .add_service(NodeServiceServer::new(state))
            .serve(addr)
            .await?;

        Ok(())
    }
}
