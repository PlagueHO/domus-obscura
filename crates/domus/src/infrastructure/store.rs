use crate::domain::{JobState, LibraryPath, Page, Question};
use crate::{Error, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod artwork;
mod artwork_questions;
mod caa;
mod catalog;
mod catalog_clock;
mod inbox;
mod job_controls;
mod library;
mod native_roots;
mod normalization;
mod planning;
mod private_roots;
mod publication;
mod queries;
mod queue;
mod sessions;
mod source_candidates;
mod transfer_evidence;
mod transfers;
mod worker_requests;
pub use artwork::ArtworkCandidate;
pub use artwork_questions::ArtworkDecision;
pub use caa::CaaArtworkEvidence;
pub use catalog::{
    CatalogCache, CatalogCompletion, CatalogPermit, CatalogProviderState, CatalogStatus,
};
pub use inbox::{QuestionDetails, QuestionRecovery};
pub use job_controls::{JobControl, JobControlAction};
pub use normalization::{NormalizationRun, NormalizationWork};
pub use planning::{
    ClaimedPlan, PlanDestination, PlanRequest, PreparedPlan, SettingsVersion, StoredPlan,
};
pub use publication::{PublicationDestination, PublicationFile};
pub use queries::{JobEvent, JobSummary};
pub use queue::QueueSelection;
pub use sessions::{CreatedSession, SessionCredential, SessionIdentity};
pub use source_candidates::SourceCandidate;
pub use transfer_evidence::{TransferEvidence, TransferObservation};
pub use transfers::{SourceFile, TransferFile, TransferSummary};
pub(crate) use worker_requests::RequestClaim;

const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/001_initial.sql"),
    include_str!("../../migrations/002_questions.sql"),
    include_str!("../../migrations/003_question_versions.sql"),
    include_str!("../../migrations/004_plans.sql"),
    include_str!("../../migrations/005_claim_generations.sql"),
    include_str!("../../migrations/006_sessions.sql"),
    include_str!("../../migrations/007_transfers.sql"),
    include_str!("../../migrations/008_catalog.sql"),
    include_str!("../../migrations/009_catalog_clock.sql"),
    include_str!("../../migrations/010_normalization.sql"),
    include_str!("../../migrations/011_private_roots.sql"),
    include_str!("../../migrations/012_artwork_approval.sql"),
    include_str!("../../migrations/013_native_roots.sql"),
    include_str!("../../migrations/014_caa_evidence.sql"),
    include_str!("../../migrations/015_artwork_questions.sql"),
    include_str!("../../migrations/016_publication.sql"),
    include_str!("../../migrations/017_job_controls.sql"),
    include_str!("../../migrations/018_source_candidates.sql"),
    include_str!("../../migrations/019_queue_presentation.sql"),
    include_str!("../../migrations/020_plan_events.sql"),
    include_str!("../../migrations/021_cache_roots.sql"),
    include_str!("../../migrations/022_transfer_evidence.sql"),
    include_str!("../../migrations/023_worker_requests.sql"),
    include_str!("../../migrations/024_sharing.sql"),
    include_str!("../../migrations/025_library_metadata.sql"),
];

pub struct Store {
    connection: Connection,
    path: PathBuf,
    cache: Option<std::sync::Arc<dyn crate::application::services::cache::Cache>>,
    worker_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

pub struct ScanClaim {
    pub id: i64,
    pub location: i64,
    pub owner: String,
}

/// A capability for one lease generation. A worker name alone cannot prove
/// ownership after expiry and reclaim, even when the same worker name is used.
#[derive(Debug)]
pub struct JobClaim {
    job: i64,
    owner: String,
    plan_version: i64,
    generation: i64,
}

impl JobClaim {
    pub(crate) fn job(&self) -> i64 {
        self.job
    }
    pub(crate) fn plan_version(&self) -> i64 {
        self.plan_version
    }
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct Location {
    pub id: i64,
    pub root: String,
    pub kind: String,
    pub coverage: String,
    pub observed_at: Option<i64>,
    pub last_error: Option<String>,
    pub native_identity: Option<String>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct Entry {
    pub id: i64,
    pub relative_path: String,
    pub kind: String,
    pub size: Option<i64>,
    pub modified_at: Option<i64>,
    pub present: bool,
    pub scan_id: i64,
}

pub struct Observation<'a> {
    pub path: &'a LibraryPath,
    pub kind: &'a str,
    pub size: Option<i64>,
    pub modified_at: Option<i64>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct StoredQuestion {
    pub id: i64,
    pub job_id: i64,
    pub plan_version: i64,
    pub prompt: String,
    pub choices: Vec<String>,
    pub answer: Option<String>,
}

fn validate_claim(owner: &str, lease_seconds: u16) -> Result<()> {
    if owner.is_empty()
        || owner.len() > 128
        || owner.chars().any(char::is_control)
        || !(1..=300).contains(&lease_seconds)
    {
        return Err(Error::InvalidInput(
            "invalid worker owner or lease duration".into(),
        ));
    }
    Ok(())
}

fn check_job_owner(connection: &Connection, claim: &JobClaim, state: JobState) -> Result<()> {
    let current: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE id = ?1 AND plan_version = ?2 AND owner = ?3
             AND claim_generation = ?4 AND lease_until > unixepoch() AND state = ?5)",
        params![
            claim.job,
            claim.plan_version,
            claim.owner,
            claim.generation,
            state.as_str()
        ],
        |row| row.get(0),
    )?;
    if !current {
        return Err(Error::Conflict(
            "job ownership, state or plan changed".into(),
        ));
    }
    Ok(())
}

fn record_job_event(connection: &Connection, claim: &JobClaim, event_type: &str) -> Result<()> {
    connection.execute(
        "INSERT INTO events(job_id, plan_version, event_type) VALUES (?1, ?2, ?3)",
        params![claim.job, claim.plan_version, event_type],
    )?;
    Ok(())
}

fn require_current_schema(connection: &Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != MIGRATIONS.len() as i64 {
        return Err(Error::Conflict(
            "operation requires the current database schema; upgrade separately".into(),
        ));
    }
    Ok(())
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            true,
        )
    }

    pub fn open_existing(path: &Path) -> Result<Self> {
        Self::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE, true)
    }

    /// Open mutable current-schema state without creating or migrating it.
    pub fn open_current_existing(path: &Path) -> Result<Self> {
        Self::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE, false)
    }

    /// Read a current-schema snapshot without creating or migrating a database.
    /// SQLite may still use its normal WAL shared-memory coordination files.
    pub fn open_read_snapshot(path: &Path) -> Result<Self> {
        let store = Self::open_read_current(path)?;
        store.connection.execute_batch("BEGIN DEFERRED;")?;
        require_current_schema(&store.connection)?;
        Ok(store)
    }

    /// Read-only current-schema connection with no long-lived transaction.
    /// Use for shared-cache reads so file I/O does not occur in a SQL snapshot.
    pub fn open_read_current(path: &Path) -> Result<Self> {
        super::filesystem::require_local_disk(path)?;
        let path = std::fs::canonicalize(path)?;
        super::filesystem::require_local_disk(&path)?;
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA query_only = ON;")?;
        require_current_schema(&connection)?;
        Ok(Self {
            connection,
            path,
            cache: None,
            worker_stop: None,
        })
    }

    fn open_with_flags(path: &Path, flags: OpenFlags, upgrade: bool) -> Result<Self> {
        super::filesystem::require_local_disk(path)?;
        let resolved = if path.try_exists()? {
            std::fs::canonicalize(path)?
        } else {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            std::fs::canonicalize(parent)?
        };
        super::filesystem::require_local_disk(&resolved)?;
        let mut connection = Connection::open_with_flags(path, flags)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        if !upgrade {
            require_current_schema(&connection)?;
        }
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !upgrade {
            require_current_schema(&transaction)?;
        }
        let version: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 0 || version > MIGRATIONS.len() as i64 {
            return Err(Error::Conflict(
                "database schema is newer than this application".into(),
            ));
        }
        for (index, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            transaction.execute_batch(migration)?;
            let next_version = i64::try_from(index + 1)
                .map_err(|_| Error::Conflict("schema version exceeds SQLite range".into()))?;
            transaction.pragma_update(None, "user_version", next_version)?;
        }
        transaction.commit()?;
        Ok(Self {
            connection,
            path: std::fs::canonicalize(path)?,
            cache: None,
            worker_stop: None,
        })
    }

    /// Use one host-selected cache without changing durable evidence or falling
    /// back to legacy provider storage when the selected adapter fails.
    pub fn with_cache(
        mut self,
        cache: std::sync::Arc<dyn crate::application::services::cache::Cache>,
    ) -> Self {
        self.cache = Some(cache);
        self
    }

    pub(crate) fn cache(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::application::services::cache::Cache>> {
        self.cache.clone()
    }

    pub fn database_path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn sharing_connection(&self) -> &Connection {
        &self.connection
    }

    pub(crate) fn with_worker_stop(
        mut self,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.worker_stop = Some(stop);
        self
    }

    pub(crate) fn check_worker_stop(&self) -> Result<()> {
        if self
            .worker_stop
            .as_ref()
            .is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Acquire))
        {
            return Err(Error::WorkerStopping);
        }
        Ok(())
    }

    pub(crate) fn release_job_claim(&self, claim: &JobClaim) -> Result<()> {
        self.connection.execute(
            "UPDATE jobs SET owner=NULL,lease_until=NULL WHERE id=?1 AND owner=?2
             AND plan_version=?3 AND claim_generation=?4",
            params![claim.job, claim.owner, claim.plan_version, claim.generation],
        )?;
        Ok(())
    }

    pub(crate) fn completed_worker_normalization(&self, job: i64, version: i64) -> Result<i64> {
        self.connection.query_row(
            "SELECT id FROM normalization_runs WHERE job_id=?1 AND plan_version=?2 AND completed_at IS NOT NULL
             ORDER BY id DESC LIMIT 1", params![job,version], |row|row.get(0),
        ).map_err(Error::from)
    }

    pub(crate) fn require_current_worker_plan(&self, job: i64, version: i64) -> Result<()> {
        let current: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=?1 AND plan_version=?2)",
            params![job, version],
            |row| row.get(0),
        )?;
        if !current {
            return Err(Error::Conflict("worker plan version changed".into()));
        }
        Ok(())
    }

    pub fn add_location(&self, root: &str, kind: &str) -> Result<i64> {
        if !matches!(kind, "main" | "backup") {
            return Err(Error::InvalidInput(
                "location kind must be main or backup".into(),
            ));
        }
        let root_path = Path::new(root);
        let (canonical, opened) = super::filesystem::LibraryDirectory::canonical(root_path)?;
        let root = canonical
            .to_str()
            .ok_or_else(|| Error::InvalidInput("library root is not valid Unicode".into()))?;
        let root_path = canonical.as_path();
        self.check_state_overlap(opened.native_root())?;
        if self.path.starts_with(root_path) {
            return Err(Error::InvalidInput(
                "library root must not contain the application database".into(),
            ));
        }
        let transaction = self.connection.unchecked_transaction()?;
        native_roots::require_bound_roots(&transaction)?;
        let (count, mains): (i64, i64) = transaction.query_row(
            "SELECT COUNT(*), COALESCE(SUM(kind = 'main'), 0) FROM locations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count >= 100 || (kind == "main" && mains != 0) {
            return Err(Error::Conflict(
                "at most 100 library roots and one main root can be registered; existing roots cannot be replaced".into(),
            ));
        }
        let mut statement = transaction
            .prepare("SELECT root FROM locations UNION ALL SELECT root FROM private_roots")?;
        for existing in statement.query_map([], |row| row.get::<_, String>(0))? {
            let existing = existing?;
            if root_path.starts_with(&existing) || Path::new(&existing).starts_with(root_path) {
                return Err(Error::InvalidInput(
                    "library locations must not overlap media or reserved private roots".into(),
                ));
            }
        }
        drop(statement);
        native_roots::register(&transaction, root, "media", opened.native_root())?;
        transaction.execute(
            "INSERT INTO locations(root, kind) VALUES (?1, ?2)",
            params![root, kind],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(id)
    }

    pub fn location(&self, id: i64) -> Result<Location> {
        self.connection
            .query_row(
                "SELECT l.id, l.root, l.kind, l.coverage, l.observed_at, l.last_error, n.identity
                 FROM locations l LEFT JOIN filesystem_roots n ON n.root = l.root AND n.kind = 'media'
                 WHERE l.id = ?1",
                [id],
                |row| {
                    Ok(Location {
                        id: row.get(0)?,
                        root: row.get(1)?,
                        kind: row.get(2)?,
                        coverage: row.get(3)?,
                        observed_at: row.get(4)?,
                        last_error: row.get(5)?,
                        native_identity: row.get(6)?,
                    })
                },
            )
            .map_err(Error::from)
    }

    pub fn begin_scan(&mut self, location: i64) -> Result<ScanClaim> {
        self.begin_scoped_scan(location, None)
    }

    pub fn begin_folder_scan(&mut self, location: i64, folder: &LibraryPath) -> Result<ScanClaim> {
        self.begin_scoped_scan(location, Some(folder.as_str()))
    }

    fn begin_scoped_scan(&mut self, location: i64, folder: Option<&str>) -> Result<ScanClaim> {
        self.location(location)?;
        let owner = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| Error::InvalidInput("system clock precedes the Unix epoch".into()))?
                .as_nanos()
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = transaction
            .query_row(
                "SELECT id, lease_until > unixepoch(), folder_scope FROM scans
             WHERE location_id = ?1 AND status = 'running'",
                [location],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        let id = match previous {
            Some((_, true, _)) => {
                return Err(Error::Conflict(
                    "location already has an active scan".into(),
                ));
            }
            Some((id, false, scope)) => {
                if scope.as_deref() != folder {
                    return Err(Error::Conflict(
                        "an interrupted scan has a different scope; resume it first".into(),
                    ));
                }
                transaction.execute(
                    "UPDATE scans SET owner = ?2, lease_until = unixepoch() + 60 WHERE id = ?1",
                    params![id, owner],
                )?;
                id
            }
            None => {
                transaction.execute(
                    "INSERT INTO scans(location_id, status, owner, lease_until,folder_scope)
                     VALUES (?1, 'running', ?2, unixepoch() + 60,?3)",
                    params![location, owner, folder],
                )?;
                let id = transaction.last_insert_rowid();
                transaction.execute(
                    "INSERT INTO scan_directories(scan_id, relative_path) VALUES (?1, ?2)",
                    params![id, folder.unwrap_or("")],
                )?;
                id
            }
        };
        if let Some(folder) = folder {
            transaction.execute(
                "INSERT INTO library_folder_observations(location_id,folder,state)
                 VALUES (?1,?2,'scanning') ON CONFLICT(location_id,folder)
                 DO UPDATE SET state='scanning',last_error=NULL",
                params![location, folder],
            )?;
        } else {
            transaction.execute(
                "UPDATE locations SET coverage = 'scanning', last_error = NULL WHERE id = ?1",
                [location],
            )?;
        }
        transaction.commit()?;
        Ok(ScanClaim {
            id,
            location,
            owner,
        })
    }

    pub fn next_directory(&self, claim: &ScanClaim) -> Result<Option<LibraryPath>> {
        self.renew_scan(claim)?;
        let path: Option<String> = self
            .connection
            .query_row(
                "SELECT relative_path FROM scan_directories WHERE scan_id = ?1 AND complete = 0
             ORDER BY relative_path LIMIT 1",
                [claim.id],
                |row| row.get(0),
            )
            .optional()?;
        path.map(|value| LibraryPath::parse(&value)).transpose()
    }

    pub(crate) fn renew_scan(&self, claim: &ScanClaim) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE scans SET lease_until = unixepoch() + 60 WHERE id = ?1
             AND location_id = ?2 AND owner = ?3 AND status = 'running'
             AND lease_until > unixepoch()",
            params![claim.id, claim.location, claim.owner],
        )?;
        if changed != 1 {
            return Err(Error::Conflict("scan ownership expired or changed".into()));
        }

        Ok(())
    }

    pub(crate) fn release_scan(&self, claim: &ScanClaim) -> Result<()> {
        self.connection.execute(
            "UPDATE scans SET lease_until=unixepoch()-1 WHERE id=?1 AND location_id=?2
             AND owner=?3 AND status='running'",
            params![claim.id, claim.location, claim.owner],
        )?;
        Ok(())
    }

    pub fn complete_directory(&self, claim: &ScanClaim, directory: &LibraryPath) -> Result<()> {
        self.renew_scan(claim)?;
        let changed = self.connection.execute(
            "UPDATE scan_directories SET complete = 1 WHERE scan_id = ?1 AND relative_path = ?2
             AND EXISTS(SELECT 1 FROM scans WHERE id = ?1 AND owner = ?3 AND status = 'running'
                        AND lease_until > unixepoch())",
            params![claim.id, directory.as_str(), claim.owner],
        )?;
        if changed != 1 {
            return Err(Error::Conflict(
                "scan directory or ownership changed".into(),
            ));
        }
        Ok(())
    }

    pub fn record_batch(
        &mut self,
        claim: &ScanClaim,
        observations: &[Observation<'_>],
    ) -> Result<()> {
        self.record_library_batch(claim, observations, &[])
    }

    pub(crate) fn record_library_batch(
        &mut self,
        claim: &ScanClaim,
        observations: &[Observation<'_>],
        metadata: &[(&LibraryPath, &crate::domain::library::LibraryMetadata)],
    ) -> Result<()> {
        if observations.len() > 128 {
            return Err(Error::InvalidInput("scan batch exceeds 128 entries".into()));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let running: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM scans WHERE id = ?1 AND location_id = ?2
             AND owner = ?3 AND lease_until > unixepoch() AND status = 'running')",
            params![claim.id, claim.location, claim.owner],
            |row| row.get(0),
        )?;
        if !running {
            return Err(Error::Conflict(
                "scan is not active for this location".into(),
            ));
        }
        let scope: Option<String> = transaction.query_row(
            "SELECT folder_scope FROM scans WHERE id=?1",
            [claim.id],
            |row| row.get(0),
        )?;
        if scope
            .as_ref()
            .is_some_and(|scope| observations.iter().any(|item| item.path.parent() != scope))
        {
            return Err(Error::Conflict(
                "folder observation is outside the claimed direct-folder scope".into(),
            ));
        }
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO entries(location_id, relative_path, parent_path, kind, size, modified_at, scan_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(location_id, relative_path) DO UPDATE SET
                 kind = excluded.kind, size = excluded.size, modified_at = excluded.modified_at,
                 scan_id = excluded.scan_id, present = 1",
            )?;
            for observation in observations {
                statement.execute(params![
                    claim.location,
                    observation.path.as_str(),
                    observation.path.parent(),
                    observation.kind,
                    observation.size,
                    observation.modified_at,
                    claim.id
                ])?;
                if observation.kind == "directory" {
                    transaction.execute(
                        "INSERT OR IGNORE INTO scan_directories(scan_id, relative_path)
                         SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM scans WHERE id=?1 AND folder_scope IS NULL)",
                        params![claim.id, observation.path.as_str()],
                    )?;
                }
            }
        }
        for (path, metadata) in metadata {
            library::save_metadata(&transaction, claim, path, metadata)?;
        }
        transaction.execute(
            "UPDATE scans SET observed_count = observed_count + ?2, lease_until = unixepoch() + 60
             WHERE id = ?1",
            params![claim.id, observations.len() as i64],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn scan_error(&mut self, claim: &ScanClaim, detail: &str) -> Result<()> {
        self.renew_scan(claim)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let owned: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM scans WHERE id = ?1 AND owner = ?2 AND status = 'running'
             AND lease_until > unixepoch())",
            params![claim.id, claim.owner],
            |row| row.get(0),
        )?;
        if !owned {
            return Err(Error::Conflict("scan ownership expired or changed".into()));
        }
        transaction.execute(
            "INSERT INTO scan_errors(scan_id, detail) VALUES (?1, ?2)",
            params![claim.id, detail],
        )?;
        transaction.execute(
            "UPDATE scans SET error_count = error_count + 1 WHERE id = ?1",
            [claim.id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn finish_scan(&mut self, claim: &ScanClaim) -> Result<bool> {
        self.renew_scan(claim)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let errors: i64 = transaction.query_row(
            "SELECT error_count FROM scans WHERE id = ?1 AND location_id = ?2 AND owner = ?3
             AND lease_until > unixepoch() AND status = 'running'",
            params![claim.id, claim.location, claim.owner],
            |row| row.get(0),
        )?;
        let pending: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM scan_directories WHERE scan_id = ?1 AND complete = 0)",
            [claim.id],
            |row| row.get(0),
        )?;
        let complete = errors == 0 && !pending;
        let scope: Option<String> = transaction.query_row(
            "SELECT folder_scope FROM scans WHERE id=?1",
            [claim.id],
            |row| row.get(0),
        )?;
        if complete {
            transaction.execute(
                "UPDATE entries SET present = 0 WHERE location_id = ?1 AND scan_id != ?2
                 AND (?3 IS NULL OR parent_path=?3)",
                params![claim.location, claim.id, scope],
            )?;
        }
        transaction.execute(
            "UPDATE scans SET status = ?2, completed_at = unixepoch() WHERE id = ?1",
            params![claim.id, if complete { "complete" } else { "failed" }],
        )?;
        if let Some(scope) = scope {
            transaction.execute(
                "UPDATE library_folder_observations SET state=?3,observed_at=unixepoch(),
                 last_error=(SELECT detail FROM scan_errors WHERE scan_id=?4 ORDER BY id DESC LIMIT 1)
                 WHERE location_id=?1 AND folder=?2",
                params![claim.location,scope,if complete {"complete"} else {"incomplete"},claim.id],
            )?;
        } else {
            transaction.execute(
            "UPDATE locations SET coverage = ?2, observed_at = unixepoch(),
             last_error = (SELECT detail FROM scan_errors WHERE scan_id = ?3 ORDER BY id DESC LIMIT 1)
             WHERE id = ?1",
            params![
                claim.location,
                if complete { "complete" } else { "incomplete" },
                claim.id
            ],
            )?;
        }
        transaction.commit()?;
        Ok(complete)
    }

    pub fn browse(&self, location: i64, folder: &LibraryPath, page: Page) -> Result<Vec<Entry>> {
        self.location(location)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT id, relative_path, kind, size, modified_at, present, scan_id FROM entries
             WHERE location_id = ?1 AND parent_path = ?2 AND id > ?3 ORDER BY id LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![location, folder.as_str(), page.after(), page.limit()],
            |row| {
                Ok(Entry {
                    id: row.get(0)?,
                    relative_path: row.get(1)?,
                    kind: row.get(2)?,
                    size: row.get(3)?,
                    modified_at: row.get(4)?,
                    present: row.get(5)?,
                    scan_id: row.get(6)?,
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::from)
    }

    pub fn enqueue(&mut self, plan_version: i64) -> Result<i64> {
        if plan_version <= 0 {
            return Err(Error::InvalidInput("plan version must be positive".into()));
        }
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO jobs(plan_version, state) VALUES (?1, 'queued')",
            [plan_version],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO events(job_id, event_type, plan_version) VALUES (?1, 'queued', ?2)",
            params![id, plan_version],
        )?;
        transaction.commit()?;
        Ok(id)
    }

    pub fn claim(&self, id: i64, owner: &str, lease_seconds: u16) -> Result<Option<JobClaim>> {
        validate_claim(owner, lease_seconds)?;
        let row: Option<(i64, i64)> = self
            .connection
            .query_row(
                "UPDATE jobs SET owner = ?2, lease_until = unixepoch() + ?3,
             claim_generation = claim_generation + 1 WHERE id = ?1
             AND state NOT IN ('complete', 'cancelled', 'failed')
             AND (state != 'awaiting_input' OR EXISTS(
                 SELECT 1 FROM questions WHERE job_id = jobs.id
                 AND plan_version = jobs.plan_version AND answer IS NOT NULL
                 AND id = (SELECT MAX(id) FROM questions WHERE job_id = jobs.id
                           AND plan_version = jobs.plan_version)
                 AND NOT EXISTS(SELECT 1 FROM artwork_questions a WHERE a.question_id = questions.id)
                 AND NOT EXISTS(SELECT 1 FROM job_controls c WHERE c.question_id=questions.id)))
             AND (owner IS NULL OR lease_until <= unixepoch())
             RETURNING plan_version, claim_generation",
                params![id, owner, lease_seconds],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(plan_version, generation)| JobClaim {
            job: id,
            owner: owner.into(),
            plan_version,
            generation,
        }))
    }

    pub fn renew_claim(&self, claim: &JobClaim, lease_seconds: u16) -> Result<()> {
        validate_claim(&claim.owner, lease_seconds)?;
        let changed = self.connection.execute(
            "UPDATE jobs SET lease_until = unixepoch() + ?5
             WHERE id = ?1 AND owner = ?2 AND plan_version = ?3 AND claim_generation = ?4
             AND lease_until > unixepoch() AND state NOT IN ('complete', 'cancelled', 'failed')",
            params![
                claim.job,
                claim.owner,
                claim.plan_version,
                claim.generation,
                lease_seconds
            ],
        )?;
        if changed != 1 {
            return Err(Error::Conflict("job claim expired or changed".into()));
        }
        Ok(())
    }

    pub fn advance(&mut self, claim: &JobClaim, expected: JobState, next: JobState) -> Result<()> {
        let id = claim.job;
        let owner = &claim.owner;
        let plan_version = claim.plan_version;
        if matches!(
            (expected, next),
            (
                JobState::Publishing,
                JobState::Complete | JobState::BackupPending
            ) | (JobState::BackupPending, JobState::Complete)
        ) {
            return Err(Error::InvalidInput(
                "use the verified destination publication journal for this transition".into(),
            ));
        }
        if !expected.can_advance_to(next) {
            return Err(Error::InvalidInput(
                "job transition is not permitted".into(),
            ));
        }
        if expected == JobState::Searching && next == JobState::Downloading {
            return Err(Error::InvalidInput(
                "prepare the acquisition journal before downloading".into(),
            ));
        }
        if (expected == JobState::Validating && next == JobState::Normalizing)
            || (expected == JobState::Normalizing && next == JobState::Publishing)
        {
            return Err(Error::InvalidInput(
                "use the bound normalization journal for this transition".into(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if expected == JobState::Downloading
            && matches!(next, JobState::Failed | JobState::Cancelled)
        {
            let uncertain: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM transfer_files WHERE job_id = ?1 AND plan_version = ?2
                     AND state IN ('submitting', 'active'))",
                params![id, plan_version],
                |row| row.get(0),
            )?;
            if uncertain {
                return Err(Error::Conflict(
                    "reconcile peer transfers before stopping the job".into(),
                ));
            }
        }
        if !matches!(
            next,
            JobState::AwaitingInput | JobState::Failed | JobState::Cancelled
        ) {
            job_controls::require_running(&transaction, id, plan_version)?;
            planning::approved_plan(&transaction, id, plan_version)?;
        }
        if expected == JobState::Downloading && next == JobState::Validating {
            transfers::check_received(&transaction, id, plan_version)?;
        }
        let changed = transaction.execute(
            "UPDATE jobs SET state = ?5 WHERE id = ?1 AND owner = ?2 AND plan_version = ?3
             AND state = ?4 AND lease_until > unixepoch() AND claim_generation = ?6
             AND (?5 IN ('awaiting_input', 'failed', 'cancelled') OR EXISTS(
                 SELECT 1 FROM job_plans WHERE job_id = jobs.id
                 AND version = jobs.plan_version AND approved_at IS NOT NULL))",
            params![
                id,
                owner,
                plan_version,
                expected.as_str(),
                next.as_str(),
                claim.generation
            ],
        )?;
        if changed != 1 {
            return Err(Error::Conflict(
                "job ownership, state or plan changed".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO events(job_id, event_type, plan_version) VALUES (?1, ?2, ?3)",
            params![id, next.as_str(), plan_version],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn job_state(&self, id: i64) -> Result<Option<String>> {
        self.connection
            .query_row("SELECT state FROM jobs WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Error::from)
    }

    pub fn ask(
        &mut self,
        claim: &JobClaim,
        paused_state: JobState,
        question: &Question,
    ) -> Result<i64> {
        let job = claim.job;
        let owner = &claim.owner;
        let plan_version = claim.plan_version;
        if !paused_state.can_advance_to(JobState::AwaitingInput) {
            return Err(Error::InvalidInput(
                "job must reach a safe boundary before asking".into(),
            ));
        }
        let choices = serde_json::to_string(question.choices())?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        job_controls::require_running(&transaction, job, plan_version)?;
        let changed = transaction.execute(
            "UPDATE jobs SET state = 'awaiting_input', owner = NULL, lease_until = NULL
             WHERE id = ?1 AND owner = ?2 AND plan_version = ?3 AND state = ?4
             AND lease_until > unixepoch() AND claim_generation = ?5",
            params![
                job,
                owner,
                plan_version,
                paused_state.as_str(),
                claim.generation
            ],
        )?;
        if changed != 1 {
            return Err(Error::Conflict(
                "job ownership, state or plan changed".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO questions(job_id, plan_version, paused_state, prompt, choices)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                job,
                plan_version,
                paused_state.as_str(),
                question.prompt(),
                choices
            ],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO events(job_id, event_type, plan_version) VALUES (?1, 'awaiting_input', ?2)",
            params![job, plan_version],
        )?;
        transaction.commit()?;
        Ok(id)
    }

    pub fn question(&self, id: i64) -> Result<StoredQuestion> {
        let (job_id, plan_version, prompt, choices, answer) = self.connection.query_row(
            "SELECT job_id, plan_version, prompt, choices, answer FROM questions WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, String>(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        Ok(StoredQuestion {
            id,
            job_id,
            plan_version,
            prompt,
            choices: serde_json::from_str(&choices)?,
            answer,
        })
    }

    /// Persist a decision without starting work. The application must recheck
    /// file/plan bindings before it resumes any operation that can change media.
    pub fn answer(&mut self, id: i64, plan_version: i64, choice: &str) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (job, choices): (i64, String) = transaction.query_row(
            "SELECT job_id, choices FROM questions WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let choices: Vec<String> = serde_json::from_str(&choices)?;
        if !choices.iter().any(|allowed| allowed == choice) {
            return Err(Error::InvalidInput(
                "answer is not an offered choice".into(),
            ));
        }
        artwork_questions::resolve(&transaction, id, plan_version, choice)?;
        let changed = transaction.execute(
            "UPDATE questions SET answer = ?3, answered_at = unixepoch()
             WHERE id = ?1 AND plan_version = ?2 AND answer IS NULL
             AND EXISTS(SELECT 1 FROM jobs WHERE id = questions.job_id
                        AND plan_version = ?2 AND state = 'awaiting_input')",
            params![id, plan_version, choice],
        )?;
        if changed != 1 {
            return Err(Error::Conflict(
                "question was answered or its plan changed".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO events(job_id, event_type, plan_version) VALUES (?1, 'question_answered', ?2)",
            params![job, plan_version],
        )?;
        transaction.commit()?;
        Ok(())
    }
}
