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
    #[error("worker is stopping at a safe boundary")]
    WorkerStopping,
    #[error("worker request is waiting for its saved decision and explicit recovery action")]
    WorkerWaiting,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Busy(&'static str),
    #[error(
        "provider request is active; completion time is unknown; retry metadata manually later"
    )]
    ProviderActive,
    #[error("provider request capacity is full; retry metadata manually later")]
    ProviderSaturated,
    #[error("authentication failed")]
    Authentication,
    #[error("{0}")]
    Remote(&'static str),
    #[error("{reason}; {advice}", advice = catalog_retry_advice(*retry_after_seconds))]
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
    #[error("Soulseek transport error: {0}")]
    SoulseekTransport(std::io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

fn catalog_retry_advice(seconds: u32) -> String {
    if seconds == u32::MAX {
        "provider requests require explicit review before resuming".into()
    } else {
        format!("wait at least {seconds} seconds, then retry metadata manually")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_pause_display_never_uses_an_unknown_duration_as_a_countdown() {
        let paused = Error::Catalog {
            reason: "provider requests are paused",
            retry_after_seconds: u32::MAX,
        }
        .to_string();
        assert!(paused.contains("explicit review"));
        assert!(!paused.contains(&u32::MAX.to_string()));
        let timed = Error::Catalog {
            reason: "provider budget is busy",
            retry_after_seconds: 120,
        }
        .to_string();
        assert!(timed.contains("wait at least 120 seconds"));
        assert!(timed.contains("retry metadata manually"));
    }
}
