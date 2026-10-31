use super::Store;
use crate::application::services::{EntityRef, artist_images::*};
use crate::infrastructure::local_file::ProcessLock;
use crate::{Error, Result};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn commons_clock_key(path: &std::path::Path) -> PathBuf {
    let mut key = path.as_os_str().to_os_string();
    key.push(".commons-clock");
    PathBuf::from(key)
}

/// The native lock stays alive through HTTP, header persistence and cache I/O.
pub(crate) struct CommonsPermit {
    _lock: ProcessLock,
    generation: i64,
    path: PathBuf,
    issued: Instant,
    started: bool,
}

#[cfg(test)]
pub(crate) mod tests;

impl CommonsPermit {
    pub(crate) async fn start(&mut self) -> Result<()> {
        tokio::time::sleep_until(tokio::time::Instant::from_std(
            self.issued + Duration::from_secs(1),
        ))
        .await;
        if self.started || self.issued.elapsed() > Duration::from_secs(5) {
            return Err(Error::Conflict(
                "Commons permit expired or was already used".into(),
            ));
        }
        self.started = true;
        Ok(())
    }

    pub(crate) fn delay(&self, store: &Store, seconds: u32) -> Result<()> {
        if store.database_path() != self.path || !self.started {
            return Err(Error::Conflict(
                "Commons permit does not own this request".into(),
            ));
        }
        if store.connection.execute(
            "UPDATE commons_gate SET delay_seconds=max(delay_seconds,?1) WHERE id=1 AND generation=?2 AND pending=1",
            params![seconds, self.generation],
        )? != 1 {
            return Err(Error::Conflict("Commons permit is stale".into()));
        }
        Ok(())
    }
}

impl Store {
    /// Only an explicit administrator command can clear an unsupported delay.
    /// Acquiring the native lock prevents cancellation of a live request.
    pub fn resume_commons(&mut self) -> Result<()> {
        let _lock = ProcessLock::acquire(&self.path, "commons")?;
        if self.connection.execute(
            "UPDATE commons_gate SET generation=generation+1,pending=0,delay_seconds=1
             WHERE id=1 AND delay_seconds=4294967295 AND generation<9223372036854775807",
            [],
        )? != 1
        {
            return Err(Error::Conflict(
                "Commons has no unsupported delay to resume".into(),
            ));
        }
        Ok(())
    }

    pub fn artist_photo_version(&self, artist: &EntityRef, provider: &str) -> Result<Option<i64>> {
        Ok(self.connection.query_row(
            "SELECT version FROM artist_photo_bindings WHERE artist_provider=?1 AND artist_id=?2 AND image_provider=?3",
            params![artist.provider, artist.id, provider], |r| r.get(0),
        ).optional()?)
    }

    pub(crate) fn claim_commons(&mut self) -> Result<CommonsPermit> {
        let lock = ProcessLock::acquire(&self.path, "commons")?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (generation, pending, delay): (i64, bool, u32) = transaction.query_row(
            "SELECT generation,pending,delay_seconds FROM commons_gate WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let key = commons_clock_key(&self.path);
        let delay = if pending { delay.max(60) } else { delay };
        let remaining = if delay == u32::MAX {
            delay
        } else {
            super::catalog_clock::remaining(&key, generation, pending, delay)?
        };
        if remaining > 0 {
            return Err(Error::Catalog {
                reason: "Commons requests are paused or in backoff",
                retry_after_seconds: remaining,
            });
        }
        let generation = generation
            .checked_add(1)
            .ok_or(Error::Internal("Commons generation exhausted"))?;
        transaction.execute(
            "UPDATE commons_gate SET generation=?1,pending=1,delay_seconds=0 WHERE id=1",
            [generation],
        )?;
        transaction.commit()?;
        Ok(CommonsPermit {
            _lock: lock,
            generation,
            path: self.path.clone(),
            issued: Instant::now(),
            started: false,
        })
    }

    pub(crate) fn complete_commons(&mut self, permit: CommonsPermit, success: bool) -> Result<()> {
        if permit.path != self.path {
            return Err(Error::Conflict(
                "Commons permit belongs to another database".into(),
            ));
        }
        if self.connection.execute(
            "UPDATE commons_gate SET pending=0,delay_seconds=max(delay_seconds,?1)
             WHERE id=1 AND generation=?2 AND pending=1",
            params![if success { 1 } else { 60 }, permit.generation],
        )? != 1
        {
            return Err(Error::Conflict("Commons completion is stale".into()));
        }
        let delay: u32 = self.connection.query_row(
            "SELECT delay_seconds FROM commons_gate WHERE id=1",
            [],
            |r| r.get(0),
        )?;
        if delay != u32::MAX {
            super::catalog_clock::remaining(
                &commons_clock_key(&self.path),
                permit.generation,
                false,
                delay,
            )?;
        }

        Ok(())
    }

    pub fn artist_photo_binding(
        &self,
        artist: &EntityRef,
        provider: &str,
    ) -> Result<Option<ArtistPhotoBinding>> {
        validate_artist(artist)?;
        let row: Option<(i64, i64, String)> = self.connection.query_row(
            "SELECT id,version,photo FROM artist_photo_bindings WHERE artist_provider=?1 AND artist_id=?2 AND image_provider=?3 AND photo IS NOT NULL",
            params![artist.provider, artist.id, provider],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).optional()?;
        row.map(|(id, version, photo)| {
            Ok(ArtistPhotoBinding {
                id,
                version,
                photo: serde_json::from_str(&photo)?,
            })
        })
        .transpose()
    }

    pub fn artist_photo_bindings(&self, after: i64, limit: u16) -> Result<Vec<ArtistPhotoBinding>> {
        if after < 0 || !(1..=50).contains(&limit) {
            return Err(Error::InvalidInput(
                "artist photo pages require a limit of 1 through 50".into(),
            ));
        }
        let mut statement = self.connection.prepare("SELECT id,version,photo FROM artist_photo_bindings WHERE id>?1 AND photo IS NOT NULL ORDER BY id LIMIT ?2")?;
        statement
            .query_map(params![after, limit], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .map(|r| {
                let (id, version, photo) = r?;
                Ok(ArtistPhotoBinding {
                    id,
                    version,
                    photo: serde_json::from_str(&photo)?,
                })
            })
            .collect()
    }

    pub fn save_artist_photo(
        &mut self,
        photo: &ArtistPhoto,
        expected: Option<i64>,
    ) -> Result<ArtistPhotoBinding> {
        photo.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(i64, i64)> = transaction.query_row(
            "SELECT id,version FROM artist_photo_bindings WHERE artist_provider=?1 AND artist_id=?2 AND image_provider=?3",
            params![photo.artist.provider, photo.artist.id, photo.provider], |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        if current.map(|(_, version)| version) != expected {
            return Err(Error::Conflict(
                "artist photo binding changed; reload before saving".into(),
            ));
        }
        let version = expected
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Internal("artist photo version exhausted"))?;
        transaction.execute(
            "INSERT INTO artist_photo_bindings(artist_provider,artist_id,image_provider,version,photo)
             VALUES(?1,?2,?3,?4,?5) ON CONFLICT(artist_provider,artist_id,image_provider)
             DO UPDATE SET version=excluded.version,photo=excluded.photo",
            params![photo.artist.provider, photo.artist.id, photo.provider, version, serde_json::to_string(photo)?],
        )?;
        let id = current.map_or_else(|| transaction.last_insert_rowid(), |(id, _)| id);
        transaction.commit()?;
        Ok(ArtistPhotoBinding {
            id,
            version,
            photo: photo.clone(),
        })
    }

    pub fn remove_artist_photo(&mut self, id: i64, expected: i64) -> Result<()> {
        if self.connection.execute(
            "UPDATE artist_photo_bindings SET photo=NULL,version=version+1 WHERE id=?1 AND version=?2 AND photo IS NOT NULL AND version<9223372036854775807",
            params![id, expected],
        )? != 1 {
            return Err(Error::Conflict("artist photo binding changed; reload before removal".into()));
        }
        Ok(())
    }
}
