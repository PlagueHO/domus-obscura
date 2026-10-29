use super::{Store, planning};
use crate::domain::Page;
use crate::infrastructure::production_worker::{WorkerOperation, WorkerRequest, WorkerStatus};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Request ownership is separate from the JobClaim used for media mutations.
pub(crate) struct RequestClaim {
    pub request: WorkerRequest,
    owner: String,
    generation: i64,
}

pub(crate) struct HostClaim {
    owner: String,
    generation: i64,
}

fn request_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerRequest> {
    let body: String = row.get(1)?;
    let operation = serde_json::from_str(&body).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(WorkerRequest {
        id: row.get(0)?,
        operation,
        state: row.get(2)?,
        last_error: row.get(3)?,
        configuration_binding: row.get(4)?,
    })
}

fn require_ready(connection: &Connection, operation: &WorkerOperation) -> Result<()> {
    if let Some((job, version)) = operation.job() {
        let state: Option<String> = connection
            .query_row(
                "SELECT state FROM jobs WHERE id=?1 AND plan_version=?2",
                params![job, version],
                |row| row.get(0),
            )
            .optional()?;
        let ready = match operation {
            WorkerOperation::Acquire { .. } => matches!(
                state.as_deref(),
                Some(
                    "queued"
                        | "searching"
                        | "downloading"
                        | "validating"
                        | "normalizing"
                        | "publishing"
                        | "backup_pending"
                )
            ),
            WorkerOperation::Publish { .. } => {
                matches!(state.as_deref(), Some("publishing" | "backup_pending"))
            }
            WorkerOperation::Scan { .. } | WorkerOperation::RefreshFolder { .. } => false,
        };
        if !ready
            || planning::read_plan(connection, job, version)?
                .approved_at
                .is_none()
        {
            return Err(Error::Conflict(
                "worker request plan or state changed".into(),
            ));
        }
        super::job_controls::require_running(connection, job, version)?;
        if matches!(state.as_deref(), Some("queued" | "searching")) {
            planning::approved_plan(connection, job, version)?;
        }
    }
    Ok(())
}

impl Store {
    pub fn worker_requests(&self, page: Page) -> Result<Vec<WorkerRequest>> {
        self.worker_requests_for_job(page, None)
    }

    pub fn worker_requests_for_job(
        &self,
        page: Page,
        job: Option<i64>,
    ) -> Result<Vec<WorkerRequest>> {
        self.worker_requests_matching(page, job, false)
    }

    pub(crate) fn unfinished_worker_requests(
        &self,
        page: Page,
        job: Option<i64>,
    ) -> Result<Vec<WorkerRequest>> {
        self.worker_requests_matching(page, job, true)
    }

    fn worker_requests_matching(
        &self,
        page: Page,
        job: Option<i64>,
        unfinished: bool,
    ) -> Result<Vec<WorkerRequest>> {
        let target = if job.is_some() {
            " AND target='job-'||?3"
        } else {
            " AND ?3 IS NULL"
        };
        let state = if unfinished {
            " AND state!='complete'"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id,body,state,last_error,configuration_binding FROM worker_requests WHERE id>?1{target}{state} ORDER BY id LIMIT ?2"
        );
        let mut statement = self.connection.prepare_cached(&sql)?;
        Ok(statement
            .query_map(params![page.after(), page.limit(), job], request_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn worker_status(&self, capacity: u16) -> Result<WorkerStatus> {
        let host = self
            .connection
            .query_row(
                "SELECT heartbeat>unixepoch()-15 AND stopping=0,revision,
                heartbeat>unixepoch()-15 AND stopping=1 FROM worker_host WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let unfinished = self.connection.query_row(
            "SELECT count(*) FROM worker_requests WHERE state!='complete'",
            [],
            |row| row.get(0),
        )?;
        let active = self.connection.query_row(
            "SELECT count(*) FROM worker_requests WHERE state='running' AND lease_until>unixepoch()",
            [],
            |row| row.get(0),
        )?;
        let (online, active_revision, stopping) = match host {
            Some((online, revision, stopping)) => (online, Some(revision), stopping),
            None => (false, None, false),
        };
        Ok(WorkerStatus {
            online,
            stopping,
            active_revision,
            capacity,
            unfinished,
            active,
        })
    }

    pub fn enqueue_worker(
        &mut self,
        operation: WorkerOperation,
        capacity: u16,
        binding: &str,
    ) -> Result<WorkerRequest> {
        if !(1..=1024).contains(&capacity)
            || binding.len() != 64
            || !binding.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::InvalidInput(
                "worker capacity must be 1 through 1024".into(),
            ));
        }
        let existing = self.connection.query_row(
            "SELECT id,body,state,last_error,configuration_binding FROM worker_requests WHERE target=?1 AND state!='complete'",
            [operation.target()], request_row,
        ).optional()?;
        if let Some(existing) = existing {
            if existing.operation == operation && existing.configuration_binding == binding {
                return Ok(existing);
            }
            return Err(Error::Conflict(
                "a different worker request already owns this target".into(),
            ));
        }
        operation.validate(self)?;
        let body = serde_json::to_string(&operation)?;
        if body.len() > 4096 {
            return Err(Error::InvalidInput(
                "worker request body exceeds 4096 bytes".into(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction.query_row(
            "SELECT id,body,state,last_error,configuration_binding FROM worker_requests WHERE target=?1 AND state!='complete'",
            [operation.target()], request_row,
        ).optional()?;
        if let Some(existing) = existing {
            if existing.operation != operation || existing.configuration_binding != binding {
                return Err(Error::Conflict(
                    "a different worker request already owns this target".into(),
                ));
            }
            return Ok(existing);
        }
        let count: i64 = transaction.query_row(
            "SELECT count(*) FROM worker_requests WHERE state!='complete'",
            [],
            |row| row.get(0),
        )?;
        if count >= i64::from(capacity) {
            return Err(Error::Busy("worker queue is full"));
        }
        require_ready(&transaction, &operation)?;

        transaction.execute(
            "INSERT INTO worker_requests(lane,target,body,configuration_binding) VALUES (?1,?2,?3,?4)",
            params![operation.lane(), operation.target(), body,binding],
        )?;
        let request = WorkerRequest {
            id: transaction.last_insert_rowid(),
            operation,
            state: "queued".into(),
            last_error: None,
            configuration_binding: binding.into(),
        };
        transaction.commit()?;
        Ok(request)
    }

    /// An explicit retry is not permission to change the request or transfer intent.
    pub fn retry_worker(&mut self, id: i64) -> Result<()> {
        let request = self.worker_request(id)?;
        request.operation.validate_recovery(self)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_ready(&transaction, &request.operation)?;
        let changed = transaction.execute(
            "UPDATE worker_requests SET state='queued',last_error=NULL,updated_at=unixepoch()
             WHERE id=?1 AND state IN ('waiting','failed') AND owner IS NULL",
            [id],
        )?;
        if changed != 1 {
            return Err(Error::Conflict(
                "worker request is not ready for an explicit retry".into(),
            ));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn worker_request(&self, id: i64) -> Result<WorkerRequest> {
        self.connection.query_row(
            "SELECT id,body,state,last_error,configuration_binding FROM worker_requests WHERE id=?1",
            [id],
            request_row,
        ).map_err(Error::from)
    }

    pub fn dismiss_worker(&mut self, id: i64) -> Result<()> {
        if self.connection.execute(
            "UPDATE worker_requests SET state='complete',updated_at=unixepoch()
             WHERE id=?1 AND state IN ('failed','waiting') AND owner IS NULL",
            [id],
        )? != 1
        {
            return Err(Error::Conflict(
                "only an idle failed or waiting request can be dismissed".into(),
            ));
        }
        Ok(())
    }

    /// Call only while holding the database's native process lock.
    pub(crate) fn start_worker_host(&mut self, owner: &str, revision: &str) -> Result<HostClaim> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let generation = transaction.query_row(
            "INSERT INTO worker_host(id,owner,generation,revision,heartbeat,stopping)
             VALUES (1,?1,1,?2,unixepoch(),0)
             ON CONFLICT(id) DO UPDATE SET owner=excluded.owner,generation=generation+1,
                 revision=excluded.revision,heartbeat=unixepoch(),stopping=0 RETURNING generation",
            params![owner, revision],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(HostClaim {
            owner: owner.into(),
            generation,
        })
    }

    pub(crate) fn heartbeat_worker(&self, host: &HostClaim, stopping: bool) -> Result<()> {
        if self.connection.execute(
            "UPDATE worker_host SET heartbeat=unixepoch(),stopping=?3
             WHERE id=1 AND owner=?1 AND generation=?2",
            params![host.owner, host.generation, stopping],
        )? != 1
        {
            return Err(Error::Conflict("worker host ownership changed".into()));
        }
        Ok(())
    }

    pub(crate) fn stop_worker_host(&self, host: &HostClaim) -> Result<()> {
        if self.connection.execute(
            "UPDATE worker_host SET heartbeat=0,stopping=0 WHERE id=1 AND owner=?1 AND generation=?2",
            params![host.owner,host.generation],
        )? != 1 {
            return Err(Error::Conflict("worker host ownership changed before stop".into()));
        }
        Ok(())
    }

    pub(crate) fn claim_worker_request(
        &mut self,
        host: &HostClaim,
        lane: &str,
    ) -> Result<Option<RequestClaim>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM worker_host WHERE id=1 AND owner=?1 AND generation=?2
             AND stopping=0 AND heartbeat>unixepoch()-15)",
            params![host.owner, host.generation],
            |row| row.get(0),
        )?;
        if !current {
            return Err(Error::Conflict("worker host is no longer active".into()));
        }
        let row = transaction
            .query_row(
                "UPDATE worker_requests SET state='running',owner=?2,generation=generation+1,
                 lease_until=unixepoch()+300,updated_at=unixepoch()
             WHERE id=(SELECT id FROM worker_requests
                 WHERE lane=?1 AND state IN ('queued','interrupted','running')
                     AND (state IN ('queued','interrupted')
                     OR (state='running' AND lease_until<=unixepoch()))
                 AND (lease_until IS NULL OR lease_until<=unixepoch())
                 ORDER BY id LIMIT 1)
             RETURNING id,body,state,last_error,configuration_binding,generation",
                params![lane, host.owner],
                |row| Ok((request_row(row)?, row.get(5)?)),
            )
            .optional()?;
        transaction.commit()?;
        Ok(row.map(|(request, generation)| RequestClaim {
            request,
            owner: host.owner.clone(),
            generation,
        }))
    }

    pub(crate) fn renew_worker_request(&self, claim: &RequestClaim) -> Result<()> {
        if self.connection.execute(
            "UPDATE worker_requests SET lease_until=unixepoch()+300
             WHERE id=?1 AND owner=?2 AND generation=?3 AND state='running' AND lease_until>unixepoch()",
            params![claim.request.id,claim.owner,claim.generation],
        )? != 1 {
            return Err(Error::Conflict("worker request lease expired or changed".into()));
        }
        Ok(())
    }

    pub(crate) fn finish_worker_request(
        &mut self,
        claim: &RequestClaim,
        state: &str,
        error: Option<&str>,
    ) -> Result<()> {
        if !matches!(state, "complete" | "failed" | "waiting" | "interrupted") {
            return Err(Error::InvalidInput("invalid worker result state".into()));
        }
        let error = error.map(|text| text.chars().take(400).collect::<String>());
        if self.connection.execute(
            "UPDATE worker_requests SET state=?4,last_error=?5,owner=NULL,lease_until=NULL,updated_at=unixepoch()
             WHERE id=?1 AND owner=?2 AND generation=?3 AND state='running' AND lease_until>unixepoch()",
            params![claim.request.id,claim.owner,claim.generation,state,error],
        )? != 1 {
            return Err(Error::Conflict("worker request result lost ownership".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
