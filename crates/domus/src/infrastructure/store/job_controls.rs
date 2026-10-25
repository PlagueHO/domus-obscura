use super::{JobClaim, Store, check_job_owner, planning, transfers};
use crate::domain::{JobState, Question};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum JobControlAction {
    Pause,
    Cancel,
}

impl JobControlAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Cancel => "cancel",
        }
    }
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct JobControl {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub action: JobControlAction,
    pub requested_at: i64,
    pub applied_at: Option<i64>,
    pub question_id: Option<i64>,
    pub paused_state: Option<String>,
    pub continued_at: Option<i64>,
}

fn conflict() -> Error {
    Error::Conflict(
        "job control requires the current version, a safe state and its current decision".into(),
    )
}

fn state(value: &str) -> Option<JobState> {
    match value {
        "queued" => Some(JobState::Queued),
        "searching" => Some(JobState::Searching),
        "downloading" => Some(JobState::Downloading),
        "validating" => Some(JobState::Validating),
        "normalizing" => Some(JobState::Normalizing),
        _ => None,
    }
}

fn read(connection: &Connection, id: i64) -> Result<JobControl> {
    Ok(connection.query_row(
        "SELECT id,job_id,plan_version,action,requested_at,applied_at,question_id,paused_state,continued_at
         FROM job_controls WHERE id=?1",[id],read_row,
    )?)
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobControl> {
    let action: String = row.get(3)?;
    Ok(JobControl {
        id: row.get(0)?,
        job_id: row.get(1)?,
        plan_version: row.get(2)?,
        action: match action.as_str() {
            "pause" => JobControlAction::Pause,
            "cancel" => JobControlAction::Cancel,
            _ => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(conflict()),
                ));
            }
        },
        requested_at: row.get(4)?,
        applied_at: row.get(5)?,
        question_id: row.get(6)?,
        paused_state: row.get(7)?,
        continued_at: row.get(8)?,
    })
}

fn uncertain(connection: &Connection, job: i64, version: i64) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM transfer_files WHERE job_id=?1 AND plan_version=?2
         AND state IN ('submitting','active'))",
        params![job, version],
        |row| row.get(0),
    )?)
}

/// This gate applies only before new effects. In-flight receipt writes must
/// remain possible so an active transfer can reach a fully received boundary.
pub(super) fn require_running(connection: &Connection, job: i64, version: i64) -> Result<()> {
    let pending:bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM job_controls WHERE job_id=?1 AND plan_version=?2 AND applied_at IS NULL)",
        params![job,version],|row|row.get(0),
    )?;
    if pending {
        return Err(Error::Conflict(
            "job has a pending pause or cancel request; settle it at a safe boundary".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_replan_allowed(
    connection: &Connection,
    job: i64,
    version: i64,
) -> Result<()> {
    let pending: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM job_controls WHERE job_id=?1 AND plan_version=?2
         AND (applied_at IS NULL OR (action='pause' AND continued_at IS NULL)))",
        params![job, version],
        |row| row.get(0),
    )?;
    if pending {
        return Err(conflict());
    }
    Ok(())
}

fn apply(connection: &Connection, id: i64, current_state: &str) -> Result<()> {
    let control = read(connection, id)?;
    if control.applied_at.is_some()
        || (state(current_state).is_none()
            && !(current_state == "awaiting_input" && control.action == JobControlAction::Cancel))
        || uncertain(connection, control.job_id, control.plan_version)?
    {
        return Err(conflict());
    }
    let question_id = if control.action == JobControlAction::Pause {
        let question = Question::new(
            "Job paused at a safe boundary. Resume the saved work or cancel it.".into(),
            vec!["Resume".into(), "Cancel".into()],
        )?;
        connection.execute(
            "INSERT INTO questions(job_id,plan_version,paused_state,prompt,choices) VALUES (?1,?2,?3,?4,?5)",
            params![control.job_id,control.plan_version,current_state,question.prompt(),serde_json::to_string(question.choices())?],
        )?;
        Some(connection.last_insert_rowid())
    } else {
        None
    };
    let next = if question_id.is_some() {
        "awaiting_input"
    } else {
        "cancelled"
    };
    if connection.execute(
        "UPDATE jobs SET state=?3,owner=NULL,lease_until=NULL,claim_generation=claim_generation+1
         WHERE id=?1 AND plan_version=?2 AND state=?4",
        params![control.job_id, control.plan_version, next, current_state],
    )? != 1
    {
        return Err(conflict());
    }
    connection.execute(
        "UPDATE job_controls SET applied_at=unixepoch(),question_id=?2,paused_state=?3 WHERE id=?1",
        params![id, question_id, current_state],
    )?;
    connection.execute(
        "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,?3)",
        params![
            control.job_id,
            control.plan_version,
            if question_id.is_some() {
                "job_paused"
            } else {
                "cancelled"
            }
        ],
    )?;
    Ok(())
}

impl Store {
    pub(crate) fn require_job_running(&self, job: i64, version: i64) -> Result<()> {
        require_running(&self.connection, job, version)
    }

    pub fn job_controls(
        &self,
        job: i64,
        version: i64,
        page: crate::domain::Page,
    ) -> Result<Vec<JobControl>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id,job_id,plan_version,action,requested_at,applied_at,question_id,paused_state,continued_at
             FROM job_controls WHERE job_id=?1 AND plan_version=?2 AND id>?3 ORDER BY id LIMIT ?4",
        )?;
        Ok(statement
            .query_map(params![job, version, page.after(), page.limit()], read_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn job_control(&self, id: i64) -> Result<JobControl> {
        read(&self.connection, id)
    }

    /// Queue a version-bound request. Idle jobs settle atomically. A live
    /// worker keeps ownership until it reaches a safe file/transfer boundary.
    /// No request interrupts uncertain transfers or any publication state.
    pub fn request_job_control(
        &mut self,
        job: i64,
        version: i64,
        action: JobControlAction,
    ) -> Result<JobControl> {
        if job <= 0 || version <= 0 {
            return Err(Error::InvalidInput(
                "job control IDs must be positive".into(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (saved_version,current_state,live):(i64,String,bool)=transaction.query_row(
            "SELECT plan_version,state,owner IS NOT NULL AND lease_until>unixepoch() FROM jobs WHERE id=?1",[job],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        )?;
        if version != saved_version
            || (state(&current_state).is_none()
                && !(current_state == "awaiting_input" && action == JobControlAction::Cancel))
        {
            return Err(conflict());
        }
        let prior:Option<i64>=transaction.query_row(
            "SELECT id FROM job_controls WHERE job_id=?1 AND plan_version=?2 AND applied_at IS NULL",
            params![job,version],|row|row.get(0),
        ).optional()?;
        let id = if let Some(id) = prior {
            if read(&transaction, id)?.action != action {
                return Err(conflict());
            }
            id
        } else {
            transaction.execute(
                "INSERT INTO job_controls(job_id,plan_version,action) VALUES (?1,?2,?3)",
                params![job, version, action.as_str()],
            )?;
            let id = transaction.last_insert_rowid();
            transaction.execute("INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'job_control_requested')",params![job,version])?;
            id
        };
        if !live && !uncertain(&transaction, job, version)? {
            apply(&transaction, id, &current_state)?;
        }
        transaction.commit()?;
        read(&self.connection, id)
    }

    /// Called between effects, never inside an active peer transfer. A durable
    /// request releases/fences the claim and returns an explicit stopped error.
    pub fn job_work_boundary(&mut self, claim: &JobClaim) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_state: String =
            transaction.query_row("SELECT state FROM jobs WHERE id=?1", [claim.job], |row| {
                row.get(0)
            })?;
        check_job_owner(
            &transaction,
            claim,
            state(&current_state).ok_or_else(conflict)?,
        )?;
        let id:Option<i64>=transaction.query_row(
            "SELECT id FROM job_controls WHERE job_id=?1 AND plan_version=?2 AND applied_at IS NULL",
            params![claim.job,claim.plan_version],|row|row.get(0),
        ).optional()?;
        if let Some(id) = id {
            apply(&transaction, id, &current_state)?;
            transaction.commit()?;
            return Err(Error::JobStopped(id));
        }
        transaction.commit()?;
        Ok(())
    }

    /// A saved answer does not restart work. This separate command rechecks the
    /// exact current plan, observations and safe transfer state, then restores
    /// only the saved state (or records cancellation). It starts no I/O.
    pub fn continue_job_control(&mut self, id: i64, version: i64) -> Result<String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let control = read(&transaction, id)?;
        let question = control.question_id.ok_or_else(conflict)?;
        if control.plan_version != version
            || control.continued_at.is_some()
            || control.action != JobControlAction::Pause
        {
            return Err(conflict());
        }
        let current: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs j JOIN questions q ON q.job_id=j.id
             WHERE j.id=?1 AND j.plan_version=?2 AND j.state='awaiting_input' AND q.id=?3
             AND q.plan_version=?2 AND (j.owner IS NULL OR j.lease_until<=unixepoch())
             AND q.id=(SELECT MAX(id) FROM questions WHERE job_id=j.id AND plan_version=?2))",
            params![control.job_id, version, question],
            |row| row.get(0),
        )?;
        if !current || uncertain(&transaction, control.job_id, version)? {
            return Err(conflict());
        }
        require_running(&transaction, control.job_id, version)?;
        let answer: Option<String> = transaction.query_row(
            "SELECT answer FROM questions WHERE id=?1",
            [question],
            |row| row.get(0),
        )?;
        let restored = control.paused_state.as_deref().ok_or_else(conflict)?;
        let next = match answer.as_deref() {
            Some("Resume") => {
                let saved = state(restored).ok_or_else(conflict)?;
                if saved != JobState::Queued {
                    planning::acquisition_plan(&transaction, control.job_id, version)?;
                } else {
                    planning::check_queued_resume(&transaction, control.job_id, version)?;
                }
                if matches!(saved, JobState::Validating | JobState::Normalizing) {
                    transfers::check_received(&transaction, control.job_id, version)?;
                }
                restored
            }
            Some("Cancel") => "cancelled",
            _ => return Err(conflict()),
        };
        transaction.execute(
            "UPDATE jobs SET state=?3,owner=NULL,lease_until=NULL,claim_generation=claim_generation+1 WHERE id=?1 AND plan_version=?2",
            params![control.job_id,version,next],
        )?;
        transaction.execute(
            "UPDATE job_controls SET continued_at=unixepoch() WHERE id=?1",
            [id],
        )?;
        transaction.execute(
            "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,?3)",
            params![
                control.job_id,
                version,
                if next == "cancelled" {
                    "cancelled"
                } else {
                    "job_resumed"
                }
            ],
        )?;
        transaction.commit()?;
        Ok(next.into())
    }
}
