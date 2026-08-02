use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid realm: {0}")]
    InvalidRealm(String),
    #[error("database belongs to realm '{actual}', not '{expected}'")]
    RealmMismatch { expected: String, actual: String },
    #[error("invalid service graph: {0}")]
    InvalidServiceGraph(String),
    #[error("runtime transition from {from} to {to} is not allowed")]
    InvalidTransition { from: String, to: String },
    #[error("invalid operation: {0}")]
    InvalidOperation(String),
    #[error("revision conflict: expected {expected}, current revision is {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("latest snapshot failed checksum validation")]
    ChecksumMismatch,
    #[error("runtime state lock is poisoned")]
    LockPoisoned,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

impl From<rusqlite::Error> for CoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error.to_string())
    }
}
