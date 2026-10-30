use super::{Location, Store, StoredQuestion};
use crate::domain::Page;
use crate::{Error, Result};
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct JobSummary {
    pub id: i64,
    pub plan_version: i64,
    pub state: String,
    pub created_at: i64,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub approved_at: Option<i64>,
    pub artwork_candidate_id: Option<i64>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct JobEvent {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: Option<i64>,
    pub event_type: String,
    pub created_at: i64,
}

impl Store {
    /// Read only the events bound to one retained plan. Unversioned historical
    /// events cannot prove which immutable plan they describe.
    pub fn plan_events(&self, job: i64, version: i64, page: Page) -> Result<Vec<JobEvent>> {
        if job <= 0 || version <= 0 {
            return Err(Error::InvalidInput(
                "positive job and plan IDs are required".into(),
            ));
        }
        self.connection.query_row(
            "SELECT version FROM job_plans WHERE job_id = ?1 AND version = ?2",
            params![job, version],
            |row| row.get::<_, i64>(0),
        )?;
        let mut statement = self.connection.prepare_cached(
            "SELECT id, job_id, plan_version, event_type, created_at FROM events
             WHERE job_id = ?1 AND plan_version = ?2 AND id > ?3
             ORDER BY id LIMIT ?4",
        )?;
        statement
            .query_map(params![job, version, page.after(), page.limit()], |row| {
                Ok(JobEvent {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    plan_version: row.get(2)?,
                    event_type: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn last_event_id(&self) -> Result<i64> {
        self.connection
            .query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(Error::from)
    }

    pub fn locations(&self, page: Page) -> Result<Vec<Location>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT l.id, l.root, l.kind, l.coverage, l.observed_at, l.last_error, n.identity
             FROM locations l LEFT JOIN filesystem_roots n ON n.root = l.root AND n.kind = 'media'
             WHERE l.id > ?1 ORDER BY l.id LIMIT ?2",
        )?;
        statement
            .query_map(params![page.after(), page.limit()], |row| {
                Ok(Location {
                    id: row.get(0)?,
                    root: row.get(1)?,
                    kind: row.get(2)?,
                    coverage: row.get(3)?,
                    observed_at: row.get(4)?,
                    last_error: row.get(5)?,
                    native_identity: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn jobs(&self, page: Page) -> Result<Vec<JobSummary>> {
        self.job_summaries(page, false)
    }

    /// Read the queue without changing durable jobs or completion evidence.
    /// Completed rows leave this view after seven days or explicit dismissal.
    pub fn queue(&self, page: Page) -> Result<Vec<JobSummary>> {
        self.job_summaries(page, true)
    }

    fn job_summaries(&self, page: Page, queue_only: bool) -> Result<Vec<JobSummary>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT j.id, j.plan_version, j.state, j.created_at,
                    json_extract(p.body, '$.edition.album'),
                    json_extract(p.body, '$.edition.album_artist'), p.approved_at,
                    a.candidate_id
             FROM jobs j LEFT JOIN job_plans p
               ON p.job_id = j.id AND p.version = j.plan_version
             LEFT JOIN artwork_approvals a
               ON a.job_id = j.id AND a.plan_version = j.plan_version
             LEFT JOIN queue_presentations q
               ON q.job_id = j.id AND q.plan_version = j.plan_version
             WHERE j.id > ?1 AND (?3 = 0 OR
               (j.state != 'cancelled' AND (j.state != 'complete' OR
                (q.dismissed_at IS NULL AND
                 (q.completed_at IS NULL OR q.completed_at > unixepoch() - 604800)))))
             ORDER BY j.id LIMIT ?2",
        )?;
        statement
            .query_map(params![page.after(), page.limit(), queue_only], |row| {
                Ok(JobSummary {
                    id: row.get(0)?,
                    plan_version: row.get(1)?,
                    state: row.get(2)?,
                    created_at: row.get(3)?,
                    album: row.get(4)?,
                    album_artist: row.get(5)?,
                    approved_at: row.get(6)?,
                    artwork_candidate_id: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn questions(&self, page: Page) -> Result<Vec<StoredQuestion>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT q.id, q.job_id, q.plan_version, q.prompt, q.choices, q.answer
             FROM questions q JOIN jobs j ON j.id = q.job_id
             WHERE q.id > ?1 AND q.answer IS NULL AND q.plan_version = j.plan_version
             AND (j.state = 'awaiting_input' OR (j.state='complete' AND EXISTS(
               SELECT 1 FROM album_restore_questions r WHERE r.question_id=q.id
               AND r.resolved_at IS NULL))) ORDER BY q.id LIMIT ?2",
        )?;
        let rows = statement.query_map(params![page.after(), page.limit()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get::<_, String>(4)?,
                row.get(5)?,
            ))
        })?;
        rows.map(|row| {
            let (id, job_id, plan_version, prompt, choices, answer) = row?;
            Ok(StoredQuestion {
                id,
                job_id,
                plan_version,
                prompt,
                choices: serde_json::from_str(&choices)?,
                answer,
            })
        })
        .collect()
    }

    pub fn events(&self, page: Page) -> Result<Vec<JobEvent>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id, job_id, plan_version, event_type, created_at FROM events
             WHERE id > ?1 ORDER BY id LIMIT ?2",
        )?;
        statement
            .query_map(params![page.after(), page.limit()], |row| {
                Ok(JobEvent {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    plan_version: row.get(2)?,
                    event_type: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }
}
