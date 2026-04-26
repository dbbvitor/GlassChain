use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid block: {0}")]
    InvalidBlock(String),

    #[error("invalid transaction: {0}")]
    InvalidTransaction(String),

    #[error("ledger is empty")]
    EmptyLedger,

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("gas exhausted after {used} units (limit: {limit})")]
    GasExhausted { used: u64, limit: u64 },

    #[error("execution error: {0}")]
    Execution(String),

    #[error("storage error: {0}")]
    Storage(String),
}
