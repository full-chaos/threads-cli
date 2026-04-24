use thiserror::Error;

/// Errors produced by the store layer.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration: {0}")]
    Migration(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

impl From<StoreError> for threads_core::Error {
    fn from(err: StoreError) -> Self {
        threads_core::Error::Store(err.to_string())
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, StoreError>;
