use super::{JobClaim, Store, TransferFile, check_job_owner, planning};
use crate::domain::{JobState, Page};
use crate::{Error, Result};
use rusqlite::{TransactionBehavior, params};

/// Evidence never authorizes a fresh queue submission or a different source.
#[derive(Debug, Clone, Copy)]
pub enum TransferObservation {
    Submitted,
    Queued(u32),
    Offered(u32),
    Denied,
    Interrupted,
    Sealed,
}

impl TransferObservation {
    fn fields(self) -> (&'static str, u32) {
        match self {
            Self::Submitted => ("submitted", 0),
            Self::Queued(place) => ("queued", place),
            Self::Offered(token) => ("offered", token),
            Self::Denied => ("denied", 0),
            Self::Interrupted => ("interrupted", 0),
            Self::Sealed => ("sealed", 0),
        }
    }
}

#[derive(Debug)]
pub struct TransferEvidence {
    pub id: i64,
    pub claim_generation: i64,
    pub session_generation: i64,
    pub connection_id: i64,
    pub kind: String,
    pub value: u32,
}

impl Store {
    pub(crate) fn submission_approval(&self, claim: &JobClaim) -> Result<()> {
        check_job_owner(&self.connection, claim, JobState::Downloading)?;
        super::job_controls::require_running(&self.connection, claim.job, claim.plan_version)?;
        planning::acquisition_plan(&self.connection, claim.job, claim.plan_version)?;
        Ok(())
    }

    /// Inspect immutable source and existing states without treating them as
    /// ready for effects. Current observations are checked separately at submit.
    pub(crate) fn reconciliation_preview(
        &self,
        job: i64,
        version: i64,
    ) -> Result<Vec<TransferFile>> {
        let plan = self.plan(job, version)?;
        if plan.approved_at.is_none() {
            return Err(Error::Conflict("source plan is not approved".into()));
        }
        planning::check_catalog_plan(&plan)?;
        let mut files = Vec::new();
        let mut after = 0;
        loop {
            let page = self.transfers(job, version, Page::new(after, 200)?)?;
            if page.is_empty() {
                break;
            }
            after = page.last().ok_or(Error::Internal("empty source page"))?.id;
            files.extend(page);
            if files.len() > 1000 {
                return Err(Error::Conflict("selected source exceeds 1000 files".into()));
            }
        }
        if files.is_empty()
            || files.len() != plan.plan.edition.tracks.len()
            || files.iter().enumerate().any(|(index, file)| {
                file.peer != files[0].peer || usize::from(file.track_number) != index + 1
            })
        {
            return Err(Error::Conflict(
                "selected source is incomplete or inconsistent".into(),
            ));
        }
        Ok(files)
    }

    pub(crate) fn reconciliation_files(&self, claim: &JobClaim) -> Result<Vec<TransferFile>> {
        check_job_owner(&self.connection, claim, JobState::Downloading)?;
        self.reconciliation_preview(claim.job, claim.plan_version)
    }

    /// A callback may run during an already submitted effect. It checks the
    /// fence, not current index observations, so sealed receipts remain possible.
    pub(crate) fn check_transfer_owner(&self, claim: &JobClaim) -> Result<()> {
        check_job_owner(&self.connection, claim, JobState::Downloading)
    }

    pub(crate) fn observe_transfer(
        &mut self,
        claim: &JobClaim,
        transfer: i64,
        binding: (u64, u64),
        observation: TransferObservation,
    ) -> Result<()> {
        let session = i64::try_from(binding.0)
            .map_err(|_| Error::InvalidInput("invalid Soulseek session generation".into()))?;
        let connection = i64::try_from(binding.1)
            .map_err(|_| Error::InvalidInput("invalid Soulseek connection identity".into()))?;
        let (kind, value) = observation.fields();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_job_owner(&transaction, claim, JobState::Downloading)?;
        let state: String = transaction.query_row(
            "SELECT state FROM transfer_files WHERE id=?1 AND job_id=?2 AND plan_version=?3",
            params![transfer, claim.job, claim.plan_version],
            |row| row.get(0),
        )?;
        if !matches!(state.as_str(), "submitting" | "active")
            || (kind == "sealed" && state != "active")
        {
            return Err(Error::Conflict(
                "transfer is not awaiting reconciliation evidence".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO transfer_evidence
             (transfer_id,claim_generation,session_generation,connection_id,kind,value)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(transfer_id,claim_generation,session_generation,connection_id,kind,value)
             DO NOTHING",
            params![transfer, claim.generation, session, connection, kind, value],
        )?;
        let count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM transfer_evidence WHERE transfer_id=?1",
            [transfer],
            |row| row.get(0),
        )?;
        if count > 4096 {
            return Err(Error::Conflict(
                "transfer evidence limit exceeded; operator review required".into(),
            ));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn transfer_evidence(&self, transfer: i64, page: Page) -> Result<Vec<TransferEvidence>> {
        if transfer <= 0 {
            return Err(Error::InvalidInput("transfer ID must be positive".into()));
        }
        let mut statement = self.connection.prepare_cached(
            "SELECT id,claim_generation,session_generation,connection_id,kind,value
             FROM transfer_evidence WHERE transfer_id=?1 AND id>?2 ORDER BY id LIMIT ?3",
        )?;
        Ok(statement
            .query_map(params![transfer, page.after(), page.limit()], |row| {
                Ok(TransferEvidence {
                    id: row.get(0)?,
                    claim_generation: row.get(1)?,
                    session_generation: row.get(2)?,
                    connection_id: row.get(3)?,
                    kind: row.get(4)?,
                    value: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
}
