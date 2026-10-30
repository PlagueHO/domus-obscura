use super::Store;
use crate::domain::Page;
use crate::{Error, Result};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct RecoveryEntry {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub category: String,
    pub location_id: Option<i64>,
    pub folder: String,
    pub recorded_bytes: Option<u64>,
    pub version: String,
    pub state: String,
    pub eligible: bool,
    pub last_error: Option<String>,
    pub operation_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct RecoveryFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct RecoveryPreview {
    pub entry: RecoveryEntry,
    pub files: Vec<RecoveryFile>,
    pub root_identity: String,
    pub directory_identity: String,
    pub current_sha256: String,
    pub preview_sha256: Option<String>,
    pub verified: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct RecoveryOperation {
    pub id: i64,
    pub entry_id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub kind: String,
    pub state: String,
    pub removed_files: u32,
    pub total_files: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct RecoveryDecision {
    pub expected_preview_sha256: String,
    pub acknowledge_irreversible_deletion: bool,
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct RecoveryContinue {
    pub expected_operation_id: i64,
}

/// Persisted authority for one operation. Paths are server-derived relative
/// identifiers; reopening a root must still pass its native ancestry binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecoveryBinding {
    pub root: String,
    pub root_identity: String,
    pub root_kind: String,
    pub directory: String,
    pub directory_identity: String,
    pub visible_directory: Option<String>,
    pub visible_identity: Option<String>,
    pub visible_files: Vec<super::ReplacementFile>,
}

#[derive(Debug, Clone)]
pub(crate) struct RecoveryItem {
    pub ordinal: i64,
    pub artifact_id: i64,
    pub path: String,
    pub file: RecoveryFile,
    pub directory: bool,
    pub state: String,
}

const ENTRY_COLUMNS: &str =
    "id,job_id,plan_version,category,location_id,folder,recorded_bytes,version,
                            state,eligible,last_error,operation_id";

fn entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecoveryEntry> {
    Ok(RecoveryEntry {
        id: row.get(0)?,
        job_id: row.get(1)?,
        plan_version: row.get(2)?,
        category: row.get(3)?,
        location_id: row.get(4)?,
        folder: row.get(5)?,
        recorded_bytes: row
            .get::<_, Option<i64>>(6)?
            .map(|v| {
                u64::try_from(v).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Integer,
                        Box::new(e),
                    )
                })
            })
            .transpose()?,
        version: row.get(7)?,
        state: row.get(8)?,
        eligible: row.get(9)?,
        last_error: row.get(10)?,
        operation_id: row.get(11)?,
    })
}

fn bytes(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(column)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(e),
        )
    })
}

impl Store {
    pub fn recovery_inventory(&self, page: Page, job: Option<i64>) -> Result<Vec<RecoveryEntry>> {
        self.recovery_inventory_filtered(page, job, None, None)
    }

    pub fn recovery_inventory_filtered(
        &self,
        page: Page,
        job: Option<i64>,
        category: Option<&str>,
        location: Option<i64>,
    ) -> Result<Vec<RecoveryEntry>> {
        if page.limit() > 100 {
            return Err(Error::InvalidInput(
                "recovery page size must be 1 through 100".into(),
            ));
        }
        if category.is_some_and(|c| !matches!(c, "input" | "scratch" | "pending" | "retained"))
            || job.is_some_and(|id| id <= 0)
            || location.is_some_and(|id| id <= 0)
        {
            return Err(Error::InvalidInput(
                "invalid recovery inventory filter".into(),
            ));
        }
        // Bound each indexed cursor walk before joining manifests or approvals.
        // Filtered history uses job/location-leading indexes to avoid scanning
        // unrelated jobs or destinations.
        let mut ids = Vec::new();
        let after = page.after() / 8;
        let limit = i64::from(page.limit()) + 1;
        for kind in ["input", "scratch", "pending", "retained"] {
            if category.is_some_and(|category| category != kind) {
                continue;
            }
            if location.is_some() && matches!(kind, "input" | "scratch") {
                continue;
            }
            match kind {
                "input" | "scratch" => {
                    let (table, offset) = if kind == "input" {
                        ("transfer_files", 0)
                    } else {
                        ("normalization_runs", 1)
                    };
                    let sql = if job.is_some() {
                        format!(
                            "SELECT id*8+{offset} FROM {table}
                                 WHERE job_id=?1 AND id>=?2 ORDER BY id LIMIT ?3"
                        )
                    } else {
                        format!(
                            "SELECT id*8+{offset} FROM {table}
                                 WHERE id>=?1 ORDER BY id LIMIT ?2"
                        )
                    };
                    let mut statement = self.connection.prepare(&sql)?;
                    let rows = if let Some(job) = job {
                        statement
                            .query_map(params![job, after, limit], |row| row.get::<_, i64>(0))?
                            .collect::<std::result::Result<Vec<_>, _>>()?
                    } else {
                        statement
                            .query_map(params![after, limit], |row| row.get::<_, i64>(0))?
                            .collect::<std::result::Result<Vec<_>, _>>()?
                    };
                    ids.extend(rows);
                }
                "pending" => {
                    let (sql, values) = match (job, location) {
                        (Some(job), Some(location)) => (
                            "SELECT id*8+2 FROM publication_destinations
                             INDEXED BY recovery_pending_job_location_cursor
                             WHERE job_id=?1 AND location_id=?2 AND id>=?3 AND published_at IS NULL
                             ORDER BY id LIMIT ?4",
                            vec![job, location, after, limit],
                        ),
                        (Some(job), None) => (
                            "SELECT id*8+2 FROM publication_destinations
                             INDEXED BY recovery_pending_job_cursor
                             WHERE job_id=?1 AND id>=?2 AND published_at IS NULL
                             ORDER BY id LIMIT ?3",
                            vec![job, after, limit],
                        ),
                        (None, Some(location)) => (
                            "SELECT id*8+2 FROM publication_destinations
                             INDEXED BY recovery_pending_location_cursor
                             WHERE location_id=?1 AND id>=?2 AND published_at IS NULL
                             ORDER BY id LIMIT ?3",
                            vec![location, after, limit],
                        ),
                        (None, None) => (
                            "SELECT id*8+2 FROM publication_destinations
                             WHERE id>=?1 AND published_at IS NULL ORDER BY id LIMIT ?2",
                            vec![after, limit],
                        ),
                    };
                    let mut statement = self.connection.prepare(sql)?;
                    ids.extend(
                        statement
                            .query_map(rusqlite::params_from_iter(values), |row| {
                                row.get::<_, i64>(0)
                            })?
                            .collect::<std::result::Result<Vec<_>, _>>()?,
                    );
                }
                "retained" => {
                    let (sql, values) = match (job, location) {
                        (Some(job), Some(location)) => (
                            "SELECT r.destination_id*8+CASE WHEN r.state='restored' THEN 4 ELSE 3 END
                             FROM publication_destinations d INDEXED BY recovery_destination_job_location_cursor
                             JOIN publication_replacement_journal r ON r.destination_id=d.id
                             WHERE d.job_id=?1 AND d.location_id=?2 AND d.id>=?3
                             AND r.state!='planned' ORDER BY d.id LIMIT ?4",
                            vec![job, location, after, limit],
                        ),
                        (Some(job), None) => (
                            "SELECT r.destination_id*8+CASE WHEN r.state='restored' THEN 4 ELSE 3 END
                             FROM publication_destinations d INDEXED BY recovery_destination_job_cursor
                             JOIN publication_replacement_journal r ON r.destination_id=d.id
                             WHERE d.job_id=?1 AND d.id>=?2 AND r.state!='planned'
                             ORDER BY d.id LIMIT ?3",
                            vec![job, after, limit],
                        ),
                        (None, Some(location)) => (
                            "SELECT r.destination_id*8+CASE WHEN r.state='restored' THEN 4 ELSE 3 END
                             FROM publication_destinations d INDEXED BY recovery_destination_location_cursor
                             JOIN publication_replacement_journal r ON r.destination_id=d.id
                             WHERE d.location_id=?1 AND d.id>=?2 AND r.state!='planned'
                             ORDER BY d.id LIMIT ?3",
                            vec![location, after, limit],
                        ),
                        (None, None) => (
                            "SELECT destination_id*8+CASE WHEN state='restored' THEN 4 ELSE 3 END
                             FROM publication_replacement_journal
                             WHERE destination_id>=?1 AND state!='planned'
                             ORDER BY destination_id LIMIT ?2",
                            vec![after, limit],
                        ),
                    };
                    let mut statement = self.connection.prepare(sql)?;
                    ids.extend(
                        statement
                            .query_map(rusqlite::params_from_iter(values), |row| {
                                row.get::<_, i64>(0)
                            })?
                            .collect::<std::result::Result<Vec<_>, _>>()?,
                    );
                }
                _ => unreachable!(),
            }
        }
        ids.retain(|id| *id > page.after());
        ids.sort_unstable();
        ids.truncate(usize::from(page.limit()));
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let mut statement=self.connection.prepare(&format!(
            "SELECT {ENTRY_COLUMNS} FROM recovery_inventory WHERE id IN ({placeholders}) ORDER BY id"
        ))?;
        Ok(statement
            .query_map(rusqlite::params_from_iter(ids), entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn recovery_entry(&self, id: i64) -> Result<RecoveryEntry> {
        Ok(self.connection.query_row(
            &format!("SELECT {ENTRY_COLUMNS} FROM recovery_inventory WHERE id=?1"),
            [id],
            entry,
        )?)
    }

    pub(crate) fn check_recovery_current(&self, selected: &RecoveryEntry) -> Result<()> {
        let current: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM recovery_inventory r JOIN jobs j ON j.id=r.job_id
             WHERE r.id=?1 AND r.job_id=?2 AND r.plan_version=?3 AND r.version=?4
             AND j.state='complete' AND j.plan_version=r.plan_version
             AND (r.category='scratch' OR
                  (r.category='retained' AND r.saved_state IN ('published','restored'))))",
            params![
                selected.id,
                selected.job_id,
                selected.plan_version,
                selected.version
            ],
            |row| row.get(0),
        )?;
        if !current {
            return Err(Error::Conflict(
                "recovery requires the current completed plan and retained version".into(),
            ));
        }
        Ok(())
    }

    pub fn recovery_operation(&self, entry_id: i64) -> Result<Option<RecoveryOperation>> {
        Ok(self.connection.query_row(
            "SELECT o.id,o.entry_id,o.job_id,o.plan_version,o.kind,o.state,
             (SELECT count(*) FROM recovery_items i WHERE i.operation_id=o.id AND i.directory=0 AND i.state='removed'),
             (SELECT count(*) FROM recovery_items i WHERE i.operation_id=o.id AND i.directory=0),o.last_error
             FROM recovery_operations o WHERE o.entry_id=?1",
            [entry_id], |row| Ok(RecoveryOperation {
                id: row.get(0)?, entry_id: row.get(1)?, job_id: row.get(2)?,
                plan_version: row.get(3)?, kind: row.get(4)?, state: row.get(5)?,
                removed_files: row.get(6)?, total_files: row.get(7)?, last_error: row.get(8)?,
            }),
        ).optional()?)
    }

    pub(crate) fn recovery_binding(&self, operation: i64) -> Result<RecoveryBinding> {
        let body: String = self.connection.query_row(
            "SELECT binding FROM recovery_operations WHERE id=?1",
            [operation],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&body)?)
    }

    pub(crate) fn recovery_items(&self, operation: i64) -> Result<Vec<RecoveryItem>> {
        let mut statement = self.connection.prepare(
            "SELECT ordinal,artifact_id,path,identity,bytes,sha256,directory,state
             FROM recovery_items WHERE operation_id=?1 ORDER BY ordinal LIMIT 4008",
        )?;
        let items = statement
            .query_map([operation], |row| {
                let path: String = row.get(2)?;
                Ok(RecoveryItem {
                    ordinal: row.get(0)?,
                    artifact_id: row.get(1)?,
                    file: RecoveryFile {
                        name: path.rsplit('/').next().unwrap_or("").into(),
                        bytes: bytes(row, 4)?,
                        sha256: row.get(5)?,
                        identity: row.get(3)?,
                    },
                    path,
                    directory: row.get(6)?,
                    state: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if items.is_empty() || items.len() >= 4008 {
            return Err(Error::Conflict(
                "recovery manifest is missing or exceeds its bound".into(),
            ));
        }
        Ok(items)
    }

    pub(crate) fn begin_recovery(
        &mut self,
        selected: &RecoveryEntry,
        digest: &str,
        binding: &RecoveryBinding,
        items: &[RecoveryItem],
    ) -> Result<i64> {
        if digest.len() != 64 || items.is_empty() || items.len() >= 4008 {
            return Err(Error::InvalidInput(
                "invalid bounded recovery approval".into(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = transaction.query_row(
            &format!("SELECT {ENTRY_COLUMNS} FROM recovery_inventory WHERE id=?1"),
            [selected.id],
            entry,
        )?;
        if current.version != selected.version || current.operation_id.is_some() {
            return Err(Error::Conflict(
                "recovery version or operation changed".into(),
            ));
        }
        let cleanup = current.category == "scratch";
        if cleanup {
            let valid: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM scratch_cleanup_admissions a
                 JOIN normalization_runs n ON n.id=a.run_id JOIN jobs j ON j.id=n.job_id
                 WHERE n.id=?1 AND n.job_id=?2 AND n.plan_version=?3
                 AND n.completed_at IS NOT NULL AND j.state='complete' AND j.plan_version=?3
                 AND a.root=?4 AND a.root_identity=?5)
                 AND NOT EXISTS(SELECT 1 FROM publication_destinations WHERE job_id=?2 AND plan_version=?3
                 AND (verified_at IS NULL OR published_at IS NULL OR last_error IS NOT NULL))",
                params![(selected.id-1)/8,selected.job_id,selected.plan_version,binding.root,binding.root_identity], |r|r.get(0),
            )?;
            if !valid {
                return Err(Error::Conflict(
                    "cleanup requires every verified destination".into(),
                ));
            }
        } else if !current.eligible || current.category != "retained" {
            return Err(Error::Conflict(
                "retained version is not eligible for deletion".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO recovery_operations(entry_id,job_id,plan_version,kind,version,preview_sha256,binding)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![selected.id,selected.job_id,selected.plan_version,
                if cleanup {"cleanup"} else {"delete"},selected.version,digest,serde_json::to_string(binding)?],
        )?;
        let id = transaction.last_insert_rowid();
        for item in items {
            transaction.execute(
                "INSERT INTO recovery_items(operation_id,ordinal,artifact_id,path,identity,bytes,sha256,directory)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![id,item.ordinal,item.artifact_id,item.path,item.file.identity,
                    i64::try_from(item.file.bytes).map_err(|_|Error::InvalidInput("recovery byte length exceeds SQLite range".into()))?,
                    item.file.sha256,item.directory],
            )?;
        }
        transaction.execute(
            "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,'recovery_prepared')",
            params![selected.job_id, selected.plan_version],
        )?;
        transaction.commit()?;
        Ok(id)
    }

    pub(crate) fn recovery_item_state(
        &mut self,
        operation: i64,
        ordinal: i64,
        from: &str,
        to: &str,
    ) -> Result<()> {
        if self.connection.execute(
            "UPDATE recovery_items SET state=?4 WHERE operation_id=?1 AND ordinal=?2 AND state=?3
             AND EXISTS(SELECT 1 FROM recovery_operations WHERE id=?1 AND state='pending')",
            params![operation, ordinal, from, to],
        )? != 1
        {
            return Err(Error::Conflict("recovery operation changed".into()));
        }
        Ok(())
    }

    pub(crate) fn recovery_result(&mut self, operation: i64, error: Option<&str>) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if transaction.execute(
            "UPDATE recovery_operations SET last_error=?2,
             state=CASE WHEN ?2 IS NULL THEN 'complete' ELSE state END,
             completed_at=CASE WHEN ?2 IS NULL THEN COALESCE(completed_at,unixepoch()) ELSE completed_at END
             WHERE id=?1 AND (?2 IS NOT NULL OR NOT EXISTS(
               SELECT 1 FROM recovery_items WHERE operation_id=?1 AND state!='removed'))",
            params![operation,error.map(|e|e.chars().take(2048).collect::<String>())],
        )? != 1 { return Err(Error::Conflict("recovery still has unfinished items".into())); }
        transaction.execute(
            "INSERT INTO events(job_id,plan_version,event_type)
             SELECT job_id,plan_version,CASE WHEN ?2 IS NULL THEN 'recovery_complete' ELSE 'recovery_error' END
             FROM recovery_operations WHERE id=?1", params![operation,error],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn scratch_admission(&self, run: i64) -> Result<(String, String)> {
        Ok(self.connection.query_row(
            "SELECT root,root_identity FROM scratch_cleanup_admissions WHERE run_id=?1",
            [run],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    pub(crate) fn scratch_error(&mut self, run: i64, error: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE scratch_cleanup_admissions SET last_error=?2 WHERE run_id=?1",
            params![run, error.chars().take(2048).collect::<String>()],
        )?;
        Ok(())
    }

    pub(crate) fn scratch_source(
        &self,
        run: i64,
    ) -> Result<(super::NormalizationRun, Vec<ScratchFile>)> {
        let normalized = super::normalization::read_run(&self.connection, run)?;
        let mut statement=self.connection.prepare(
            "SELECT f.transfer_id,f.input_bytes,f.input_sha256,f.staging_name,r.output_bytes,r.output_sha256
             FROM normalization_files f JOIN normalization_receipts r
             ON r.run_id=f.run_id AND r.track_number=f.track_number
             JOIN transfer_files t ON t.id=f.transfer_id
             WHERE f.run_id=?1 AND t.state='received' ORDER BY f.track_number LIMIT 1001",
        )?;
        let files = statement
            .query_map([run], |r| {
                Ok(ScratchFile {
                    transfer_id: r.get(0)?,
                    input_bytes: bytes(r, 1)?,
                    input_sha256: r.get(2)?,
                    name: r.get(3)?,
                    bytes: bytes(r, 4)?,
                    sha256: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if normalized.completed_at.is_none()
            || files.len() != usize::from(normalized.files)
            || files.len() > 1000
        {
            return Err(Error::Conflict(
                "scratch requires complete sealed normalization evidence".into(),
            ));
        }
        Ok((normalized, files))
    }

    pub(crate) fn recovery_publication_files(
        &self,
        destination: i64,
    ) -> Result<Vec<super::ReplacementFile>> {
        let mut statement = self.connection.prepare(
            "SELECT name,bytes,sha256 FROM publication_files WHERE destination_id=?1
             AND verified_at IS NOT NULL ORDER BY ordinal LIMIT 1002",
        )?;
        Ok(statement
            .query_map([destination], |r| {
                Ok(super::ReplacementFile {
                    name: r.get(0)?,
                    bytes: bytes(r, 1)?,
                    sha256: r.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

pub(crate) struct ScratchFile {
    pub transfer_id: i64,
    pub input_bytes: u64,
    pub input_sha256: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
