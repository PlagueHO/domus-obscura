use crate::infrastructure::publication::PublicationDirectory;
use crate::infrastructure::staging::StagingRoot;
use crate::infrastructure::store::{JobClaim, PublicationDestination, Store};
use crate::{Error, Result};
use std::cell::Cell;
use std::time::{Duration, Instant};

struct Heartbeat {
    checked: Cell<Instant>,
}

impl Heartbeat {
    fn new() -> Self {
        Self {
            checked: Cell::new(Instant::now()),
        }
    }

    fn tick(&self, check: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
        if self.checked.get().elapsed() >= Duration::from_secs(30) {
            check()?;
            self.checked.set(Instant::now());
        }
        Ok(())
    }
}

/// Publish only into new, journal-owned album folders. A conflict never grants
/// replacement authority. Failures retain staged input and partial destination
/// copies; a later run verifies them instead of blindly overwriting.
pub fn publish_new_album(
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    run: i64,
) -> Result<String> {
    staging.check_store(store)?;
    store.renew_claim(claim, 300)?;
    let (normalized, _, work) = store.publication_source(claim, run)?;
    let source = staging.open_normalization(&normalized)?;
    {
        let heartbeat = Heartbeat::new();
        let tick = || {
            heartbeat.tick(|| {
                store
                    .renew_claim(claim, 300)
                    .map_err(std::io::Error::other)?;
                staging.check_store(store).map_err(std::io::Error::other)
            })
        };
        source.publication_input(None, &tick)?;
        for item in &work {
            source.publication_input(Some(item), &tick)?;
        }
    }
    let mut targets = store.begin_publication(claim, run)?;
    targets.sort_by_key(|target| (target.kind != "main", target.id));
    for target in targets {
        let result: Result<()> = (|| {
            store.renew_claim(claim, 300)?;
            let directory = PublicationDirectory::open(store, &target)?;
            if target.album_identity.is_none() {
                store.bind_publication_album(claim, target.id, directory.identity())?;
            }
            let target = store.check_publication(claim, target.id)?;
            let files = store.publication_files(claim, target.id)?;
            for file in &files {
                store.renew_claim(claim, 300)?;
                let item = if file.ordinal == 0 {
                    None
                } else {
                    Some(
                        work.get(usize::from(file.ordinal - 1))
                            .ok_or(Error::Internal("publication track is missing"))?,
                    )
                };
                {
                    check(store, claim, staging, &directory, &target)?;
                    let heartbeat = Heartbeat::new();
                    let tick =
                        || heartbeat.tick(|| check(store, claim, staging, &directory, &target));
                    let mut input = source.publication_input(item, &tick)?;
                    check(store, claim, staging, &directory, &target)?;
                    directory.copy(file, &mut input, &tick)?;
                    check(store, claim, staging, &directory, &target)?;
                }
                store.record_publication_file(claim, target.id, file)?;
            }
            let heartbeat = Heartbeat::new();
            check(store, claim, staging, &directory, &target)?;
            directory.verify(&files, &|| {
                heartbeat.tick(|| check(store, claim, staging, &directory, &target))
            })?;
            check(store, claim, staging, &directory, &target)?;
            store.finish_publication_destination(claim, target.id)
        })();
        if let Err(error) = result {
            store.publication_error(claim, target.id, &error.to_string())?;
            if target.kind == "main" {
                return Err(error);
            }
        }
    }
    // A backup can take hours. Recheck every successful destination before
    // completing, including the primary copy made before the backup work.
    let mut verified = store.publication_destinations(claim.job(), claim.plan_version())?;
    verified.sort_by_key(|target| target.kind == "main");
    for target in verified {
        if target.verified_at.is_none() || target.last_error.is_some() {
            continue;
        }
        let result: Result<()> = (|| {
            let directory = PublicationDirectory::open(store, &target)?;
            let files = store.publication_files(claim, target.id)?;
            let heartbeat = Heartbeat::new();
            check(store, claim, staging, &directory, &target)?;
            directory.verify(&files, &|| {
                heartbeat.tick(|| check(store, claim, staging, &directory, &target))
            })?;
            check(store, claim, staging, &directory, &target)?;
            Ok(())
        })();
        if let Err(error) = result {
            store.publication_error(claim, target.id, &error.to_string())?;
            if target.kind == "main" {
                return Err(error);
            }
        }
    }
    store.finish_publication(claim, run)
}

fn check(
    store: &Store,
    claim: &JobClaim,
    staging: &StagingRoot,
    directory: &PublicationDirectory,
    target: &PublicationDestination,
) -> std::io::Result<()> {
    store
        .renew_claim(claim, 300)
        .map_err(std::io::Error::other)?;
    staging.check_store(store).map_err(std::io::Error::other)?;
    store
        .check_publication(claim, target.id)
        .map_err(std::io::Error::other)?;
    directory
        .check(store, target)
        .map_err(std::io::Error::other)
}
