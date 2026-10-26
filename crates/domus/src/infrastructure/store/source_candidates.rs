use super::{SourceFile, Store, StoredPlan, planning};
use crate::domain::{
    Page,
    sources::{SourceAssessment, SourceFolder, assess_source},
};
use crate::{Error, Result};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Immutable operator-supplied listing and an assessment against one frozen
/// approved plan. Its digest binds review, not independent edition or rights proof.
#[derive(Debug, Serialize, ts_rs::TS)]
pub struct SourceCandidate {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub plan_sha256: String,
    pub review_sha256: String,
    pub listing: SourceFolder,
    pub assessment: SourceAssessment,
    pub created_at: i64,
    pub approved_at: Option<i64>,
}

struct Saved {
    id: i64,
    job: i64,
    version: i64,
    plan_sha256: String,
    review_sha256: String,
    listing: String,
    created_at: i64,
    approved_at: Option<i64>,
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Saved> {
    Ok(Saved {
        id: row.get(0)?,
        job: row.get(1)?,
        version: row.get(2)?,
        plan_sha256: row.get(3)?,
        review_sha256: row.get(4)?,
        listing: row.get(5)?,
        created_at: row.get(6)?,
        approved_at: row.get(7)?,
    })
}

fn conflict() -> Error {
    Error::Conflict("source review needs the exact current approved plan, idle queued/searching job and unchanged destination observations".into())
}

fn fingerprint(plan: &StoredPlan) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(plan)?)))
}

fn digest(job: i64, version: i64, plan: &str, listing: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"domus-source-review-v1\0");
    hash.update(job.to_be_bytes());
    hash.update(version.to_be_bytes());
    hash.update(plan.as_bytes());
    hash.update(listing.as_bytes());
    format!("{:x}", hash.finalize())
}

fn eligible(connection: &Connection, job: i64, version: i64, idle: bool) -> Result<StoredPlan> {
    if job <= 0 || version <= 0 {
        return Err(Error::InvalidInput(
            "source review IDs must be positive".into(),
        ));
    }
    super::job_controls::require_running(connection, job, version)?;
    let allowed: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=?1 AND plan_version=?2
          AND state IN ('queued','searching')
          AND (?3=0 OR owner IS NULL OR lease_until<=unixepoch()))
          AND NOT EXISTS(SELECT 1 FROM acquisition_sources WHERE job_id=?1 AND plan_version=?2)",
        params![job, version, idle],
        |row| row.get(0),
    )?;
    if !allowed {
        return Err(conflict());
    }
    planning::acquisition_plan(connection, job, version)
}

fn candidate(plan: &StoredPlan, saved: Saved) -> Result<SourceCandidate> {
    if fingerprint(plan)? != saved.plan_sha256
        || digest(saved.job, saved.version, &saved.plan_sha256, &saved.listing)
            != saved.review_sha256
    {
        return Err(Error::Conflict("source review binding is damaged".into()));
    }
    let listing: SourceFolder = serde_json::from_str(&saved.listing)?;
    let assessment = assess_source(&plan.plan.edition, plan.plan.format, &listing)?;
    Ok(SourceCandidate {
        id: saved.id,
        job_id: saved.job,
        plan_version: saved.version,
        plan_sha256: saved.plan_sha256,
        review_sha256: saved.review_sha256,
        listing,
        assessment,
        created_at: saved.created_at,
        approved_at: saved.approved_at,
    })
}

fn read(connection: &Connection, id: i64) -> Result<SourceCandidate> {
    if id <= 0 {
        return Err(Error::InvalidInput(
            "source candidate ID must be positive".into(),
        ));
    }
    let saved = connection.query_row(
        "SELECT id,job_id,plan_version,plan_sha256,review_sha256,listing,created_at,approved_at
         FROM source_candidates WHERE id=?1",
        [id],
        row,
    )?;
    candidate(
        &planning::read_plan(connection, saved.job, saved.version)?,
        saved,
    )
}

fn selectable(candidate: &SourceCandidate, plan: &StoredPlan) -> Result<Vec<SourceFile>> {
    if !candidate.assessment.can_approve {
        return Err(Error::Conflict("source candidate has incomplete/ambiguous mapping or different advertised quality; it cannot be approved".into()));
    }
    let files = candidate
        .assessment
        .tracks
        .iter()
        .map(|track| SourceFile {
            remote_path: track.remote_path.clone(),
            expected_bytes: track.expected_bytes,
        })
        .collect::<Vec<_>>();
    super::transfers::validate_source(&candidate.listing.peer, &files, plan)?;
    Ok(files)
}

pub(super) fn reviewed_source(
    connection: &Connection,
    job: i64,
    version: i64,
    id: i64,
    idle: bool,
) -> Result<(String, Vec<SourceFile>)> {
    let plan = eligible(connection, job, version, idle)?;
    let candidate = read(connection, id)?;
    if candidate.job_id != job
        || candidate.plan_version != version
        || candidate.approved_at.is_none()
        || candidate.plan_sha256 != fingerprint(&plan)?
    {
        return Err(conflict());
    }
    let files = selectable(&candidate, &plan)?;
    Ok((candidate.listing.peer, files))
}

impl Store {
    /// Save an unapproved local review. The listing is untrusted operator input,
    /// not a signed provider response or independent proof of exact edition.
    /// Preparation never claims a job, contacts a peer or creates transfer intent.
    pub fn prepare_source_candidate(
        &mut self,
        job: i64,
        version: i64,
        listing: SourceFolder,
    ) -> Result<SourceCandidate> {
        listing.validate()?;
        let body = serde_json::to_string(&listing)?;
        if body.len() > 1_048_576 {
            return Err(Error::InvalidInput("source listing exceeds 1 MiB".into()));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plan = eligible(&transaction, job, version, true)?;
        assess_source(&plan.plan.edition, plan.plan.format, &listing)?;
        let plan_sha256 = fingerprint(&plan)?;
        let review_sha256 = digest(job, version, &plan_sha256, &body);
        let count: i64 = transaction.query_row(
            "SELECT count(*) FROM source_candidates WHERE job_id=?1 AND plan_version=?2",
            params![job, version],
            |row| row.get(0),
        )?;
        let existing: Option<i64> = {
            use rusqlite::OptionalExtension;
            transaction.query_row("SELECT id FROM source_candidates WHERE job_id=?1 AND plan_version=?2 AND review_sha256=?3",
                params![job,version,review_sha256],|row|row.get(0)).optional()?
        };
        let id = if let Some(id) = existing {
            id
        } else {
            if count >= 25 {
                return Err(Error::Conflict("this plan already has 25 retained source candidates; prepare a new plan before another review".into()));
            }
            transaction.execute("INSERT INTO source_candidates(job_id,plan_version,plan_sha256,review_sha256,listing) VALUES (?1,?2,?3,?4,?5)",
                params![job,version,plan_sha256,review_sha256,body])?;
            let id = transaction.last_insert_rowid();
            transaction.execute("INSERT INTO events(job_id,event_type,plan_version) VALUES (?1,'source_candidate_prepared',?2)",params![job,version])?;
            id
        };
        let result = read(&transaction, id)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Approval is a separate exact-digest acknowledgement of edition review and
    /// permitted download. It cannot turn metadata into encoding or rights proof.
    pub fn approve_source_candidate(
        &mut self,
        id: i64,
        version: i64,
        review_sha256: &str,
        acknowledge_exact_edition_and_permitted_download: bool,
    ) -> Result<SourceCandidate> {
        if !acknowledge_exact_edition_and_permitted_download
            || review_sha256.len() != 64
            || !review_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::InvalidInput("source approval requires the exact review SHA-256 and explicit edition/permitted-use acknowledgement".into()));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved = read(&transaction, id)?;
        let plan = eligible(&transaction, saved.job_id, version, true)?;
        if saved.plan_version != version
            || saved.review_sha256 != review_sha256
            || saved.approved_at.is_some()
        {
            return Err(conflict());
        }
        selectable(&saved, &plan)?;
        transaction.execute("UPDATE source_candidates SET approved_at=unixepoch() WHERE id=?1 AND approved_at IS NULL",[id])?;
        transaction.execute("INSERT INTO events(job_id,event_type,plan_version) VALUES (?1,'source_candidate_approved',?2)",params![saved.job_id,version])?;
        let result = read(&transaction, id)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn source_candidate(&self, id: i64) -> Result<SourceCandidate> {
        read(&self.connection, id)
    }

    pub fn source_candidates(
        &self,
        job: i64,
        version: i64,
        page: Page,
    ) -> Result<Vec<SourceCandidate>> {
        let mut statement=self.connection.prepare_cached(
            "SELECT id,job_id,plan_version,plan_sha256,review_sha256,listing,created_at,approved_at
             FROM source_candidates WHERE job_id=?1 AND plan_version=?2 AND id>?3 ORDER BY id LIMIT ?4")?;
        let saved = statement
            .query_map(
                params![job, version, page.after(), page.limit().min(5)],
                row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if saved.is_empty() {
            return Ok(vec![]);
        }
        let plan = planning::read_plan(&self.connection, job, version)?;
        saved
            .into_iter()
            .map(|saved| candidate(&plan, saved))
            .collect()
    }

    pub(crate) fn approved_source_candidate(
        &self,
        job: i64,
        version: i64,
        id: i64,
    ) -> Result<(String, Vec<SourceFile>)> {
        reviewed_source(&self.connection, job, version, id, true)
    }

    /// Read-only receipt for the candidate consumed by a source. Manual and
    /// legacy selections return None; they never gain synthetic review evidence.
    pub fn acquisition_source_candidate(&self, job: i64, version: i64) -> Result<Option<i64>> {
        use rusqlite::OptionalExtension;
        Ok(self.connection.query_row("SELECT candidate_id FROM acquisition_source_reviews WHERE job_id=?1 AND plan_version=?2",
            params![job,version],|row|row.get(0)).optional()?)
    }
}
