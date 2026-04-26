use crate::error::NetworkError;
use crate::peer::PeerConnection;
use crate::protocol::{Message, PROTOCOL_VERSION};
use glasschain_contracts::ContractEngine;
use glasschain_core::{Block, Ledger, SmartContractDef, Transaction, TransactionKind};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex};

/// Events emitted by the node that callers may observe.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// A new transaction was accepted into the pending pool.
    TransactionAccepted(Transaction),
    /// A block was successfully mined **by this node** and appended to the chain.
    BlockMined { index: u64, hash: String },
    /// A block received from a remote peer was validated and appended to the chain.
    BlockReceived { index: u64, hash: String },
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
        let listen_addr = self.listen_addr.clone();
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        let addr = peer_addr.to_string();
                        log::info!("Inbound connection from {}", addr);
                        let state2 = Arc::clone(&state);
                        let nid = node_id.clone();
                        let la = listen_addr.clone();
                        let etx = event_tx.clone();
                        tokio::spawn(async move {
                            handle_peer(stream, addr, la, state2, nid, etx).await;
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
            let listen_addr = self.listen_addr.clone();
            let event_tx = self.event_tx.clone();
            tokio::spawn(async move {
                connect_to_peer(peer_addr, listen_addr, state, node_id, event_tx).await;
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
    ///
    /// The CPU-heavy Proof-of-Work loop runs **outside** the state mutex so
    /// that other node activity (transaction submission, message handling) is
    /// not blocked during mining.
    pub async fn mine(&self) -> Result<(), NetworkError> {
        // Step 1 – snapshot what we need and drain the pending pool, then drop
        // the lock before starting PoW.
        let (index, prev_hash, transactions, difficulty) = {
            let mut state = self.state.lock().await;
            state.ledger.prepare_mining()?
        };

        // Step 2 – CPU-heavy PoW runs without holding the mutex.
        let mut block = Block::new(index, transactions, prev_hash.clone());
        block.mine(difficulty);

        // Step 3 – re-acquire the lock and append the block only if the chain
        // tip has not moved since we started mining.
        let appended = {
            let mut state = self.state.lock().await;
            state.ledger.commit_mined_block(block.clone(), &prev_hash)?
        };

        if appended {
            log::info!("Mined block {} ({})", block.index, &block.hash[..8]);
            let _ = self.event_tx.send(NodeEvent::BlockMined {
                index: block.index,
                hash: block.hash.clone(),
            });
            self.broadcast(Message::Block(block)).await;
        }

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
///
/// Peer registration is deferred until the remote node's `Hello` message is
/// received, so that the stable `listen_addr` advertised in the handshake
/// (rather than the ephemeral TCP source port) is stored in `known_peers`.
async fn handle_peer(
    stream: TcpStream,
    addr: String,
    listen_addr: String,
    state: Arc<Mutex<NodeState>>,
    node_id: String,
    event_tx: broadcast::Sender<NodeEvent>,
) {
    let mut conn = PeerConnection::new(stream, addr.clone());

    // Send our Hello including our stable listening address so the remote peer
    // can register us correctly.
    let chain_length = {
        let s = state.lock().await;
        s.ledger.chain.len() as u64
    };
    let hello = Message::Hello {
        node_id: node_id.clone(),
        chain_length,
        version: PROTOCOL_VERSION.to_owned(),
        listen_addr: listen_addr.clone(),
    };
    if let Err(e) = conn.send(&hello).await {
        log::warn!("Failed to send Hello to {addr}: {e}");
        return;
    }

    // `peer_stable_addr` is populated by the Hello handler and used on
    // disconnect to remove the right entry from `known_peers`.
    // `std::sync::Mutex` (not `tokio::sync::Mutex`) is intentional: the lock
    // is only ever acquired in non-async, non-blocking code so it will never
    // be held across an `.await` point and therefore cannot stall the runtime.
    let peer_stable_addr: std::sync::Mutex<Option<String>> =
        std::sync::Mutex::new(None);

    // Message loop.
    loop {
        match conn.receive().await {
            Ok(msg) => {
                process_message(
                    msg,
                    &addr,
                    &state,
                    &node_id,
                    &mut conn,
                    &event_tx,
                    &peer_stable_addr,
                )
                .await;
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

    // Unregister peer using the stable address if we managed to complete the
    // handshake, otherwise fall back to the TCP source address.
    let registered = peer_stable_addr.lock().unwrap().clone();
    let display_addr = registered.clone().unwrap_or_else(|| addr.clone());
    if let Some(ref stable) = registered {
        let mut s = state.lock().await;
        s.known_peers.remove(stable);
    }
    let _ = event_tx.send(NodeEvent::PeerDisconnected(display_addr));
}

/// Connect outbound to a peer and run its handle loop.
async fn connect_to_peer(
    peer_addr: String,
    listen_addr: String,
    state: Arc<Mutex<NodeState>>,
    node_id: String,
    event_tx: broadcast::Sender<NodeEvent>,
) {
    match TcpStream::connect(&peer_addr).await {
        Ok(stream) => {
            log::info!("Connected to peer {}", peer_addr);
            handle_peer(stream, peer_addr, listen_addr, state, node_id, event_tx).await;
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
    peer_stable_addr: &std::sync::Mutex<Option<String>>,
) {
    match msg {
        Message::Hello {
            node_id: peer_id,
            chain_length,
            listen_addr: peer_listen_addr,
            ..
        } => {
            conn.peer_id = Some(peer_id.clone());
            log::info!(
                "Hello from {} (id={}, chain_len={}, listen_addr={})",
                addr,
                peer_id,
                chain_length,
                peer_listen_addr,
            );

            // Register the peer using its stable listening address (not the
            // ephemeral TCP source port).
            {
                let mut s = state.lock().await;
                s.known_peers.insert(peer_listen_addr.clone());
            }
            *peer_stable_addr.lock().unwrap() = Some(peer_listen_addr.clone());
            let _ = event_tx.send(NodeEvent::PeerConnected(peer_listen_addr.clone()));

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
                    let difficulty = s.ledger.difficulty;
                    if block.chains_to(prev).is_ok() && block.has_valid_pow(difficulty) {
                        let idx = block.index;
                        let hash = block.hash.clone();

                        // Remove any pending transactions that are already
                        // included in this block to avoid re-committing them.
                        let committed_ids: HashSet<&str> =
                            HashSet::from_iter(block.transactions.iter().map(|t| t.id.as_str()));
                        s.ledger
                            .pending_transactions
                            .retain(|t| !committed_ids.contains(t.id.as_str()));

                        s.ledger.chain.push(block);
                        // Use BlockReceived (not BlockMined) to distinguish
                        // remotely sourced blocks from locally mined ones.
                        let _ = event_tx
                            .send(NodeEvent::BlockReceived { index: idx, hash });
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
            let stable = peer_stable_addr.lock().unwrap().clone();
            let exclude = stable.as_deref().unwrap_or(addr);
            let peers: Vec<String> = state
                .lock()
                .await
                .known_peers
                .iter()
                .filter(|p| p.as_str() != exclude)
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
