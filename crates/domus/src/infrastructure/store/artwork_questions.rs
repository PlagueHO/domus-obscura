use super::{JobClaim, Store, artwork, planning, transfers};
use crate::domain::{ArtworkIssue, Question};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::Digest;

const USE_COVER: &str = "Use approved cover";

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct ArtworkDecision {
    pub question_id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub issue: ArtworkIssue,
    pub paused_state: String,
    pub candidate_id: Option<i64>,
    pub sha256: Option<String>,
    pub resumed_at: Option<i64>,
}

fn conflict() -> Error {
    Error::Conflict(
        "artwork decision needs its current plan, safe state and exact approved preview".into(),
    )
}

fn safe_transfers(connection: &Connection, job: i64, version: i64, state: &str) -> Result<()> {
    let uncertain: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM transfer_files WHERE job_id=?1 AND plan_version=?2
          AND state IN ('submitting','active'))",
        params![job, version],
        |row| row.get(0),
    )?;
    if uncertain {
        return Err(Error::Conflict(
            "reconcile peer transfers before pausing or resuming artwork".into(),
        ));
    }
    if matches!(state, "validating" | "normalizing") {
        transfers::check_received(connection, job, version)?;
    }
    Ok(())
}

pub(super) fn resolve(connection: &Connection, id: i64, version: i64, choice: &str) -> Result<()> {
    let job: Option<i64> = connection
        .query_row(
            "SELECT q.job_id FROM questions q JOIN artwork_questions a ON a.question_id=q.id
         WHERE q.id=?1 AND q.plan_version=?2",
            params![id, version],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(job) = job {
        let pending: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM questions q JOIN jobs j ON j.id=q.job_id
             WHERE q.id=?1 AND q.answer IS NULL AND j.state='awaiting_input'
             AND j.plan_version=q.plan_version
             AND q.id=(SELECT MAX(id) FROM questions WHERE job_id=q.job_id AND plan_version=q.plan_version))",
            [id], |row| row.get(0),
        )?;
        if choice != USE_COVER || !pending {
            return Err(conflict());
        }
        artwork::current_plan(connection, job, version)?;
        let candidate = artwork::approved(connection, job, version)?;
        let jpeg = super::artwork::load_jpeg(connection, job, version, candidate.id)?;
        if candidate.sha256 != format!("{:x}", sha2::Sha256::digest(jpeg.bytes())) {
            return Err(conflict());
        }
        connection.execute(
            "INSERT INTO artwork_question_resolutions(question_id,candidate_id,sha256) VALUES (?1,?2,?3)",
            params![id, candidate.id, candidate.sha256],
        )?;
    }
    Ok(())
}

impl Store {
    pub fn artwork_decision(&self, id: i64) -> Result<ArtworkDecision> {
        let (job_id, plan_version, issue, paused_state, candidate_id, sha256, resumed_at):
            (i64, i64, String, String, Option<i64>, Option<String>, Option<i64>) =
            self.connection.query_row(
                "SELECT q.job_id,q.plan_version,a.issue,q.paused_state,r.candidate_id,r.sha256,r.resumed_at
                 FROM artwork_questions a JOIN questions q ON q.id=a.question_id
                 LEFT JOIN artwork_question_resolutions r ON r.question_id=q.id WHERE q.id=?1",
                [id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
            )?;
        Ok(ArtworkDecision {
            question_id: id,
            job_id,
            plan_version,
            issue: serde_json::from_value(serde_json::Value::String(issue))?,
            paused_state,
            candidate_id,
            sha256,
            resumed_at,
        })
    }

    /// Check without fetching or approving a cover. Missing artwork persists a
    /// question only when no live worker or uncertain transfer can be disturbed.
    pub fn check_artwork(&mut self, job: i64, version: i64) -> Result<Option<ArtworkDecision>> {
        artwork::current_plan(&self.connection, job, version)?;
        if self.job_state(job)?.as_deref() == Some("awaiting_input") {
            let id: i64 = self.connection.query_row(
                "SELECT a.question_id FROM artwork_questions a JOIN questions q ON q.id=a.question_id
                 WHERE q.job_id=?1 AND q.plan_version=?2
                 AND q.id=(SELECT MAX(id) FROM questions WHERE job_id=?1 AND plan_version=?2)",
                params![job,version], |row| row.get(0),
            ).optional()?.ok_or_else(conflict)?;
            return Ok(Some(self.artwork_decision(id)?));
        }
        let approved: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM artwork_approvals WHERE job_id=?1 AND plan_version=?2)",
            params![job, version],
            |row| row.get(0),
        )?;
        if approved {
            self.approved_artwork(job, version)?;
            return Ok(None);
        }
        let candidates: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM artwork_candidates WHERE job_id=?1 AND plan_version=?2)",
            params![job, version],
            |row| row.get(0),
        )?;
        Ok(Some(self.pause_artwork(
            job,
            version,
            if candidates {
                ArtworkIssue::PreviewApproval
            } else {
                ArtworkIssue::Missing
            },
            None,
        )?))
    }

    /// A worker must supply its fenced claim. All transfer effects must already
    /// be at a safe boundary. The question releases ownership atomically.
    pub fn ask_artwork(
        &mut self,
        claim: &JobClaim,
        issue: ArtworkIssue,
    ) -> Result<ArtworkDecision> {
        self.pause_artwork(claim.job, claim.plan_version, issue, Some(claim))
    }

    pub(super) fn pause_artwork(
        &mut self,
        job: i64,
        version: i64,
        issue: ArtworkIssue,
        claim: Option<&JobClaim>,
    ) -> Result<ArtworkDecision> {
        let question = Question::new(issue.prompt(), vec![USE_COVER.into()])?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::job_controls::require_running(&transaction, job, version)?;
        artwork::current_plan(&transaction, job, version)?;
        let (state, owner, generation, live): (String, Option<String>, i64, bool) = transaction
            .query_row(
                "SELECT state,owner,claim_generation,COALESCE(lease_until>unixepoch(),0) FROM jobs
             WHERE id=?1 AND plan_version=?2",
                params![job, version],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        if state == "awaiting_input" && claim.is_none() {
            let id: Option<i64> = transaction
                .query_row(
                    "SELECT q.id FROM questions q JOIN artwork_questions a ON a.question_id=q.id
                 WHERE q.job_id=?1 AND q.plan_version=?2
                 AND q.id=(SELECT MAX(id) FROM questions WHERE job_id=?1 AND plan_version=?2)
                 AND NOT EXISTS(SELECT 1 FROM artwork_question_resolutions r WHERE r.question_id=q.id AND r.resumed_at IS NOT NULL)",
                    params![job, version],
                    |row| row.get(0),
                )
                .optional()?;
            let id = id.ok_or_else(conflict)?;
            let answered: bool = transaction.query_row(
                "SELECT answer IS NOT NULL FROM questions WHERE id=?1",
                [id],
                |row| row.get(0),
            )?;
            if answered {
                return Err(conflict());
            }
            let saved_issue = serde_json::to_value(issue)?
                .as_str()
                .ok_or_else(conflict)?
                .to_owned();
            if transaction.execute(
                "UPDATE artwork_questions SET issue=?2 WHERE question_id=?1 AND issue!=?2",
                params![id, saved_issue],
            )? == 1
            {
                transaction.execute(
                    "UPDATE questions SET prompt=?2 WHERE id=?1 AND answer IS NULL",
                    params![id, question.prompt()],
                )?;
                transaction.execute(
                    "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'artwork_question_updated')",
                    params![job,version],
                )?;
            }
            transaction.commit()?;
            return self.artwork_decision(id);
        }
        if !matches!(
            state.as_str(),
            "queued" | "searching" | "downloading" | "validating" | "normalizing"
        ) {
            return Err(conflict());
        }
        match claim {
            Some(claim)
                if owner.as_deref() == Some(&claim.owner)
                    && live
                    && generation == claim.generation => {}
            None if owner.is_none() || !live => {}
            _ => return Err(conflict()),
        }
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM artwork_approvals WHERE job_id=?1 AND plan_version=?2)",
            params![job, version],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(conflict());
        }
        safe_transfers(&transaction, job, version, &state)?;
        transaction.execute(
            "UPDATE jobs SET state='awaiting_input',owner=NULL,lease_until=NULL,
             claim_generation=claim_generation+1 WHERE id=?1 AND plan_version=?2",
            params![job, version],
        )?;
        transaction.execute(
            "INSERT INTO questions(job_id,plan_version,paused_state,prompt,choices) VALUES (?1,?2,?3,?4,?5)",
            params![job, version, state, question.prompt(), serde_json::to_string(question.choices())?],
        )?;
        let id = transaction.last_insert_rowid();
        let issue = serde_json::to_value(issue)?
            .as_str()
            .ok_or_else(conflict)?
            .to_owned();
        transaction.execute(
            "INSERT INTO artwork_questions(question_id,issue) VALUES (?1,?2)",
            params![id, issue],
        )?;
        transaction.execute(
            "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'awaiting_input')",
            params![job, version],
        )?;
        transaction.commit()?;
        self.artwork_decision(id)
    }

    /// Resume only the saved safe state. This does not claim a job, contact a
    /// provider, rewrite a receipt or start a media operation.
    pub fn resume_artwork(&mut self, id: i64, version: i64) -> Result<String> {
        let decision = self.artwork_decision(id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::job_controls::require_running(&transaction, decision.job_id, version)?;
        if decision.plan_version != version
            || decision.resumed_at.is_some()
            || !matches!(
                decision.paused_state.as_str(),
                "queued" | "searching" | "downloading" | "validating" | "normalizing"
            )
        {
            return Err(conflict());
        }
        let plan = planning::approved_plan(&transaction, decision.job_id, version)?;
        artwork::current_plan(&transaction, decision.job_id, version)?;
        planning::check_catalog_plan(&plan)?;
        let approved = artwork::approved(&transaction, decision.job_id, version)?;
        super::artwork::load_jpeg(&transaction, decision.job_id, version, approved.id)?;
        if decision.candidate_id != Some(approved.id)
            || decision.sha256.as_deref() != Some(&approved.sha256)
        {
            return Err(conflict());
        }
        safe_transfers(
            &transaction,
            decision.job_id,
            version,
            &decision.paused_state,
        )?;
        let changed = transaction.execute(
            "UPDATE jobs SET state=?3,owner=NULL,lease_until=NULL,claim_generation=claim_generation+1
             WHERE id=?1 AND plan_version=?2 AND state='awaiting_input'
             AND (owner IS NULL OR lease_until<=unixepoch())
             AND EXISTS(SELECT 1 FROM questions WHERE id=?4 AND job_id=?1 AND plan_version=?2 AND answer=?5)
             AND ?4=(SELECT MAX(id) FROM questions WHERE job_id=?1 AND plan_version=?2)",
            params![decision.job_id, version, decision.paused_state, id, USE_COVER],
        )?;
        if changed != 1 {
            return Err(conflict());
        }
        if transaction.execute(
            "UPDATE artwork_question_resolutions SET resumed_at=unixepoch() WHERE question_id=?1 AND resumed_at IS NULL", [id],
        )? != 1 { return Err(conflict()); }
        transaction.execute(
            "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'artwork_resumed')",
            params![decision.job_id, version],
        )?;
        transaction.commit()?;
        Ok(decision.paused_state)
    }

    pub(super) fn artwork_preparation_result(
        &mut self,
        job: i64,
        version: i64,
        result: Result<super::ArtworkCandidate>,
    ) -> Result<super::ArtworkCandidate> {
        match result {
            Err(Error::ArtworkInput(issue)) => {
                let decision = self.pause_artwork(job, version, issue, None)?;
                Err(Error::ArtworkRequired(decision.question_id))
            }
            result => result,
        }
    }
}
