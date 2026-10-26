use super::{JobClaim, Store, check_job_owner, planning, record_job_event};
use crate::domain::{JobState, Page};
use crate::{Error, Result};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::Serialize;
use std::collections::HashSet;

/// Remote paths are protocol data, never paths to open on this host.
/// Ordered files map one complete peer source to the frozen release order.
pub struct SourceFile {
    pub remote_path: String,
    pub expected_bytes: i64,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct TransferFile {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub track_number: u16,
    pub peer: String,
    pub remote_path: String,
    pub expected_bytes: i64,
    pub state: String,
    pub remote_token: Option<u32>,
    pub received_bytes: i64,
    pub payload_sha256: Option<Vec<u8>>,
}

/// Saved ledger counts for the whole exact plan, independent of receipt pages.
/// This is not staging verification or authority to resume an uncertain transfer.
#[derive(Debug, Serialize, ts_rs::TS)]
pub struct TransferSummary {
    pub job_id: i64,
    pub plan_version: i64,
    pub expected_tracks: u16,
    pub total: u16,
    pub planned: u16,
    pub submitting: u16,
    pub active: u16,
    pub received: u16,
    pub all_received: bool,
}

impl Store {
    /// Read the frozen track count and all transfer states in one SQL snapshot.
    /// Never infer complete input from a prefix of paginated receipts.
    pub fn transfer_summary(&self, job: i64, version: i64) -> Result<TransferSummary> {
        if job <= 0 || version <= 0 {
            return Err(Error::InvalidInput(
                "positive job and plan IDs are required".into(),
            ));
        }
        self.connection
            .query_row(
                "SELECT json_array_length(p.body, '$.edition.tracks'), COUNT(f.id),
                    COALESCE(SUM(f.state = 'planned'), 0),
                    COALESCE(SUM(f.state = 'submitting'), 0),
                    COALESCE(SUM(f.state = 'active'), 0),
                    COALESCE(SUM(f.state = 'received'), 0),
                    COALESCE(SUM(f.track_number <= json_array_length(p.body, '$.edition.tracks')
                      AND f.state = 'received' AND f.received_bytes = f.expected_bytes
                      AND length(f.payload_sha256) = 32), 0)
             FROM job_plans p LEFT JOIN transfer_files f
               ON f.job_id = p.job_id AND f.plan_version = p.version
             WHERE p.job_id = ?1 AND p.version = ?2
             GROUP BY p.job_id, p.version",
                params![job, version],
                |row| {
                    let expected_tracks: u16 = row.get(0)?;
                    let total: u16 = row.get(1)?;
                    let sealed: u16 = row.get(6)?;
                    Ok(TransferSummary {
                        job_id: job,
                        plan_version: version,
                        expected_tracks,
                        total,
                        planned: row.get(2)?,
                        submitting: row.get(3)?,
                        active: row.get(4)?,
                        received: row.get(5)?,
                        all_received: expected_tracks > 0
                            && total == expected_tracks
                            && sealed == expected_tracks,
                    })
                },
            )
            .map_err(Error::from)
    }

    pub(crate) fn validate_acquisition_source(
        &self,
        job: i64,
        version: i64,
        peer: &str,
        files: &[SourceFile],
    ) -> Result<()> {
        let plan = planning::acquisition_plan(&self.connection, job, version)?;
        validate_source(peer, files, &plan)
    }

    /// Load the complete immutable source under the downloading claim. Refuse
    /// uncertainty across the whole album before a worker queues another file.
    pub(crate) fn acquisition_files(&self, claim: &JobClaim) -> Result<Vec<TransferFile>> {
        check_job_owner(&self.connection, claim, JobState::Downloading)?;
        self.acquisition_preview(claim.job, claim.plan_version)
    }

    /// Read-only diagnostics, not worker authority. Effects must still use
    /// acquisition_files with a live generation-fenced claim.
    pub(crate) fn acquisition_preview(&self, job: i64, version: i64) -> Result<Vec<TransferFile>> {
        let plan = planning::acquisition_plan(&self.connection, job, version)?;
        let mut files = Vec::new();
        let mut after = 0;
        loop {
            let page = self.transfers(job, version, Page::new(after, 200)?)?;
            if page.is_empty() {
                break;
            }
            after = page
                .last()
                .ok_or(Error::Internal("nonempty transfer page is empty"))?
                .id;
            files.extend(page);
            if files.len() > 1000 {
                return Err(Error::Conflict(
                    "acquisition exceeds its selected-file limit".into(),
                ));
            }
        }
        if files.is_empty()
            || files.len() != plan.plan.edition.tracks.len()
            || files
                .iter()
                .any(|file| !matches!(file.state.as_str(), "planned" | "received"))
            || files.iter().any(|file| file.peer != files[0].peer)
        {
            return Err(Error::Conflict(
                "reconcile the complete selected source before downloading".into(),
            ));
        }
        Ok(files)
    }

    /// Persist every selected file before the first peer effect. This records
    /// intent, not proof of edition matching, quality, media validation or consent
    /// to replace a destination. The source cannot be changed after this call.
    pub fn begin_acquisition(
        &mut self,
        claim: &JobClaim,
        peer: &str,
        files: &[SourceFile],
    ) -> Result<()> {
        self.begin_acquisition_inner(claim, peer, files, None)
    }

    /// Consume only an approved candidate bound to this exact frozen plan.
    /// Recheck it inside the source-intent transaction before creating transfers.
    pub fn begin_reviewed_acquisition(&mut self, claim: &JobClaim, candidate: i64) -> Result<()> {
        let (peer, files) = super::source_candidates::reviewed_source(
            &self.connection,
            claim.job,
            claim.plan_version,
            candidate,
            false,
        )?;
        self.begin_acquisition_inner(claim, &peer, &files, Some(candidate))
    }

    fn begin_acquisition_inner(
        &mut self,
        claim: &JobClaim,
        peer: &str,
        files: &[SourceFile],
        candidate: Option<i64>,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_job_owner(&transaction, claim, JobState::Searching)?;
        super::job_controls::require_running(&transaction, claim.job, claim.plan_version)?;
        let plan = planning::acquisition_plan(&transaction, claim.job, claim.plan_version)?;
        if let Some(candidate) = candidate {
            let (reviewed_peer, reviewed_files) = super::source_candidates::reviewed_source(
                &transaction,
                claim.job,
                claim.plan_version,
                candidate,
                false,
            )?;
            if reviewed_peer != peer
                || reviewed_files.len() != files.len()
                || reviewed_files.iter().zip(files).any(|(reviewed, file)| {
                    reviewed.remote_path != file.remote_path
                        || reviewed.expected_bytes != file.expected_bytes
                })
            {
                return Err(Error::Conflict(
                    "source candidate changed before source intent".into(),
                ));
            }
        }
        validate_source(peer, files, &plan)?;
        let existing: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM acquisition_sources WHERE job_id = ?1 AND plan_version = ?2)",
            params![claim.job, claim.plan_version], |row| row.get(0),
        )?;
        if existing {
            return Err(Error::Conflict(
                "a source already exists; reconcile its transfers before any retry".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO acquisition_sources(job_id, plan_version, peer) VALUES (?1, ?2, ?3)",
            params![claim.job, claim.plan_version, peer],
        )?;
        if let Some(candidate) = candidate {
            transaction.execute("INSERT INTO acquisition_source_reviews(job_id,plan_version,candidate_id) VALUES (?1,?2,?3)",
                params![claim.job,claim.plan_version,candidate])?;
        }
        for (track_number, file) in (1u16..).zip(files) {
            transaction.execute(
                "INSERT INTO transfer_files(job_id, plan_version, track_number, remote_path, expected_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![claim.job, claim.plan_version, track_number, file.remote_path, file.expected_bytes],
            )?;
        }
        transaction.execute(
            "UPDATE jobs SET state = 'downloading' WHERE id = ?1",
            [claim.job],
        )?;
        record_job_event(&transaction, claim, "acquisition_prepared")?;
        transaction.commit()?;
        Ok(())
    }

    pub fn transfers(&self, job: i64, version: i64, page: Page) -> Result<Vec<TransferFile>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT f.id, f.job_id, f.plan_version, f.track_number, s.peer, f.remote_path,
                    f.expected_bytes, f.state, f.remote_token, f.received_bytes, f.payload_sha256
             FROM transfer_files f JOIN acquisition_sources s
               ON s.job_id = f.job_id AND s.plan_version = f.plan_version
             WHERE f.job_id = ?1 AND f.plan_version = ?2 AND f.id > ?3
             ORDER BY f.id LIMIT ?4",
        )?;
        statement
            .query_map(params![job, version, page.after(), page.limit()], |row| {
                Ok(TransferFile {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    plan_version: row.get(2)?,
                    track_number: row.get(3)?,
                    peer: row.get(4)?,
                    remote_path: row.get(5)?,
                    expected_bytes: row.get(6)?,
                    state: row.get(7)?,
                    remote_token: row.get(8)?,
                    received_bytes: row.get(9)?,
                    payload_sha256: row.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    /// Reload authoritative receive parameters under the current worker fence.
    /// Remote callers must never supply a TransferFile as receive authority.
    pub fn active_transfer(&self, claim: &JobClaim, file: i64) -> Result<TransferFile> {
        let transfer = self.transfer_in_state(claim, file, "active")?;
        if transfer.remote_token.is_none() {
            return Err(Error::Conflict(
                "active transfer has no accepted token".into(),
            ));
        }
        Ok(transfer)
    }

    pub fn planned_transfer(&self, claim: &JobClaim, file: i64) -> Result<TransferFile> {
        self.transfer_in_state(claim, file, "planned")
    }

    pub fn submitting_transfer(&self, claim: &JobClaim, file: i64) -> Result<TransferFile> {
        let transfer = self.transfer_in_state(claim, file, "submitting")?;
        planning::acquisition_plan(&self.connection, claim.job, claim.plan_version)?;
        Ok(transfer)
    }

    fn transfer_in_state(&self, claim: &JobClaim, file: i64, state: &str) -> Result<TransferFile> {
        check_job_owner(&self.connection, claim, JobState::Downloading)?;
        if file <= 0 {
            return Err(Error::InvalidInput("transfer ID must be positive".into()));
        }
        let transfer = self
            .transfers(claim.job, claim.plan_version, Page::new(file - 1, 1)?)?
            .pop()
            .filter(|transfer| transfer.id == file && transfer.state == state)
            .ok_or_else(|| {
                Error::Conflict(
                    "transfer is not in the required state for this worker claim".into(),
                )
            })?;
        Ok(transfer)
    }

    /// Commit before submission. A crash leaves `submitting`, never `planned`.
    /// Reclaim does not reset it: an adapter must reconcile the peer first.
    pub fn start_transfer(&mut self, claim: &JobClaim, file: i64) -> Result<()> {
        self.transfer_update(claim, "transfer_submitting", true, |connection| {
            Ok(connection.execute(
                "UPDATE transfer_files SET state = 'submitting', updated_at = unixepoch()
                 WHERE id = ?1 AND job_id = ?2 AND plan_version = ?3 AND state = 'planned'",
                params![file, claim.job, claim.plan_version],
            )?)
        })
    }

    /// Record an observed remote transfer, including after a process restart.
    pub fn accept_transfer(&mut self, claim: &JobClaim, file: i64, token: u32) -> Result<()> {
        self.transfer_update(claim, "transfer_active", false, |connection| {
            Ok(connection.execute(
                "UPDATE transfer_files SET state = 'active', remote_token = ?4, updated_at = unixepoch()
                 WHERE id = ?1 AND job_id = ?2 AND plan_version = ?3 AND state = 'submitting'",
                params![file, claim.job, claim.plan_version, token],
            )?)
        })
    }

    pub fn transfer_progress(&mut self, claim: &JobClaim, file: i64, received: i64) -> Result<()> {
        if received < 0 {
            return Err(Error::InvalidInput(
                "received byte count cannot be negative".into(),
            ));
        }
        self.transfer_update(claim, "transfer_progress", false, |connection| {
            Ok(connection.execute(
                "UPDATE transfer_files SET received_bytes = ?4, updated_at = unixepoch()
                 WHERE id = ?1 AND job_id = ?2 AND plan_version = ?3 AND state = 'active'
                 AND received_bytes <= ?4 AND expected_bytes >= ?4",
                params![file, claim.job, claim.plan_version, received],
            )?)
        })
    }

    /// The worker must sync and hash its own scratch file before this call.
    /// Receiving the declared bytes does not verify audio quality or metadata.
    pub fn receive_transfer(
        &mut self,
        claim: &JobClaim,
        file: i64,
        received: i64,
        sha256: &[u8; 32],
    ) -> Result<()> {
        self.transfer_update(claim, "transfer_received", false, |connection| {
            Ok(connection.execute(
                "UPDATE transfer_files SET state = 'received', received_bytes = ?4,
                     payload_sha256 = ?5, updated_at = unixepoch()
                 WHERE id = ?1 AND job_id = ?2 AND plan_version = ?3 AND state = 'active'
                 AND expected_bytes = ?4 AND received_bytes <= ?4",
                params![
                    file,
                    claim.job,
                    claim.plan_version,
                    received,
                    sha256.as_slice()
                ],
            )?)
        })
    }

    fn transfer_update(
        &mut self,
        claim: &JobClaim,
        event_type: &str,
        requires_current_approval: bool,
        update: impl FnOnce(&Connection) -> Result<usize>,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_job_owner(&transaction, claim, JobState::Downloading)?;
        // A changed index stops new effects, not receipts for effects already
        // submitted. Otherwise an uncertain transfer could never reconcile.
        if requires_current_approval {
            super::job_controls::require_running(&transaction, claim.job, claim.plan_version)?;
            planning::acquisition_plan(&transaction, claim.job, claim.plan_version)?;
        }
        if update(&transaction)? != 1 {
            return Err(Error::Conflict(
                "transfer state, source or byte count changed".into(),
            ));
        }
        record_job_event(&transaction, claim, event_type)?;
        transaction.commit()?;
        Ok(())
    }
}

pub(super) fn validate_source(
    peer: &str,
    files: &[SourceFile],
    plan: &super::StoredPlan,
) -> Result<()> {
    if peer.trim().is_empty() || peer.len() > 256 || peer.chars().any(char::is_control) {
        return Err(Error::InvalidInput("peer name is empty or invalid".into()));
    }
    if files.is_empty() || files.len() > 1000 {
        return Err(Error::InvalidInput(
            "source needs 1 through 1000 ordered audio files".into(),
        ));
    }
    let mut paths = HashSet::new();
    for file in files {
        if file.remote_path.trim().is_empty()
            || file.remote_path.len() > 4096
            || file.remote_path.chars().any(char::is_control)
            || file
                .remote_path
                .split(['\\', '/'])
                .any(|part| matches!(part, "." | ".."))
            || file.expected_bytes <= 0
            || !paths.insert(file.remote_path.replace('/', "\\").to_lowercase())
        {
            return Err(Error::InvalidInput(
                "source files must have distinct paths and positive sizes".into(),
            ));
        }
    }
    if files.len() != plan.plan.edition.tracks.len() {
        return Err(Error::InvalidInput(
            "source must map every selected track exactly once".into(),
        ));
    }
    let suffix = format!(".{}", plan.plan.format.extension());
    if files
        .iter()
        .any(|file| !file.remote_path.to_lowercase().ends_with(&suffix))
        || files
            .iter()
            .try_fold(0i64, |sum, file| sum.checked_add(file.expected_bytes))
            .is_none()
    {
        return Err(Error::InvalidInput(
            "source format or total byte count is invalid".into(),
        ));
    }
    Ok(())
}

pub(super) fn check_received(connection: &Connection, job: i64, version: i64) -> Result<()> {
    let complete: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM transfer_files WHERE job_id = ?1 AND plan_version = ?2)
             AND NOT EXISTS(SELECT 1 FROM transfer_files
                 WHERE job_id = ?1 AND plan_version = ?2 AND state != 'received')",
        params![job, version],
        |row| row.get(0),
    )?;
    if !complete {
        return Err(Error::Conflict(
            "all selected transfers must be received before validation".into(),
        ));
    }
    Ok(())
}
