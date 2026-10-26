use super::Store;
use crate::{Error, Result};
use rusqlite::{TransactionBehavior, params};
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct QueueSelection {
    pub job_id: i64,
    pub plan_version: i64,
}

impl Store {
    /// Hide only the selected completed versions. Keep job, media and recovery
    /// records intact. Validate the entire bounded selection before committing.
    pub fn dismiss_finished(&mut self, selection: &[QueueSelection]) -> Result<()> {
        let mut seen = HashSet::new();
        if selection.is_empty()
            || selection.len() > 50
            || selection
                .iter()
                .any(|item| item.job_id <= 0 || item.plan_version <= 0 || !seen.insert(item.job_id))
        {
            return Err(Error::InvalidInput(
                "select 1 to 50 distinct completed request versions".into(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for item in selection {
            let current: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM jobs j JOIN queue_presentations q
                 ON q.job_id = j.id AND q.plan_version = j.plan_version
                 WHERE j.id = ?1 AND j.plan_version = ?2 AND j.state = 'complete')",
                params![item.job_id, item.plan_version],
                |row| row.get(0),
            )?;
            if !current {
                return Err(Error::Conflict(
                    "queue selection is stale or the request is not complete".into(),
                ));
            }
            if transaction.execute(
                "UPDATE queue_presentations SET dismissed_at = unixepoch()
                 WHERE job_id = ?1 AND plan_version = ?2 AND dismissed_at IS NULL",
                params![item.job_id, item.plan_version],
            )? == 1
            {
                transaction.execute(
                    "INSERT INTO events(job_id, plan_version, event_type)
                     VALUES (?1, ?2, 'queue_dismissed')",
                    params![item.job_id, item.plan_version],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}
