use super::Store;
use crate::infrastructure::catalog::{CatalogQuery, MAX_CATALOG_BODY};
use crate::{Error, Result};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A single-use network capability. The OS lock survives suspension and wall
/// clock changes. Durable fencing also preserves abandoned-request state.
pub struct CatalogPermit {
    generation: i64,
    issued: Instant,
    started: bool,
    database: PathBuf,
    _lock: File,
}

impl CatalogPermit {
    pub(crate) async fn start(&mut self) -> Result<()> {
        if !self.started {
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                self.issued + Duration::from_secs(1),
            ))
            .await;
        }
        self.check_start()
    }

    pub(crate) fn check_start(&mut self) -> Result<()> {
        let elapsed = self.issued.elapsed();
        if self.started || !(Duration::from_secs(1)..Duration::from_secs(5)).contains(&elapsed) {
            return Err(Error::Catalog {
                reason: "MusicBrainz request permit expired or was already used",
                retry_after_seconds: 5,
            });
        }
        self.started = true;
        Ok(())
    }
}

pub struct CatalogCache {
    pub body: Vec<u8>,
    pub fetched_at: i64,
    pub payload_sha256: Vec<u8>,
}

pub struct CatalogCompletion {
    pub cache: Option<CatalogCache>,
    pub retry_after_seconds: u32,
    pub(crate) fetched_at: i64,
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum CatalogProviderState {
    Available,
    Active,
    Backoff,
    Paused,
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct CatalogStatus {
    pub state: CatalogProviderState,
    pub retry_after_seconds: u32,
    pub failures: u32,
}

impl Store {
    pub fn catalog_status(&self) -> Result<CatalogStatus> {
        let (owned, _lock) = match self.lock_catalog_request() {
            Ok(lock) => (false, Some(lock)),
            Err(Error::Catalog { .. }) => (true, None),
            Err(error) => return Err(error),
        };
        let (generation, lease, next, failures, delay): (i64, i64, i64, u32, u32) =
            self.connection.query_row(
                "SELECT generation, lease_until_ms, next_request_ms, failures, delay_seconds
             FROM catalog_gate WHERE id=1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
        let remaining = if next == i64::MAX {
            u32::MAX
        } else if owned {
            1
        } else {
            super::catalog_clock::remaining(
                &self.path,
                generation,
                lease != 0,
                if lease != 0 { 60 } else { delay },
            )?
        };
        let state = if next == i64::MAX {
            CatalogProviderState::Paused
        } else if owned || (lease != 0 && remaining > 0) {
            CatalogProviderState::Active
        } else if remaining > 0 {
            CatalogProviderState::Backoff
        } else {
            CatalogProviderState::Available
        };
        Ok(CatalogStatus {
            state,
            retry_after_seconds: remaining.max(u32::from(owned)),
            failures,
        })
    }

    /// Only an unsupported-response or legacy-timing pause can be cleared.
    /// Ordinary backoff and live ownership cannot be overridden.
    pub fn resume_catalog_requests(&mut self, acknowledge_pause: bool) -> Result<()> {
        if !acknowledge_pause {
            return Err(Error::InvalidInput(
                "explicit provider-pause acknowledgment is required".into(),
            ));
        }
        let _lock = self.lock_catalog_request()?;
        let changed = self.connection.execute(
            "UPDATE catalog_gate SET generation=generation+1, lease_until_ms=0, failures=0,
                 next_request_ms=CAST(unixepoch('subsec') * 1000 AS INTEGER)+1000, delay_seconds=1
             WHERE id=1 AND next_request_ms=?1 AND lease_until_ms=0
                 AND generation<9223372036854775807",
            [i64::MAX],
        )?;
        if changed != 1 {
            return Err(Error::Conflict(
                "provider is not paused, has a live request, or its generation is exhausted".into(),
            ));
        }
        Ok(())
    }

    pub fn catalog_cache(&mut self, query: &CatalogQuery) -> Result<Option<CatalogCache>> {
        let cached = self.peek_catalog_cache(query)?;
        if cached.is_some() {
            self.connection.execute(
                "UPDATE catalog_cache SET accessed_at_ms = CAST(unixepoch('subsec') * 1000 AS INTEGER) WHERE source = ?1",
                [query.source()],
            )?;
        }
        Ok(cached)
    }

    pub(crate) fn peek_catalog_cache(&self, query: &CatalogQuery) -> Result<Option<CatalogCache>> {
        let cached = self
            .connection
            .query_row(
                "SELECT body, fetched_at, payload_sha256 FROM catalog_cache
             WHERE source = ?1 AND expires_at > unixepoch()",
                [query.source()],
                |row| {
                    Ok(CatalogCache {
                        body: row.get(0)?,
                        fetched_at: row.get(1)?,
                        payload_sha256: row.get(2)?,
                    })
                },
            )
            .optional()?;
        if let Some(cached) = &cached {
            let digest: [u8; 32] = Sha256::digest(&cached.body).into();
            if cached.payload_sha256.as_slice() != digest.as_slice() {
                return Err(Error::Internal("catalog cache checksum failed"));
            }
        }
        Ok(cached)
    }

    fn lock_catalog_request(&self) -> Result<File> {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".musicbrainz.lock");
        let lock_path = PathBuf::from(name);
        match std::fs::symlink_metadata(&lock_path) {
            Ok(metadata) if super::super::filesystem::is_unsafe_link(&metadata) => {
                return Err(Error::InvalidInput(
                    "provider lock must not be a link".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(lock_path)?;
        if !lock.metadata()?.is_file() {
            return Err(Error::InvalidInput(
                "provider lock must be a regular file".into(),
            ));
        }
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(Error::Catalog {
                    reason: "MusicBrainz request is owned by another live capability",
                    retry_after_seconds: 1,
                });
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        Ok(lock)
    }

    /// Commit before network I/O. One live request and at least one second
    /// after acquisition of the OS lock are required before transport starts.
    pub fn claim_catalog_request(&mut self) -> Result<CatalogPermit> {
        let lock = self.lock_catalog_request()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (now, generation, lease, next, delay): (i64, i64, i64, i64, u32) = transaction.query_row(
            "SELECT CAST(unixepoch('subsec') * 1000 AS INTEGER), generation, lease_until_ms, next_request_ms, delay_seconds
             FROM catalog_gate WHERE id = 1", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )?;
        let remaining = if next == i64::MAX {
            u32::MAX
        } else {
            super::catalog_clock::remaining(
                &self.path,
                generation,
                lease != 0,
                if lease != 0 { 60 } else { delay },
            )?
        };
        if remaining > 0 {
            return Err(Error::Catalog {
                reason: "MusicBrainz request is active or in backoff",
                retry_after_seconds: remaining,
            });
        }
        let generation = generation
            .checked_add(1)
            .ok_or(Error::Internal("catalog request generation exhausted"))?;
        transaction.execute(
            "UPDATE catalog_gate SET generation = ?1, lease_until_ms = ?2 WHERE id = 1",
            params![generation, now + 60000],
        )?;
        transaction.commit()?;
        Ok(CatalogPermit {
            generation,
            issued: Instant::now(),
            started: false,
            database: self.path.clone(),
            _lock: lock,
        })
    }

    /// Cache only a validated successful response. Failure preserves a bounded
    /// exponential cooldown and honors a longer provider Retry-After interval.
    pub fn complete_catalog_request(
        &mut self,
        permit: CatalogPermit,
        retry_after_seconds: u32,
        success: Option<(&CatalogQuery, &[u8])>,
    ) -> Result<CatalogCompletion> {
        if let Some((query, body)) = success {
            if body.is_empty() || body.len() > MAX_CATALOG_BODY {
                return Err(Error::InvalidInput(
                    "catalog cache body exceeds its bound".into(),
                ));
            }
            query.validate_response(body)?;
        }
        self.finish_catalog_request(
            permit,
            retry_after_seconds,
            success.is_some(),
            success.map(|(query, body)| (query.source(), body)),
            None,
        )
    }

    pub(crate) fn complete_uncached_catalog_request(
        &mut self,
        permit: CatalogPermit,
        retry_after_seconds: u32,
        successful: bool,
    ) -> Result<CatalogCompletion> {
        self.finish_catalog_request(permit, retry_after_seconds, successful, None, None)
    }

    pub(crate) fn complete_caa_request(
        &mut self,
        permit: CatalogPermit,
        retry_after_seconds: u32,
        request: &crate::infrastructure::caa::CaaRequest,
        fetched: &crate::infrastructure::caa::CaaFetch,
    ) -> Result<CatalogCompletion> {
        request.validate_sources(&fetched.sources)?;
        request.validate_payload(&fetched.payload)?;
        self.finish_catalog_request(
            permit,
            retry_after_seconds,
            true,
            None,
            Some((request, fetched)),
        )
    }

    fn finish_catalog_request(
        &mut self,
        permit: CatalogPermit,
        retry_after_seconds: u32,
        successful: bool,
        success: Option<(&str, &[u8])>,
        caa: Option<(
            &crate::infrastructure::caa::CaaRequest,
            &crate::infrastructure::caa::CaaFetch,
        )>,
    ) -> Result<CatalogCompletion> {
        if permit.database != self.path {
            return Err(Error::Conflict(
                "catalog permit belongs to another database".into(),
            ));
        }
        if successful && !permit.started {
            return Err(Error::Conflict(
                "catalog response has no started network request".into(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (now, generation, lease, failures): (i64, i64, i64, u32) = transaction.query_row(
            "SELECT CAST(unixepoch('subsec') * 1000 AS INTEGER), generation, lease_until_ms, failures
             FROM catalog_gate WHERE id = 1", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        if generation != permit.generation || lease <= 0 {
            return Err(Error::Conflict(
                "catalog request lease changed or expired".into(),
            ));
        }
        let (cooldown, failures) = if successful {
            (retry_after_seconds.max(1), 0)
        } else {
            (
                retry_after_seconds.max((5u32 << failures.min(6)).min(300)),
                (failures + 1).min(10),
            )
        };
        transaction.execute(
            "UPDATE catalog_gate SET lease_until_ms = 0, next_request_ms = ?1, failures = ?2, delay_seconds=?3 WHERE id = 1",
            params![if cooldown == u32::MAX { i64::MAX } else { now + i64::from(cooldown) * 1000 }, failures, cooldown],
        )?;
        if let Some((request, fetched)) = caa {
            super::caa::persist_cache(&transaction, request, fetched, now)?;
        }
        let cached = if let Some((source, body)) = success {
            let fetched_at = now / 1000;
            let payload_sha256 = Sha256::digest(body).to_vec();
            transaction.execute(
                "DELETE FROM catalog_cache WHERE expires_at <= unixepoch()",
                [],
            )?;
            transaction.execute(
                "INSERT INTO catalog_cache(source, body, payload_sha256, fetched_at, expires_at, accessed_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(source) DO UPDATE SET body=excluded.body, payload_sha256=excluded.payload_sha256,
                     fetched_at=excluded.fetched_at, expires_at=excluded.expires_at, accessed_at_ms=excluded.accessed_at_ms",
                params![source, body, payload_sha256, fetched_at, fetched_at + 86400, now],
            )?;
            transaction.execute(
                "DELETE FROM catalog_cache WHERE source NOT IN (
                    SELECT source FROM (
                      SELECT source, row_number() OVER (ORDER BY accessed_at_ms DESC, source) AS rank,
                        sum(length(body)) OVER (ORDER BY accessed_at_ms DESC, source) AS bytes
                      FROM catalog_cache)
                    WHERE rank <= 64 AND bytes <= 16777216)",
                [],
            )?;
            Some(CatalogCache {
                body: body.to_vec(),
                fetched_at,
                payload_sha256,
            })
        } else {
            None
        };
        transaction.commit()?;
        if cooldown != u32::MAX {
            super::catalog_clock::remaining(&self.path, generation, false, cooldown)?;
        }
        Ok(CatalogCompletion {
            cache: cached,
            retry_after_seconds: cooldown,
            fetched_at: now / 1000,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn permit(store: &mut Store) -> Result<CatalogPermit> {
        store.connection.execute(
            "UPDATE catalog_gate SET lease_until_ms=0, next_request_ms=0, delay_seconds=0",
            [],
        )?;
        let mut permit = store.claim_catalog_request()?;
        permit.issued -= Duration::from_secs(1);
        permit.check_start()?;
        Ok(permit)
    }

    fn body() -> Vec<u8> {
        br#"{"offset":0,"count":0,"artists":[]}"#.to_vec()
    }

    #[test]
    fn uncached_catalog_completion_keeps_fencing_failure_backoff_and_empty_cache() -> TestResult {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(&directory.path().join("state.db"))?;
        let unstarted = store.claim_catalog_request()?;
        assert!(
            store
                .complete_uncached_catalog_request(unstarted, 0, true)
                .is_err()
        );
        let grant = permit(&mut store)?;
        let complete = store.complete_uncached_catalog_request(grant, 0, true)?;
        assert!(complete.cache.is_none());
        assert_eq!(complete.retry_after_seconds, 1);
        assert!(store.claim_catalog_request().is_err());
        let grant = permit(&mut store)?;
        let complete = store.complete_uncached_catalog_request(grant, 900, false)?;
        assert_eq!(complete.retry_after_seconds, 900);
        assert_eq!(store.catalog_status()?.failures, 1);
        let count: i64 =
            store
                .connection
                .query_row("SELECT count(*) FROM catalog_cache", [], |row| row.get(0))?;
        assert_eq!(count, 0);
        let other = tempfile::tempdir()?;
        let mut other = Store::open(&other.path().join("other.db"))?;
        let grant = permit(&mut store)?;
        assert!(
            other
                .complete_uncached_catalog_request(grant, 0, true)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn catalog_cache_is_bounded_expiring_and_checksum_verified() -> TestResult {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(&directory.path().join("state.db"))?;
        for index in 0..70 {
            let query = CatalogQuery::artists(&format!("Fixture {index}"), 0, 25)?;
            let permit = permit(&mut store)?;
            store.complete_catalog_request(permit, 0, Some((&query, &body())))?;
        }
        let count: i64 =
            store
                .connection
                .query_row("SELECT count(*) FROM catalog_cache", [], |row| row.get(0))?;
        assert_eq!(count, 64);
        let mut large = body();
        large.resize(MAX_CATALOG_BODY, b' ');
        for index in 0..9 {
            let query = CatalogQuery::artists(&format!("Large {index}"), 0, 25)?;
            let permit = permit(&mut store)?;
            store.complete_catalog_request(permit, 0, Some((&query, &large)))?;
        }
        let bytes: i64 = store.connection.query_row(
            "SELECT sum(length(body)) FROM catalog_cache",
            [],
            |row| row.get(0),
        )?;
        assert!(bytes <= 16 * 1024 * 1024);
        let query = CatalogQuery::artists("Checksum", 0, 25)?;
        let permit = permit(&mut store)?;
        store.complete_catalog_request(permit, 0, Some((&query, &body())))?;
        assert_eq!(
            store.catalog_cache(&query)?.ok_or("cache missing")?.body,
            body()
        );
        store.connection.execute(
            "UPDATE catalog_cache SET payload_sha256=zeroblob(32) WHERE source=?1",
            [query.source()],
        )?;
        assert!(matches!(
            store.catalog_cache(&query),
            Err(Error::Internal(_))
        ));
        store.connection.execute("UPDATE catalog_cache SET fetched_at=unixepoch()-100000, expires_at=unixepoch()-1 WHERE source=?1", [query.source()])?;
        assert!(store.catalog_cache(&query)?.is_none());
        Ok(())
    }

    #[test]
    fn catalog_permit_is_single_use_and_cache_commit_is_atomic() -> TestResult {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(&directory.path().join("state.db"))?;
        let mut grant = store.claim_catalog_request()?;
        assert!(grant.check_start().is_err());
        grant.issued -= Duration::from_secs(5);
        assert!(grant.check_start().is_err());
        grant.issued = Instant::now() - Duration::from_secs(1);
        grant.check_start()?;
        assert!(grant.check_start().is_err());
        let query = CatalogQuery::artists("Fixture", 0, 25)?;
        store.connection.execute_batch(
            "CREATE TRIGGER fail_catalog_cache BEFORE INSERT ON catalog_cache
             BEGIN SELECT RAISE(ABORT, 'fixture cache failure'); END;",
        )?;
        assert!(
            store
                .complete_catalog_request(grant, 0, Some((&query, &body())))
                .is_err()
        );
        assert!(store.catalog_cache(&query)?.is_none());
        assert!(matches!(
            store.claim_catalog_request(),
            Err(Error::Catalog { .. })
        ));
        store
            .connection
            .execute_batch("DROP TRIGGER fail_catalog_cache")?;
        let grant = permit(&mut store)?;
        assert!(
            store
                .complete_catalog_request(grant, 0, Some((&query, b"not JSON")))
                .is_err()
        );
        assert!(store.catalog_cache(&query)?.is_none());
        Ok(())
    }

    #[test]
    fn catalog_os_owner_survives_expired_wall_lease_and_cannot_complete_another_database()
    -> TestResult {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(&directory.path().join("state.db"))?;
        let mut other = Store::open(&directory.path().join("other.db"))?;
        let query = CatalogQuery::artists("Fixture", 0, 25)?;
        let grant = permit(&mut store)?;
        store
            .connection
            .execute("UPDATE catalog_gate SET lease_until_ms=1", [])?;
        assert!(matches!(
            store.catalog_status()?.state,
            CatalogProviderState::Active
        ));
        store.complete_catalog_request(grant, 0, Some((&query, &body())))?;
        assert!(store.catalog_cache(&query)?.is_some());
        let grant = permit(&mut store)?;
        let _other_grant = other.claim_catalog_request()?;
        assert!(matches!(
            other.complete_catalog_request(grant, 0, Some((&query, &body()))),
            Err(Error::Conflict(message)) if message.contains("another database")
        ));
        assert!(other.catalog_cache(&query)?.is_none());
        Ok(())
    }

    #[test]
    fn catalog_backoff_ignores_past_and_future_wall_deadlines() -> TestResult {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(&directory.path().join("state.db"))?;
        let grant = store.claim_catalog_request()?;
        store.complete_catalog_request(grant, 120, None)?;
        for next in [1i64, i64::MAX - 1] {
            store
                .connection
                .execute("UPDATE catalog_gate SET next_request_ms=?1", [next])?;
            assert!(matches!(
                store.catalog_status()?.state,
                CatalogProviderState::Backoff
            ));
            assert!(matches!(
                store.claim_catalog_request(),
                Err(Error::Catalog {
                    retry_after_seconds: 119..=120,
                    ..
                })
            ));
        }
        Ok(())
    }
}
