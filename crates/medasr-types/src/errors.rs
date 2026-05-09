use thiserror::Error;

/// Top-level error type returned across crate boundaries. Per-crate errors
/// flatten into this via `From` impls in their owning crates.
#[derive(Debug, Error)]
pub enum Error {
    #[error("audio: {0}")]
    Audio(String),

    #[error("asr: {0}")]
    Asr(String),

    #[error("inject: {0}")]
    Inject(String),

    #[error("focus: {0}")]
    Focus(String),

    #[error("model: {0}")]
    Model(String),

    #[error("permissions: {0}")]
    Permissions(String),

    #[error("audit: {0}")]
    Audit(String),

    #[error("settings: {0}")]
    Settings(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
