use super::{ArtworkDecision, JobControl, Store, StoredQuestion};
use crate::domain::Page;
use crate::{Error, Result};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionRecovery {
    Artwork {
        decision: ArtworkDecision,
    },
    JobControl {
        control: JobControl,
    },
    AlbumRestore {
        destination_id: i64,
        location_id: i64,
        state: String,
    },
    Manual,
}

/// A read snapshot, not authority to answer, resume, claim or start work.
#[derive(Serialize, ts_rs::TS)]
pub struct QuestionDetails {
    pub question: StoredQuestion,
    pub job_state: String,
    pub current_plan_version: i64,
    pub current: bool,
    pub recovery: QuestionRecovery,
}

impl Store {
    /// Include saved answers while their jobs still await a separate recovery
    /// action. The unanswered-only question API keeps its existing behavior.
    pub fn inbox(&self, page: Page) -> Result<Vec<StoredQuestion>> {
        let transaction = self
            .connection
            .is_autocommit()
            .then(|| self.connection.unchecked_transaction())
            .transpose()?;
        let mut statement = self.connection.prepare_cached(
            "SELECT q.id FROM questions q JOIN jobs j ON j.id=q.job_id
             WHERE q.id>?1 AND q.plan_version=j.plan_version AND (
               (j.state='awaiting_input'
                AND q.id=(SELECT MAX(latest.id) FROM questions latest
                          WHERE latest.job_id=q.job_id AND latest.plan_version=q.plan_version))
               OR (j.state='complete' AND EXISTS(
                 SELECT 1 FROM album_restore_questions r
                 WHERE r.question_id=q.id AND r.resolved_at IS NULL)))
             ORDER BY q.id LIMIT ?2",
        )?;
        let ids = statement
            .query_map(params![page.after(), page.limit()], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let questions = ids
            .into_iter()
            .map(|id| self.question(id))
            .collect::<Result<_>>()?;
        if let Some(transaction) = transaction {
            transaction.commit()?;
        }
        Ok(questions)
    }

    pub fn job_decision(&self, job: i64, version: i64) -> Result<Option<QuestionDetails>> {
        if job <= 0 || version <= 0 {
            return Err(Error::InvalidInput(
                "job and plan IDs must be positive".into(),
            ));
        }
        let transaction = self
            .connection
            .is_autocommit()
            .then(|| self.connection.unchecked_transaction())
            .transpose()?;
        let id: Option<i64> = self.connection.query_row(
            "SELECT MAX(id) FROM questions WHERE job_id=?1 AND plan_version=?2",
            params![job, version],
            |row| row.get(0),
        )?;
        let details = id.map(|id| self.question_details(id)).transpose()?;
        if let Some(transaction) = transaction {
            transaction.commit()?;
        }
        Ok(details)
    }

    /// Read question identity, currentness and the actual recovery relationship
    /// together. Never infer a workflow from prompt or choice text.
    pub fn question_details(&self, id: i64) -> Result<QuestionDetails> {
        if id <= 0 {
            return Err(Error::InvalidInput("question ID must be positive".into()));
        }
        let transaction = self
            .connection
            .is_autocommit()
            .then(|| self.connection.unchecked_transaction())
            .transpose()?;
        let question = self.question(id)?;
        let (job_state, current_plan_version, latest): (String, i64, i64) =
            self.connection.query_row(
                "SELECT state,plan_version,
                 (SELECT MAX(id) FROM questions WHERE job_id=?1 AND plan_version=?2)
                 FROM jobs WHERE id=?1",
                params![question.job_id, question.plan_version],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        let restore: Option<(i64, i64, String, bool)> = self
            .connection
            .query_row(
                "SELECT d.id,d.location_id,j.state,r.resolved_at IS NULL
                 FROM album_restore_questions r
                 JOIN publication_destinations d ON d.id=r.destination_id
                 JOIN publication_replacement_journal j ON j.destination_id=d.id
                 WHERE r.question_id=?1 AND d.job_id=?2 AND d.plan_version=?3",
                params![id, question.job_id, question.plan_version],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let (artwork, control): (bool, Option<i64>) = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM artwork_questions WHERE question_id=?1),
             (SELECT id FROM job_controls WHERE question_id=?1)",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let restore_current = restore.as_ref().is_some_and(|binding| binding.3);
        let recovery = match (artwork, control, restore) {
            (true, None, None) => QuestionRecovery::Artwork {
                decision: self.artwork_decision(id)?,
            },
            (false, Some(id), None) => QuestionRecovery::JobControl {
                control: self.job_control(id)?,
            },
            (false, None, Some((destination_id, location_id, state, _))) => {
                QuestionRecovery::AlbumRestore {
                    destination_id,
                    location_id,
                    state,
                }
            }
            (false, None, None) => QuestionRecovery::Manual,
            _ => {
                return Err(Error::Internal(
                    "question has conflicting recovery bindings",
                ));
            }
        };
        let current = question.plan_version == current_plan_version
            && ((job_state == "awaiting_input" && question.id == latest)
                || (job_state == "complete" && restore_current));
        if let Some(transaction) = transaction {
            transaction.commit()?;
        }
        Ok(QuestionDetails {
            question,
            job_state,
            current_plan_version,
            current,
            recovery,
        })
    }
}
