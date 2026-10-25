use crate::domain::Page;
use crate::infrastructure::staging::StagingRoot;
use crate::infrastructure::store::{JobClaim, Store};
use crate::{Error, Result};
use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Copy a complete normalized album to a new playback tree inside private
/// staging. This is test output, not primary/backup publication or job completion.
pub fn export_playback_album(
    store: &Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    run_id: i64,
) -> Result<PathBuf> {
    staging.check_store(store)?;
    let (run, plan) = store.completed_normalization(claim, run_id)?;
    let cover = store.approved_artwork(plan.job_id, plan.version)?;
    let mut work = Vec::new();
    let mut after = 0;
    loop {
        let page = store.completed_normalization_work(claim, run_id, Page::new(after, 200)?)?;
        if page.is_empty() {
            break;
        }
        after = i64::from(
            page.last()
                .ok_or(Error::Internal("nonempty work page is empty"))?
                .track_number,
        );
        work.extend(page);
        if work.len() > 1000 {
            return Err(Error::Conflict(
                "playback export exceeds its album limit".into(),
            ));
        }
    }
    let renewed = Cell::new(Instant::now());
    let check = || {
        if renewed.get().elapsed() >= Duration::from_secs(30) {
            store
                .renew_claim(claim, 300)
                .map_err(std::io::Error::other)?;
            renewed.set(Instant::now());
        }
        store
            .completed_normalization(claim, run_id)
            .map_err(std::io::Error::other)?;
        Ok(())
    };
    staging.export_playback(&run, &work, &plan.plan.outputs, &cover, &check)
}
