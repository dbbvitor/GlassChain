//! # GlassChain libp2p Swarm
//!
//! This module implements a Kademlia DHT + Gossipsub peer-to-peer networking
//! layer for GlassChain using [`libp2p`] as the underlying transport and
//! protocol stack. It runs in parallel with the legacy TLS/TCP transport and
//! is designed to progressively replace it.
//!
//! ## Transport Stack
//!
//! ```text
//! TCP (tokio)
//!   └── Noise (encrypted handshake / authenticated key exchange)
//!         └── Yamux (multiplexed bidirectional streams)
//! ```
//!
//! ## Behaviour Composition
//!
//! | Layer      | Purpose                                                      |
//! |------------|--------------------------------------------------------------|
//! | Gossipsub  | Epidemic pub/sub for propagating transactions and blocks     |
//! | Kademlia   | Distributed hash table for peer routing and discovery        |
//! | Identify   | Exchanges public keys and listen addresses on every connect  |
//! | mDNS       | Zero-configuration local-network peer discovery              |
//!
//! ## Gossipsub Topics
//!
//! * [`TOPIC_TRANSACTIONS`] (`glasschain/transactions`) — new unconfirmed
//!   [`Transaction`]s, JSON-encoded via [`serde_json`].
//! * [`TOPIC_BLOCKS`] (`glasschain/blocks`) — newly mined [`Block`]s,
//!   JSON-encoded via [`serde_json`].
//!
//! ## Kademlia DHT
//!
//! Each node runs Kademlia in **Server** mode so it actively participates in
//! routing queries from other peers. Bootstrap peers supplied via
//! [`LibP2pConfig::bootstrap_peers`] are seeded into the routing table at
//! startup. Once connected, the Identify protocol keeps the routing table
//! updated with fresh listen addresses.
//!
//! ## Event Loop
//!
//! [`LibP2pNode::new`] spawns a background Tokio task that drives the swarm
//! event loop. Communication with the caller happens over two bounded MPSC
//! channels:
//!
//! * [`SwarmCommand`]   — caller → swarm (dial, publish, shutdown)
//! * [`SwarmNodeEvent`] — swarm  → caller (peer events, received messages)
//!
//! Poll for inbound events using [`LibP2pNode::try_recv_event`] (non-blocking)
//! or by locking [`LibP2pNode::event_rx`] directly.

#![allow(clippy::module_name_repetitions)]

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use glasschain_core::{Block, Transaction};
use libp2p::{
    gossipsub, identify, kad, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use tokio::sync::{mpsc, Mutex, RwLock};

// ── Topic constants ───────────────────────────────────────────────────────────

/// Gossipsub topic name for broadcasting new unconfirmed [`Transaction`]s.
pub const TOPIC_TRANSACTIONS: &str = "glasschain/transactions";

/// Gossipsub topic name for broadcasting newly mined [`Block`]s.
pub const TOPIC_BLOCKS: &str = "glasschain/blocks";

// ── Network Behaviour ─────────────────────────────────────────────────────────

/// Combined libp2p [`NetworkBehaviour`] for every GlassChain swarm node.
///
/// The four sub-behaviours are composed via the derive macro, which generates
/// the `GlasschainBehaviourEvent` dispatch enum automatically — one variant per
/// field, named after the field in `PascalCase`.
#[derive(NetworkBehaviour)]
pub struct GlasschainBehaviour {
    /// Gossipsub pub/sub overlay for propagating transactions and blocks.
    pub gossipsub: gossipsub::Behaviour,
    /// Kademlia DHT — distributed peer routing, discovery, and address storage.
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    /// Identify protocol — exchanges keypairs and listen addresses on connect.
    pub identify: identify::Behaviour,
    /// mDNS — zero-configuration peer discovery on the local network segment.
    pub mdns: mdns::tokio::Behaviour,
}

// ── Command / Event types ─────────────────────────────────────────────────────

/// Commands sent **to** the background swarm task from the caller.
///
/// Each variant is dispatched inside the event loop via an MPSC channel.
/// Use the async helper methods on [`LibP2pNode`] to enqueue commands rather
/// than sending to the channel directly.
#[derive(Debug)]
pub enum SwarmCommand {
    /// Connect to a remote peer at the given multiaddress.
    Dial(Multiaddr),
    /// JSON-encode and gossipsub-publish a new [`Transaction`].
    PublishTransaction(Transaction),
    /// JSON-encode and gossipsub-publish a newly mined [`Block`].
    PublishBlock(Block),
    /// Register a known peer's address in the Kademlia routing table.
    AddKnownPeer(PeerId, Multiaddr),
    /// Gracefully stop the event loop and terminate the background task.
    Shutdown,
}

/// Events emitted **from** the background swarm task to the caller.
///
/// Retrieve events via [`LibP2pNode::try_recv_event`] (non-blocking) or by
/// locking [`LibP2pNode::event_rx`] and calling `recv().await`.
#[derive(Debug, Clone)]
pub enum SwarmNodeEvent {
    /// A new peer-to-peer connection has been established.
    PeerConnected(PeerId),
    /// A previously established peer connection has been closed.
    PeerDisconnected(PeerId),
    /// A [`Transaction`] arrived from the network via Gossipsub.
    TransactionReceived(Transaction),
    /// A [`Block`] arrived from the network via Gossipsub.
    BlockReceived(Block),
    /// The Kademlia routing table received an update.
    RoutingTableUpdated,
    /// A non-fatal error occurred inside the swarm event loop.
    Error(String),
}

// ── Configuration ─────────────────────────────────────────────────────────────

/// Configuration passed to [`LibP2pNode::new`] to set up the libp2p swarm.
#[derive(Debug, Clone)]
pub struct LibP2pConfig {
    /// Multiaddress this node will listen on.
    ///
    /// # Examples
    ///
    /// * `/ip4/0.0.0.0/tcp/9000` — listen on all IPv4 interfaces, port 9000
    /// * `/ip6/::/tcp/9001`       — listen on all IPv6 interfaces, port 9001
    pub listen_addr: Multiaddr,

    /// Known bootstrap peers used to seed the Kademlia routing table at
    /// startup.
    ///
    /// Each entry is a `(PeerId, Multiaddr)` pair. At least one bootstrap peer
    /// is required on non-local networks; mDNS covers the local-network case
    /// automatically.
    pub bootstrap_peers: Vec<(PeerId, Multiaddr)>,
}

// ── Node handle ───────────────────────────────────────────────────────────────

/// A running libp2p node participating in the GlassChain P2P network.
///
/// `LibP2pNode::new` constructs the swarm, binds the listen address, and
/// spawns a background Tokio task that drives the event loop. All further
/// interaction happens via the async methods below.
///
/// # Example
///
/// ```no_run
/// use glasschain_network::libp2p_swarm::{LibP2pConfig, LibP2pNode};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = LibP2pConfig {
///     listen_addr: "/ip4/0.0.0.0/tcp/9000".parse()?,
///     bootstrap_peers: vec![],
/// };
/// let node = LibP2pNode::new(config)?;
/// println!("Local peer id: {}", node.local_peer_id);
/// # Ok(())
/// # }
/// ```
pub struct LibP2pNode {
    /// Sender half for [`SwarmCommand`]s dispatched to the swarm task.
    command_tx: mpsc::Sender<SwarmCommand>,

    /// Shared receiver for [`SwarmNodeEvent`]s emitted by the swarm task.
    ///
    /// Wrapped in `Arc<Mutex<…>>` so ownership can be shared without cloning
    /// the receiver itself (which is not `Clone`).
    pub event_rx: Arc<Mutex<mpsc::Receiver<SwarmNodeEvent>>>,

    /// The [`PeerId`] derived from the node's freshly generated keypair.
    pub local_peer_id: PeerId,

    /// Live set of currently connected peer IDs maintained by the swarm task.
    known_peers: Arc<RwLock<HashSet<PeerId>>>,
}

impl LibP2pNode {
    /// Build a new `LibP2pNode`, start the libp2p swarm, and spawn the
    /// background event-loop task.
    ///
    /// The constructor is synchronous but requires an active Tokio runtime
    /// (it calls [`tokio::spawn`] internally). Call it from within an
    /// `async fn` or from a `#[tokio::main]` context.
    ///
    /// # Errors
    ///
    /// Returns `Err` if any of the following fail:
    ///
    /// * TCP transport cannot be constructed.
    /// * Gossipsub configuration or behaviour initialisation fails.
    /// * mDNS socket cannot be opened.
    /// * `config.listen_addr` cannot be bound.
    #[allow(clippy::too_many_lines, clippy::cognitive_complexity, clippy::pedantic)]
    pub fn new(config: LibP2pConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // ── Shared error type used inside the with_behaviour closure ──────────
        type BehErr = Box<dyn std::error::Error + Send + Sync + 'static>;

        // ── Channels ──────────────────────────────────────────────────────────
        let (command_tx, mut command_rx) = mpsc::channel::<SwarmCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<SwarmNodeEvent>(256);

        let known_peers: Arc<RwLock<HashSet<PeerId>>> = Arc::new(RwLock::new(HashSet::new()));
        let known_peers_task = Arc::clone(&known_peers);

        // Pre-compute topic hashes once; cheap `==` comparisons in the hot loop.
        let tx_topic_hash = gossipsub::IdentTopic::new(TOPIC_TRANSACTIONS).hash();
        let blk_topic_hash = gossipsub::IdentTopic::new(TOPIC_BLOCKS).hash();

        // ── Build the libp2p Swarm ─────────────────────────────────────────────
        let mut swarm = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| -> Result<GlasschainBehaviour, BehErr> {
                // ── Gossipsub ─────────────────────────────────────────────────
                let gs_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(10))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .build()
                    .map_err(|e| -> BehErr { e.into() })?;

                let mut gossipsub_beh = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gs_config,
                )
                .map_err(|e| -> BehErr { e.into() })?;

                gossipsub_beh
                    .subscribe(&gossipsub::IdentTopic::new(TOPIC_TRANSACTIONS))
                    .map_err(|e| -> BehErr { e.to_string().into() })?;

                gossipsub_beh
                    .subscribe(&gossipsub::IdentTopic::new(TOPIC_BLOCKS))
                    .map_err(|e| -> BehErr { e.to_string().into() })?;

                // ── Kademlia DHT ──────────────────────────────────────────────
                let local_peer_id = key.public().to_peer_id();
                let kad_store = kad::store::MemoryStore::new(local_peer_id);
                let mut kademlia = kad::Behaviour::new(local_peer_id, kad_store);
                // Run in Server mode so this node participates in routing queries.
                kademlia.set_mode(Some(kad::Mode::Server));

                // ── Identify ──────────────────────────────────────────────────
                let identify = identify::Behaviour::new(identify::Config::new(
                    "/glasschain/1.0.0".to_string(),
                    key.public(),
                ));

                // ── mDNS ──────────────────────────────────────────────────────
                let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)
                    .map_err(|e| -> BehErr { e.into() })?;

                Ok(GlasschainBehaviour {
                    gossipsub: gossipsub_beh,
                    kademlia,
                    identify,
                    mdns,
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        let local_peer_id = *swarm.local_peer_id();

        // Bind to the configured listen address.
        swarm.listen_on(config.listen_addr)?;

        // Seed the Kademlia routing table with pre-configured bootstrap peers.
        for (peer_id, addr) in &config.bootstrap_peers {
            swarm
                .behaviour_mut()
                .kademlia
                .add_address(peer_id, addr.clone());
        }

        // ── Background event loop ──────────────────────────────────────────────
        tokio::spawn(async move {
            log::info!(
                "GlassChain libp2p swarm running (local_peer_id={})",
                swarm.local_peer_id()
            );

            loop {
                tokio::select! {
                    // ── Swarm → caller ──────────────────────────────────────
                    event = swarm.select_next_some() => {
                        match event {
                            // ── Transport events ────────────────────────────
                            SwarmEvent::NewListenAddr { address, .. } => {
                                log::info!("Swarm listening on {address}");
                            }

                            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                                log::info!("Peer connected: {peer_id}");
                                known_peers_task.write().await.insert(peer_id);
                                let _ = event_tx
                                    .send(SwarmNodeEvent::PeerConnected(peer_id))
                                    .await;
                            }

                            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                                log::info!("Peer disconnected: {peer_id}");
                                known_peers_task.write().await.remove(&peer_id);
                                let _ = event_tx
                                    .send(SwarmNodeEvent::PeerDisconnected(peer_id))
                                    .await;
                            }

                            // ── Gossipsub: inbound message ───────────────────
                            SwarmEvent::Behaviour(
                                GlasschainBehaviourEvent::Gossipsub(
                                    gossipsub::Event::Message { message, .. },
                                ),
                            ) => {
                                if message.topic == tx_topic_hash {
                                    match serde_json::from_slice::<Transaction>(
                                        &message.data,
                                    ) {
                                        Ok(tx) => {
                                            log::debug!(
                                                "Gossipsub: received transaction {}",
                                                tx.id
                                            );
                                            let _ = event_tx
                                                .send(SwarmNodeEvent::TransactionReceived(tx))
                                                .await;
                                        }
                                        Err(e) => {
                                            log::warn!(
                                                "Gossipsub: failed to decode transaction: {e}"
                                            );
                                            let _ = event_tx
                                                .send(SwarmNodeEvent::Error(format!(
                                                    "Failed to decode transaction: {e}"
                                                )))
                                                .await;
                                        }
                                    }
                                } else if message.topic == blk_topic_hash {
                                    match serde_json::from_slice::<Block>(&message.data) {
                                        Ok(block) => {
                                            log::debug!(
                                                "Gossipsub: received block index={}",
                                                block.index
                                            );
                                            let _ = event_tx
                                                .send(SwarmNodeEvent::BlockReceived(block))
                                                .await;
                                        }
                                        Err(e) => {
                                            log::warn!(
                                                "Gossipsub: failed to decode block: {e}"
                                            );
                                            let _ = event_tx
                                                .send(SwarmNodeEvent::Error(format!(
                                                    "Failed to decode block: {e}"
                                                )))
                                                .await;
                                        }
                                    }
                                } else {
                                    log::debug!(
                                        "Gossipsub: message on unrecognised topic {:?}",
                                        message.topic
                                    );
                                }
                            }

                            // ── Kademlia events ──────────────────────────────
                            SwarmEvent::Behaviour(
                                GlasschainBehaviourEvent::Kademlia(kad_event),
                            ) => {
                                log::debug!("Kademlia event: {kad_event:?}");
                                let _ = event_tx
                                    .send(SwarmNodeEvent::RoutingTableUpdated)
                                    .await;
                            }

                            // ── Identify: refresh Kademlia addresses ─────────
                            SwarmEvent::Behaviour(
                                GlasschainBehaviourEvent::Identify(
                                    identify::Event::Received { peer_id, info, .. },
                                ),
                            ) => {
                                log::debug!(
                                    "Identified peer {peer_id}: {} listen addr(s)",
                                    info.listen_addrs.len()
                                );
                                for addr in info.listen_addrs {
                                    swarm
                                        .behaviour_mut()
                                        .kademlia
                                        .add_address(&peer_id, addr);
                                }
                            }

                            // ── mDNS: newly discovered local peers ───────────
                            SwarmEvent::Behaviour(
                                GlasschainBehaviourEvent::Mdns(
                                    mdns::Event::Discovered(peers),
                                ),
                            ) => {
                                for (peer_id, addr) in peers {
                                    log::info!(
                                        "mDNS: discovered {peer_id} at {addr}"
                                    );
                                    swarm
                                        .behaviour_mut()
                                        .kademlia
                                        .add_address(&peer_id, addr);
                                    if let Err(e) = swarm.dial(peer_id) {
                                        log::warn!(
                                            "mDNS: failed to dial {peer_id}: {e}"
                                        );
                                    }
                                }
                            }

                            // ── mDNS: expired local peers ────────────────────
                            SwarmEvent::Behaviour(
                                GlasschainBehaviourEvent::Mdns(
                                    mdns::Event::Expired(peers),
                                ),
                            ) => {
                                for (peer_id, addr) in &peers {
                                    log::debug!("mDNS: {peer_id} expired at {addr}");
                                }
                            }

                            other => {
                                log::debug!("Unhandled swarm event: {other:?}");
                            }
                        }
                    }

                    // ── Caller → swarm ──────────────────────────────────────
                    cmd = command_rx.recv() => {
                        match cmd {
                            Some(SwarmCommand::Dial(addr)) => {
                                log::info!("Dialling {addr}");
                                if let Err(e) = swarm.dial(addr) {
                                    log::warn!("Dial error: {e}");
                                }
                            }

                            Some(SwarmCommand::PublishTransaction(tx)) => {
                                match serde_json::to_vec(&tx) {
                                    Ok(data) => {
                                        let topic =
                                            gossipsub::IdentTopic::new(TOPIC_TRANSACTIONS);
                                        match swarm
                                            .behaviour_mut()
                                            .gossipsub
                                            .publish(topic, data)
                                        {
                                            Ok(_) => {
                                                log::debug!(
                                                    "Published transaction {} via gossipsub",
                                                    tx.id
                                                );
                                            }
                                            Err(e) => {
                                                log::warn!(
                                                    "Gossipsub publish transaction {}: {e}",
                                                    tx.id
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "Failed to serialise transaction {}: {e}",
                                            tx.id
                                        );
                                    }
                                }
                            }

                            Some(SwarmCommand::PublishBlock(block)) => {
                                match serde_json::to_vec(&block) {
                                    Ok(data) => {
                                        let topic =
                                            gossipsub::IdentTopic::new(TOPIC_BLOCKS);
                                        match swarm
                                            .behaviour_mut()
                                            .gossipsub
                                            .publish(topic, data)
                                        {
                                            Ok(_) => {
                                                log::debug!(
                                                    "Published block {} via gossipsub",
                                                    block.index
                                                );
                                            }
                                            Err(e) => {
                                                log::warn!(
                                                    "Gossipsub publish block {}: {e}",
                                                    block.index
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "Failed to serialise block {}: {e}",
                                            block.index
                                        );
                                    }
                                }
                            }

                            Some(SwarmCommand::AddKnownPeer(peer_id, addr)) => {
                                log::info!(
                                    "Adding peer {peer_id} at {addr} to Kademlia routing table"
                                );
                                swarm
                                    .behaviour_mut()
                                    .kademlia
                                    .add_address(&peer_id, addr);
                            }

                            Some(SwarmCommand::Shutdown) | None => {
                                log::info!("GlassChain libp2p swarm shutting down");
                                break;
                            }
                        }
                    }
                }
            }

            log::info!("GlassChain libp2p swarm event loop exited");
        });

        Ok(Self {
            command_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
            local_peer_id,
            known_peers,
        })
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Dial a remote peer by its multiaddress.
    ///
    /// The dial attempt is dispatched asynchronously to the swarm task; any
    /// transport-level errors are logged inside the event loop.
    pub async fn dial(&self, addr: Multiaddr) {
        if let Err(e) = self.command_tx.send(SwarmCommand::Dial(addr)).await {
            log::warn!("Failed to enqueue Dial command: {e}");
        }
    }

    /// Gossipsub-publish a [`Transaction`] to all subscribed peers.
    ///
    /// The transaction is JSON-encoded and sent on the
    /// [`TOPIC_TRANSACTIONS`] topic. If no mesh peers are subscribed, the
    /// publish will fail silently with a warning log.
    pub async fn publish_transaction(&self, tx: Transaction) {
        if let Err(e) = self
            .command_tx
            .send(SwarmCommand::PublishTransaction(tx))
            .await
        {
            log::warn!("Failed to enqueue PublishTransaction command: {e}");
        }
    }

    /// Gossipsub-publish a [`Block`] to all subscribed peers.
    ///
    /// The block is JSON-encoded and sent on the [`TOPIC_BLOCKS`] topic.
    pub async fn publish_block(&self, block: Block) {
        if let Err(e) = self
            .command_tx
            .send(SwarmCommand::PublishBlock(block))
            .await
        {
            log::warn!("Failed to enqueue PublishBlock command: {e}");
        }
    }

    /// Register a known peer's address in the Kademlia routing table.
    ///
    /// Useful when bootstrap addresses are learned out-of-band (e.g. from a
    /// configuration file or a registry service).
    pub async fn add_known_peer(&self, peer_id: PeerId, addr: Multiaddr) {
        if let Err(e) = self
            .command_tx
            .send(SwarmCommand::AddKnownPeer(peer_id, addr))
            .await
        {
            log::warn!("Failed to enqueue AddKnownPeer command: {e}");
        }
    }

    /// Non-blocking poll for the next [`SwarmNodeEvent`].
    ///
    /// Returns `None` immediately if no event is currently available in the
    /// channel buffer. Suitable for integration into a polling loop alongside
    /// other work.
    pub async fn try_recv_event(&self) -> Option<SwarmNodeEvent> {
        self.event_rx.lock().await.try_recv().ok()
    }

    /// Return a snapshot of the currently connected peer IDs.
    ///
    /// The returned `Vec` is a point-in-time copy; it may be stale by the time
    /// the caller inspects it.
    pub async fn known_peers(&self) -> Vec<PeerId> {
        self.known_peers.read().await.iter().copied().collect()
    }

    /// Send a [`SwarmCommand::Shutdown`] to gracefully stop the background
    /// event-loop task.
    ///
    /// After shutdown the node's channels are closed; further calls to any
    /// method will log warnings but will not panic.
    pub async fn shutdown(&self) {
        if let Err(e) = self.command_tx.send(SwarmCommand::Shutdown).await {
            log::warn!("Failed to enqueue Shutdown command: {e}");
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that a [`LibP2pConfig`] can be constructed with a valid multiaddr
    /// and that both fields are stored correctly.
    #[test]
    fn test_lib_p2p_config_default() {
        let addr: Multiaddr = "/ip4/0.0.0.0/tcp/9000"
            .parse()
            .expect("valid multiaddr string");

        let config = LibP2pConfig {
            listen_addr: addr.clone(),
            bootstrap_peers: vec![],
        };
        assert_eq!(config.listen_addr, addr);
        assert!(config.bootstrap_peers.is_empty());
    }

    /// Verify that the `Debug` impl on [`SwarmCommand`] formats each variant
    /// correctly so it can be inspected in logs and test output.
    #[test]
    fn test_swarm_command_debug() {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001"
            .parse()
            .expect("valid multiaddr string");

        let dial_cmd = SwarmCommand::Dial(addr);
        assert!(format!("{dial_cmd:?}").contains("Dial"));

        let shutdown_cmd = SwarmCommand::Shutdown;
        assert_eq!(format!("{shutdown_cmd:?}"), "Shutdown");

        let add_cmd = SwarmCommand::AddKnownPeer(
            PeerId::random(),
            "/ip4/10.0.0.1/tcp/4001"
                .parse()
                .expect("valid multiaddr string"),
        );
        assert!(format!("{add_cmd:?}").contains("AddKnownPeer"));
    }

    /// Verify that [`SwarmNodeEvent`] variants can be cloned and that the
    /// `Debug` output reflects the cloned value correctly.
    #[test]
    fn test_swarm_node_event_clone() {
        let peer_id = PeerId::random();

        let connected = SwarmNodeEvent::PeerConnected(peer_id);
        let cloned = connected.clone();
        assert!(format!("{cloned:?}").contains("PeerConnected"));

        let disconnected = SwarmNodeEvent::PeerDisconnected(peer_id);
        let cloned = disconnected.clone();
        assert!(format!("{cloned:?}").contains("PeerDisconnected"));

        let routing = SwarmNodeEvent::RoutingTableUpdated;
        let cloned = routing.clone();
        assert!(format!("{cloned:?}").contains("RoutingTableUpdated"));

        let error = SwarmNodeEvent::Error("test error message".to_string());
        let cloned = error.clone();
        assert!(format!("{cloned:?}").contains("test error message"));
    }
}
