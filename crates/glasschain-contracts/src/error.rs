use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("contract not found: {0}")]
    NotFound(String),

    #[error("contract already exists: {0}")]
    AlreadyExists(String),

    #[error("contract {0} is no longer active")]
    Inactive(String),

    #[error("conditions not met: {0}")]
    ConditionsNotMet(String),

    #[error("core error: {0}")]
    Core(#[from] glasschain_core::CoreError),
}
