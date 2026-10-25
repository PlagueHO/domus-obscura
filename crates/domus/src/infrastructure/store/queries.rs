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
        let mut statement = self.connection.prepare_cached(
            "SELECT id, plan_version, state, created_at FROM jobs
             WHERE id > ?1 ORDER BY id LIMIT ?2",
        )?;
        statement
            .query_map(params![page.after(), page.limit()], |row| {
                Ok(JobSummary {
                    id: row.get(0)?,
                    plan_version: row.get(1)?,
                    state: row.get(2)?,
                    created_at: row.get(3)?,
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
             AND j.state = 'awaiting_input' ORDER BY q.id LIMIT ?2",
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
