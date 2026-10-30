use crate::domain::ReplacementAction;
use crate::infrastructure::publication::PublicationDirectory;
use crate::infrastructure::staging::StagingRoot;
use crate::infrastructure::store::{
    AlbumReplacementPreview, AlbumReplacementRestoreDecision, JobClaim, PublicationDestination,
    Store,
};
use crate::{Error, Result};
use std::cell::Cell;
use std::time::{Duration, Instant};

pub fn preview_album_replacements(
    store: &Store,
    job: i64,
    plan_version: i64,
) -> Result<Vec<AlbumReplacementPreview>> {
    crate::infrastructure::publication::album_replacement_previews(store, job, plan_version)
}

pub fn restore_album_replacement(
    store: &mut Store,
    job: i64,
    plan_version: i64,
    location_id: i64,
    decision: AlbumReplacementRestoreDecision,
) -> Result<PublicationDestination> {
    let _visibility = crate::infrastructure::shares::publication_visibility_write(store)?;
    crate::infrastructure::publication::restore_album_replacement(
        store,
        job,
        plan_version,
        location_id,
        decision,
    )?;
    store
        .publication_destinations(job, plan_version)?
        .into_iter()
        .find(|destination| destination.location_id == location_id)
        .ok_or_else(|| Error::Conflict("replacement destination is no longer planned".into()))
}

/// Inspect one destination without granting approval or changing media.
pub fn preview_album_restore(
    store: &mut Store,
    job: i64,
    plan_version: i64,
    location_id: i64,
) -> Result<crate::infrastructure::store::AlbumRestorePreview> {
    let _visibility = crate::infrastructure::shares::publication_visibility_write(store)?;
    crate::infrastructure::publication::preview_album_restore(store, job, plan_version, location_id)
}

pub(crate) fn preview_album_replacement_for_location(
    store: &Store,
    job: i64,
    plan_version: i64,
    location_id: i64,
) -> Result<AlbumReplacementPreview> {
    crate::infrastructure::publication::album_replacement_preview_for_location(
        store,
        job,
        plan_version,
        location_id,
    )
}

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
    store.check_worker_stop()?;
    // Acquire before durable destination intent and retain through verification.
    // Recovery and replacement must use this same cross-process boundary.
    let _visibility = crate::infrastructure::shares::publication_visibility_write(store)?;
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
    let reviews = store.album_replacement_reviews(claim.job(), claim.plan_version())?;
    targets.sort_by_key(|target| (target.kind != "main", target.id));
    for target in targets {
        store.check_worker_stop()?;
        let result: Result<()> = (|| {
            store.renew_claim(claim, 300)?;
            let files = store.publication_files(claim, target.id)?;
            if target.verified_at.is_none()
                && target.replacement.is_none()
                && let Some(review) = reviews.iter().find(|review| {
                    review.location_id == target.location_id
                        && review.action == ReplacementAction::Copy
                })
            {
                let current = preview_album_replacement_for_location(
                    store,
                    claim.job(),
                    claim.plan_version(),
                    target.location_id,
                )?;
                if current.manifest_sha256 != review.manifest_sha256
                    || current.album_identity.as_deref() != Some(review.album_identity.as_str())
                    || current.root_identity != review.root_identity
                {
                    return Err(Error::Conflict(
                        "the reviewed album changed; save a new replacement decision".into(),
                    ));
                }
            }
            let mut preparation = store.prepare_publication_path(claim, target.id)?;
            let mut opened = None;
            for _ in 0..32 {
                opened = PublicationDirectory::open(store, &target, &preparation)?;
                if opened.is_some() {
                    break;
                }
                preparation = store.prepare_publication_path(claim, target.id)?;
            }
            let mut directory = opened.ok_or(Error::Conflict(
                "could not reserve a private publication folder".into(),
            ))?;
            if target.album_identity.is_none() {
                store.bind_publication_album(claim, target.id, directory.identity())?;
            }
            let target = store.check_publication(claim, target.id)?;
            if directory.is_visible() {
                let heartbeat = Heartbeat::new();
                check(store, claim, staging, &directory, &target)?;
                directory.verify(&files, &|| {
                    heartbeat.tick(|| check(store, claim, staging, &directory, &target))
                })?;
                check(store, claim, staging, &directory, &target)?;
                if target.replacement.is_some() {
                    if target
                        .replacement
                        .as_ref()
                        .is_some_and(|replacement| replacement.state == "planned")
                    {
                        store.record_publication_replacement_state(claim, target.id, "retained")?;
                    }
                    store.record_publication_replacement_state(claim, target.id, "published")?;
                }
                store.record_publication_visible(claim, target.id)?;
                return store.finish_publication_destination(claim, target.id);
            }
            for file in &files {
                store.check_worker_stop()?;
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
            directory.publish(store, claim, &target)?;
            let heartbeat = Heartbeat::new();
            directory.verify(&files, &|| {
                heartbeat.tick(|| check(store, claim, staging, &directory, &target))
            })?;
            check(store, claim, staging, &directory, &target)?;
            store.record_publication_visible(claim, target.id)?;
            store.finish_publication_destination(claim, target.id)
        })();
        if let Err(error) = result {
            if matches!(error, Error::WorkerStopping) {
                return Err(error);
            }
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
            let preparation = store.publication_preparation(claim, target.id)?;
            let directory = PublicationDirectory::open(store, &target, &preparation)?.ok_or(
                Error::Conflict("published album folder is unavailable".into()),
            )?;
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
    drop(source);
    let (path, identity) = staging.cleanup_binding()?;
    let state = store.finish_publication(claim, run, (&path, &identity))?;
    if state == "complete" {
        // Album completion and cleanup have separate durable results.
        if let Err(error) = super::recovery::cleanup_scratch_locked(store, staging, run) {
            eprintln!("album publication completed; scratch cleanup needs explicit retry: {error}");
        }
    }
    Ok(state)
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
