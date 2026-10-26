//! Shared application rules and local infrastructure for Domus Obscura.
//!
//! Domain policy has no database or filesystem dependency. The application layer
//! coordinates observations; infrastructure owns SQLite and filesystem access.
pub mod application;
pub mod domain;
pub mod infrastructure;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    ArtworkInput(#[from] domain::ArtworkIssue),
    #[error("artwork needs user input; question {0} is saved")]
    ArtworkRequired(i64),
    #[error("job stopped at a safe boundary; inspect job control {0}")]
    JobStopped(i64),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Busy(&'static str),
    #[error("authentication failed")]
    Authentication,
    #[error("{0}")]
    Remote(&'static str),
    #[error("{reason}; retry after {retry_after_seconds} seconds")]
    Catalog {
        reason: &'static str,
        retry_after_seconds: u32,
    },
    #[error("{0}")]
    Internal(&'static str),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
