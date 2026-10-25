use crate::domain::Page;
use crate::infrastructure::artwork::CoverJpeg;
use crate::infrastructure::staging::StagingRoot;
use crate::infrastructure::store::{JobClaim, Store};
use crate::{Error, Result};
use std::cell::Cell;
use std::time::{Duration, Instant};

struct ClaimHeartbeat<'a> {
    store: &'a Store,
    claim: &'a JobClaim,
    renewed: Cell<Instant>,
}

impl ClaimHeartbeat<'_> {
    fn tick(&self) -> std::io::Result<()> {
        if self.renewed.get().elapsed() >= Duration::from_secs(30) {
            self.store
                .renew_claim(self.claim, 300)
                .map_err(std::io::Error::other)?;
            self.renewed.set(Instant::now());
        }
        Ok(())
    }
}

/// Normalize only already-received private inputs with the durably approved
/// cover. This does not fetch, publish, replace or delete media.
pub fn normalize_album(
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    cover: &CoverJpeg,
) -> Result<i64> {
    staging.check_store(store)?;
    store.renew_claim(claim, 300)?;
    store.job_work_boundary(claim)?;
    let run = match store.normalization_run(claim)? {
        Some(run) => run,
        None => store.begin_normalization(claim, cover)?,
    };
    let cover_hash = cover
        .sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if run.cover_sha256 != cover_hash
        || run.cover_bytes != cover.bytes().len() as u64
        || (run.cover_width, run.cover_height) != cover.dimensions()
    {
        return Err(Error::Conflict(
            "cover differs from the frozen normalization recipe".into(),
        ));
    }
    let guard = store.normalization_guard(claim, run.id)?;
    store.check_normalization_guard(claim, &guard)?;
    let directory = match staging.create_normalization(&run) {
        Ok(directory) => directory,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            staging.open_normalization(&run)?
        }
        Err(error) => return Err(error),
    };
    let mut after = 0;
    loop {
        let work = store.normalization_work(claim, run.id, Page::new(after, 200)?)?;
        if work.is_empty() {
            break;
        }
        for item in work {
            store.renew_claim(claim, 300)?;
            store.job_work_boundary(claim)?;
            store.check_normalization_guard(claim, &guard)?;
            let evidence = {
                let lease = ClaimHeartbeat {
                    store,
                    claim,
                    renewed: Cell::new(Instant::now()),
                };
                let heartbeat = || lease.tick();
                if item.receipt.is_some() {
                    directory.verify_audio_with_progress(&item, &heartbeat)?;
                    None
                } else {
                    let mut input = staging.open_input_with_progress(&item, &heartbeat)?;
                    Some(directory.normalize_with_progress(&item, &mut input, cover, &heartbeat)?)
                }
            };
            if let Some(evidence) = evidence {
                store.record_normalized_audio(claim, run.id, item.track_number, &evidence)?;
            }
            after = i64::from(item.track_number);
        }
    }
    store.renew_claim(claim, 300)?;
    store.job_work_boundary(claim)?;
    store.check_normalization_guard(claim, &guard)?;
    if run.cover_verified_at.is_some() {
        directory.verify_cover()?;
    } else {
        directory.write_cover(cover)?;
        store.record_normalized_cover(claim, run.id, run.cover_bytes, &run.cover_sha256)?;
    }
    store.job_work_boundary(claim)?;
    store.finish_normalization(claim, run.id)?;
    Ok(run.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_heartbeat_renews_at_thirty_seconds_and_rejects_reclaimed_ownership()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let job = store.enqueue(1)?;
        let claim = store.claim(job, "worker", 300)?.ok_or("missing claim")?;
        let raw = rusqlite::Connection::open(database)?;
        raw.execute_batch(
            "CREATE TRIGGER heartbeat_test AFTER UPDATE OF lease_until ON jobs
            BEGIN INSERT INTO events(job_id, event_type, plan_version)
            VALUES (NEW.id, 'heartbeat_test', NEW.plan_version); END;",
        )?;
        let heartbeat = ClaimHeartbeat {
            store: &store,
            claim: &claim,
            renewed: Cell::new(Instant::now() - Duration::from_secs(29)),
        };
        let count = || {
            raw.query_row(
                "SELECT count(*) FROM events WHERE event_type = 'heartbeat_test'",
                [],
                |row| row.get::<_, i64>(0),
            )
        };
        heartbeat.tick()?;
        assert_eq!(count()?, 0);
        heartbeat
            .renewed
            .set(Instant::now() - Duration::from_secs(31));
        heartbeat.tick()?;
        assert_eq!(count()?, 1);
        let remaining: i64 = raw.query_row(
            "SELECT lease_until - unixepoch() FROM jobs WHERE id = ?1",
            [job],
            |row| row.get(0),
        )?;
        assert!((299..=300).contains(&remaining));
        heartbeat.tick()?;
        assert_eq!(count()?, 1);
        raw.execute(
            "UPDATE jobs SET lease_until = unixepoch() - 1 WHERE id = ?1",
            [job],
        )?;
        let _new_claim = store
            .claim(job, "worker", 300)?
            .ok_or("missing reclaimed owner")?;
        let before = count()?;
        heartbeat
            .renewed
            .set(Instant::now() - Duration::from_secs(31));
        assert!(heartbeat.tick().is_err());
        assert_eq!(count()?, before);
        Ok(())
    }
}
