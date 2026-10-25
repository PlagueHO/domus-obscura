use serde::{Deserialize, Serialize};

/// Durable progress, not a description inferred from an in-memory transfer.
/// Backup progress is separate: a verified primary can already be usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Searching,
    Downloading,
    Validating,
    Normalizing,
    Publishing,
    AwaitingInput,
    BackupPending,
    Failed,
    Cancelled,
    Complete,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Searching => "searching",
            Self::Downloading => "downloading",
            Self::Validating => "validating",
            Self::Normalizing => "normalizing",
            Self::Publishing => "publishing",
            Self::AwaitingInput => "awaiting_input",
            Self::BackupPending => "backup_pending",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Complete => "complete",
        }
    }

    /// Waiting, failure and cancellation need an application-specific resume
    /// decision. They must not bypass validation or publication reconciliation.
    pub fn can_advance_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Searching)
                | (Self::Searching, Self::Downloading)
                | (Self::Downloading, Self::Validating)
                | (Self::Validating, Self::Normalizing)
                | (Self::Normalizing, Self::Publishing)
                | (Self::Publishing, Self::BackupPending | Self::Complete)
                | (Self::BackupPending, Self::Complete)
        ) || (matches!(next, Self::AwaitingInput | Self::Failed | Self::Cancelled)
            && !matches!(
                self,
                Self::Complete | Self::Cancelled | Self::Failed | Self::Publishing
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_cannot_be_cancelled_without_reconciliation() {
        assert!(!JobState::Publishing.can_advance_to(JobState::Cancelled));
        assert!(JobState::Publishing.can_advance_to(JobState::BackupPending));
    }

    #[test]
    fn completion_requires_publication_or_verified_backup_work() {
        assert!(!JobState::Downloading.can_advance_to(JobState::Complete));
        assert!(!JobState::AwaitingInput.can_advance_to(JobState::Publishing));
        assert!(!JobState::Complete.can_advance_to(JobState::Queued));
    }
}
