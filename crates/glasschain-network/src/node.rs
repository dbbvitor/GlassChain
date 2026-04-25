use crate::error::NetworkError;
use crate::peer::PeerConnection;
use crate::protocol::{Message, PROTOCOL_VERSION};
use glasschain_contracts::ContractEngine;
use glasschain_core::{Ledger, SmartContractDef, Transaction, TransactionKind};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex};

/// Events emitted by the node that callers may observe.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// A new transaction was accepted into the pending pool.
    TransactionAccepted(Transaction),
    /// A new block was mined and appended to the chain.
    BlockMined { index: u64, hash: String },
    /// A peer connected.
    PeerConnected(String),
    /// A peer disconnected.
    PeerDisconnected(String),
    /// A smart contract was auto-executed.
    ContractExecuted { contract_id: String, quantity: u64 },
}

/// Shared mutable node state protected by a [`Mutex`].
struct NodeState {
    ledger: Ledger,
    engine: ContractEngine,
    known_peers: HashSet<String>,
}

/// A GlassChain network node.
///
/// Listens for inbound TCP connections from peers, connects to known seed
/// peers on start-up, and exposes methods to submit transactions and mine
/// blocks.
pub struct Node {
    pub node_id: String,
    listen_addr: String,
    state: Arc<Mutex<NodeState>>,
    event_tx: broadcast::Sender<NodeEvent>,
}

impl Node {
    /// Create a new node.
    ///
    /// * `node_id`     – unique identifier for this node (e.g. a UUID or hostname)
    /// * `listen_addr` – TCP address to listen on (e.g. `"0.0.0.0:8000"`)
    /// * `difficulty`  – PoW difficulty (number of leading zero nibbles)
    pub fn new(node_id: impl Into<String>, listen_addr: impl Into<String>, difficulty: usize) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            node_id: node_id.into(),
            listen_addr: listen_addr.into(),
            state: Arc::new(Mutex::new(NodeState {
                ledger: Ledger::new(difficulty),
                engine: ContractEngine::new(),
                known_peers: HashSet::new(),
            })),
            event_tx,
        }
    }

    /// Subscribe to node events.
    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.event_tx.subscribe()
    }

    /// Start the node: spawn a listener task and connect to seed peers.
    ///
    /// Returns immediately; all network activity runs in background tasks.
    pub async fn start(&self, seed_peers: Vec<String>) -> Result<(), NetworkError> {
        // Spawn listener task.
        let listener = TcpListener::bind(&self.listen_addr).await?;
        log::info!("Node {} listening on {}", self.node_id, self.listen_addr);

        let state = Arc::clone(&self.state);
        let node_id = self.node_id.clone();
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        let addr = peer_addr.to_string();
                        log::info!("Inbound connection from {}", addr);
                        let state2 = Arc::clone(&state);
                        let nid = node_id.clone();
                        let etx = event_tx.clone();
                        tokio::spawn(async move {
                            handle_peer(stream, addr, state2, nid, etx).await;
                        });
                    }
                    Err(e) => {
                        log::error!("Accept error: {e}");
                    }
                }
            }
        });

        // Connect to seed peers.
        for peer_addr in seed_peers {
            let state = Arc::clone(&self.state);
            let node_id = self.node_id.clone();
            let event_tx = self.event_tx.clone();
            tokio::spawn(async move {
                connect_to_peer(peer_addr, state, node_id, event_tx).await;
            });
        }

        Ok(())
    }

    /// Submit a transaction to the local pending pool and broadcast it to peers.
    pub async fn submit_transaction(&self, tx: Transaction) -> Result<(), NetworkError> {
        let generated = {
            let mut state = self.state.lock().await;
            state.ledger.add_transaction(tx.clone())?;

            // If this is a supply offer, evaluate against smart contracts.
            let mut generated = Vec::new();
            if let TransactionKind::SupplyOffer(ref offer) = tx.kind {
                generated = state.engine.evaluate_supply_offer(offer, &tx.id);
                for gen_tx in &generated {
                    let _ = state.ledger.add_transaction(gen_tx.clone());
                }
            }

            // If this is a contract creation, register it.
            if let TransactionKind::ContractCreation(ref def) = tx.kind {
                let _ = state.engine.register_contract(def.clone());
            }

            generated
        };

        let _ = self
            .event_tx
            .send(NodeEvent::TransactionAccepted(tx.clone()));

        // Emit contract-execution events.
        for gen_tx in &generated {
            if let TransactionKind::ContractExecution(ref exec) = gen_tx.kind {
                let _ = self.event_tx.send(NodeEvent::ContractExecuted {
                    contract_id: exec.contract_id.clone(),
                    quantity: exec.quantity,
                });
            }
        }

        // Broadcast to peers.
        self.broadcast(Message::Transaction(tx)).await;
        for gen_tx in generated {
            self.broadcast(Message::Transaction(gen_tx)).await;
        }

        Ok(())
    }

    /// Mine a new block containing all pending transactions and broadcast it.
    pub async fn mine(&self) -> Result<(), NetworkError> {
        let block = {
            let mut state = self.state.lock().await;
            state.ledger.mine_pending_transactions()?.clone()
        };
        log::info!("Mined block {} ({})", block.index, &block.hash[..8]);
        let _ = self.event_tx.send(NodeEvent::BlockMined {
            index: block.index,
            hash: block.hash.clone(),
        });
        self.broadcast(Message::Block(block)).await;
        Ok(())
    }

    /// Return a snapshot of the current ledger state.
    pub async fn ledger_snapshot(&self) -> Ledger {
        self.state.lock().await.ledger.clone()
    }

    /// Return the list of known peer addresses.
    pub async fn known_peers(&self) -> Vec<String> {
        self.state
            .lock()
            .await
            .known_peers
            .iter()
            .cloned()
            .collect()
    }

    /// Broadcast a message to all connected peers.
    ///
    /// Silently ignores failures; the peer-handling tasks are responsible for
    /// cleaning up broken connections.
    async fn broadcast(&self, message: Message) {
        let peers: Vec<String> = self
            .state
            .lock()
            .await
            .known_peers
            .iter()
            .cloned()
            .collect();

        for peer_addr in peers {
            let msg = message.clone();
            tokio::spawn(async move {
                if let Ok(stream) = TcpStream::connect(&peer_addr).await {
                    let mut conn = PeerConnection::new(stream, peer_addr.clone());
                    let _ = conn.send(&msg).await;
                }
            });
        }
    }
}

/// Handle a single peer connection (inbound or outbound).
async fn handle_peer(
    stream: TcpStream,
    addr: String,
    state: Arc<Mutex<NodeState>>,
    node_id: String,
    event_tx: broadcast::Sender<NodeEvent>,
) {
    let mut conn = PeerConnection::new(stream, addr.clone());

    // Send hello.
    let chain_length = {
        let s = state.lock().await;
        s.ledger.chain.len() as u64
    };
    let hello = Message::Hello {
        node_id: node_id.clone(),
        chain_length,
        version: PROTOCOL_VERSION.to_owned(),
    };
    if let Err(e) = conn.send(&hello).await {
        log::warn!("Failed to send Hello to {addr}: {e}");
        return;
    }

    // Register peer.
    {
        let mut s = state.lock().await;
        s.known_peers.insert(addr.clone());
    }
    let _ = event_tx.send(NodeEvent::PeerConnected(addr.clone()));

    // Message loop.
    loop {
        match conn.receive().await {
            Ok(msg) => {
                process_message(msg, &addr, &state, &node_id, &mut conn, &event_tx).await;
            }
            Err(crate::error::NetworkError::PeerDisconnected(_)) => {
                log::info!("Peer {} disconnected", addr);
                break;
            }
            Err(e) => {
                log::warn!("Error reading from {addr}: {e}");
                break;
            }
        }
    }

    // Unregister peer.
    {
        let mut s = state.lock().await;
        s.known_peers.remove(&addr);
    }
    let _ = event_tx.send(NodeEvent::PeerDisconnected(addr));
}

/// Connect outbound to a peer and run its handle loop.
async fn connect_to_peer(
    peer_addr: String,
    state: Arc<Mutex<NodeState>>,
    node_id: String,
    event_tx: broadcast::Sender<NodeEvent>,
) {
    match TcpStream::connect(&peer_addr).await {
        Ok(stream) => {
            log::info!("Connected to peer {}", peer_addr);
            handle_peer(stream, peer_addr, state, node_id, event_tx).await;
        }
        Err(e) => {
            log::warn!("Could not connect to {peer_addr}: {e}");
        }
    }
}

/// Dispatch an inbound message from a peer.
async fn process_message(
    msg: Message,
    addr: &str,
    state: &Arc<Mutex<NodeState>>,
    _node_id: &str,
    conn: &mut PeerConnection,
    event_tx: &broadcast::Sender<NodeEvent>,
) {
    match msg {
        Message::Hello {
            node_id: peer_id,
            chain_length,
            ..
        } => {
            conn.peer_id = Some(peer_id.clone());
            log::info!(
                "Hello from {} (id={}, chain_len={})",
                addr,
                peer_id,
                chain_length
            );
            // If the peer has a longer chain, request it.
            let local_len = state.lock().await.ledger.chain.len() as u64;
            if chain_length > local_len {
                if let Err(e) = conn.send(&Message::RequestChain).await {
                    log::warn!("Could not request chain from {addr}: {e}");
                }
            }
        }

        Message::Transaction(tx) => {
            let mut s = state.lock().await;
            if let TransactionKind::SupplyOffer(ref offer) = tx.kind {
                let generated = s.engine.evaluate_supply_offer(offer, &tx.id);
                for gen_tx in generated {
                    if let TransactionKind::ContractExecution(ref exec) = gen_tx.kind {
                        let _ = event_tx.send(NodeEvent::ContractExecuted {
                            contract_id: exec.contract_id.clone(),
                            quantity: exec.quantity,
                        });
                    }
                    let _ = s.ledger.add_transaction(gen_tx);
                }
            }
            if let TransactionKind::ContractCreation(ref def) = tx.kind {
                let def: SmartContractDef = def.clone();
                s.engine.load_from_ledger(def);
            }
            if let Err(e) = s.ledger.add_transaction(tx.clone()) {
                log::warn!("Could not add tx from {addr}: {e}");
            } else {
                let _ = event_tx.send(NodeEvent::TransactionAccepted(tx));
            }
        }

        Message::Block(block) => {
            let mut s = state.lock().await;
            let expected_index = s.ledger.chain.len() as u64;
            if block.index == expected_index {
                if let Some(prev) = s.ledger.chain.last() {
                    if block.chains_to(prev).is_ok() {
                        let idx = block.index;
                        let hash = block.hash.clone();
                        s.ledger.chain.push(block);
                        let _ = event_tx.send(NodeEvent::BlockMined { index: idx, hash });
                    } else {
                        log::warn!("Received invalid block from {addr}");
                    }
                }
            } else if block.index > expected_index {
                // We're behind; request the full chain.
                drop(s);
                if let Err(e) = conn.send(&Message::RequestChain).await {
                    log::warn!("Could not request chain from {addr}: {e}");
                }
            }
        }

        Message::RequestChain => {
            let chain = state.lock().await.ledger.chain.clone();
            if let Err(e) = conn.send(&Message::Chain(chain)).await {
                log::warn!("Could not send chain to {addr}: {e}");
            }
        }

        Message::Chain(candidate) => {
            let mut s = state.lock().await;
            s.ledger.try_replace_chain(candidate);
        }

        Message::RequestPeers => {
            let peers: Vec<String> = state
                .lock()
                .await
                .known_peers
                .iter()
                .filter(|p| p.as_str() != addr)
                .cloned()
                .collect();
            if let Err(e) = conn.send(&Message::Peers(peers)).await {
                log::warn!("Could not send peers to {addr}: {e}");
            }
        }

        Message::Peers(addrs) => {
            let mut s = state.lock().await;
            for a in addrs {
                s.known_peers.insert(a);
            }
        }

        Message::Goodbye { reason } => {
            log::info!("Peer {addr} says goodbye: {reason}");
        }
    }
}
