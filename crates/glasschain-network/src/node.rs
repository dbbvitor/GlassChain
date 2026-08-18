use crate::error::NetworkError;
use crate::peer::{PeerReader, PeerWriter};
use crate::protocol::{Message, PROTOCOL_VERSION};
use glasschain_contracts::{ContractEngine, InventoryTrigger, WatcherService};
use glasschain_core::crypto::sha256;
use glasschain_core::providers::in_memory::InMemoryStorageProvider;
use glasschain_core::{
    Block, ExecutionProvider, Ledger, StorageProvider, Transaction, TransactionKind,
};
use glasschain_identity::CertChainVerifier;
use glasschain_identity::Identity;
use glasschain_indexer::{EventBusProvider, InMemoryEventBus, InMemoryIndexer, IndexerProvider};
use rcgen;
use rustls;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{
    error::TrySendError, Receiver, Sender, UnboundedReceiver, UnboundedSender,
};
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
    acceptor: Arc<TlsAcceptor>,
    /// Pre-built connector for use only when insecure TLS is enabled.
    /// `None` in normal (verified) mode — outbound connections use
    /// [`Node::connector_for_peer_cert`] to obtain a per-peer connector.
    connector: Option<Arc<TlsConnector>>,
    cert_der: CertificateDer<'static>,
    cert_fingerprint: String,
    /// Result of [`Node::insecure_tls_allowed`] cached once at build time so
    /// that outbound-connection code does not re-read the environment variable
    /// on every attempt.
    insecure: bool,
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
    /// TOFU peer registry: verified peer identities keyed by stable listen address.
    peer_registry: PeerRegistry,
    /// Optional CA certificate verifier; when set, peer certs must be org-issued.
    cert_verifier: Option<Arc<CertChainVerifier>>,
    /// Optional node identity for signing autonomous watcher transactions.
    identity: Option<Arc<Identity>>,
}

// ── TOFU Peer Registry ───────────────────────────────────────────────────────

/// A verified peer identity recorded on first contact (Trust On First Use).
#[derive(Debug, Clone)]
struct VerifiedPeer {
    node_id: String,
    cert_fingerprint: String,
}

/// Stable peer identity store using a TOFU (Trust On First Use) model.
///
/// The first time a peer connects from a given listen address, its identity
/// (node ID + TLS certificate fingerprint) is recorded.  On subsequent
/// connections the identity is verified against the stored record — any change
/// is treated as a potential impersonation and rejected.
///
/// Records are **not** removed on disconnect so that reconnecting peers are
/// still verified against their original identity.
struct PeerRegistry {
    peers: HashMap<String, VerifiedPeer>,
}

impl PeerRegistry {
    fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Verify and optionally register a peer.
    ///
    /// * First contact → identity is recorded, returns `Ok(true)`.
    /// * Known peer, identity matches → returns `Ok(false)`.
    /// * Known peer, identity **changed** → returns `Err(reason)`.
    fn verify_or_register(
        &mut self,
        listen_addr: &str,
        node_id: &str,
        cert_fingerprint: &str,
    ) -> Result<bool, String> {
        if let Some(existing) = self.peers.get(listen_addr) {
            if existing.node_id != node_id {
                return Err(format!(
                    "node_id changed: expected '{}', got '{node_id}'",
                    existing.node_id,
                ));
            }
            if existing.cert_fingerprint != cert_fingerprint {
                return Err(format!(
                    "TLS certificate fingerprint changed for node '{node_id}'"
                ));
            }
            Ok(false)
        } else {
            self.peers.insert(
                listen_addr.to_owned(),
                VerifiedPeer {
                    node_id: node_id.to_owned(),
                    cert_fingerprint: cert_fingerprint.to_owned(),
                },
            );
            Ok(true)
        }
    }
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
    fn with_components(
        node_id: String,
        listen_addr: String,
        ledger: Arc<Mutex<Ledger>>,
        storage: Arc<dyn StorageProvider>,
        identity: Option<Arc<Identity>>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            node_id,
            listen_addr,
            ledger,
            state: Arc::new(Mutex::new(NodeState {
                engine: ContractEngine::new(),
                watcher: WatcherService::new(),
                known_peers: HashSet::new(),
                peer_senders: HashMap::new(),
                peer_registry: PeerRegistry::new(),
                cert_verifier: None,
                identity: identity.clone(),
            })),
            event_tx,
            indexer: Arc::new(InMemoryIndexer::new()),
            event_bus: Arc::new(InMemoryEventBus::new(4096)),
            storage,
            tls: Arc::new(Self::build_tls(identity)),
        }
    }

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
        Self::with_components(
            node_id.into(),
            listen_addr.into(),
            Arc::new(Mutex::new(Ledger::new(difficulty))),
            Arc::new(InMemoryStorageProvider::new()),
            None,
        )
    }

    /// Restore a [`Ledger`] from `storage`, validating the full chain on the
    /// way back.  Falls back to a fresh empty ledger if storage is empty or
    /// the stored chain fails validation.
    fn restore_ledger(storage: &Arc<dyn StorageProvider>, difficulty: usize) -> Arc<Mutex<Ledger>> {
        let mut l = Ledger::new(difficulty);
        if let Ok(Some(latest_idx)) = storage.latest_block_index() {
            let mut chain = Vec::with_capacity((latest_idx + 1) as usize);
            let mut valid = true;
            for i in 0..=latest_idx {
                if let Ok(Some(block)) = storage.get_block(i) {
                    chain.push(block);
                } else {
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
        let ledger = Self::restore_ledger(&storage, difficulty);
        Self::with_components(node_id.into(), listen_addr.into(), ledger, storage, None)
    }

    /// Create a new node with an identity-backed TLS certificate.
    pub fn new_with_identity(
        node_id: impl Into<String>,
        listen_addr: impl Into<String>,
        difficulty: usize,
        identity: Arc<Identity>,
    ) -> Self {
        Self::with_components(
            node_id.into(),
            listen_addr.into(),
            Arc::new(Mutex::new(Ledger::new(difficulty))),
            Arc::new(InMemoryStorageProvider::new()),
            Some(identity),
        )
    }

    /// Create a persistent node with an identity-backed TLS certificate.
    pub fn new_with_storage_and_identity(
        node_id: impl Into<String>,
        listen_addr: impl Into<String>,
        difficulty: usize,
        storage: Arc<dyn StorageProvider>,
        identity: Arc<Identity>,
    ) -> Self {
        let ledger = Self::restore_ledger(&storage, difficulty);
        Self::with_components(
            node_id.into(),
            listen_addr.into(),
            ledger,
            storage,
            Some(identity),
        )
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

    /// Generate a TLS certificate and build the node's TLS context.
    fn build_tls(identity: Option<Arc<Identity>>) -> NodeTls {
        // Ensure the ring crypto provider is installed (required by rustls 0.23).
        let _ = rustls::crypto::ring::default_provider().install_default();

        let (cert_der, key_der) = if let Some(identity) = identity {
            let key_pair = identity
                .rcgen_key_pair()
                .expect("TLS key generation from identity");
            let mut params = rcgen::CertificateParams::new(vec!["glasschain-node".to_string()])
                .expect("TLS cert params");
            let mut dn = rcgen::DistinguishedName::new();
            dn.push(rcgen::DnType::CommonName, identity.node_id.clone());
            params.distinguished_name = dn;
            let cert = params
                .self_signed(&key_pair)
                .expect("TLS self-signed cert from identity");
            (
                cert.der().clone(),
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der())),
            )
        } else {
            let key_pair = rcgen::KeyPair::generate().expect("TLS key generation");
            let params = rcgen::CertificateParams::new(vec!["glasschain-node".to_string()])
                .expect("TLS cert params");
            let cert = params.self_signed(&key_pair).expect("TLS self-signed cert");
            (
                cert.der().clone(),
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der())),
            )
        };

        let cert_fingerprint = sha256(cert_der.as_ref());

        let server_cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .expect("server TLS config");

        let insecure = Self::insecure_tls_allowed();
        let connector = if insecure {
            log::warn!(
                "Network TLS certificate verification is disabled (dev mode). \
                 Use trusted roots in production."
            );
            let cfg = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
                .with_no_client_auth();
            Some(Arc::new(TlsConnector::from(Arc::new(cfg))))
        } else {
            // In normal mode `tls.connector` is never used: each outbound
            // connection builds a fresh per-peer connector via
            // `connector_for_peer_cert`.
            None
        };

        NodeTls {
            acceptor: Arc::new(TlsAcceptor::from(Arc::new(server_cfg))),
            connector,
            cert_der,
            cert_fingerprint,
            insecure,
        }
    }

    /// Insecure verifier is allowed only when explicitly requested.
    fn insecure_tls_allowed() -> bool {
        cfg!(feature = "insecure-tls")
            || std::env::var("GLASSCHAIN_INSECURE_TLS").is_ok_and(|v| v == "1")
    }

    /// Build a client TLS connector that trusts the supplied peer certificate.
    fn connector_for_peer_cert(peer_cert: CertificateDer<'static>) -> Arc<TlsConnector> {
        let mut roots = RootCertStore::empty();
        roots
            .add(peer_cert)
            .expect("add peer certificate to root store");
        let client_cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(TlsConnector::from(Arc::new(client_cfg)))
    }

    /// Rebuild watcher runtime state by replaying committed inventory updates,
    /// or restoring from a persisted snapshot if one is available.
    async fn rebuild_runtime_state_from_chain(
        ledger: &Arc<Mutex<Ledger>>,
        state: &Arc<Mutex<NodeState>>,
        storage: &Arc<dyn StorageProvider>,
    ) {
        let chain = { ledger.lock().await.chain.clone() };
        let mut s = state.lock().await;
        // Always replay contracts from chain (authoritative source).
        s.engine = ContractEngine::rebuild_from_chain(&chain);

        // For watcher state (inventory levels + fire counts), try the persisted
        // snapshot first — it is more up-to-date than chain replay because it
        // captures post-commit updates that haven't been mined into a block yet.
        let restored_from_storage = if let Ok(Some(bytes)) = storage.get_state("watcher:state") {
            match s.watcher.restore_from_bytes(&bytes) {
                Ok(()) => {
                    log::info!("Restored watcher inventory state from storage snapshot");
                    true
                }
                Err(e) => {
                    log::warn!(
                        "Failed to restore watcher state from storage: {e}; using chain replay"
                    );
                    false
                }
            }
        } else {
            false
        };

        if !restored_from_storage {
            // Fall back: replay InventoryUpdate txs from committed blocks.
            let mut watcher = WatcherService::new();
            for tx in chain.iter().flat_map(|block| block.transactions.iter()) {
                if let TransactionKind::InventoryUpdate(ref update) = tx.kind {
                    let _ = watcher.on_inventory_update(update);
                }
            }
            s.watcher = watcher;
        }
    }

    /// Attach a WASM execution provider to the contract engine.
    ///
    /// After this call, contracts that carry a `wasm_code_b64` payload will be
    /// evaluated through the provider before the standard Rust condition matching.
    pub async fn set_execution_provider(&self, executor: Arc<dyn ExecutionProvider>) {
        let mut s = self.state.lock().await;
        s.engine.set_executor(Arc::clone(&executor));
        s.watcher.set_executor(executor);
    }

    /// Enable CA-backed certificate verification for peer authentication.
    ///
    /// When set, the Hello handshake rejects any peer whose TLS certificate
    /// was not issued by this organization's Root CA.
    pub async fn set_cert_verifier(&self, verifier: CertChainVerifier) {
        self.state.lock().await.cert_verifier = Some(Arc::new(verifier));
    }

    /// Set the node identity used to sign autonomous watcher transactions.
    ///
    /// After this call, every `PurchaseOrder` generated by the
    /// [`WatcherService`] is signed and stored in the state backend.
    pub async fn set_signing_identity(&self, identity: Arc<Identity>) {
        self.state.lock().await.identity = Some(identity);
    }

    /// Start the node: rebuild runtime state, spawn a TCP listener task, and connect to seed peers.
    ///
    /// Returns immediately; all network activity runs in background tasks.
    pub async fn start(&self, seed_peers: Vec<String>) -> Result<(), NetworkError> {
        Self::rebuild_runtime_state_from_chain(&self.ledger, &self.state, &self.storage).await;

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
                        let local_tls_cert_fingerprint = tls2.cert_fingerprint.clone();
                        tokio::spawn(async move {
                            let mut stream = stream;

                            let mut peer_len_buf = [0u8; 4];
                            if let Err(e) =
                                tokio::io::AsyncReadExt::read_exact(&mut stream, &mut peer_len_buf)
                                    .await
                            {
                                log::warn!(
                                    "Failed to read peer TLS certificate length from {addr}: {e}"
                                );
                                return;
                            }
                            let peer_len = u32::from_be_bytes(peer_len_buf) as usize;
                            if peer_len == 0 || peer_len > 64 * 1024 {
                                log::warn!("Peer {addr} advertised invalid TLS certificate length {peer_len}");
                                return;
                            }
                            let mut peer_cert_buf = vec![0u8; peer_len];
                            if let Err(e) =
                                tokio::io::AsyncReadExt::read_exact(&mut stream, &mut peer_cert_buf)
                                    .await
                            {
                                log::warn!("Failed to read peer TLS certificate from {addr}: {e}");
                                return;
                            }
                            let local_cert = tls2.cert_der.as_ref();
                            let cert_len = match u32::try_from(local_cert.len()) {
                                Ok(v) => v,
                                Err(_) => {
                                    log::warn!(
                                        "Local TLS certificate too large to advertise to {addr}"
                                    );
                                    return;
                                }
                            };
                            if let Err(e) = tokio::io::AsyncWriteExt::write_all(
                                &mut stream,
                                &cert_len.to_be_bytes(),
                            )
                            .await
                            {
                                log::warn!("Failed to send TLS certificate length to {addr}: {e}");
                                return;
                            }
                            if let Err(e) =
                                tokio::io::AsyncWriteExt::write_all(&mut stream, local_cert).await
                            {
                                log::warn!("Failed to send TLS certificate to {addr}: {e}");
                                return;
                            }

                            let acceptor = Arc::clone(&tls2.acceptor);

                            let tls_stream = match acceptor.accept(stream).await {
                                Ok(s) => s,
                                Err(e) => {
                                    log::warn!("TLS accept error from {addr}: {e}");
                                    return;
                                }
                            };
                            let observed_cert_fingerprint = sha256(&peer_cert_buf);
                            let (r, w) = tokio::io::split(tls_stream);
                            let reader = PeerReader::new(r, addr.clone());
                            let writer = PeerWriter::new(w, addr.clone());
                            handle_peer(
                                reader,
                                writer,
                                addr,
                                PeerContext {
                                    ledger: l2,
                                    state: s2,
                                    node_id: ni,
                                    listen_addr: la,
                                    local_tls_cert_fingerprint,
                                    event_tx: et,
                                    indexer: ix,
                                    event_bus: eb,
                                    dial_tx: dx,
                                    storage: st2,
                                },
                                observed_cert_fingerprint,
                                peer_cert_buf,
                            )
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
    pub async fn mine_async(&self) -> Result<(), NetworkError> {
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

    /// Mine a new block and wait for completion.
    ///
    /// This is the synchronous convenience wrapper for callers that want the
    /// original blocking semantics while still reusing the async mining path.
    pub async fn mine(&self) -> Result<(), NetworkError> {
        self.mine_async().await
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

        // ── Sign autonomous transactions with the node's organisational key ──────
        // If this node has an identity, every autonomously generated PurchaseOrder
        // is signed and stored in the state backend so external verifiers can
        // confirm it originated from this node.
        {
            let identity = state.lock().await.identity.clone();
            if let Some(ref id) = identity {
                for order in &watcher_orders {
                    match id.sign_transaction(order.clone()) {
                        Ok(signed) => {
                            let key = format!("signed_tx:{}", order.id);
                            if let Ok(json) = serde_json::to_vec(&signed) {
                                if let Err(e) = storage.put_state(&key, &json) {
                                    log::warn!(
                                        "Failed to persist signed autonomous tx {}: {e}",
                                        order.id
                                    );
                                } else {
                                    log::debug!(
                                        "Signed autonomous tx {} with node '{}'",
                                        order.id,
                                        id.node_id
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to sign autonomous tx {}: {e}", order.id);
                        }
                    }
                }
            }
        }

        // ── Persist watcher state for crash recovery ─────────────────────────────
        {
            let s = state.lock().await;
            match s.watcher.serialize_state() {
                Ok(bytes) => {
                    if let Err(e) = storage.put_state("watcher:state", &bytes) {
                        log::warn!("Failed to persist watcher state: {e}");
                    }
                }
                Err(e) => {
                    log::warn!("Failed to serialize watcher state: {e}");
                }
            }
        }

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

/// Stable per-connection context bundling shared state that is invariant
/// across all messages within a single peer session.
///
/// Grouping these into one struct reduces the parameter count of
/// [`handle_peer`] and [`process_message`] from ~15 arguments each down to
/// 4–5 without changing any semantics.
struct PeerContext {
    ledger: Arc<Mutex<Ledger>>,
    state: Arc<Mutex<NodeState>>,
    node_id: String,
    listen_addr: String,
    /// TLS fingerprint of *this* node's own certificate.
    /// Used by [`process_message`] to detect self-connections regardless of
    /// how the listen address was formatted (handles wildcard bind addresses).
    local_tls_cert_fingerprint: String,
    event_tx: broadcast::Sender<NodeEvent>,
    indexer: Arc<InMemoryIndexer>,
    event_bus: Arc<InMemoryEventBus>,
    dial_tx: UnboundedSender<String>,
    storage: Arc<dyn StorageProvider>,
}

/// Handle a single peer connection (inbound or outbound).
///
/// Reader and writer halves are passed in directly (already split from a TLS
/// stream).  A dedicated writer task drains an mpsc channel so broadcast never
/// blocks on I/O.  The read loop runs in the calling task.
async fn handle_peer(
    mut reader: PeerReader,
    writer: PeerWriter,
    addr: String,
    ctx: PeerContext,
    observed_peer_cert_fingerprint: String,
    peer_cert_der: Vec<u8>,
) {
    let (write_tx, mut write_rx): (Sender<Message>, Receiver<Message>) =
        tokio::sync::mpsc::channel(256);

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

    // Connection-scoped: the observed cert fingerprint is passed directly
    // through the call chain — no shared mutable state needed.

    let chain_length = ctx.ledger.lock().await.chain.len() as u64;
    let hello = Message::Hello {
        node_id: ctx.node_id.clone(),
        tls_cert_fingerprint: ctx.local_tls_cert_fingerprint.clone(),
        chain_length,
        version: PROTOCOL_VERSION.to_owned(),
        listen_addr: ctx.listen_addr.clone(),
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
                    &ctx,
                    &write_tx,
                    peer_stable_addr.as_deref(),
                    &observed_peer_cert_fingerprint,
                    &peer_cert_der,
                )
                .await;

                if let Some(stable) = effect.stable_addr {
                    peer_stable_addr = Some(stable);
                }
                if effect.disconnect {
                    log::info!("Terminating unauthenticated connection to {addr}");
                    break;
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
        let mut s = ctx.state.lock().await;
        s.known_peers.remove(stable);
        s.peer_senders.remove(stable);
        // NOTE: we deliberately do NOT remove the peer from peer_registry
        // here.  TOFU records persist across reconnects so that a returning
        // peer is still verified against its original identity.
    }
    let _ = ctx.event_tx.send(NodeEvent::PeerDisconnected(display_addr));

    if let Some(stable) = registered {
        let dtx = ctx.dial_tx.clone();
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
        Ok(mut stream) => {
            log::info!("Connected to peer {peer_addr}");

            let local_cert = tls.cert_der.as_ref();
            let cert_len = match u32::try_from(local_cert.len()) {
                Ok(v) => v,
                Err(_) => {
                    log::warn!("Local TLS certificate too large to advertise to {peer_addr}");
                    return;
                }
            };
            if let Err(e) =
                tokio::io::AsyncWriteExt::write_all(&mut stream, &cert_len.to_be_bytes()).await
            {
                log::warn!("Failed to send TLS certificate length to {peer_addr}: {e}");
                return;
            }
            if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut stream, local_cert).await {
                log::warn!("Failed to send TLS certificate to {peer_addr}: {e}");
                return;
            }

            let mut peer_len_buf = [0u8; 4];
            if let Err(e) =
                tokio::io::AsyncReadExt::read_exact(&mut stream, &mut peer_len_buf).await
            {
                log::warn!("Failed to read peer TLS certificate length from {peer_addr}: {e}");
                return;
            }
            let peer_len = u32::from_be_bytes(peer_len_buf) as usize;
            if peer_len == 0 || peer_len > 64 * 1024 {
                log::warn!("Peer {peer_addr} advertised invalid TLS certificate length {peer_len}");
                return;
            }
            let mut peer_cert_buf = vec![0u8; peer_len];
            if let Err(e) =
                tokio::io::AsyncReadExt::read_exact(&mut stream, &mut peer_cert_buf).await
            {
                log::warn!("Failed to read peer TLS certificate from {peer_addr}: {e}");
                return;
            }

            let observed_cert_fingerprint = sha256(&peer_cert_buf);
            let peer_cert_der_for_hello = peer_cert_buf.clone();
            let connector = if tls.insecure {
                Arc::clone(
                    tls.connector
                        .as_ref()
                        .expect("insecure connector is present when tls.insecure is true"),
                )
            } else {
                Node::connector_for_peer_cert(CertificateDer::from(peer_cert_buf))
            };

            let server_name = ServerName::try_from("glasschain-node")
                .expect("valid server name")
                .to_owned();
            let tls_stream = match connector.connect(server_name, stream).await {
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
                PeerContext {
                    ledger,
                    state,
                    node_id,
                    listen_addr,
                    local_tls_cert_fingerprint: tls.cert_fingerprint.clone(),
                    event_tx,
                    indexer,
                    event_bus,
                    dial_tx,
                    storage,
                },
                observed_cert_fingerprint,
                peer_cert_der_for_hello,
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
    /// When `true`, the `handle_peer` read loop must break immediately after
    /// returning this effect.  Used to terminate connections that fail Hello
    /// authentication (fingerprint mismatch, TOFU rejection, self-connection).
    disconnect: bool,
}

async fn process_message(
    msg: Message,
    addr: &str,
    ctx: &PeerContext,
    write_tx: &Sender<Message>,
    current_stable_addr: Option<&str>,
    observed_cert_fingerprint: &str,
    peer_cert_der: &[u8],
) -> MessageEffect {
    match msg {
        Message::Hello {
            node_id: peer_id,
            tls_cert_fingerprint,
            chain_length,
            listen_addr: peer_listen_addr,
            ..
        } => {
            log::info!(
                "Hello from {addr} (id={peer_id}, chain_len={chain_length}, listen={peer_listen_addr})"
            );

            // Detect self-connections by comparing the peer's advertised TLS
            // fingerprint against our own local certificate fingerprint.
            // A simple address comparison would fail when the node is bound to
            // a wildcard address (0.0.0.0) because the peer's Hello reports a
            // concrete IP instead.
            if tls_cert_fingerprint == ctx.local_tls_cert_fingerprint {
                log::debug!("Ignoring Hello from own TLS certificate (self-connection)");
                return MessageEffect {
                    disconnect: true,
                    ..Default::default()
                };
            }

            // ── Step 1: session-level fingerprint verification ────────
            // The Hello message carries the peer's self-reported TLS cert
            // fingerprint.  Compare it against the fingerprint we observed
            // during the TLS handshake (connection-scoped, passed as a
            // parameter — never stored in shared mutable state).
            if tls_cert_fingerprint != observed_cert_fingerprint {
                log::warn!(
                    "Rejecting peer {peer_id} at {addr}: advertised TLS fingerprint \
                     does not match observed session certificate \
                     (advertised={}, observed={})",
                    &tls_cert_fingerprint[..16.min(tls_cert_fingerprint.len())],
                    &observed_cert_fingerprint[..16.min(observed_cert_fingerprint.len())],
                );
                return MessageEffect {
                    disconnect: true,
                    ..Default::default()
                };
            }

            // ── Step 2: TOFU peer registry ────────────────────────────
            // First contact  → record identity (node_id + cert fingerprint).
            // Reconnection   → verify identity has not changed.
            // Identity drift → reject the peer.
            {
                let mut s = ctx.state.lock().await;
                match s.peer_registry.verify_or_register(
                    &peer_listen_addr,
                    &peer_id,
                    observed_cert_fingerprint,
                ) {
                    Ok(is_new) => {
                        if is_new {
                            log::info!(
                                "TOFU: recorded new peer identity for {peer_listen_addr} \
                                 (node_id={peer_id})"
                            );
                        } else {
                            log::debug!(
                                "TOFU: verified returning peer {peer_listen_addr} \
                                 (node_id={peer_id})"
                            );
                        }
                    }
                    Err(reason) => {
                        log::warn!("Rejecting peer {peer_id} at {peer_listen_addr}: {reason}");
                        return MessageEffect {
                            disconnect: true,
                            ..Default::default()
                        };
                    }
                }

                // ── Step 3: CA certificate verification (org mode) ───────────────
                // If this node is configured with an Organization Root CA verifier,
                // require that the peer's TLS certificate was issued by that CA.
                // Peers with self-signed certs are still accepted in dev mode
                // (when no cert_verifier is configured).
                if let Some(ref verifier) = s.cert_verifier {
                    if let Err(e) = verifier.verify_cert_der(peer_cert_der) {
                        log::warn!(
                            "Rejecting peer {peer_id} at {peer_listen_addr}: CA cert \
                             verification failed: {e}"
                        );
                        return MessageEffect {
                            disconnect: true,
                            ..Default::default()
                        };
                    }
                    log::info!(
                        "CA cert verified for peer {peer_id} (org={})",
                        verifier.org_name()
                    );
                }

                // ── Step 4: register live connection state ────────────
                s.known_peers.insert(peer_listen_addr.clone());
                s.peer_senders
                    .insert(peer_listen_addr.clone(), write_tx.clone());
            }
            let _ = ctx
                .event_tx
                .send(NodeEvent::PeerConnected(peer_listen_addr.clone()));

            let local_len = ctx.ledger.lock().await.chain.len() as u64;
            if chain_length > local_len {
                let _ = write_tx.try_send(Message::RequestChain);
            }

            MessageEffect {
                stable_addr: Some(peer_listen_addr),
                disconnect: false,
            }
        }

        Message::Transaction(tx) => {
            // Reject messages from peers that have not completed a successful Hello.
            if current_stable_addr.is_none() {
                log::warn!("Ignoring transaction from unauthenticated peer {addr}");
                return MessageEffect::default();
            }
            let generated = {
                let mut s = ctx.state.lock().await;
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
                    let _ = ctx.event_tx.send(NodeEvent::ContractExecuted {
                        contract_id: exec.contract_id.clone(),
                        quantity: exec.quantity,
                    });
                }
            }

            {
                let mut l = ctx.ledger.lock().await;
                if let Err(e) = l.add_transaction(tx.clone()) {
                    log::warn!("Could not add tx from {addr}: {e}");
                } else {
                    let _ = ctx.event_tx.send(NodeEvent::TransactionAccepted(tx));
                }
                for gen_tx in &generated {
                    let _ = l.add_transaction(gen_tx.clone());
                }
            }

            let senders: Vec<Sender<Message>> = ctx
                .state
                .lock()
                .await
                .peer_senders
                .values()
                .cloned()
                .collect();
            for gen_tx in generated {
                for s in &senders {
                    let _ = s.try_send(Message::Transaction(gen_tx.clone()));
                }
            }
            MessageEffect::default()
        }

        Message::Block(block) => {
            // Reject messages from peers that have not completed a successful Hello.
            if current_stable_addr.is_none() {
                log::warn!("Ignoring block from unauthenticated peer {addr}");
                return MessageEffect::default();
            }
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
                let l = ctx.ledger.lock().await;
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
                let _ = write_tx.try_send(Message::RequestChain);
                return MessageEffect::default();
            }

            if should_append {
                {
                    let mut l = ctx.ledger.lock().await;
                    let committed: std::collections::HashSet<&str> =
                        block.transactions.iter().map(|t| t.id.as_str()).collect();
                    l.pending_transactions
                        .retain(|t| !committed.contains(t.id.as_str()));
                    l.chain.push(block.clone());
                }

                let generated = Node::after_block_commit(
                    &ctx.ledger,
                    &ctx.state,
                    &ctx.event_tx,
                    &ctx.indexer,
                    &ctx.event_bus,
                    &block,
                    &ctx.storage,
                )
                .await;

                let _ = ctx.event_tx.send(NodeEvent::BlockReceived {
                    index: block.index,
                    hash: block.hash.clone(),
                });

                let senders: Vec<Sender<Message>> = ctx
                    .state
                    .lock()
                    .await
                    .peer_senders
                    .values()
                    .cloned()
                    .collect();
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
            let chain = ctx.ledger.lock().await.chain.clone();
            let _ = write_tx.try_send(Message::Chain(chain));
            MessageEffect::default()
        }

        Message::Chain(candidate) => {
            let replaced = {
                let mut l = ctx.ledger.lock().await;
                l.try_replace_chain(candidate)
            };
            if replaced {
                // Persist the new chain.
                let new_chain = { ctx.ledger.lock().await.chain.clone() };
                for block in &new_chain {
                    if let Err(e) = ctx.storage.put_block(block) {
                        log::warn!("Storage: failed to persist block {}: {e}", block.index);
                    }
                }

                Node::rebuild_runtime_state_from_chain(&ctx.ledger, &ctx.state, &ctx.storage).await;
                log::info!(
                    "Contract engine and watcher state rebuilt from synced chain ({} blocks)",
                    new_chain.len()
                );
            }
            MessageEffect::default()
        }

        Message::RequestPeers => {
            let exclude = current_stable_addr.unwrap_or(addr).to_owned();
            let peers: Vec<String> = ctx
                .state
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
                let mut s = ctx.state.lock().await;
                addrs
                    .into_iter()
                    .filter(|a| s.known_peers.insert(a.clone()))
                    .collect()
            };
            for peer in new_peers {
                let _ = ctx.dial_tx.send(peer);
            }
            MessageEffect::default()
        }

        Message::Goodbye { reason } => {
            log::info!("Peer {addr} says goodbye: {reason}");
            MessageEffect::default()
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tofu_first_contact_records_identity() {
        let mut reg = PeerRegistry::new();
        let result = reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123");
        assert_eq!(result, Ok(true), "first contact should return Ok(true)");
        assert_eq!(reg.peers.len(), 1);
        let peer = &reg.peers["127.0.0.1:8000"];
        assert_eq!(peer.node_id, "node-a");
        assert_eq!(peer.cert_fingerprint, "abc123");
    }

    #[test]
    fn tofu_returning_peer_with_same_identity_passes() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123")
            .unwrap();
        let result = reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123");
        assert_eq!(
            result,
            Ok(false),
            "returning peer with same identity should return Ok(false)"
        );
    }

    #[test]
    fn tofu_rejects_node_id_change() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123")
            .unwrap();
        let result = reg.verify_or_register("127.0.0.1:8000", "node-IMPOSTER", "abc123");
        assert!(result.is_err(), "changed node_id should be rejected");
    }

    #[test]
    fn tofu_rejects_cert_fingerprint_change() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123")
            .unwrap();
        let result = reg.verify_or_register("127.0.0.1:8000", "node-a", "TAMPERED");
        assert!(
            result.is_err(),
            "changed cert fingerprint should be rejected"
        );
    }

    #[test]
    fn tofu_independent_addresses_are_independent() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "aaa")
            .unwrap();
        reg.verify_or_register("127.0.0.1:9000", "node-b", "bbb")
            .unwrap();
        assert_eq!(reg.peers.len(), 2);
        // Each address keeps its own identity.
        assert!(reg
            .verify_or_register("127.0.0.1:8000", "node-a", "aaa")
            .is_ok());
        assert!(reg
            .verify_or_register("127.0.0.1:9000", "node-b", "bbb")
            .is_ok());
        // Cross-contamination is rejected.
        assert!(reg
            .verify_or_register("127.0.0.1:8000", "node-b", "aaa")
            .is_err());
    }
}
