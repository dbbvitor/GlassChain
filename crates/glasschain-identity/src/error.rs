use thiserror::Error;

/// Errors produced by the `glasschain-identity` crate.
#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("certificate generation failed: {0}")]
    CertGen(String),

    #[error("signature verification failed")]
    VerificationFailed,

    #[error("invalid public key bytes")]
    InvalidPublicKey,

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("identity not found: {0}")]
    NotFound(String),

    #[error("channel error: {0}")]
    Channel(String),
}
