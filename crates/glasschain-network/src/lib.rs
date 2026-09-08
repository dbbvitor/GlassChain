// `multiple_crate_versions` fires on duplicate `pem`/`yasna`/`rcgen` versions
// pulled in by libp2p's *optional* `tls` feature (libp2p-tls 0.6 → rcgen 0.13 →
// pem 3 / yasna 0.5) even though this crate never enables that feature and
// builds its own rustls transport; rcgen 0.14 needs pem 4 / yasna 0.6.
#![allow(clippy::multiple_crate_versions)]

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
pub use node::{ContractSummary, Node, NodeEvent};
pub use peer::{PeerConnection, PeerReader, PeerWriter};
pub use protocol::{Message, PROTOCOL_VERSION};
