use crate::error::NetworkError;
use crate::peer::{PeerReader, PeerWriter};
use crate::protocol::{Message, PROTOCOL_VERSION};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use glasschain_contracts::ContractEngine;
use glasschain_core::crypto::sha256;
use glasschain_core::providers::in_memory::InMemoryStorageProvider;
#[cfg(feature = "bft")]
use glasschain_core::BFT_CONSENSUS_CAPABILITY_ID;
use glasschain_core::PDC_CAPABILITY_ID;
use glasschain_core::{
    evaluate_transaction_endorsements, Block, CapabilityAdvertisement, CapabilityHistory,
    CommitNotification, CoreError, EndorsementEvaluation, EndorsementProvider, EndorsementRequest,
    ExecutionLimits, ExecutionProvider, ExecutionResult, Ledger, PersistentWrite, PolicyHistory,
    QuorumCertificate, StorageProvider, Transaction, TransactionKind, WriteOp, WriteVisibility,
    CAPABILITY_V1, ENDORSEMENT_CAPABILITY_ID,
};
use glasschain_identity::CertChainVerifier;
use glasschain_identity::{Channel, Identity};
use glasschain_indexer::{
    indexed_transactions_of, AnalyticalFlattener, EventBusProvider, InMemoryEventBus,
    InMemoryIndexer, IndexedBlock, IndexerProvider, ProvenanceIndex,
};
use glasschain_storage::TransientStore;
use glasschain_workflows::{InventoryTrigger, WatcherService};
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
// Keep transaction payloads inline to preserve the public event API.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// A new transaction was accepted into the pending pool.
    TransactionAccepted(Transaction),
    /// A block was successfully mined **by this node** and appended to the
    /// chain. `certificate` is the quorum certificate attesting the block
    /// (degenerate for the retained `PoW` dev/test consensus).
    BlockMined {
        index: u64,
        hash: String,
        certificate: QuorumCertificate,
    },
    /// A block received from a remote peer was validated and appended to the
    /// chain, with its quorum certificate.
    BlockReceived {
        index: u64,
        hash: String,
        certificate: QuorumCertificate,
    },
    /// A private data collection payload was received and stored in the
    /// transient store (ADR-003, ticket #46). The event carries the collection
    /// and the commitment — never the payload bytes.
    PrivatePayloadReceived {
        /// The collection the payload belongs to.
        collection: String,
        /// SHA-256 of the payload (the chain's commitment).
        commitment: String,
    },
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
    /// Cumulative outbound messages dropped because a peer's 256-slot write
    /// channel was full — the tail-at-scale straggler counter (#62 §5.1/§5.6).
    /// A chronically rising count is a peer that cannot keep up and is
    /// silently missing blocks until it resyncs.
    dropped_outbound: HashMap<String, u64>,
    /// TOFU peer registry: verified peer identities keyed by stable listen address.
    peer_registry: PeerRegistry,
    /// Optional CA certificate verifier; when set, peer certs must be org-issued.
    cert_verifier: Option<Arc<CertChainVerifier>>,
    /// Optional node identity for signing autonomous watcher transactions.
    identity: Option<Arc<Identity>>,
    /// Derived world-state cache: the materialized committed write sets,
    /// keyed by `ws:<channel>:<contract>:<key>`.  Rebuilt from committed
    /// blocks on restart — never re-executed (ADR-007 decision 2).  PDC-scoped
    /// keys hold their commitment until the private payload arrives through
    /// ADR-003 dissemination (#46/#47).
    world_state: HashMap<String, Vec<u8>>,
    /// Optional VM execution provider, used to produce a candidate block's
    /// write set at mining time.
    executor: Option<Arc<dyn ExecutionProvider>>,
    /// Optional endorsement provider, invoked at the commit path when the
    /// `endorsement` capability is active (ADR-008 handoff 4).
    endorsement: Option<Arc<dyn EndorsementProvider>>,
    /// The private data collections this node is configured with (ADR-003,
    /// ticket #46). Membership gates every private-payload path; membership is
    /// never an endorsement (ADR-008).
    collections: Vec<Channel>,
    /// Transient pre-commit store for received private payloads, over the
    /// node's `StorageProvider` (ADR-003; retention/purge is #47).
    transient: TransientStore,
    /// Optional Tendermint-class BFT provider (ticket #42, default-off). When
    /// set **and** the `bft_consensus` capability is active at the candidate
    /// height, the node attests blocks with a real quorum certificate instead
    /// of dev/test `PoW`; the commit consumer is unchanged either way.
    #[cfg(feature = "bft")]
    consensus: Option<Arc<glasschain_core::BftConsensusProvider>>,
    /// Endorsement-policy history replayed from committed blocks (ADR-008
    /// decision 4): the pre-block policy set used for evaluation.
    policies: PolicyHistory,
}

impl NodeState {
    /// This node's organization: the identity's node identifier when
    /// identity-backed, the plain node identifier otherwise. The
    /// collection-membership principal (ADR-003).
    fn local_org(&self, node_id: &str) -> String {
        self.identity
            .clone()
            .map_or_else(|| node_id.to_owned(), |identity| identity.node_id.clone())
    }

    /// The configured collection with `name`, if any.
    fn collection(&self, name: &str) -> Option<&Channel> {
        self.collections.iter().find(|c| c.config.name == name)
    }

    /// `true` when `org` is a member of the collection `name` (ADR-003).
    /// Membership is a read/write/receipt control and is never an endorsement
    /// (ADR-008).
    fn is_collection_member(&self, name: &str, org: &str) -> bool {
        self.collection(name)
            .is_some_and(|collection| collection.is_member(org))
    }

    /// The org a connected peer advertised in its `Hello`, if known.
    fn peer_org(&self, addr: &str) -> Option<String> {
        self.peer_registry
            .peers
            .get(addr)
            .map(|peer| peer.org.clone())
    }

    /// Write channels of peers whose org is a member of `collection` — the
    /// point-to-point private-payload targets (ADR-003). Nodes without the
    /// collection or org never appear here.
    fn payload_targets(&self, collection: &Channel) -> Vec<Sender<Message>> {
        self.peer_senders
            .iter()
            .filter(|(addr, _)| {
                self.peer_org(addr)
                    .is_some_and(|org| collection.is_member(&org))
            })
            .map(|(_, sender)| sender.clone())
            .collect()
    }

    /// Write channels of peers that support the `active` capability set;
    /// read-only observers are excluded from active-write relay (ADR-010
    /// decision 6).
    fn relay_targets(&self, active: &glasschain_core::CapabilitySet) -> Vec<Sender<Message>> {
        self.peer_senders
            .iter()
            .filter(|(addr, _)| !self.peer_registry.is_read_only(addr, active))
            .map(|(_, sender)| sender.clone())
            .collect()
    }
}

/// The capability set effective at the local chain tip (ADR-010 decision 5).
async fn active_set_at_tip(ledger: &Arc<Mutex<Ledger>>) -> glasschain_core::CapabilitySet {
    let ledger_guard = ledger.lock().await;
    match CapabilityHistory::build_from_blocks(&ledger_guard.chain) {
        Ok(history) => history.effective_set(ledger_guard.chain.len().saturating_sub(1) as u64),
        Err(e) => {
            log::warn!("Capability history invalid; using genesis set for admission: {e}");
            glasschain_core::CapabilitySet::genesis()
        }
    }
}

// ── TOFU Peer Registry ───────────────────────────────────────────────────────

/// A verified peer identity recorded on first contact (Trust On First Use).
#[derive(Debug, Clone)]
struct VerifiedPeer {
    node_id: String,
    cert_fingerprint: String,
    /// The peer's organization (the collection-membership principal, ADR-003).
    org: String,
    /// `true` when the org was verified against the peer's TLS certificate
    /// subject CN under a configured organization Root CA (ticket #47). Bare
    /// TOFU leaves this `false`: the org is self-asserted.
    org_verified: bool,
    /// Capabilities the peer advertised in its most recent `Hello`.
    advertised: Vec<CapabilityAdvertisement>,
}

impl VerifiedPeer {
    /// `true` when the peer supports every capability in `set`, matching
    /// `(id, version)`. A peer that cannot support an active capability may
    /// parse and validate history but not propose, vote, or relay active
    /// writes (ADR-010 decision 6).
    fn supports(&self, set: &glasschain_core::CapabilitySet) -> bool {
        CAPABILITY_V1.iter().all(|c| {
            let Some((active_version, _)) = set.active_version(c.id) else {
                return true;
            };
            self.advertised
                .iter()
                .any(|advert| advert.id == c.id && advert.version == active_version)
        })
    }
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
    /// Whether the peer at `addr` claimed `org` with a certificate-verified
    /// identity (ticket #47).
    fn org_verified(&self, addr: &str, org: &str) -> Option<bool> {
        self.peers
            .get(addr)
            .filter(|peer| peer.org == org)
            .map(|peer| peer.org_verified)
    }

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
        org: &str,
        org_verified: bool,
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
            // Org drift: a returning peer claiming a different organization is
            // either a re-keyed node or an impersonation; both are rejections
            // (ticket #47 — the org gates private-payload delivery).
            if existing.org != org {
                return Err(format!(
                    "org changed for node '{node_id}': expected '{}', got '{org}'",
                    existing.org
                ));
            }
            Ok(false)
        } else {
            self.peers.insert(
                listen_addr.to_owned(),
                VerifiedPeer {
                    node_id: node_id.to_owned(),
                    cert_fingerprint: cert_fingerprint.to_owned(),
                    org: org.to_owned(),
                    org_verified,
                    advertised: Vec::new(),
                },
            );
            Ok(true)
        }
    }

    /// Record the capabilities a peer advertised in its latest `Hello`.
    fn set_advertised(&mut self, listen_addr: &str, advertised: Vec<CapabilityAdvertisement>) {
        if let Some(peer) = self.peers.get_mut(listen_addr) {
            peer.advertised = advertised;
        }
    }

    /// `true` when the peer at `listen_addr` cannot support `set` and is
    /// therefore a read-only observer.
    fn is_read_only(&self, listen_addr: &str, set: &glasschain_core::CapabilitySet) -> bool {
        self.peers
            .get(listen_addr)
            .is_none_or(|peer| !peer.supports(set))
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
#[allow(clippy::struct_field_names)]
pub struct Node {
    pub node_id: String,
    listen_addr: String,
    /// The address advertised to peers in `Hello` — the address they dial and
    /// key TOFU/reconnect state by. Defaults to `listen_addr`; override it
    /// when the node sits behind a NAT, port-forward, or test-harness proxy
    /// that is the only route to it.
    advertise_addr: Option<String>,
    /// The distributed ledger — accessible directly for sharing with the RPC layer.
    ledger: Arc<Mutex<Ledger>>,
    state: Arc<Mutex<NodeState>>,
    event_tx: broadcast::Sender<NodeEvent>,
    indexer: Arc<InMemoryIndexer>,
    event_bus: Arc<InMemoryEventBus>,
    /// Provenance index: custody chains by canonical asset id, ingested on
    /// every commit and rebuilt from the chain on start/sync.
    provenance: Arc<Mutex<ProvenanceIndex>>,
    /// Analytical flattener: flat asset records for lineage queries.
    flattener: Arc<Mutex<AnalyticalFlattener>>,
    /// Block storage backend — persists committed blocks across restarts.
    storage: Arc<dyn StorageProvider>,
    /// TLS context used to encrypt all peer connections.
    tls: Arc<NodeTls>,
    /// The dial queue spawned by [`start`](Self::start); explicit reconnects
    /// route through it.
    dial_tx: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<String>>,
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
            advertise_addr: None,
            dial_tx: std::sync::OnceLock::new(),
            ledger,
            state: Arc::new(Mutex::new(NodeState {
                engine: ContractEngine::new(),
                watcher: WatcherService::new(),
                known_peers: HashSet::new(),
                peer_senders: HashMap::new(),
                dropped_outbound: HashMap::new(),
                peer_registry: PeerRegistry::new(),
                cert_verifier: None,
                identity: identity.clone(),
                world_state: HashMap::new(),
                executor: None,
                endorsement: None,
                policies: PolicyHistory::default(),
                collections: Vec::new(),
                transient: TransientStore::new(Arc::clone(&storage)),
                #[cfg(feature = "bft")]
                consensus: None,
            })),
            event_tx,
            indexer: Arc::new(InMemoryIndexer::new()),
            event_bus: Arc::new(InMemoryEventBus::new(4096)),
            provenance: Arc::new(Mutex::new(ProvenanceIndex::new())),
            flattener: Arc::new(Mutex::new(AnalyticalFlattener::new())),
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
            let mut chain = Vec::new();
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

    /// Return a clone of the provenance index handle (custody chains keyed by
    /// canonical asset id).
    #[must_use]
    pub fn provenance_index(&self) -> Arc<Mutex<ProvenanceIndex>> {
        Arc::clone(&self.provenance)
    }

    /// Return a clone of the analytical flattener handle (flat asset records
    /// for lineage queries).
    #[must_use]
    pub fn analytical_flattener(&self) -> Arc<Mutex<AnalyticalFlattener>> {
        Arc::clone(&self.flattener)
    }

    /// Snapshot of the derived world state (committed write sets, keyed by
    /// `ws:<channel>:<contract>:<key>`).
    pub async fn world_state(&self) -> HashMap<String, Vec<u8>> {
        self.state.lock().await.world_state.clone()
    }

    /// Return the TCP address this node listens on.
    #[must_use]
    pub fn listen_addr(&self) -> &str {
        &self.listen_addr
    }

    /// Override the address advertised to peers in `Hello` (the address they
    /// dial and key TOFU/reconnect state by). Must be called before
    /// [`start`](Self::start). Use when the node is reachable only through a
    /// NAT, port-forward, or test-harness proxy rather than its bind address.
    pub fn set_advertise_addr(&mut self, addr: impl Into<String>) {
        self.advertise_addr = Some(addr.into());
    }

    /// The address this node advertises to peers.
    fn advertised_addr(&self) -> String {
        self.advertise_addr
            .clone()
            .unwrap_or_else(|| self.listen_addr.clone())
    }

    /// Dial `addr` and run the peer lifecycle. Seeds are dialed by `start`;
    /// this exposes the same path for explicit reconnection after a network
    /// partition is repaired (the built-in reconnect is a single 5-second
    /// retry that gives up if the peer is unreachable).
    pub fn connect_peer(&self, addr: &str) {
        // Reaches the dial queue spawned by `start`; a no-op before start.
        if let Some(dial_tx) = self.dial_tx.get() {
            let _ = dial_tx.send(addr.to_owned());
        }
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

        let (cert_der, key_der) = identity.map_or_else(
            || {
                let key_pair = rcgen::KeyPair::generate().expect("TLS key generation");
                let params = rcgen::CertificateParams::new(vec!["glasschain-node".to_string()])
                    .expect("TLS cert params");
                let cert = params.self_signed(&key_pair).expect("TLS self-signed cert");
                (
                    cert.der().clone(),
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der())),
                )
            },
            |identity| {
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
            },
        );

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
    /// or restoring from a persisted snapshot if one is available, and rebuild
    /// the derived world state and the analytics projections from committed
    /// blocks.
    async fn rebuild_runtime_state_from_chain(
        ledger: &Arc<Mutex<Ledger>>,
        state: &Arc<Mutex<NodeState>>,
        storage: &Arc<dyn StorageProvider>,
        provenance: &Arc<Mutex<ProvenanceIndex>>,
        flattener: &Arc<Mutex<AnalyticalFlattener>>,
    ) {
        let chain = { ledger.lock().await.chain.clone() };

        // World state is a materialized projection of the chain: rebuild it
        // from committed write sets in block order (never re-executing guest
        // code), healing any partial apply and refreshing the storage state.
        match Self::rebuild_world_state(storage) {
            Ok(world_state) => {
                state.lock().await.world_state = world_state;
            }
            Err(e) => {
                log::warn!("Failed to rebuild world state from chain: {e}");
            }
        }

        // Provenance and flattener are materialized projections of the chain:
        // rebuild them from committed blocks (the chain is the authority).
        {
            let mut provenance_guard = provenance.lock().await;
            *provenance_guard = ProvenanceIndex::new();
            for block in &chain {
                provenance_guard.ingest_block(block);
            }
        }
        {
            let mut flattener_guard = flattener.lock().await;
            *flattener_guard = AnalyticalFlattener::new();
            for block in &chain {
                let txs = match indexed_transactions_of(block) {
                    Ok(txs) => txs,
                    Err(e) => {
                        log::warn!(
                            "flattener: failed to index block {} during rebuild: {e}",
                            block.index
                        );
                        continue;
                    }
                };
                flattener_guard.ingest_indexed_block(&IndexedBlock::from(block), &txs);
            }
        }

        let mut s = state.lock().await;
        // Always replay contracts from chain (authoritative source).
        s.engine = ContractEngine::rebuild_from_chain(&chain);

        // Endorsement-policy metadata is replayed from committed blocks the
        // same way (ADR-008 decision 4): versioned, append-only, deterministic.
        match PolicyHistory::build_from_blocks(&chain) {
            Ok(policies) => s.policies = policies,
            Err(e) => log::warn!("Failed to rebuild policy history from chain: {e}"),
        }

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

    /// Rebuild the derived world state from committed blocks in block order.
    ///
    /// Reads the persisted chain and applies each block's committed write set
    /// to a fresh cache, re-applying it to the storage state (healing a
    /// backend failure that persisted the block but not the state).  No guest
    /// code is executed anywhere on this path (ADR-007 decision 2).
    fn rebuild_world_state(
        storage: &Arc<dyn StorageProvider>,
    ) -> Result<HashMap<String, Vec<u8>>, NetworkError> {
        let mut world_state = HashMap::new();
        let Some(latest) = storage.latest_block_index()? else {
            return Ok(world_state);
        };
        for index in 0..=latest {
            let Some(block) = storage.get_block(index)? else {
                // A gap in the persisted chain is storage corruption; surface
                // it loudly and continue so the remaining blocks still heal.
                log::warn!("world-state rebuild: block {index} missing from storage");
                continue;
            };
            for write in &block.write_set {
                match &write.op {
                    glasschain_core::WriteOp::Set(value) => {
                        world_state.insert(write.state_key(), value.clone());
                        storage.put_state(&write.state_key(), value)?;
                    }
                    glasschain_core::WriteOp::Delete => {
                        world_state.remove(&write.state_key());
                        storage.delete_state(&write.state_key())?;
                    }
                }
            }
        }
        Ok(world_state)
    }

    /// Attach a WASM execution provider to the contract engine.
    ///
    /// After this call, contracts that carry a `wasm_code_b64` payload will be
    /// evaluated through the provider before the standard Rust condition matching.
    pub async fn set_execution_provider(&self, executor: Arc<dyn ExecutionProvider>) {
        let mut s = self.state.lock().await;
        s.engine.set_executor(Arc::clone(&executor));
        s.watcher.set_executor(Arc::clone(&executor));
        s.executor = Some(executor);
    }

    /// Attach an endorsement provider (ADR-008 handoff 4).
    ///
    /// After this call, transaction and block admission evaluate the committed
    /// endorsement policies through the provider — but only once the
    /// `endorsement` capability is active at the candidate height (ADR-010).
    pub async fn set_endorsement_provider(&self, provider: Arc<dyn EndorsementProvider>) {
        self.state.lock().await.endorsement = Some(provider);
    }

    /// Configure this node's private data collections (ADR-003, ticket #46).
    ///
    /// Membership gates every private-payload path: `submit_private_payload`
    /// requires the local org to be a member, payloads are sent only to member
    /// peers, and received payloads are stored only when this node is a
    /// member. Membership is never an endorsement (ADR-008).
    pub async fn set_collections(&self, collections: Vec<Channel>) {
        self.state.lock().await.collections = collections;
    }

    /// Submit a private payload to the collection `collection` (ADR-003,
    /// ticket #46).
    ///
    /// The payload is held in this member's transient store and sent
    /// **point-to-point** to member peers only; the globally replicated chain
    /// receives only `sha256(payload)` via the redacted write set. Rejected
    /// when the `pdc` capability is inactive or the local org is not a member
    /// of the collection.
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] when the capability gate, membership gate, or
    /// transient-store write fails.
    pub async fn submit_private_payload(
        &self,
        collection: &str,
        payload: Vec<u8>,
    ) -> Result<(), NetworkError> {
        // Membership gate (the collection-scoped check first, so a non-member
        // gets the accurate rejection regardless of chain state).
        {
            let s = self.state.lock().await;
            let org = s.local_org(&self.node_id);
            if !s.is_collection_member(collection, &org) {
                return Err(CoreError::InvalidTransaction(format!(
                    "org '{org}' is not a member of collection '{collection}'"
                ))
                .into());
            }
        }
        // Capability gate: the write carrying this payload commits at the
        // NEXT height, so the gate is the capability set effective there
        // (ADR-010 §4: a block is validated under the set active at its own
        // height).
        let pdc_active = {
            let ledger = self.ledger.lock().await;
            let next_height = ledger.chain.len() as u64;
            CapabilityHistory::build_from_blocks(&ledger.chain).is_ok_and(|history| {
                history
                    .effective_set(next_height)
                    .is_active(PDC_CAPABILITY_ID)
            })
        };
        if !pdc_active {
            return Err(CoreError::InvalidTransaction(
                "private payloads require the 'pdc' capability to be active".into(),
            )
            .into());
        }
        let (commitment, targets, transient, retention_secs) = {
            let s = self.state.lock().await;
            let Some(configured) = s.collection(collection) else {
                return Err(CoreError::InvalidTransaction(format!(
                    "collection '{collection}' is not configured"
                ))
                .into());
            };
            let targets = s.payload_targets(configured);
            let transient = s.transient.clone();
            let retention_secs = configured.config.retention_secs;
            let commitment = glasschain_core::crypto::sha256(&payload);
            drop(s);
            (commitment, targets, transient, retention_secs)
        };
        // Store outside the state lock (the store is a cheap Arc handle).
        transient.put(collection, &commitment, &payload, retention_secs)?;
        for target in targets {
            if let Err(e) = target.try_send(Message::PrivatePayload {
                collection: collection.to_owned(),
                commitment: commitment.clone(),
                payload: payload.clone(),
            }) {
                log::warn!("Private payload delivery to a member peer failed: {e}");
            }
        }
        log::info!(
            "Private payload stored (collection={collection}, commitment={}...)",
            &commitment[..8]
        );
        Ok(())
    }

    /// Purge expired private payloads from the transient store (ADR-003
    /// decision 3, ticket #47): payloads vanish, the chain's hash commitments
    /// persist forever. Call periodically or after operations; retention is
    /// also enforced on read.
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] when the backend fails.
    pub async fn purge_expired_private_payloads(&self) -> Result<usize, NetworkError> {
        let transient = self.state.lock().await.transient.clone();
        Ok(transient.purge_expired()?)
    }

    /// Pull-reconcile private payloads for `collection` (ADR-003, ticket
    /// #47): a peer that was offline at dissemination time scans the committed
    /// chain for the collection's PDC writes and requests every payload its
    /// transient store is missing from a member peer. The chain's commitments
    /// drive the request, so nothing is invented and nothing leaks — the
    /// request names only a commitment this node can already see.
    ///
    /// Returns the number of payloads requested (0 when this node holds
    /// everything or is not a member).
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] when a transient-store read fails.
    pub async fn reconcile_private_payloads(
        &self,
        collection: &str,
    ) -> Result<usize, NetworkError> {
        // Membership gate + transient handle; the guard is released before the
        // (possibly long) chain scan.
        let transient = {
            let s = self.state.lock().await;
            if !s.is_collection_member(collection, &s.local_org(&self.node_id)) {
                return Ok(0);
            }
            s.transient.clone()
        };
        // Snapshot the chain: the scan reads a consistent view and the guard is
        // released immediately (reconcile is an infrequent operator action).
        let chain = self.ledger.lock().await.chain.clone();
        let mut missing: Vec<String> = Vec::new();
        for block in &chain {
            for write in &block.write_set {
                let (WriteOp::Set(committed), WriteVisibility::Pdc(name)) =
                    (&write.op, &write.visibility)
                else {
                    continue;
                };
                if name != collection {
                    continue;
                }
                let commitment = String::from_utf8(committed.clone()).map_err(|_| {
                    CoreError::InvalidBlock(format!(
                        "PDC commitment for '{collection}' is not valid UTF-8"
                    ))
                })?;
                if transient.get(collection, &commitment)?.is_none() {
                    missing.push(commitment);
                }
            }
        }
        missing.sort_unstable();
        missing.dedup();
        let targets = {
            let s = self.state.lock().await;
            let Some(configured) = s.collection(collection) else {
                return Ok(0);
            };
            let targets = s.payload_targets(configured);
            drop(s);
            targets
        };
        if targets.is_empty() {
            log::warn!("Reconcile: no member peer is connected for collection '{collection}'");
            return Ok(0);
        }
        // Fan every missing-payload request across **all** member peers (#62
        // §5.3): one slow or payload-less member must not starve
        // reconciliation, which is what picking a single arbitrary target
        // did. Responders answer asynchronously via `Message::PrivatePayload`.
        let mut sent = 0usize;
        for target in &targets {
            for commitment in &missing {
                if let Err(e) = target.try_send(Message::RequestPrivatePayload {
                    collection: collection.to_owned(),
                    commitment: commitment.clone(),
                }) {
                    log::warn!("Reconcile request for '{collection}' failed: {e}");
                } else {
                    sent += 1;
                }
            }
        }
        log::info!(
            "Reconcile: requested {} missing payloads for collection '{collection}' \
             across {} member peer(s) ({sent} requests sent)",
            missing.len(),
            targets.len()
        );
        Ok(missing.len())
    }

    /// Read a private payload from the transient store (member read path,
    /// ADR-003): `Some(payload)` only when this node holds it as a member.
    pub async fn transient_payload(&self, collection: &str, commitment: &str) -> Option<Vec<u8>> {
        self.state
            .lock()
            .await
            .transient
            .get(collection, commitment)
            .ok()
            .flatten()
    }

    /// Attach a Tendermint-class BFT provider (ticket #42, default-off).
    ///
    /// After this call the node attests blocks with `provider`'s real quorum
    /// certificate instead of dev/test `PoW` — but only once the `bft_consensus`
    /// capability is active at the candidate height (ADR-010). The commit
    /// consumer is unchanged either way.
    #[cfg(feature = "bft")]
    pub async fn set_bft_consensus(&self, provider: Arc<glasschain_core::BftConsensusProvider>) {
        self.state.lock().await.consensus = Some(provider);
    }

    /// Enable CA-backed certificate verification (ticket #47).
    ///
    /// When set, the Hello handshake rejects any peer whose **Hello-carried
    /// organization certificate** does not verify against this organization's
    /// Root CA with a subject CN equal to the claimed org (the TLS certificate
    /// itself is a transport-only self-signed cert and is not used for
    /// organization trust). The private-payload path additionally requires
    /// senders to be certificate-verified; a reconnecting peer whose claimed
    /// org changed is rejected (org drift).
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
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] if the listening address cannot be bound.
    // Keep listener setup and connection fan-out in one lifecycle method.
    #[allow(clippy::too_many_lines)]
    pub async fn start(&self, seed_peers: Vec<String>) -> Result<(), NetworkError> {
        Self::rebuild_runtime_state_from_chain(
            &self.ledger,
            &self.state,
            &self.storage,
            &self.provenance,
            &self.flattener,
        )
        .await;

        // Ensure the persisted chain matches the in-memory chain: a fresh
        // node's genesis (and any restored-but-unpersisted blocks) must land
        // through the atomic boundary so later blocks can chain to it.
        {
            let chain = self.ledger.lock().await.chain.clone();
            for block in &chain {
                let already_persisted = self.storage.get_block(block.index)?.is_some();
                if already_persisted {
                    continue;
                }
                if let Err(e) = self.storage.apply_block(block) {
                    log::warn!(
                        "Storage: failed to persist block {} on start: {e}",
                        block.index
                    );
                }
            }
        }

        let listener = TcpListener::bind(&self.listen_addr).await?;
        log::info!("Node {} listening on {}", self.node_id, self.listen_addr);

        let (dial_tx, dial_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let _ = self.dial_tx.set(dial_tx.clone());

        Self::spawn_dial_queue(
            dial_rx,
            dial_tx.clone(),
            Arc::clone(&self.ledger),
            Arc::clone(&self.state),
            self.node_id.clone(),
            self.advertised_addr(),
            self.event_tx.clone(),
            Arc::clone(&self.indexer),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.provenance),
            Arc::clone(&self.flattener),
            Arc::clone(&self.storage),
            Arc::clone(&self.tls),
        );

        let ledger = Arc::clone(&self.ledger);
        let state = Arc::clone(&self.state);
        let node_id = self.node_id.clone();
        // The advertised address rides `Hello` and keys remote TOFU/reconnect
        // state, so peers behind NAT/proxies are reachable on reconnect.
        let listen_addr = self.advertised_addr();
        let event_tx = self.event_tx.clone();
        let indexer = Arc::clone(&self.indexer);
        let event_bus = Arc::clone(&self.event_bus);
        let provenance = Arc::clone(&self.provenance);
        let flattener = Arc::clone(&self.flattener);

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
                        let pv = Arc::clone(&provenance);
                        let fl = Arc::clone(&flattener);
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
                            let Ok(cert_len) = u32::try_from(local_cert.len()) else {
                                log::warn!(
                                    "Local TLS certificate too large to advertise to {addr}"
                                );
                                return;
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
                                    provenance: pv,
                                    flattener: fl,
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
    // The task needs each shared node component to service outbound peers.
    #[allow(clippy::too_many_arguments)]
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
        provenance: Arc<Mutex<ProvenanceIndex>>,
        flattener: Arc<Mutex<AnalyticalFlattener>>,
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
                let pv = Arc::clone(&provenance);
                let fl = Arc::clone(&flattener);
                let dx = dial_tx.clone();
                let st = Arc::clone(&storage);
                let tls_c = Arc::clone(&tls);
                tokio::spawn(async move {
                    connect_to_peer(addr, la, l2, s2, ni, et, ix, eb, pv, fl, dx, st, tls_c).await;
                });
            }
        });
    }

    /// Submit a transaction to the local pending pool and broadcast it to peers.
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] if the transaction cannot be added locally.
    pub async fn submit_transaction(&self, tx: Transaction) -> Result<(), NetworkError> {
        // Transaction admission (ADR-008 §4): when enforcement is active, the
        // transaction's declared carriers are evaluated immediately — an
        // unauthorized policy update or record is rejected before it can sit
        // in any pending pool. Write-scope binding happens at block admission.
        {
            let provider = self.state.lock().await.endorsement.clone();
            if let Some(provider) = provider {
                // Sequential lock acquisitions: the ledger guard is dropped
                // before the state guard is taken (state→ledger is the only
                // nested order used elsewhere).
                let active = {
                    let ledger = self.ledger.lock().await;
                    let next_height = ledger.chain.len() as u64;
                    CapabilityHistory::build_from_blocks(&ledger.chain)?
                        .effective_set(next_height)
                        .is_active(ENDORSEMENT_CAPABILITY_ID)
                };
                if active {
                    let policies = self.state.lock().await.policies.clone();
                    evaluate_transaction_endorsements(provider.as_ref(), &policies, &tx, &[])?;
                }
            }
        }
        {
            let mut ledger = self.ledger.lock().await;
            ledger.add_transaction(tx.clone())?;
        }

        let generated = {
            let mut s = self.state.lock().await;
            let gen = if let TransactionKind::SupplyOffer(ref offer) = tx.kind {
                s.engine.evaluate_supply_offer(offer, &tx.id)
            } else {
                Vec::new()
            };
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
    ///
    /// This is the **dev/test consensus driver**: the retained Proof-of-Work
    /// path supplies a degenerate quorum certificate on the commit notification
    /// (ADR-002 keeps `PoW` for testing). When a BFT provider is attached
    /// (ticket #42, default-off) **and** the `bft_consensus` capability is
    /// active at the candidate height, the block is attested with a real
    /// quorum certificate instead; the commit consumer is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] if mining preparation or commit fails.
    pub async fn mine_async(&self) -> Result<(), NetworkError> {
        let (index, prev_hash, transactions, difficulty) = {
            let mut ledger = self.ledger.lock().await;
            ledger.prepare_mining()?
        };

        // The committed block carries the canonical write set of the accepted
        // persistent VM writes (ADR-007 decision 2): execute the candidate's
        // ContractExecution transactions against the committed snapshot,
        // canonicalize, and redact PDC values to commitments before inclusion.
        let (write_set, per_tx_writes) =
            Self::compute_write_set(&self.state, &transactions).await?;
        // Consensus boundary (ADR-010 §1, #46): a PDC-scoped write may only be
        // committed while the `pdc` capability is active at the candidate
        // height — otherwise the candidate is dropped whole.
        if write_set
            .iter()
            .any(|write| matches!(write.visibility, WriteVisibility::Pdc(_)))
        {
            let pdc_active = CapabilityHistory::build_from_blocks(&self.ledger.lock().await.chain)
                .is_ok_and(|history| history.effective_set(index).is_active(PDC_CAPABILITY_ID));
            if !pdc_active {
                Self::restore_pending(&self.ledger, transactions).await;
                return Err(CoreError::InvalidTransaction(
                    "PDC-scoped writes require the 'pdc' capability to be active at the \
                     candidate height"
                        .into(),
                )
                .into());
            }
        }
        let mut block =
            Block::with_write_set(index, transactions.clone(), prev_hash.clone(), write_set);

        // Endorsement enforcement (ADR-008 §4): every declared carrier must
        // satisfy every applicable policy layer, and the committed write set
        // must stay inside the signed scopes — before any materialization.
        if let Err(error) =
            Self::enforce_block_endorsements(&self.state, &self.ledger, &block, &per_tx_writes)
                .await
        {
            // No partial state: the candidate is dropped and its transactions
            // return to the pending pool (the stale-tip path's semantics).
            Self::restore_pending(&self.ledger, transactions).await;
            return Err(error);
        }

        // Attest the candidate: a BFT provider supplies a real quorum
        // certificate when the `bft_consensus` capability is active at this
        // height (ADR-010, ticket #42); otherwise dev/test `PoW` mines a
        // degenerate one. The commit consumer below is identical either way.
        #[cfg(feature = "bft")]
        let notification = {
            let history = CapabilityHistory::build_from_blocks(&self.ledger.lock().await.chain);
            let bft_active = match history {
                Ok(history) => history
                    .effective_set(index)
                    .is_active(BFT_CONSENSUS_CAPABILITY_ID),
                Err(e) => {
                    log::warn!(
                        "Capability history invalid at height {index}; BFT stays dormant: {e}"
                    );
                    false
                }
            };
            let provider = if bft_active {
                self.state.lock().await.consensus.clone()
            } else {
                None
            };
            if let Some(provider) = provider {
                provider.attest(block)
            } else {
                block.mine(difficulty);
                CommitNotification::for_pow_block(block)
            }
        };
        #[cfg(not(feature = "bft"))]
        let notification = {
            block.mine(difficulty);
            CommitNotification::for_pow_block(block)
        };
        let block = notification.block;

        let appended = {
            let mut ledger = self.ledger.lock().await;
            ledger.commit_mined_block(block.clone(), &prev_hash)?
        };

        if appended {
            // Broadcast before persistence (#62 §5.5/§5.6-3): the in-memory
            // ledger is already authoritative and rebuild-from-chain heals any
            // storage divergence (ADR-007 decision 2), so the leader's disk
            // latency must not sit in front of fan-out to every peer.
            self.broadcast(Message::Block(block.clone())).await;
            let generated = Self::after_block_commit(
                &self.ledger,
                &self.state,
                &self.event_tx,
                &self.indexer,
                &self.event_bus,
                &self.provenance,
                &self.flattener,
                &block,
                &self.storage,
            )
            .await;

            log::info!("Committed block {} ({}...)", block.index, &block.hash[..8]);
            let _ = self.event_tx.send(NodeEvent::BlockMined {
                index: block.index,
                hash: block.hash.clone(),
                certificate: notification.certificate,
            });
            // Disseminate this node's raw PDC writes point-to-point to
            // collection members (ADR-003, #46): the block carries only the
            // commitments, so members receive the payload out of band. The
            // writer holds its own payloads in the transient store too.
            self.disseminate_private_writes(&per_tx_writes).await;
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
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] if the mining operation fails.
    pub async fn mine(&self) -> Result<(), NetworkError> {
        self.mine_async().await
    }

    /// Send raw PDC-scoped writes to member peers' transient stores
    /// (ADR-003, ticket #46). `per_tx_writes` carries the pre-redaction
    /// values; the block itself receives only `block_form` commitments.
    async fn disseminate_private_writes(&self, per_tx_writes: &[Vec<PersistentWrite>]) {
        for writes in per_tx_writes {
            for write in writes {
                let (WriteOp::Set(value), WriteVisibility::Pdc(collection)) =
                    (&write.op, &write.visibility)
                else {
                    continue;
                };
                let commitment = glasschain_core::crypto::sha256(value);
                let (transient, targets, retention_secs) = {
                    let s = self.state.lock().await;
                    // A non-member node must never hold private cleartext, even
                    // when it mines a relayed execution (ADR-003 boundary c).
                    if !s.is_collection_member(collection, &s.local_org(&self.node_id)) {
                        log::warn!(
                            "Not disseminating payload for '{collection}': local org is \
                             not a member"
                        );
                        continue;
                    }
                    let Some(configured) = s.collection(collection) else {
                        log::warn!(
                            "Write targets collection '{collection}' but it is not configured"
                        );
                        continue;
                    };
                    let targets = s.payload_targets(configured);
                    let transient = s.transient.clone();
                    let retention_secs = configured.config.retention_secs;
                    drop(s);
                    (transient, targets, retention_secs)
                };
                // Outside the state lock (the store is a cheap Arc handle).
                if transient
                    .put(collection, &commitment, value, retention_secs)
                    .is_err()
                {
                    log::warn!("Failed to hold own payload for collection '{collection}'");
                }
                for target in targets {
                    if let Err(e) = target.try_send(Message::PrivatePayload {
                        collection: collection.clone(),
                        commitment: commitment.clone(),
                        payload: value.clone(),
                    }) {
                        log::warn!("Private payload delivery to a member peer failed: {e}");
                    }
                }
            }
        }
    }

    /// The peer-block append: re-validate the candidate against the CURRENT
    /// tip under the push lock, prune its transactions from the pending pool,
    /// and push. The admission check ran outside this lock, so the tip may
    /// have moved in between — pushing a stale candidate would fork the local
    /// chain. Returns `true` when the block was appended.
    ///
    /// The guard must live across the re-validation, the pending prune, and
    /// the push: they are one atomic append against the tip that the
    /// admission check can no longer see.
    #[allow(clippy::significant_drop_tightening)]
    async fn append_peer_block(ledger: &Arc<Mutex<Ledger>>, block: &Block) -> bool {
        let mut l = ledger.lock().await;
        let still_valid = l
            .chain
            .last()
            .map_or(block.index == 0 && block.previous_hash == "0", |tip| {
                block.chains_to(tip).is_ok()
            });
        if !still_valid {
            log::warn!("Dropping stale peer block {}: the tip moved", block.index);
            return false;
        }
        let committed: std::collections::HashSet<&str> =
            block.transactions.iter().map(|t| t.id.as_str()).collect();
        l.pending_transactions
            .retain(|t| !committed.contains(t.id.as_str()));
        l.chain.push(block.clone());
        true
    }

    /// Compute the candidate block's canonical write set: execute every
    /// `ContractExecution` transaction against the committed world-state
    /// snapshot, canonicalize the collected writes, and redact PDC values to
    /// their commitments (ADR-007 decision 2 — the block never holds a
    /// private value).
    ///
    /// Returns the block-form aggregate plus the per-transaction canonical
    /// contributions (same order as `transactions`), which the endorsement
    /// gate binds to each transaction's declared scopes (ADR-008 §4).
    ///
    /// Deterministic: the snapshot, transaction order, and canonicalized
    /// output are all functions of committed chain state.  A transaction
    /// whose execution fails (invalid WASM, gas exhaustion, …) accepts **no**
    /// writes; the failure is a deterministic function of the same inputs, so
    /// every node computes the identical write set and the block stays
    /// consistent — an empty contribution is the complete write set for that
    /// transaction, not a partial one.
    async fn compute_write_set(
        state: &Arc<Mutex<NodeState>>,
        transactions: &[Transaction],
    ) -> Result<(Vec<PersistentWrite>, Vec<Vec<PersistentWrite>>), NetworkError> {
        let s = state.lock().await;
        let Some(executor) = &s.executor else {
            return Ok((Vec::new(), vec![Vec::new(); transactions.len()]));
        };
        // The snapshot exposes exactly the committed state this node holds:
        // public values directly, PDC-scoped keys as their commitment until
        // the private payload arrives through ADR-003 dissemination (#46/#47).
        let mut result = ExecutionResult::default();
        let mut per_transaction: Vec<Vec<PersistentWrite>> = Vec::with_capacity(transactions.len());
        for tx in transactions {
            let TransactionKind::ContractExecution(ref execution) = tx.kind else {
                per_transaction.push(Vec::new());
                continue;
            };
            let Some(contract) = s.engine.get_contract(&execution.contract_id) else {
                per_transaction.push(Vec::new());
                continue;
            };
            let Some(wasm_b64) = contract.definition.wasm_code_b64.as_ref() else {
                per_transaction.push(Vec::new());
                continue;
            };
            let wasm = match BASE64_STANDARD.decode(wasm_b64) {
                Ok(bytes) => bytes,
                Err(error) => {
                    log::warn!(
                        "write-set: contract {} carried invalid WASM: {error}",
                        execution.contract_id
                    );
                    per_transaction.push(Vec::new());
                    continue;
                }
            };
            let execution_id = format!("commit:{}:{}", execution.contract_id, tx.id);
            match executor.execute_with_state(
                &execution_id,
                &wasm,
                s.world_state.clone(),
                ExecutionLimits::new(100_000, 100_000),
            ) {
                Ok(execution_result) => {
                    // Canonicalize per transaction so the endorsement gate can
                    // bind this contribution to the transaction's declared
                    // scopes (validates scope components, rejects
                    // intra-transaction duplicates, sorts deterministically).
                    let contribution = ExecutionResult {
                        ephemeral: Vec::new(),
                        writes: execution_result.writes,
                    }
                    .canonicalize()?;
                    result.writes.extend(contribution.writes.iter().cloned());
                    per_transaction.push(contribution.writes);
                }
                Err(error) => {
                    log::warn!(
                        "write-set: execution of contract {} failed: {error}",
                        execution.contract_id
                    );
                    per_transaction.push(Vec::new());
                }
            }
        }
        // Canonicalize (validates scopes, rejects intra-execution duplicates,
        // sorts deterministically) and redact PDC values for the block.
        let canonical = result.canonicalize()?;
        Ok((
            canonical
                .writes
                .iter()
                .map(PersistentWrite::block_form)
                .collect(),
            per_transaction,
        ))
    }

    /// Endorsement enforcement for a candidate replacement chain (ADR-008
    /// §4): the sync path adopts blocks wholesale, so every candidate block is
    /// evaluated under the capability set and policy history derived from the
    /// candidate chain itself — a chain that would break enforcement cannot be
    /// adopted. Carriers are evaluated against each block's pre-block policy
    /// history; committed writes are checked for aggregate carrier coverage
    /// (no per-transaction attribution on replay paths).
    async fn enforce_chain_endorsements(
        state: &Arc<Mutex<NodeState>>,
        candidate: &[Block],
    ) -> Result<(), NetworkError> {
        let provider = state.lock().await.endorsement.clone();
        let Some(provider) = provider else {
            return Ok(());
        };
        let mut capabilities = CapabilityHistory::default();
        let mut policies = PolicyHistory::default();
        for block in candidate {
            capabilities.validate_block(block)?;
            if capabilities
                .effective_set(block.index)
                .is_active(ENDORSEMENT_CAPABILITY_ID)
            {
                for tx in &block.transactions {
                    evaluate_transaction_endorsements(provider.as_ref(), &policies, tx, &[])?;
                }
                for write in &block.write_set {
                    if !block
                        .transactions
                        .iter()
                        .any(|tx| tx.endorsements.iter().any(|e| e.covers(write)))
                    {
                        return Err(NetworkError::Core(CoreError::InvalidTransaction(format!(
                            "endorsement: committed write '{}' on ({}, {}) falls outside every \
                             declared endorsement scope",
                            write.key, write.channel, write.contract
                        ))));
                    }
                }
            }
            policies.validate_block(block)?;
        }
        Ok(())
    }

    /// Return drained transactions to the pending pool after a rejected
    /// candidate (stale tip or failed endorsement).
    // The ledger guard must span the whole restore loop: the transactions go
    // back atomically with the rejection.
    #[allow(clippy::significant_drop_tightening)]
    async fn restore_pending(ledger: &Arc<Mutex<Ledger>>, transactions: Vec<Transaction>) {
        let mut l = ledger.lock().await;
        for tx in transactions {
            if let Err(e) = l.add_transaction(tx) {
                log::warn!("Failed to restore transaction to the pending pool: {e}");
            }
        }
    }

    /// Endorsement enforcement for a candidate block (ADR-008 handoff 4):
    /// invoked at the network commit path before any materialization.
    ///
    /// Gated on the `endorsement` capability being active at the candidate
    /// height (ADR-010) **and** a provider being configured. Validates the
    /// block's policy metadata and the same-block policy/write conflict rule,
    /// then evaluates every transaction's declared carriers against the
    /// *pre-block* policy history (a same-block policy update applies only
    /// from the next block). `per_tx_writes` carries the per-transaction
    /// contributions when the caller can attribute writes (mining); replay
    /// paths pass an empty slice and get an aggregate coverage check instead.
    async fn enforce_block_endorsements(
        state: &Arc<Mutex<NodeState>>,
        ledger: &Arc<Mutex<Ledger>>,
        block: &Block,
        per_tx_writes: &[Vec<PersistentWrite>],
    ) -> Result<(), NetworkError> {
        let provider = state.lock().await.endorsement.clone();
        let Some(provider) = provider else {
            return Ok(());
        };
        let active = {
            let l = ledger.lock().await;
            CapabilityHistory::build_from_blocks(&l.chain)?
                .effective_set(block.index)
                .is_active(ENDORSEMENT_CAPABILITY_ID)
        };
        if !active {
            return Ok(());
        }
        let policies = state.lock().await.policies.clone();
        // Structural metadata + same-block conflicts, on a scratch history —
        // signature evaluation below uses the pre-block policy set.
        let mut scratch = policies.clone();
        scratch.validate_block(block)?;
        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let writes = per_tx_writes.get(tx_index).map_or(&[][..], Vec::as_slice);
            evaluate_transaction_endorsements(provider.as_ref(), &policies, tx, writes)?;
        }
        if per_tx_writes.is_empty() {
            // No write attribution on replay paths: every committed write
            // must still sit inside some declared carrier.
            for write in &block.write_set {
                if !block
                    .transactions
                    .iter()
                    .any(|tx| tx.endorsements.iter().any(|e| e.covers(write)))
                {
                    return Err(NetworkError::Core(CoreError::InvalidTransaction(format!(
                        "endorsement: committed write '{}' on ({}, {}) falls outside every \
                         declared endorsement scope",
                        write.key, write.channel, write.contract
                    ))));
                }
            }
        }
        Ok(())
    }

    /// Evaluate an endorsement request against the committed policies
    /// (ADR-008): every applicable policy layer for the request's target is
    /// evaluated through the configured provider. Backs the
    /// `VerifyEndorsement` RPC.
    ///
    /// # Errors
    ///
    /// Returns [`NetworkError`] when no provider is configured, a signer
    /// cannot be authenticated, or a policy is not valid v1 metadata.
    pub async fn verify_endorsement(
        &self,
        request: EndorsementRequest,
    ) -> Result<Vec<EndorsementEvaluation>, NetworkError> {
        let s = self.state.lock().await;
        let Some(provider) = &s.endorsement else {
            return Err(NetworkError::Core(CoreError::InvalidTransaction(
                "no endorsement provider configured on this node".into(),
            )));
        };
        let policies = s
            .policies
            .policies_for(&request.target.channel, &request.target.contract);
        let provider = provider.clone();
        drop(s);
        let mut evaluations = Vec::new();
        for policy in policies.applicable(&request.target) {
            evaluations.push(provider.evaluate(&policy, &request)?);
        }
        Ok(evaluations)
    }

    /// Rebuild the derived world state from committed blocks in block order.
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
    ///
    /// Transaction relays skip read-only observers: they may parse and
    /// validate history, but not participate in relaying active writes
    /// (ADR-010 decision 6). Blocks and sync traffic still reach them.
    async fn broadcast(&self, message: Message) {
        let senders: Vec<(String, Sender<Message>)> = {
            let s = self.state.lock().await;
            if matches!(message, Message::Transaction(_)) {
                let active = active_set_at_tip(&self.ledger).await;
                // Relayed transactions are counted under a shared label: the
                // per-peer counter exists to expose stragglers on the block
                // fan-out path, which is where a full channel costs a peer
                // its chain position (#62 §5.1).
                s.relay_targets(&active)
                    .into_iter()
                    .map(|sender| ("relay".to_owned(), sender))
                    .collect()
            } else {
                s.peer_senders
                    .iter()
                    .map(|(addr, sender)| (addr.clone(), sender.clone()))
                    .collect()
            }
        };
        for (addr, sender) in senders {
            match sender.try_send(message.clone()) {
                Err(TrySendError::Full(_)) => {
                    let total = {
                        let mut s = self.state.lock().await;
                        let entry = s.dropped_outbound.entry(addr.clone()).or_insert(0);
                        *entry += 1;
                        let total = *entry;
                        drop(s);
                        total
                    };
                    log::warn!(
                        "Dropping outbound message: peer channel full (peer {addr}, \
                         {total} dropped cumulative)"
                    );
                }
                Ok(()) | Err(TrySendError::Closed(_)) => {}
            }
        }
    }

    /// Cumulative outbound drops per peer address (read-only visibility for
    /// operators and tests; #62 §5.6 item 2).
    pub async fn dropped_outbound(&self, addr: &str) -> u64 {
        self.state
            .lock()
            .await
            .dropped_outbound
            .get(addr)
            .copied()
            .unwrap_or(0)
    }

    /// Post-commit hook: persist the block, index it, fire the event bus,
    /// ingest the analytics projections, run watcher triggers, and add any
    /// autonomous transactions to the ledger.
    // Each parameter is an injected seam (`Ledger`, `NodeState`, `StorageProvider`, …);
    // bundling them would hide the injection graph behind an opaque context type.
    // The body is one linear post-commit pipeline; split it when a caller needs a piece.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn after_block_commit(
        ledger: &Arc<Mutex<Ledger>>,
        state: &Arc<Mutex<NodeState>>,
        event_tx: &broadcast::Sender<NodeEvent>,
        indexer: &Arc<InMemoryIndexer>,
        event_bus: &Arc<InMemoryEventBus>,
        provenance: &Arc<Mutex<ProvenanceIndex>>,
        flattener: &Arc<Mutex<AnalyticalFlattener>>,
        block: &Block,
        storage: &Arc<dyn StorageProvider>,
    ) -> Vec<Transaction> {
        // Persist the block and apply its canonical write set through one
        // atomic commit boundary; on success, mirror the writes into the
        // derived world-state cache.  On failure the chain stays authoritative
        // (ADR-007 decision 2): the block is already committed to the ledger,
        // and the next rebuild heals the storage divergence from the chain.
        //
        // `apply_block` is blocking disk+CPU work, so it runs on the blocking
        // thread pool — on the peer path this function is called from the
        // per-peer read task, and an inline call would block a runtime worker
        // and head-of-line-block that connection (#62 §5.5). Awaiting the
        // handle keeps per-peer block ordering.
        let persist_storage = storage.clone();
        let owned_block = block.clone();
        let apply_result =
            tokio::task::spawn_blocking(move || persist_storage.apply_block(&owned_block))
                .await
                .unwrap_or_else(|e| {
                    Err(CoreError::InvalidBlock(format!("persist task failed: {e}")))
                });
        match apply_result {
            Ok(()) => {
                let mut s = state.lock().await;
                for write in &block.write_set {
                    write.apply_to_cache(&mut s.world_state);
                }
            }
            Err(e) => {
                log::warn!("Storage: failed to apply block {}: {e}", block.index);
            }
        }

        // A committed policy update activates now: replay the policy history
        // from the chain so evaluation uses the post-commit policy set
        // (ADR-008 decision 4 — the new policy applies from the next block).
        if block
            .transactions
            .iter()
            .any(|tx| matches!(tx.kind, TransactionKind::PolicyUpdate(_)))
        {
            let chain = ledger.lock().await.chain.clone();
            match PolicyHistory::build_from_blocks(&chain) {
                Ok(policies) => state.lock().await.policies = policies,
                Err(e) => {
                    log::warn!(
                        "Policy history rebuild failed after block {}: {e}",
                        block.index
                    );
                }
            }
        }

        if let Err(e) = indexer.index_block(block) {
            log::warn!("Indexer error: {e}");
        }
        if let Err(e) = event_bus.publish_block(block) {
            log::warn!("EventBus error: {e}");
        }

        // Analytics projections: custody provenance and flat records.
        {
            let mut provenance_guard = provenance.lock().await;
            provenance_guard.ingest_block(block);
        }
        match indexed_transactions_of(block) {
            Ok(txs) => {
                flattener
                    .lock()
                    .await
                    .ingest_indexed_block(&IndexedBlock::from(block), &txs);
            }
            Err(e) => {
                log::warn!("flattener: failed to index block {}: {e}", block.index);
            }
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
    provenance: Arc<Mutex<ProvenanceIndex>>,
    flattener: Arc<Mutex<AnalyticalFlattener>>,
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
    let (org, certificate_pem) = {
        let s = ctx.state.lock().await;
        let certificate_pem = s
            .identity
            .as_ref()
            .and_then(|identity| identity.certificate_pem.clone());
        (s.local_org(&ctx.node_id), certificate_pem)
    };
    let hello = Message::Hello {
        node_id: ctx.node_id.clone(),
        tls_cert_fingerprint: ctx.local_tls_cert_fingerprint.clone(),
        chain_length,
        version: PROTOCOL_VERSION.to_owned(),
        capabilities: CAPABILITY_V1
            .iter()
            .map(|c| CapabilityAdvertisement {
                id: c.id.to_owned(),
                version: c.version,
            })
            .collect(),
        org,
        certificate_pem,
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
// Each argument is a shared component required by the peer lifecycle task.
#[allow(clippy::too_many_arguments)]
async fn connect_to_peer(
    peer_addr: String,
    listen_addr: String,
    ledger: Arc<Mutex<Ledger>>,
    state: Arc<Mutex<NodeState>>,
    node_id: String,
    event_tx: broadcast::Sender<NodeEvent>,
    indexer: Arc<InMemoryIndexer>,
    event_bus: Arc<InMemoryEventBus>,
    provenance: Arc<Mutex<ProvenanceIndex>>,
    flattener: Arc<Mutex<AnalyticalFlattener>>,
    dial_tx: UnboundedSender<String>,
    storage: Arc<dyn StorageProvider>,
    tls: Arc<NodeTls>,
) {
    match TcpStream::connect(&peer_addr).await {
        Ok(mut stream) => {
            log::info!("Connected to peer {peer_addr}");

            let local_cert = tls.cert_der.as_ref();
            let Ok(cert_len) = u32::try_from(local_cert.len()) else {
                log::warn!("Local TLS certificate too large to advertise to {peer_addr}");
                return;
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
                    provenance,
                    flattener,
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

// This function is the single peer-message state machine; keep its branches together.
#[allow(clippy::too_many_lines)]
async fn process_message(
    msg: Message,
    addr: &str,
    ctx: &PeerContext,
    write_tx: &Sender<Message>,
    current_stable_addr: Option<&str>,
    observed_cert_fingerprint: &str,
    _peer_cert_der: &[u8],
) -> MessageEffect {
    match msg {
        Message::Hello {
            node_id: peer_id,
            tls_cert_fingerprint,
            chain_length,
            listen_addr: peer_listen_addr,
            version,
            capabilities,
            org: peer_org,
            certificate_pem: peer_certificate_pem,
        } => {
            log::info!(
                "Hello from {addr} (id={peer_id}, chain_len={chain_length}, listen={peer_listen_addr})"
            );

            // ── Step 0: wire-encoding compatibility gate (ADR-010 decision 6) ──
            // PROTOCOL_VERSION is a wire-encoding gate, separate from ledger
            // capabilities; incompatible peers are rejected.
            if version != PROTOCOL_VERSION {
                log::warn!(
                    "Rejecting peer {peer_id} at {addr}: protocol version '{version}' \
                     is incompatible with '{PROTOCOL_VERSION}'"
                );
                return MessageEffect {
                    disconnect: true,
                    ..Default::default()
                };
            }

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
                // The mismatch itself is the security event; the fingerprint
                // values are deliberately not logged (CodeQL
                // cleartext-logging) — the peer id and address identify the
                // offender, and the fingerprints are recomputable from the
                // presented certificates.
                log::warn!(
                    "Rejecting peer {peer_id} at {addr}: advertised TLS fingerprint \
                     does not match the observed session certificate"
                );
                return MessageEffect {
                    disconnect: true,
                    ..Default::default()
                };
            }

            // ── Step 2.5: certificate-verified org (ticket #47) ──────────
            // When a verifier is configured, the claimed org counts only if
            // the peer's organization-issued certificate verifies against this
            // org's Root CA and its subject CN equals the claimed org. The TLS
            // certificate is transport-only (self-signed), so this check runs
            // on the Hello-carried certificate.
            let org_verified = {
                let s = ctx.state.lock().await;
                let has_verifier = s.cert_verifier.is_some();
                let verified = has_verifier
                    && s.cert_verifier.as_ref().is_some_and(|verifier| {
                        peer_certificate_pem.as_ref().is_some_and(|pem| {
                            verifier
                                .verified_subject_cn_pem(pem)
                                .is_ok_and(|cn| cn == peer_org)
                        })
                    });
                let rejected = has_verifier && !verified;
                drop(s);
                if rejected {
                    // ADR-011 decision: an unverified organization stays
                    // connected (it may still sync and verify public history)
                    // but every org-gated path — private-payload send and
                    // receive — fails closed against its self-asserted org.
                    log::warn!(
                        "Peer {peer_id} at {peer_listen_addr}: organization \
                         '{peer_org}' is not certificate-verified; org-gated \
                         paths will not trust it"
                    );
                }
                verified
            };

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
                    &peer_org,
                    org_verified,
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

                // ── Step 3: certificate-verified org (moved to Step 2.5, #47) ──
                // The TLS certificate is a transport-only self-signed cert, so
                // organization trust does NOT ride on it: Step 2.5 verifies the
                // Hello-carried organization certificate against the configured
                // Root CA and gates both the connection and the private-payload
                // path on that verification.

                // ── Step 4: record advertised capabilities ──────────────
                // Read-only status is not cached here: it is evaluated at
                // each proposal/relay against the capability set effective at
                // the current tip, so a peer that predates an activation loses
                // write rights the moment the activation takes effect.
                s.peer_registry
                    .set_advertised(&peer_listen_addr, capabilities);

                // ── Step 5: register live connection state ────────────
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
            let Some(stable_addr) = current_stable_addr else {
                log::warn!("Ignoring transaction from unauthenticated peer {addr}");
                return MessageEffect::default();
            };
            // Read-only observers may not propose writes (ADR-010 decision 6).
            {
                let active = active_set_at_tip(&ctx.ledger).await;
                let s = ctx.state.lock().await;
                if s.peer_registry.is_read_only(stable_addr, &active) {
                    log::warn!("Ignoring transaction from read-only observer {addr}");
                    return MessageEffect::default();
                }
            }
            let generated = {
                let mut s = ctx.state.lock().await;
                let gen = if let TransactionKind::SupplyOffer(ref offer) = tx.kind {
                    s.engine.evaluate_supply_offer(offer, &tx.id)
                } else {
                    Vec::new()
                };
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

            let senders: Vec<Sender<Message>> = {
                let active = active_set_at_tip(&ctx.ledger).await;
                let s = ctx.state.lock().await;
                s.relay_targets(&active)
            };
            for gen_tx in generated {
                for s in &senders {
                    let _ = s.try_send(Message::Transaction(gen_tx.clone()));
                }
            }
            MessageEffect::default()
        }

        Message::Block(block) => {
            // Reject messages from peers that have not completed a successful Hello.
            let Some(stable_addr) = current_stable_addr else {
                log::warn!("Ignoring block from unauthenticated peer {addr}");
                return MessageEffect::default();
            };
            // Read-only observers may not propose blocks (ADR-010 decision 6).
            {
                let active = active_set_at_tip(&ctx.ledger).await;
                let s = ctx.state.lock().await;
                if s.peer_registry.is_read_only(stable_addr, &active) {
                    log::warn!("Ignoring block from read-only observer {addr}");
                    return MessageEffect::default();
                }
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
                let result = if block.index == expected {
                    let diff = l.difficulty;
                    l.chain.last().map_or((false, false), |prev| {
                        let valid = block.chains_to(prev).is_ok()
                            && block.has_valid_pow(diff)
                            && CapabilityHistory::build_from_blocks(&l.chain)
                                .and_then(|mut history| history.validate_block(&block))
                                .is_ok();
                        (valid, false)
                    })
                } else {
                    (false, block.index > expected)
                };
                drop(l);
                result
            };

            if too_far_ahead {
                let _ = write_tx.try_send(Message::RequestChain);
                return MessageEffect::default();
            }

            if should_append {
                // Endorsement enforcement on the peer-admission path
                // (ADR-008 §4): policy metadata, the same-block rule, and
                // declared-carrier evaluation run before the block is
                // appended; write attribution is not recomputable here (no
                // re-execution), so coverage is checked in aggregate.
                if let Err(e) =
                    Node::enforce_block_endorsements(&ctx.state, &ctx.ledger, &block, &[]).await
                {
                    log::warn!("Rejected block {} from {addr}: {e}", block.index);
                    return MessageEffect::default();
                }
                Node::append_peer_block(&ctx.ledger, &block).await;

                let generated = Node::after_block_commit(
                    &ctx.ledger,
                    &ctx.state,
                    &ctx.event_tx,
                    &ctx.indexer,
                    &ctx.event_bus,
                    &ctx.provenance,
                    &ctx.flattener,
                    &block,
                    &ctx.storage,
                )
                .await;

                let _ = ctx.event_tx.send(NodeEvent::BlockReceived {
                    index: block.index,
                    hash: block.hash.clone(),
                    // PoW's attestation is the valid nonce in the block itself:
                    // a verifying member derives and validates the degenerate
                    // certificate on receipt. BFT-attested blocks are not
                    // admissible here yet: certificate wire transport and
                    // peer-path quorum verification are ADR-010 adoption-gate
                    // work (staged, ticket #42).
                    certificate: QuorumCertificate::pow(&block),
                });

                let senders: Vec<Sender<Message>> = {
                    let active = active_set_at_tip(&ctx.ledger).await;
                    let s = ctx.state.lock().await;
                    s.relay_targets(&active)
                };
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
            // A synced chain is adopted wholesale: endorsement enforcement
            // must hold on the candidate itself before any block is adopted
            // (ADR-008 §4 — no commit path bypasses evaluation).
            if let Err(e) = Node::enforce_chain_endorsements(&ctx.state, &candidate).await {
                log::warn!("Rejected chain replacement from {addr}: {e}");
                return MessageEffect::default();
            }
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

                // Every block adopted by sync is a commit: emit the
                // certificate-bearing notification so commit consumers receive
                // the attestation set on this path too (degenerate `PoW`
                // certificate here; BFT certificate replay on sync is ADR-010
                // adoption-gate work — certificates are not persisted with
                // blocks yet, ticket #42).
                for block in &new_chain {
                    if block.index == 0 {
                        continue;
                    }
                    let _ = ctx.event_tx.send(NodeEvent::BlockReceived {
                        index: block.index,
                        hash: block.hash.clone(),
                        certificate: QuorumCertificate::pow(block),
                    });
                }

                Node::rebuild_runtime_state_from_chain(
                    &ctx.ledger,
                    &ctx.state,
                    &ctx.storage,
                    &ctx.provenance,
                    &ctx.flattener,
                )
                .await;
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

        Message::PrivatePayload {
            collection,
            commitment,
            payload,
        } => {
            // ── The private-payload transport boundary (ADR-003, #46) ──────
            // A payload is accepted only when the `pdc` capability is active,
            // this node's org is a collection member, the sender's org is a
            // member, and the commitment matches the payload. A non-member
            // never holds private cleartext; the chain carries only the
            // commitment.
            // The payload is a pre-commit artifact for a write that lands at
            // the NEXT height — it may arrive before its block does — so the
            // gate is the capability set effective there (matching the
            // submission gate). One lock scope for height and history so the
            // chain cannot advance in between.
            let pdc_active = {
                let ledger = ctx.ledger.lock().await;
                let next_height = ledger.chain.len() as u64;
                CapabilityHistory::build_from_blocks(&ledger.chain).is_ok_and(|history| {
                    history
                        .effective_set(next_height)
                        .is_active(PDC_CAPABILITY_ID)
                })
            };
            if !pdc_active {
                log::warn!(
                    "Rejecting private payload for '{collection}' from {addr}: \
                     the 'pdc' capability is not active"
                );
                return MessageEffect::default();
            }
            // Only peers that completed the handshake may send payloads.
            let Some(stable_addr) = current_stable_addr else {
                log::warn!("Ignoring private payload from unauthenticated peer {addr}");
                return MessageEffect::default();
            };
            let local_org = ctx
                .state
                .lock()
                .await
                .identity
                .clone()
                .map_or_else(|| ctx.node_id.clone(), |identity| identity.node_id.clone());
            let s = ctx.state.lock().await;
            let sender_org = s.peer_org(stable_addr);
            let sender_verified = sender_org
                .as_ref()
                .and_then(|org| s.peer_registry.org_verified(stable_addr, org));
            // When this node runs certificate verification, a private payload
            // may only come from a peer whose org was certificate-verified
            // (ticket #47): the self-asserted Hello org is not trusted here.
            let verification_required = s.cert_verifier.is_some();
            let sender_ok = sender_org
                .as_ref()
                .is_some_and(|org| s.is_collection_member(&collection, org))
                && (!verification_required || sender_verified == Some(true));
            let commitment_ok = glasschain_core::crypto::sha256(&payload) == commitment;
            let rejection = match (s.is_collection_member(&collection, &local_org), sender_ok) {
                (false, _) => Some(format!("local org '{local_org}' is not a member")),
                (true, false) => Some(sender_org.map_or_else(
                    || "sender not in the peer registry".to_owned(),
                    |org| format!("sender org '{org}' is not a member"),
                )),
                (true, true) if !commitment_ok => Some("commitment mismatch".to_owned()),
                (true, true) => None,
            };
            if let Some(reason) = rejection {
                log::warn!("Rejecting private payload for '{collection}' from {addr}: {reason}");
            } else if s
                .transient
                .put(
                    &collection,
                    &commitment,
                    &payload,
                    s.collection(&collection).map_or(
                        glasschain_identity::default_retention_secs(),
                        |configured| configured.config.retention_secs,
                    ),
                )
                .is_ok()
            {
                log::info!(
                    "Private payload stored (collection={collection}, commitment={}...)",
                    &commitment[..8]
                );
                drop(s);
                let _ = ctx.event_tx.send(NodeEvent::PrivatePayloadReceived {
                    collection,
                    commitment,
                });
            } else {
                log::warn!(
                    "Rejecting private payload for '{collection}' from {addr}: \
                     transient store failure"
                );
            }
            MessageEffect::default()
        }
        Message::RequestPrivatePayload {
            collection,
            commitment,
        } => {
            // Pull reconciliation (ticket #47): only a member may ask, and
            // only a member holding the payload may answer — the response is
            // the ordinary PrivatePayload message, so every transport-boundary
            // check applies to the answer too.
            let Some(stable_addr) = current_stable_addr else {
                log::warn!("Ignoring private-payload request from unauthenticated peer {addr}");
                return MessageEffect::default();
            };
            let (transient, requester_member, holder_member) = {
                let s = ctx.state.lock().await;
                let requester_member = s
                    .peer_org(stable_addr)
                    .is_some_and(|org| s.is_collection_member(&collection, &org));
                let holder_member = s.is_collection_member(&collection, &s.local_org(&ctx.node_id));
                let transient = s.transient.clone();
                drop(s);
                (transient, requester_member, holder_member)
            };
            if !requester_member || !holder_member {
                log::warn!(
                    "Ignoring private-payload request for '{collection}' from {addr}: \
                     requester or holder is not a member"
                );
                return MessageEffect::default();
            }
            match transient.get(&collection, &commitment) {
                Ok(Some(payload)) => {
                    if let Err(e) = write_tx.try_send(Message::PrivatePayload {
                        collection,
                        commitment,
                        payload,
                    }) {
                        log::warn!("Reconcile response delivery failed: {e}");
                    }
                }
                Ok(None) => {
                    log::debug!("Reconcile: payload {commitment} not held for '{collection}'");
                }
                Err(e) => log::warn!("Reconcile lookup failed: {e}"),
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
    use glasschain_core::{
        ContractExecution, CoreError, InventoryUpdate, PurchaseConditions, SmartContractDef,
        SupplyOffer, TraceableAsset, TraceableAssetRegistration, WriteOp, WriteVisibility,
    };
    use glasschain_identity::SignedTransaction;
    use glasschain_vm::WasmExecutionProvider;

    #[test]
    fn tofu_first_contact_records_identity() {
        let mut reg = PeerRegistry::new();
        let result = reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123", "org-a", false);
        assert_eq!(result, Ok(true), "first contact should return Ok(true)");
        assert_eq!(reg.peers.len(), 1);
        let peer = &reg.peers["127.0.0.1:8000"];
        assert_eq!(peer.node_id, "node-a");
        assert_eq!(peer.cert_fingerprint, "abc123");
    }

    #[test]
    fn tofu_returning_peer_with_same_identity_passes() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123", "org-a", false)
            .unwrap();
        let result = reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123", "org-a", false);
        assert_eq!(
            result,
            Ok(false),
            "returning peer with same identity should return Ok(false)"
        );
    }

    #[test]
    fn tofu_rejects_node_id_change() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123", "org-a", false)
            .unwrap();
        let result =
            reg.verify_or_register("127.0.0.1:8000", "node-IMPOSTER", "abc123", "org-b", false);
        assert!(result.is_err(), "changed node_id should be rejected");
    }

    #[test]
    fn tofu_rejects_cert_fingerprint_change() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123", "org-a", false)
            .unwrap();
        let result = reg.verify_or_register("127.0.0.1:8000", "node-a", "TAMPERED", "org-a", false);
        assert!(
            result.is_err(),
            "changed cert fingerprint should be rejected"
        );
    }

    #[test]
    fn tofu_rejects_org_drift() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123", "org-a", true)
            .unwrap();
        // A returning peer claiming a different organization is rejected:
        // the org gates private-payload delivery (ticket #47).
        let result = reg.verify_or_register("127.0.0.1:8000", "node-a", "abc123", "org-b", true);
        let err = result.expect_err("org drift should be rejected");
        assert!(err.contains("org changed"), "{err}");
    }

    #[test]
    fn tofu_independent_addresses_are_independent() {
        let mut reg = PeerRegistry::new();
        reg.verify_or_register("127.0.0.1:8000", "node-a", "aaa", "org-a", false)
            .unwrap();
        reg.verify_or_register("127.0.0.1:9000", "node-b", "bbb", "org-b", false)
            .unwrap();
        assert_eq!(reg.peers.len(), 2);
        // Each address keeps its own identity.
        assert!(reg
            .verify_or_register("127.0.0.1:8000", "node-a", "aaa", "org-a", false)
            .is_ok());
        assert!(reg
            .verify_or_register("127.0.0.1:9000", "node-b", "bbb", "org-b", false)
            .is_ok());
        // Cross-contamination is rejected.
        assert!(reg
            .verify_or_register("127.0.0.1:8000", "node-b", "aaa", "org-b", false)
            .is_err());
    }

    // ── helpers for chain-restore tests ───────────────────────────────────────

    /// Mine a `n_blocks`-long valid chain and persist it to `storage`.
    fn seed_storage(
        storage: &Arc<dyn StorageProvider>,
        difficulty: usize,
        n_blocks: usize,
    ) -> Vec<Block> {
        let mut ledger = Ledger::new(difficulty);
        for _ in 1..n_blocks {
            ledger.mine_pending_transactions().unwrap();
        }
        let chain = ledger.chain.clone();
        for block in &chain {
            storage.put_block(block).unwrap();
        }
        chain
    }

    // ── 1. restore_ledger ─────────────────────────────────────────────────────

    #[test]
    fn restore_ledger_reloads_valid_chain() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let seeded = seed_storage(&storage, 2, 3);

        let ledger = Node::restore_ledger(&storage, 2);
        let chain = ledger.try_lock().unwrap().chain.clone();

        assert_eq!(chain.len(), 3, "all seeded blocks should be restored");
        assert_eq!(chain, seeded, "restored chain must match the persisted one");
    }

    #[test]
    fn restore_ledger_falls_back_on_invalid_chain_link() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let mut chain = seed_storage(&storage, 2, 2);
        // Corrupt block 1 so it no longer satisfies the PoW target.
        chain[1].nonce = chain[1].nonce.wrapping_add(12345);
        chain[1].hash = chain[1].calculate_hash();
        storage.put_block(&chain[1]).unwrap();

        let ledger = Node::restore_ledger(&storage, 2);
        let restored = ledger.try_lock().unwrap().chain.clone();

        assert_eq!(
            restored.len(),
            1,
            "an invalid chain must fall back to a fresh genesis-only ledger"
        );
        assert_eq!(restored[0].previous_hash, "0");
    }

    #[test]
    fn restore_ledger_falls_back_on_missing_block() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let genesis = Ledger::new(2).chain[0].clone();
        storage.put_block(&genesis).unwrap();
        // Latest index is 2 but block 1 is missing → the load loop must abort.
        let mut b2 = Block::new(2, vec![], genesis.hash);
        b2.mine(2);
        storage.put_block(&b2).unwrap();
        assert_eq!(storage.latest_block_index().unwrap(), Some(2));

        let ledger = Node::restore_ledger(&storage, 2);
        let restored = ledger.try_lock().unwrap().chain.clone();

        assert_eq!(
            restored.len(),
            1,
            "a gap in the stored chain must trigger the fresh-ledger fallback"
        );
    }

    #[test]
    fn restore_ledger_rejects_invalid_genesis() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        // Genesis with previous_hash != "0" is invalid regardless of PoW.
        let mut genesis = Block::new(0, vec![], "not-zero".into());
        genesis.mine(2);
        storage.put_block(&genesis).unwrap();

        let ledger = Node::restore_ledger(&storage, 2);
        assert_eq!(ledger.try_lock().unwrap().chain.len(), 1);
    }

    // ── 2. build_tls — identity-backed certificate ────────────────────────────

    #[test]
    fn build_tls_identity_branch_sets_common_name_to_node_id() {
        // build_tls reads GLASSCHAIN_INSECURE_TLS; serialize with the env test.
        let _env_guard = TLS_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let identity = Identity::generate("node-42");
        let tls = Node::build_tls(Some(Arc::new(identity)));

        // The served cert fingerprint is derived from the served cert bytes.
        assert_eq!(tls.cert_fingerprint, sha256(tls.cert_der.as_ref()));
        // With the insecure-tls feature off (default), no permissive connector
        // is pre-built; the feature turns it on regardless of env.
        assert_eq!(tls.insecure, cfg!(feature = "insecure-tls"));
        assert_eq!(tls.connector.is_some(), cfg!(feature = "insecure-tls"));

        let (_, cert) = x509_parser::parse_x509_certificate(tls.cert_der.as_ref())
            .expect("identity cert must parse as X.509");
        let cns: Vec<&str> = cert
            .subject()
            .iter_common_name()
            .map(|attr| attr.as_str().unwrap())
            .collect();
        assert_eq!(cns, vec!["node-42"], "cert CN must equal the node id");
    }

    #[test]
    fn build_tls_anonymous_branch_uses_default_common_name() {
        // build_tls reads GLASSCHAIN_INSECURE_TLS; serialize with the env test.
        let _env_guard = TLS_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tls = Node::build_tls(None);
        let (_, cert) = x509_parser::parse_x509_certificate(tls.cert_der.as_ref()).unwrap();
        let cns: Vec<&str> = cert
            .subject()
            .iter_common_name()
            .map(|attr| attr.as_str().unwrap())
            .collect();
        // Without an identity, rcgen's built-in default CN is used (which is
        // distinct from the identity branch, which sets CN = node_id).
        assert_eq!(cns, vec!["rcgen self signed cert"]);
    }

    // ── 3. build_tls / insecure_tls_allowed ───────────────────────────────────

    /// RAII guard that restores a process env var after the test body runs.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    // Env-var manipulation is process-global, so serialize the tests that touch it.
    static TLS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn tls_insecure_env_var_controls_permissive_connector() {
        let _guard = TLS_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // GLASSCHAIN_INSECURE_TLS=1 → permissive AcceptAnyCert connector is built.
        let env_guard = EnvGuard::set("GLASSCHAIN_INSECURE_TLS", "1");
        let insecure_tls = Node::build_tls(None);
        assert!(insecure_tls.insecure);
        assert!(
            insecure_tls.connector.is_some(),
            "permissive connector expected"
        );
        drop(env_guard);

        // With the variable unset (and the crate feature off in default builds),
        // the connector stays None and verification is the default.
        let secure_tls = Node::build_tls(None);
        assert_eq!(secure_tls.insecure, cfg!(feature = "insecure-tls"));
        assert_eq!(
            secure_tls.connector.is_some(),
            cfg!(feature = "insecure-tls")
        );
    }

    // ── 4. set_cert_verifier ──────────────────────────────────────────────────

    #[tokio::test]
    async fn set_cert_verifier_stores_org_verifier() {
        let node = Node::new("n1", "127.0.0.1:0", 2);
        assert!(node.state.lock().await.cert_verifier.is_none());

        // Build a self-signed root cert to act as the org trust anchor.
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["Corp Root CA".into()]).unwrap();
        let root = params.self_signed(&key).unwrap();
        let verifier = CertChainVerifier::from_der("Corp", root.der()).unwrap();

        node.set_cert_verifier(verifier).await;

        let stored = node.state.lock().await.cert_verifier.clone();
        assert!(stored.is_some(), "CA verifier must be stored on the node");
        assert_eq!(stored.unwrap().org_name, "Corp");
    }

    // ── 5. set_signing_identity + signing branch of after_block_commit ────────

    #[tokio::test]
    async fn after_block_commit_signs_autonomous_watcher_transaction() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let node = Node::new_with_storage("n1", "127.0.0.1:0", 2, Arc::clone(&storage));

        node.set_signing_identity(Arc::new(Identity::generate("signer-1")))
            .await;
        node.register_inventory_trigger(InventoryTrigger {
            trigger_id: "trig-1".into(),
            product_id: "SKU".into(),
            owner_id: "buyer-1".into(),
            reorder_threshold: 0,
            reorder_quantity: 25,
            seller_id: "seller-1".into(),
            price_per_unit: 100,
            currency: "USD".into(),
            active: true,
            wasm_code_b64: None,
        })
        .await;

        // Commit a block containing an inventory update that drops below the
        // threshold so the watcher emits a PurchaseOrder.
        let prev_hash = node.ledger.lock().await.chain[0].hash.clone();
        let tx = Transaction::with_id(
            "inv-1",
            TransactionKind::InventoryUpdate(InventoryUpdate {
                product_id: "SKU".into(),
                owner_id: "buyer-1".into(),
                quantity_delta: -100,
                reason: "consumption".into(),
            }),
        );
        let mut block = Block::new(1, vec![tx], prev_hash);
        block.mine(2);

        let generated = Node::after_block_commit(
            &node.ledger,
            &node.state,
            &node.event_tx,
            &node.indexer,
            &node.event_bus,
            &node.provenance,
            &node.flattener,
            &block,
            &node.storage,
        )
        .await;

        assert!(!generated.is_empty(), "watcher should emit a PurchaseOrder");
        let order_tx = &generated[0];
        assert!(
            matches!(
                order_tx.kind,
                TransactionKind::PurchaseOrder(ref po) if po.product_id == "SKU"
            ),
            "generated tx should be a PurchaseOrder for SKU"
        );

        // The identity must have signed it and persisted the result to storage.
        let key = format!("signed_tx:{}", order_tx.id);
        let bytes = storage
            .get_state(&key)
            .unwrap()
            .expect("signed autonomous tx must be persisted");
        let signed: SignedTransaction = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(signed.signer_node_id, "signer-1");
        assert_eq!(signed.transaction.id, order_tx.id);
        signed.verify().unwrap();
    }

    // ── 6. rebuild_runtime_state_from_chain ───────────────────────────────────

    #[tokio::test]
    async fn rebuild_runtime_state_restores_watcher_snapshot() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());

        // Persist a watcher snapshot carrying non-trivial inventory.
        let mut watcher = WatcherService::new();
        watcher.on_inventory_update(&InventoryUpdate {
            product_id: "SKU".into(),
            owner_id: "buyer-1".into(),
            quantity_delta: 42,
            reason: "seed".into(),
        });
        let bytes = watcher.serialize_state().unwrap();
        storage.put_state("watcher:state", &bytes).unwrap();

        let node = Node::new_with_storage("n1", "127.0.0.1:0", 2, Arc::clone(&storage));
        Node::rebuild_runtime_state_from_chain(
            &node.ledger,
            &node.state,
            &node.storage,
            &node.provenance,
            &node.flattener,
        )
        .await;

        let restored = node
            .state
            .lock()
            .await
            .watcher
            .inventory_level("SKU", "buyer-1");
        assert_eq!(
            restored, 42,
            "watcher state should restore from the snapshot"
        );
    }

    #[tokio::test]
    async fn rebuild_runtime_state_replays_chain_when_snapshot_invalid() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        storage
            .put_state("watcher:state", b"not-valid-json")
            .unwrap();

        // Build a persisted chain that commits an InventoryUpdate and an
        // AssetRegistration (so the analytics projections have data to rebuild).
        let genesis = Ledger::new(2).chain[0].clone();
        storage.put_block(&genesis).unwrap();
        let tx = Transaction::with_id(
            "inv-1",
            TransactionKind::InventoryUpdate(InventoryUpdate {
                product_id: "SKU".into(),
                owner_id: "buyer-1".into(),
                quantity_delta: 7,
                reason: "x".into(),
            }),
        );
        let asset_tx = Transaction::with_id(
            "asset-1",
            TransactionKind::AssetRegistration(TraceableAssetRegistration {
                asset: TraceableAsset {
                    gtin: Some("07891234100016".into()),
                    batch_number: None,
                    expiry_date: None,
                    serial_number: Some("SN-REBUILD".into()),
                    anvisa_registration: None,
                    manufacturer_id: None,
                    product_name: "Dipirona 500mg".into(),
                    custodian_id: "plant-1".into(),
                    country_of_origin: None,
                    storage_temp_celsius: None,
                    quantity: 1,
                },
                event_type: "manufacture".into(),
                originator_id: "plant-1".into(),
                purchase_order_ref: None,
            }),
        );
        let mut block = Block::new(1, vec![tx, asset_tx], genesis.hash);
        block.mine(2);
        storage.put_block(&block).unwrap();

        let node = Node::new_with_storage("n1", "127.0.0.1:0", 2, Arc::clone(&storage));
        Node::rebuild_runtime_state_from_chain(
            &node.ledger,
            &node.state,
            &node.storage,
            &node.provenance,
            &node.flattener,
        )
        .await;

        let restored = node
            .state
            .lock()
            .await
            .watcher
            .inventory_level("SKU", "buyer-1");
        assert_eq!(
            restored, 7,
            "invalid snapshot must fall back to replaying committed inventory updates"
        );

        // The analytics projections must rebuild from the committed chain.
        let canonical_id = "GTIN:07891234100016:SN:SN-REBUILD";
        assert_eq!(
            node.provenance
                .lock()
                .await
                .get_custody_chain(canonical_id)
                .len(),
            1,
            "provenance must rebuild custody chains from persisted blocks"
        );
        assert_eq!(
            node.flattener.lock().await.records().len(),
            1,
            "flattener must rebuild flat records from persisted blocks"
        );
    }

    // ── 7. contract_summaries ─────────────────────────────────────────────────

    #[tokio::test]
    async fn contract_summaries_project_engine_contracts() {
        let node = Node::new("n1", "127.0.0.1:0", 2);

        // Register a contract via a ledger-committed ContractCreation tx.
        let def = SmartContractDef {
            contract_id: "c1".into(),
            buyer_id: "buyer-1".into(),
            product_id: "SKU-1".into(),
            conditions: PurchaseConditions {
                max_price_per_unit: 1000,
                min_quantity: 1,
                max_quantity: 50,
                max_lead_time_days: 5,
                preferred_seller_id: None,
                currency: "USD".into(),
                auto_execute: true,
            },
            wasm_code_b64: None,
        };
        node.submit_transaction(Transaction::new(TransactionKind::ContractCreation(def)))
            .await
            .unwrap();

        // Submit a matching offer so the engine auto-executes a purchase and
        // advances `quantity_purchased` on the live contract.
        let offer = SupplyOffer {
            product_id: "SKU-1".into(),
            product_name: "Widget".into(),
            seller_id: "seller-1".into(),
            quantity_available: 10,
            price_per_unit: 100,
            lead_time_days: 2,
            currency: "USD".into(),
        };
        node.submit_transaction(Transaction::new(TransactionKind::SupplyOffer(offer)))
            .await
            .unwrap();

        let summaries = node.contract_summaries().await;
        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0];
        assert_eq!(summary.id, "c1");
        assert_eq!(summary.buyer_id, "buyer-1");
        assert_eq!(summary.product_id, "SKU-1");
        assert_eq!(summary.status, "Active");
        assert_eq!(summary.quantity_purchased, 10);
        assert_eq!(summary.max_quantity, 50);
    }

    // ── 8. Committed write sets (ticket #41) ────────────────────────────────

    /// A contract that persists one public write and one PDC write.
    fn write_set_wasm_b64() -> String {
        let wat = r#"
(module
  (import "env" "persist_state" (func $persist (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "env" "set_state" (func $set_state (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "ch")
  (data (i32.const 16) "contract-1")
  (data (i32.const 32) "public-key")
  (data (i32.const 48) "public-value")
  (data (i32.const 64) "pdc-key")
  (data (i32.const 80) "secret")
  (data (i32.const 96) "collection-1")
  (data (i32.const 112) "approve")
  (data (i32.const 120) "1")
  (func (export "execute")
    ;; public set: ch / contract-1 / public-key = public-value
    (call $persist (i32.const 0) (i32.const 2) (i32.const 16) (i32.const 10)
                   (i32.const 32) (i32.const 10) (i32.const 48) (i32.const 12)
                   (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0))
    (drop)
    ;; PDC set: ch / contract-1 / pdc-key = secret → collection-1
    (call $persist (i32.const 0) (i32.const 2) (i32.const 16) (i32.const 10)
                   (i32.const 64) (i32.const 7) (i32.const 80) (i32.const 6)
                   (i32.const 0) (i32.const 1) (i32.const 96) (i32.const 12))
    (drop)
    (call $set_state (i32.const 112) (i32.const 7) (i32.const 120) (i32.const 1))
  )
)
"#;
        let wasm = wat::parse_str(wat).expect("fixture WAT must compile");
        BASE64_STANDARD.encode(wasm)
    }

    fn write_set_contract() -> SmartContractDef {
        SmartContractDef {
            contract_id: "c-ws".into(),
            buyer_id: "buyer-1".into(),
            product_id: "SKU-1".into(),
            conditions: PurchaseConditions {
                max_price_per_unit: 1_000,
                min_quantity: 1,
                max_quantity: 10,
                max_lead_time_days: 5,
                preferred_seller_id: None,
                currency: "USD".into(),
                auto_execute: false,
            },
            wasm_code_b64: Some(write_set_wasm_b64()),
        }
    }

    fn write_set_execution() -> Transaction {
        Transaction::new(TransactionKind::ContractExecution(ContractExecution {
            contract_id: "c-ws".into(),
            purchase_order_tx_id: "po-1".into(),
            buyer_id: "buyer-1".into(),
            seller_id: "seller-1".into(),
            product_id: "SKU-1".into(),
            quantity: 5,
            total_price: 5_000,
            currency: "USD".into(),
        }))
    }

    async fn commit_write_set_scenario(
        storage: Arc<dyn StorageProvider>,
    ) -> (Arc<Node>, HashMap<String, Vec<u8>>) {
        let node = Node::new_with_storage("n-ws", "127.0.0.1:0", 1, Arc::clone(&storage));
        node.start(vec![]).await.unwrap();
        node.set_execution_provider(Arc::new(
            WasmExecutionProvider::new().expect("wasmtime must init"),
        ))
        .await;
        // PDC-scoped writes require the `pdc` capability active at the
        // candidate height (ADR-010, ticket #46): activate it for height 2 so
        // the execution block below may carry the PDC write.
        node.submit_transaction(Transaction::with_id(
            "cap:pdc:2".to_owned(),
            TransactionKind::CapabilityActivation(CapabilityActivation {
                capability_id: "pdc".into(),
                version: 1,
                hash: capability_hash("pdc", 1),
                activation_height: 2,
                signatures: vec![RecordSignature {
                    algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
                    signer: "org-gov".into(),
                    signature_bytes: vec![0x42],
                }],
            }),
        ))
        .await
        .unwrap();
        node.mine().await.unwrap();
        node.submit_transaction(Transaction::new(TransactionKind::ContractCreation(
            write_set_contract(),
        )))
        .await
        .unwrap();
        node.submit_transaction(write_set_execution())
            .await
            .unwrap();
        node.mine().await.unwrap();

        let expected = HashMap::from([
            (
                "ws:ch:contract-1:public-key".to_owned(),
                b"public-value".to_vec(),
            ),
            (
                "ws:ch:contract-1:pdc-key".to_owned(),
                sha256(b"secret").into_bytes(),
            ),
        ]);
        (Arc::new(node), expected)
    }

    #[tokio::test]
    async fn committed_block_carries_canonical_write_set_with_pdc_commitment() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let (node, expected) = commit_write_set_scenario(Arc::clone(&storage)).await;

        let block = storage
            .get_block(2)
            .unwrap()
            .expect("execution block committed");
        assert_eq!(block.write_set.len(), 2, "one public + one PDC write");
        // Canonicalized: scope-sorted, deterministic order.
        assert_eq!(block.write_set[0].key, "pdc-key");
        assert_eq!(block.write_set[1].key, "public-key");

        let public_write = &block.write_set[1];
        assert_eq!(
            public_write.op,
            WriteOp::Set(b"public-value".to_vec()),
            "public writes carry their value"
        );

        let pdc_write = &block.write_set[0];
        assert_eq!(
            pdc_write.visibility,
            WriteVisibility::Pdc("collection-1".into())
        );
        let WriteOp::Set(commitment) = &pdc_write.op else {
            panic!("expected a commitment Set");
        };
        assert_eq!(commitment, &sha256(b"secret").into_bytes());
        assert_ne!(
            commitment, b"secret",
            "the private value must never enter the block"
        );

        // The derived cache holds the public value and the PDC commitment.
        assert_eq!(node.world_state().await, expected);
    }

    #[tokio::test]
    async fn restart_rebuilds_world_state_from_committed_write_sets_without_reexecution() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let (_, expected) = commit_write_set_scenario(Arc::clone(&storage)).await;

        // A fresh node over the same storage, with **no** execution provider:
        // the world state must rebuild from the committed write sets alone —
        // no guest code is re-executed anywhere on the rebuild path.
        let restarted = Node::new_with_storage("n-restart", "127.0.0.1:0", 1, storage);
        restarted.start(vec![]).await.unwrap();
        assert_eq!(
            restarted.world_state().await,
            expected,
            "restart rebuild must materialize the same committed state"
        );
    }

    /// A backend that persists the block durably and then fails before the
    /// derived state is applied — the AC3 failure shape.
    struct FailStateApply {
        inner: Arc<dyn StorageProvider>,
    }

    impl StorageProvider for FailStateApply {
        fn put_block(&self, block: &Block) -> Result<(), CoreError> {
            self.inner.put_block(block)
        }
        fn get_block(&self, index: u64) -> Result<Option<Block>, CoreError> {
            self.inner.get_block(index)
        }
        fn latest_block_index(&self) -> Result<Option<u64>, CoreError> {
            self.inner.latest_block_index()
        }
        fn apply_block(&self, block: &Block) -> Result<(), CoreError> {
            // Simulated crash: the block lands durably, then the backend dies
            // before any state application.
            self.inner.put_block(block)?;
            Err(CoreError::Storage(
                "simulated crash after block durability".to_owned(),
            ))
        }
        fn put_state(&self, key: &str, value: &[u8]) -> Result<(), CoreError> {
            self.inner.put_state(key, value)
        }
        fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
            self.inner.get_state(key)
        }
        fn delete_state(&self, key: &str) -> Result<(), CoreError> {
            self.inner.delete_state(key)
        }
        fn name(&self) -> &'static str {
            "fail-state-apply"
        }
    }

    #[tokio::test]
    async fn failure_after_block_durable_is_healed_by_rebuild() {
        let inner: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let failing: Arc<dyn StorageProvider> = Arc::new(FailStateApply {
            inner: Arc::clone(&inner),
        });
        let (node, expected) = commit_write_set_scenario(Arc::clone(&failing)).await;

        // Every apply_block failed after the block write: history is durable,
        // the derived cache and storage state are not.
        assert!(
            node.world_state().await.is_empty(),
            "the derived cache must be empty after the failures"
        );
        assert!(inner
            .get_state("ws:ch:contract-1:public-key")
            .unwrap()
            .is_none());

        // Rebuild consumes the committed write sets in block order — no
        // rollback, no history edit — and heals both projections.
        let healed = Node::rebuild_world_state(&failing).expect("rebuild must succeed");
        assert_eq!(healed, expected);
        assert_eq!(
            inner.get_state("ws:ch:contract-1:public-key").unwrap(),
            Some(b"public-value".to_vec()),
            "rebuild must heal the storage state"
        );
        assert_eq!(
            inner.get_block(2).unwrap().unwrap().write_set.len(),
            2,
            "history must stay untouched"
        );
    }

    #[tokio::test]
    async fn sled_backed_restart_rebuilds_world_state_across_persistence() {
        use glasschain_storage::SledStorageProvider;
        let dir = tempfile::tempdir().expect("temp dir");
        let storage: Arc<dyn StorageProvider> =
            Arc::new(SledStorageProvider::open(dir.path()).expect("sled must open"));
        let (_, expected) = commit_write_set_scenario(Arc::clone(&storage)).await;

        // A genuinely fresh node over the same on-disk directory, without an
        // execution provider: the committed write sets alone must rebuild the
        // world state (persistence + restart rebuild, AC5).
        let restarted = Node::new_with_storage("n-sled-restart", "127.0.0.1:0", 1, storage);
        restarted.start(vec![]).await.unwrap();
        assert_eq!(restarted.world_state().await, expected);
    }

    #[tokio::test]
    async fn failing_execution_accepts_no_writes_and_block_stays_consistent() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorageProvider::new());
        let node = Node::new_with_storage("n-bad-wasm", "127.0.0.1:0", 1, Arc::clone(&storage));
        node.start(vec![]).await.unwrap();
        node.set_execution_provider(Arc::new(
            WasmExecutionProvider::new().expect("wasmtime must init"),
        ))
        .await;

        // A contract whose WASM is not valid base64: execution fails, the
        // transaction accepts no writes, and the block still commits with an
        // empty write set — the same result on every node (deterministic).
        let mut def = write_set_contract();
        def.wasm_code_b64 = Some("not-base64".to_owned());
        node.submit_transaction(Transaction::new(TransactionKind::ContractCreation(def)))
            .await
            .unwrap();
        node.mine().await.unwrap();
        node.submit_transaction(write_set_execution())
            .await
            .unwrap();
        node.mine().await.unwrap();

        let block = storage
            .get_block(2)
            .unwrap()
            .expect("execution block committed");
        assert!(
            block.write_set.is_empty(),
            "a failed execution accepts no writes"
        );
        assert!(node.world_state().await.is_empty());
    }

    // ── 7. enforce_chain_endorsements (sync-path gate, ADR-008 §4) ────────────

    use glasschain_core::{
        capability_hash, CapabilityActivation, EndorserIdentity, PolicyExpression, PolicyUpdate,
        RecordSignature, ScopedPolicies, ScopedTarget,
    };
    use glasschain_identity::MspEndorsementProvider;

    fn chain_activation_tx(height: u64) -> Transaction {
        Transaction::with_id(
            format!("cap:endorsement:{height}"),
            TransactionKind::CapabilityActivation(CapabilityActivation {
                capability_id: "endorsement".into(),
                version: 1,
                hash: capability_hash("endorsement", 1),
                activation_height: height,
                signatures: vec![RecordSignature {
                    algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
                    signer: "governance".into(),
                    signature_bytes: vec![0x42],
                }],
            }),
        )
    }

    fn chain_policy_update_tx(signer: Option<&Identity>) -> Transaction {
        let mut tx = Transaction::new(TransactionKind::PolicyUpdate(PolicyUpdate {
            channel: "supply".into(),
            contract: String::new(),
            policies: ScopedPolicies {
                channel_default: PolicyExpression::signed_by("org-a"),
                contract_default: None,
                collection_policy: None,
                key_policies: Vec::new(),
            },
        }));
        if let Some(identity) = signer {
            let payload = glasschain_core::TransactionEndorsement::payload(&tx).unwrap();
            tx.endorsements
                .push(glasschain_core::TransactionEndorsement {
                    target: ScopedTarget {
                        channel: "supply".into(),
                        contract: String::new(),
                        keys: Vec::new(),
                        collection: None,
                    },
                    signers: vec![EndorserIdentity {
                        algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
                        claimed_principal: glasschain_core::Principal::new("network-governance"),
                        public_key: identity.public_key_bytes().to_vec(),
                        signature: identity.sign_bytes(&payload),
                    }],
                });
        }
        tx
    }

    fn candidate_chain(second_tx: Transaction) -> Vec<Block> {
        let genesis = Ledger::new(1).chain.remove(0);
        let mut b1 = Block::new(1, vec![chain_activation_tx(2)], genesis.hash.clone());
        b1.mine(1);
        let mut b2 = Block::new(2, vec![second_tx], b1.hash.clone());
        b2.mine(1);
        vec![genesis, b1, b2]
    }

    #[tokio::test]
    async fn sync_gate_rejects_an_unsigned_policy_update_in_a_candidate_chain() {
        let node = Node::new("n-sync", "127.0.0.1:0", 1);
        let mut msp = MspEndorsementProvider::new();
        let gov = Identity::generate("gov");
        msp.register_identity(&gov, glasschain_core::Principal::new("network-governance"));
        node.set_endorsement_provider(Arc::new(msp)).await;

        let candidate = candidate_chain(chain_policy_update_tx(None));
        let error = Node::enforce_chain_endorsements(&node.state, &candidate)
            .await
            .expect_err("an unsigned policy update must reject the candidate chain");
        assert!(error.to_string().contains("no endorsement"), "{error}");
    }

    #[tokio::test]
    async fn sync_gate_accepts_a_fully_endorsed_candidate_chain() {
        let node = Node::new("n-sync", "127.0.0.1:0", 1);
        let mut msp = MspEndorsementProvider::new();
        let gov = Identity::generate("gov");
        msp.register_identity(&gov, glasschain_core::Principal::new("network-governance"));
        node.set_endorsement_provider(Arc::new(msp)).await;

        let candidate = candidate_chain(chain_policy_update_tx(Some(&gov)));
        Node::enforce_chain_endorsements(&node.state, &candidate)
            .await
            .expect("a signed policy update must pass the sync gate");
    }
}
