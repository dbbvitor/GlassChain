// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
pub mod error;
pub mod libp2p_swarm;
pub mod node;
pub mod peer;
pub mod protocol;
pub mod rounds;

pub use error::NetworkError;
pub use libp2p_swarm::{
    LibP2pConfig, LibP2pNode, SwarmCommand, SwarmNodeEvent, TOPIC_BLOCKS, TOPIC_TRANSACTIONS,
};
pub use node::{
    adopt_prebound_listener, stash_prebound_listener, ContractSummary, Node, NodeEvent,
    PendingPoolStats,
};
pub use peer::{PeerConnection, PeerReader, PeerWriter};
pub use protocol::{Message, PROTOCOL_VERSION};
