use crate::error::NetworkError;
use crate::peer::{PeerReader, PeerWriter};
use crate::protocol::{Message, PROTOCOL_VERSION};
use glasschain_contracts::{ContractEngine, InventoryTrigger, WatcherService};
use glasschain_core::providers::in_memory::InMemoryStorageProvider;
use glasschain_core::{
    Block, ExecutionProvider, Ledger, StorageProvider, Transaction, TransactionKind,
};
use glasschain_indexer::{EventBusProvider, InMemoryEventBus, InMemoryIndexer, IndexerProvider};
use rcgen;
use rustls;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{error::TrySendError, Receiver, Sender, UnboundedReceiver, UnboundedSender};
use tokio::sync::{broadcast, Mutex};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// Events emitted by the node that callers may observe.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// A new transaction was accepted into the pending pool.
    TransactionAccepted(Transaction),
    /// A block was successfully mined **by this node** and appended to the chain.
    BlockMined { index: u64, hash: String },
    /// A block received from a remote peer was validated and appended to the chain.
    BlockReceived { index: u64, hash: String },
    /// A peer connected and completed the handshake.
    PeerConnected(String),
    /// A peer disconnected.
    PeerDisconnected(String),
    /// A smart contract was auto-executed.
    ContractExecuted { contract_id: String, quantity: u64 },
    /// A watcher-triggered autonomous transaction was generated.
    AutonomousTransactionGenerated {
        trigger_id: String,
        transaction_id: String,
    },
}

/// A summary of a live contract's runtime state (for external display).
#[derive(Debug, Clone)]
pub struct ContractSummary {
    pub id: String,
    pub buyer_id: String,
    pub product_id: String,
    pub status: String,
    pub quantity_purchased: u64,
    pub max_quantity: u64,
}

// ── TLS context ───────────────────────────────────────────────────────────────

/// TLS context shared by all peer connections on this node.
///
/// Each node generates a fresh self-signed certificate on startup.
/// The [`AcceptAnyCert`] verifier accepts any peer certificate so that
/// all connections are encrypted without requiring a shared CA.
struct NodeTls {
    /// Accepts inbound TLS connections.
    acceptor: Arc<TlsAcceptor>,
    /// Initiates outbound TLS connections.
    connector: Arc<TlsConnector>,
}

/// A TLS [`ServerCertVerifier`] that accepts any certificate.
///
/// This provides transport encryption without mutual authentication.
/// Replace with an organization-CA verifier to enforce identity.
#[derive(Debug)]
struct AcceptAnyCert;

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// ── Internal state ────────────────────────────────────────────────────────────

/// Mutable state that requires short-duration exclusive access.
///
/// The ledger is stored separately in [`Node`] so it can be shared with the
/// gRPC layer without going through [`NodeState`].
struct NodeState {
    engine: ContractEngine,
    watcher: WatcherService,
    known_peers: HashSet<String>,
    /// Per-peer write channels; keyed by the peer's stable listen address.
    peer_senders: HashMap<String, Sender<Message>>,
}

// ── Node ──────────────────────────────────────────────────────────────────────

/// A `GlassChain` network node.
///
/// Listens for inbound TCP connections from peers, connects to known seed
/// peers on start-up, and exposes methods to submit transactions and mine
/// blocks.
///
/// The **ledger** is stored in a separate `Arc<Mutex<Ledger>>` so it can be
/// accessed directly by the gRPC server without going through the full node
/// state lock.
pub struct Node {
    pub node_id: String,
    listen_addr: String,
    /// The distributed ledger — accessible directly for sharing with the RPC layer.
    ledger: Arc<Mutex<Ledger>>,
    state: Arc<Mutex<NodeState>>,
    event_tx: broadcast::Sender<NodeEvent>,
    indexer: Arc<InMemoryIndexer>,
    event_bus: Arc<InMemoryEventBus>,
    /// Block storage backend — persists committed blocks across restarts.
    storage: Arc<dyn StorageProvider>,
    /// TLS context used to encrypt all peer connections.
    tls: Arc<NodeTls>,
}

impl Node {
    /// Create a new node.
    ///
    /// * `node_id`     – unique identifier for this node (e.g. a UUID or hostname)
    /// * `listen_addr` – TCP address to listen on (e.g. `"0.0.0.0:8000"`)
    /// * `difficulty`  – `PoW` difficulty (number of leading zero hex nibbles)
    pub fn new(
        node_id: impl Into<String>,
        listen_addr: impl Into<String>,
        difficulty: usize,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            node_id: node_id.into(),
            listen_addr: listen_addr.into(),
            ledger: Arc::new(Mutex::new(Ledger::new(difficulty))),
            state: Arc::new(Mutex::new(NodeState {
                engine: ContractEngine::new(),
                watcher: WatcherService::new(),
                known_peers: HashSet::new(),
                peer_senders: HashMap::new(),
            })),
            event_tx,
            indexer: Arc::new(InMemoryIndexer::new()),
            event_bus: Arc::new(InMemoryEventBus::new(4096)),
            storage: Arc::new(InMemoryStorageProvider::new()),
            tls: Arc::new(Self::build_tls()),
        }
    }

    /// Create a node backed by a persistent [`StorageProvider`].
    ///
    /// Existing blocks in the store are loaded and validated on startup so
    /// that the node resumes from where it left off after a restart.
    pub fn new_with_storage(
        node_id: impl Into<String>,
        listen_addr: impl Into<String>,
        difficulty: usize,
        storage: Arc<dyn StorageProvider>,
    ) -> Self {
        // Attempt to restore the chain from storage.
        let ledger = {
            let mut l = Ledger::new(difficulty);
            if let Ok(Some(latest_idx)) = storage.latest_block_index() {
                let mut chain = Vec::with_capacity((latest_idx + 1) as usize);
                let mut valid = true;
                for i in 0..=latest_idx {
                    if let Ok(Some(block)) = storage.get_block(i) { chain.push(block) } else {
                        valid = false;
                        break;
                    }
                }
                if valid && !chain.is_empty() {
                    let genesis_ok = chain[0].is_valid()
                        && chain[0].has_valid_pow(difficulty)
                        && chain[0].previous_hash == "0";
                    let chain_ok = genesis_ok
                        && chain
                        .windows(2)
                        .all(|w| w[1].chains_to(&w[0]).is_ok() && w[1].has_valid_pow(difficulty));
                    if chain_ok {
                        l.chain = chain;
                        log::info!("Restored {} blocks from storage", l.chain.len());
                    } else {
                        log::warn!("Stored chain failed validation; starting fresh");
                    }
                }
            }
            Arc::new(Mutex::new(l))
        };

        let (event_tx, _) = broadcast::channel(256);
        Self {
            node_id: node_id.into(),
            listen_addr: listen_addr.into(),
            ledger,
            state: Arc::new(Mutex::new(NodeState {
                engine: ContractEngine::new(),
                watcher: WatcherService::new(),
                known_peers: HashSet::new(),
                peer_senders: HashMap::new(),
            })),
            event_tx,
            indexer: Arc::new(InMemoryIndexer::new()),
            event_bus: Arc::new(InMemoryEventBus::new(4096)),
            storage,
            tls: Arc::new(Self::build_tls()),
        }
    }

    /// Subscribe to node events (transactions, blocks, peers, contracts).
    #[must_use] 
    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.event_tx.subscribe()
    }

    /// Return a clone of the shared ledger handle.
    #[must_use] 
    pub fn shared_ledger(&self) -> Arc<Mutex<Ledger>> {
        Arc::clone(&self.ledger)
    }

    /// Return the TCP address this node listens on.
    #[must_use] 
    pub fn listen_addr(&self) -> &str {
        &self.listen_addr
    }

    /// Register an inventory watcher trigger on this node.
    pub async fn register_inventory_trigger(&self, trigger: InventoryTrigger) {
        self.state.lock().await.watcher.add_trigger(trigger);
    }

    /// Return a snapshot of all registered contracts for display.
    pub async fn contract_summaries(&self) -> Vec<ContractSummary> {
        self.state
            .lock()
            .await
            .engine
            .contracts()
            .map(|c| ContractSummary {
                id: c.id().to_owned(),
                buyer_id: c.buyer_id().to_owned(),
                product_id: c.product_id().to_owned(),
                status: c.status.to_string(),
                quantity_purchased: c.quantity_purchased,
                max_quantity: c.conditions().max_quantity,
            })
            .collect()
    }

    /// Generate a self-signed TLS certificate and build the node's TLS context.
    fn build_tls() -> NodeTls {
        // Ensure the ring crypto provider is installed (required by rustls 0.23).
        let _ = rustls::crypto::ring::default_provider().install_default();

        let key_pair = rcgen::KeyPair::generate().expect("TLS key generation");
        let params = rcgen::CertificateParams::new(vec!["glasschain-node".to_string()])
            .expect("TLS cert params");
        let cert = params.self_signed(&key_pair).expect("TLS self-signed cert");

        let cert_der: CertificateDer<'static> = cert.der().clone();
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

        let server_cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .expect("server TLS config");

        let client_cfg = if Self::insecure_tls_allowed() {
            log::warn!(
                "Network TLS certificate verification is disabled (dev mode). \
                 Use trusted roots in production."
            );
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
                .with_no_client_auth()
        } else {
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth()
        };

        NodeTls {
            acceptor: Arc::new(TlsAcceptor::from(Arc::new(server_cfg))),
            connector: Arc::new(TlsConnector::from(Arc::new(client_cfg))),
        }
    }

    /// Insecure verifier is allowed only in debug builds, via feature flag,
    /// or explicitly via `GLASSCHAIN_INSECURE_TLS=1`.
    fn insecure_tls_allowed() -> bool {
        cfg!(debug_assertions)
            || cfg!(feature = "insecure-tls")
            || std::env::var("GLASSCHAIN_INSECURE_TLS").is_ok_and(|v| v == "1")
    }

    /// Attach a WASM execution provider to the contract engine.
    ///
    /// After this call, contracts that carry a `wasm_code_b64` payload will be
    /// evaluated through the provider before the standard Rust condition matching.
    pub async fn set_execution_provider(&self, executor: Arc<dyn ExecutionProvider>) {
        self.state.lock().await.engine.set_executor(executor);
    }

    /// Start the node: spawn a TCP listener task and connect to seed peers.
    ///
    /// Returns immediately; all network activity runs in background tasks.
    pub async fn start(&self, seed_peers: Vec<String>) -> Result<(), NetworkError> {
        let listener = TcpListener::bind(&self.listen_addr).await?;
        log::info!("Node {} listening on {}", self.node_id, self.listen_addr);

        let (dial_tx, dial_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        Self::spawn_dial_queue(
            dial_rx,
            dial_tx.clone(),
            Arc::clone(&self.ledger),
            Arc::clone(&self.state),
            self.node_id.clone(),
            self.listen_addr.clone(),
            self.event_tx.clone(),
            Arc::clone(&self.indexer),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.storage),
            Arc::clone(&self.tls),
        );

        let ledger = Arc::clone(&self.ledger);
        let state = Arc::clone(&self.state);
        let node_id = self.node_id.clone();
        let listen_addr = self.listen_addr.clone();
        let event_tx = self.event_tx.clone();
        let indexer = Arc::clone(&self.indexer);
        let event_bus = Arc::clone(&self.event_bus);

        let dtx = dial_tx.clone();
        let tls_l = Arc::clone(&self.tls);
        let storage_l = Arc::clone(&self.storage);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        let addr = peer_addr.to_string();
                        log::info!("Inbound TLS connection from {addr}");
                        let l2 = Arc::clone(&ledger);
                        let s2 = Arc::clone(&state);
                        let ni = node_id.clone();
                        let la = listen_addr.clone();
                        let et = event_tx.clone();
                        let ix = Arc::clone(&indexer);
                        let eb = Arc::clone(&event_bus);
                        let dx = dtx.clone();
                        let tls2 = Arc::clone(&tls_l);
                        let st2 = Arc::clone(&storage_l);
                        tokio::spawn(async move {
                            let tls_stream = match tls2.acceptor.accept(stream).await {
                                Ok(s) => s,
                                Err(e) => {
                                    log::warn!("TLS accept error from {addr}: {e}");
                                    return;
                                }
                            };
                            let (r, w) = tokio::io::split(tls_stream);
                            let reader = PeerReader::new(r, addr.clone());
                            let writer = PeerWriter::new(w, addr.clone());
                            handle_peer(reader, writer, addr, la, l2, s2, ni, et, ix, eb, dx, st2)
                                .await;
                        });
                    }
                    Err(e) => log::error!("Accept error: {e}"),
                }
            }
        });

        for peer_addr in seed_peers {
            let _ = dial_tx.send(peer_addr);
        }

        Ok(())
    }

    /// Spawn the dial-queue consumer task.
    fn spawn_dial_queue(
        mut dial_rx: UnboundedReceiver<String>,
        dial_tx: UnboundedSender<String>,
        ledger: Arc<Mutex<Ledger>>,
        state: Arc<Mutex<NodeState>>,
        node_id: String,
        listen_addr: String,
        event_tx: broadcast::Sender<NodeEvent>,
        indexer: Arc<InMemoryIndexer>,
        event_bus: Arc<InMemoryEventBus>,
        storage: Arc<dyn StorageProvider>,
        tls: Arc<NodeTls>,
    ) {
        tokio::spawn(async move {
            while let Some(addr) = dial_rx.recv().await {
                let l2 = Arc::clone(&ledger);
                let s2 = Arc::clone(&state);
                let ni = node_id.clone();
                let la = listen_addr.clone();
                let et = event_tx.clone();
                let ix = Arc::clone(&indexer);
                let eb = Arc::clone(&event_bus);
                let dx = dial_tx.clone();
                let st = Arc::clone(&storage);
                let tls_c = Arc::clone(&tls);
                tokio::spawn(async move {
                    connect_to_peer(addr, la, l2, s2, ni, et, ix, eb, dx, st, tls_c).await;
                });
            }
        });
    }

    /// Submit a transaction to the local pending pool and broadcast it to peers.
    pub async fn submit_transaction(&self, tx: Transaction) -> Result<(), NetworkError> {
        {
            let mut ledger = self.ledger.lock().await;
            ledger.add_transaction(tx.clone())?;
        }

        let generated = {
            let mut s = self.state.lock().await;
            let mut gen = Vec::new();
            if let TransactionKind::SupplyOffer(ref offer) = tx.kind {
                gen = s.engine.evaluate_supply_offer(offer, &tx.id);
            }
            if let TransactionKind::ContractCreation(ref def) = tx.kind {
                let _ = s.engine.register_contract(def.clone());
            }
            gen
        };

        {
            let mut ledger = self.ledger.lock().await;
            for gen_tx in &generated {
                let _ = ledger.add_transaction(gen_tx.clone());
            }
        }

        let _ = self
            .event_tx
            .send(NodeEvent::TransactionAccepted(tx.clone()));
        for gen_tx in &generated {
            if let TransactionKind::ContractExecution(ref exec) = gen_tx.kind {
                let _ = self.event_tx.send(NodeEvent::ContractExecuted {
                    contract_id: exec.contract_id.clone(),
                    quantity: exec.quantity,
                });
            }
        }

        self.broadcast(Message::Transaction(tx)).await;
        for gen_tx in generated {
            self.broadcast(Message::Transaction(gen_tx)).await;
        }

        Ok(())
    }

    /// Mine a new block containing all pending transactions and broadcast it.
    pub async fn mine(&self) -> Result<(), NetworkError> {
        let (index, prev_hash, transactions, difficulty) = {
            let mut ledger = self.ledger.lock().await;
            ledger.prepare_mining()?
        };

        let mut block = Block::new(index, transactions, prev_hash.clone());
        block.mine(difficulty);

        let appended = {
            let mut ledger = self.ledger.lock().await;
            ledger.commit_mined_block(block.clone(), &prev_hash)?
        };

        if appended {
            let generated = Self::after_block_commit(
                &self.ledger,
                &self.state,
                &self.event_tx,
                &self.indexer,
                &self.event_bus,
                &block,
                &self.storage,
            )
            .await;

            log::info!("Mined block {} ({}...)", block.index, &block.hash[..8]);
            let _ = self.event_tx.send(NodeEvent::BlockMined {
                index: block.index,
                hash: block.hash.clone(),
            });
            self.broadcast(Message::Block(block)).await;
            for tx in generated {
                self.broadcast(Message::Transaction(tx)).await;
            }
        }

        Ok(())
    }

    /// Return a snapshot of the current ledger state.
    pub async fn ledger_snapshot(&self) -> Ledger {
        self.ledger.lock().await.clone()
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

    /// Broadcast a message to all connected peers via their persistent write channels.
    async fn broadcast(&self, message: Message) {
        let senders: Vec<Sender<Message>> = {
            self.state
                .lock()
                .await
                .peer_senders
                .values()
                .cloned()
                .collect()
        };
        for sender in senders {
            match sender.try_send(message.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    log::warn!("Dropping outbound message: peer channel full");
                }
                Err(TrySendError::Closed(_)) => {}
            }
        }
    }

    /// Post-commit hook: persist the block, index it, fire the event bus, run
    /// watcher triggers, and add any autonomous transactions to the ledger.
    async fn after_block_commit(
        ledger: &Arc<Mutex<Ledger>>,
        state: &Arc<Mutex<NodeState>>,
        event_tx: &broadcast::Sender<NodeEvent>,
        indexer: &Arc<InMemoryIndexer>,
        event_bus: &Arc<InMemoryEventBus>,
        block: &Block,
        storage: &Arc<dyn StorageProvider>,
    ) -> Vec<Transaction> {
        // Persist this block so it survives a restart.
        if let Err(e) = storage.put_block(block) {
            log::warn!("Storage: failed to persist block {}: {e}", block.index);
        }

        if let Err(e) = indexer.index_block(block) {
            log::warn!("Indexer error: {e}");
        }
        if let Err(e) = event_bus.publish_block(block) {
            log::warn!("EventBus error: {e}");
        }

        let watcher_orders: Vec<Transaction> = {
            let mut s = state.lock().await;
            let mut orders = Vec::new();
            for tx in &block.transactions {
                if let TransactionKind::InventoryUpdate(ref update) = tx.kind {
                    orders.extend(s.watcher.on_inventory_update(update));
                }
            }
            orders
        };

        let mut generated = Vec::new();
        {
            let mut ledger = ledger.lock().await;
            for order in watcher_orders {
                let trigger_id = match &order.kind {
                    TransactionKind::PurchaseOrder(po) => po
                        .contract_id
                        .clone()
                        .unwrap_or_else(|| "watcher".to_owned()),
                    _ => "watcher".to_owned(),
                };
                if ledger.add_transaction(order.clone()).is_ok() {
                    let _ = event_tx.send(NodeEvent::AutonomousTransactionGenerated {
                        trigger_id,
                        transaction_id: order.id.clone(),
                    });
                    generated.push(order);
                }
            }
        }

        generated
    }
}

// ── Free functions ────────────────────────────────────────────────────────────

/// Handle a single peer connection (inbound or outbound).
///
/// Reader and writer halves are passed in directly (already split from a TLS
/// stream).  A dedicated writer task drains an mpsc channel so broadcast never
/// blocks on I/O.  The read loop runs in the calling task.
async fn handle_peer(
    mut reader: PeerReader,
    writer: PeerWriter,
    addr: String,
    listen_addr: String,
    ledger: Arc<Mutex<Ledger>>,
    state: Arc<Mutex<NodeState>>,
    node_id: String,
    event_tx: broadcast::Sender<NodeEvent>,
    indexer: Arc<InMemoryIndexer>,
    event_bus: Arc<InMemoryEventBus>,
    dial_tx: UnboundedSender<String>,
    storage: Arc<dyn StorageProvider>,
) {
    let (write_tx, mut write_rx): (Sender<Message>, Receiver<Message>) = tokio::sync::mpsc::channel(256);

    {
        let waddr = addr.clone();
        tokio::spawn(async move {
            let mut writer = writer;
            while let Some(msg) = write_rx.recv().await {
                if let Err(e) = writer.send(&msg).await {
                    log::warn!("Write error to {waddr}: {e}");
                    break;
                }
            }
        });
    }

    let chain_length = ledger.lock().await.chain.len() as u64;
    let hello = Message::Hello {
        node_id: node_id.clone(),
        chain_length,
        version: PROTOCOL_VERSION.to_owned(),
        listen_addr: listen_addr.clone(),
    };
    if write_tx.try_send(hello).is_err() {
        log::warn!("Failed to queue Hello for {addr}");
        return;
    }

    let mut peer_stable_addr: Option<String> = None;

    loop {
        match reader.receive().await {
            Ok(msg) => {
                let effect = process_message(
                    msg,
                    &addr,
                    &ledger,
                    &state,
                    &node_id,
                    &write_tx,
                    &event_tx,
                    peer_stable_addr.as_deref(),
                    &listen_addr,
                    &indexer,
                    &event_bus,
                    &dial_tx,
                    &storage,
                )
                .await;

                if let Some(stable) = effect.stable_addr {
                    peer_stable_addr = Some(stable);
                }
            }
            Err(crate::error::NetworkError::PeerDisconnected(_)) => {
                log::info!("Peer {addr} disconnected");
                break;
            }
            Err(e) => {
                log::warn!("Error reading from {addr}: {e}");
                break;
            }
        }
    }

    let registered = peer_stable_addr.clone();
    let display_addr = registered.clone().unwrap_or_else(|| addr.clone());
    if let Some(ref stable) = registered {
        let mut s = state.lock().await;
        s.known_peers.remove(stable);
        s.peer_senders.remove(stable);
    }
    let _ = event_tx.send(NodeEvent::PeerDisconnected(display_addr));

    if let Some(stable) = registered {
        let dtx = dial_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            log::info!("Attempting to reconnect to {stable}");
            let _ = dtx.send(stable);
        });
    }
}

/// Connect outbound to `peer_addr`, wrap in TLS, and run the peer handle loop.
async fn connect_to_peer(
    peer_addr: String,
    listen_addr: String,
    ledger: Arc<Mutex<Ledger>>,
    state: Arc<Mutex<NodeState>>,
    node_id: String,
    event_tx: broadcast::Sender<NodeEvent>,
    indexer: Arc<InMemoryIndexer>,
    event_bus: Arc<InMemoryEventBus>,
    dial_tx: UnboundedSender<String>,
    storage: Arc<dyn StorageProvider>,
    tls: Arc<NodeTls>,
) {
    match TcpStream::connect(&peer_addr).await {
        Ok(stream) => {
            log::info!("Connected to peer {peer_addr}");
            let server_name = ServerName::try_from("glasschain-node")
                .expect("valid server name")
                .to_owned();
            let tls_stream = match tls.connector.connect(server_name, stream).await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("TLS connect error to {peer_addr}: {e}");
                    return;
                }
            };
            let (r, w) = tokio::io::split(tls_stream);
            let reader = PeerReader::new(r, peer_addr.clone());
            let writer = PeerWriter::new(w, peer_addr.clone());
            handle_peer(
                reader,
                writer,
                peer_addr,
                listen_addr,
                ledger,
                state,
                node_id,
                event_tx,
                indexer,
                event_bus,
                dial_tx,
                storage,
            )
            .await;
        }
        Err(e) => {
            log::warn!("Could not connect to {peer_addr}: {e}");
        }
    }
}

#[derive(Default)]
struct MessageEffect {
    stable_addr: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn process_message(
    msg: Message,
    addr: &str,
    ledger: &Arc<Mutex<Ledger>>,
    state: &Arc<Mutex<NodeState>>,
    _node_id: &str,
    write_tx: &Sender<Message>,
    event_tx: &broadcast::Sender<NodeEvent>,
    current_stable_addr: Option<&str>,
    listen_addr: &str,
    indexer: &Arc<InMemoryIndexer>,
    event_bus: &Arc<InMemoryEventBus>,
    dial_tx: &UnboundedSender<String>,
    storage: &Arc<dyn StorageProvider>,
) -> MessageEffect {
    match msg {
        Message::Hello {
            node_id: peer_id,
            chain_length,
            listen_addr: peer_listen_addr,
            ..
        } => {
            log::info!(
                "Hello from {addr} (id={peer_id}, chain_len={chain_length}, listen={peer_listen_addr})"
            );

            // Don't register our own listen address as a peer.
            if peer_listen_addr == listen_addr {
                log::debug!(
                    "Ignoring Hello from own listen address {peer_listen_addr}"
                );
                return MessageEffect::default();
            }

            {
                let mut s = state.lock().await;
                s.known_peers.insert(peer_listen_addr.clone());
                s.peer_senders
                    .insert(peer_listen_addr.clone(), write_tx.clone());
            }
            let _ = event_tx.send(NodeEvent::PeerConnected(peer_listen_addr.clone()));

            let local_len = ledger.lock().await.chain.len() as u64;
            if chain_length > local_len {
                let _ = write_tx.try_send(Message::RequestChain);
            }

            MessageEffect {
                stable_addr: Some(peer_listen_addr),
            }
        }

        Message::Transaction(tx) => {
            let generated = {
                let mut s = state.lock().await;
                let mut gen = Vec::new();
                if let TransactionKind::SupplyOffer(ref offer) = tx.kind {
                    gen = s.engine.evaluate_supply_offer(offer, &tx.id);
                }
                if let TransactionKind::ContractCreation(ref def) = tx.kind {
                    s.engine.load_from_ledger(def.clone());
                }
                gen
            };

            for gen_tx in &generated {
                if let TransactionKind::ContractExecution(ref exec) = gen_tx.kind {
                    let _ = event_tx.send(NodeEvent::ContractExecuted {
                        contract_id: exec.contract_id.clone(),
                        quantity: exec.quantity,
                    });
                }
            }

            {
                let mut l = ledger.lock().await;
                if let Err(e) = l.add_transaction(tx.clone()) {
                    log::warn!("Could not add tx from {addr}: {e}");
                } else {
                    let _ = event_tx.send(NodeEvent::TransactionAccepted(tx));
                }
                for gen_tx in &generated {
                    let _ = l.add_transaction(gen_tx.clone());
                }
            }

            let senders: Vec<Sender<Message>> =
                state.lock().await.peer_senders.values().cloned().collect();
            for gen_tx in generated {
                for s in &senders {
                    let _ = s.try_send(Message::Transaction(gen_tx.clone()));
                }
            }
            MessageEffect::default()
        }

        Message::Block(block) => {
            // Reject blocks with implausible timestamps (> 2 hours in the future).
            // Block 0 (genesis) uses timestamp 0 by design and is exempt.
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if block.index > 0 && block.timestamp > now_secs + 7_200 {
                log::warn!(
                    "Rejected block {} from {addr}: timestamp {} is {} seconds in the future",
                    block.index,
                    block.timestamp,
                    block.timestamp.saturating_sub(now_secs)
                );
                return MessageEffect::default();
            }

            let (should_append, too_far_ahead) = {
                let l = ledger.lock().await;
                let expected = l.chain.len() as u64;
                if block.index == expected {
                    if let Some(prev) = l.chain.last() {
                        let diff = l.difficulty;
                        let valid = block.chains_to(prev).is_ok() && block.has_valid_pow(diff);
                        (valid, false)
                    } else {
                        (false, false)
                    }
                } else {
                    (false, block.index > expected)
                }
            };

            if too_far_ahead {
                let _ = write_tx.send(Message::RequestChain);
                return MessageEffect::default();
            }

            if should_append {
                {
                    let mut l = ledger.lock().await;
                    let committed: std::collections::HashSet<&str> =
                        block.transactions.iter().map(|t| t.id.as_str()).collect();
                    l.pending_transactions
                        .retain(|t| !committed.contains(t.id.as_str()));
                    l.chain.push(block.clone());
                }

                let generated = Node::after_block_commit(
                    ledger, state, event_tx, indexer, event_bus, &block, storage,
                )
                .await;

                let _ = event_tx.send(NodeEvent::BlockReceived {
                    index: block.index,
                    hash: block.hash.clone(),
                });

                let senders: Vec<Sender<Message>> =
                    state.lock().await.peer_senders.values().cloned().collect();
                for tx in generated {
                    for s in &senders {
                        let _ = s.try_send(Message::Transaction(tx.clone()));
                    }
                }
            } else {
                log::warn!("Received invalid or stale block from {addr}");
            }
            MessageEffect::default()
        }

        Message::RequestChain => {
            let chain = ledger.lock().await.chain.clone();
            let _ = write_tx.try_send(Message::Chain(chain));
            MessageEffect::default()
        }

        Message::Chain(candidate) => {
            let replaced = {
                let mut l = ledger.lock().await;
                l.try_replace_chain(candidate)
            };

            if replaced {
                // Persist the new chain.
                let new_chain = { ledger.lock().await.chain.clone() };
                for block in &new_chain {
                    if let Err(e) = storage.put_block(block) {
                        log::warn!("Storage: failed to persist block {}: {e}", block.index);
                    }
                }

                // Rebuild the contract engine from the newly synced chain.
                {
                    let mut s = state.lock().await;
                    s.engine = ContractEngine::rebuild_from_chain(&new_chain);
                }
                log::info!(
                    "Contract engine rebuilt from synced chain ({} blocks)",
                    new_chain.len()
                );
            }
            MessageEffect::default()
        }

        Message::RequestPeers => {
            let exclude = current_stable_addr.unwrap_or(addr).to_owned();
            let peers: Vec<String> = state
                .lock()
                .await
                .known_peers
                .iter()
                .filter(|p| p.as_str() != exclude.as_str())
                .cloned()
                .collect();
            let _ = write_tx.try_send(Message::Peers(peers));
            MessageEffect::default()
        }

        Message::Peers(addrs) => {
            let new_peers: Vec<String> = {
                let mut s = state.lock().await;
                addrs
                    .into_iter()
                    .filter(|a| s.known_peers.insert(a.clone()))
                    .collect()
            };
            for peer in new_peers {
                let _ = dial_tx.send(peer);
            }
            MessageEffect::default()
        }

        Message::Goodbye { reason } => {
            log::info!("Peer {addr} says goodbye: {reason}");
            MessageEffect::default()
        }
    }
}
