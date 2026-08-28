//! The `Event` half of the Corda Action/Event/TransitionResult algebra: the
//! inputs a flow reacts to.

use glasschain_core::{CanonicalRecord, TransactionKind};
use serde::{Deserialize, Serialize};

/// An input to a flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// A canonical v1 record committed to the ledger.
    RecordCommitted(CanonicalRecord),

    /// A legacy transaction committed to the ledger (supply offer, inventory
    /// update, autonomous purchase order, …).
    TransactionCommitted(TransactionKind),

    /// The flow was woken after an interruption (restart, triage).  The string
    /// carries the reason for operators.
    Resumed(String),
}
