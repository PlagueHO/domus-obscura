use super::*;
use domus::application::{normalize_album, publish_new_album};
use serde_json::json;
use std::path::Path;
use std::process::Command;

#[path = "recovery.rs"]
mod recovery;

fn normalized(fixture: &mut Fixture) -> Result<i64, Box<dyn std::error::Error>> {
    Ok(normalize_album(
        &mut fixture.store,
        &fixture.claim,
        fixture.staging.as_ref().ok_or("staging")?,
        &fixture.cover,
    )?)
}

#[test]
fn sharing_publication_visibility_retains_failed_exclusions_until_exact_completion() -> TestResult {
    use domus::application::sharing::{
        BindShareFolder, InspectShareFolder, bind_share_folder, inspect_share_folder,
    };
    use domus::domain::sharing::{ShareAccess, SharePolicy, ShareRoot};
    use domus::infrastructure::shares::ShareCatalog;
    use std::sync::{Arc, atomic::AtomicBool};
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let run = normalized(&mut fixture)?;
    let plan = fixture.store.plan(fixture.job, fixture.version)?;
    let location_id = plan
        .plan
        .destinations
        .iter()
        .find(|destination| destination.kind == "main")
        .ok_or("main destination")?
        .location_id;
    let inspected = inspect_share_folder(
        &fixture.store,
        &InspectShareFolder {
            location_id,
            relative_path: String::new(),
        },
    )?;
    let binding = bind_share_folder(
        &fixture.store,
        &BindShareFolder {
            location_id,
            relative_path: String::new(),
            expected_identity: inspected.native_identity,
        },
    )?;
    let policy = SharePolicy {
        access: ShareAccess::All,
        roots: vec![ShareRoot {
            binding_id: binding.binding_id.ok_or("share binding")?,
            alias: "Music".into(),
        }],
        ..SharePolicy::default()
    };
    let build =
        || ShareCatalog::build(&fixture.database, &policy, Arc::new(AtomicBool::new(false)));
    let before = build()?;
    let response = before.check()?;
    assert!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )
        .is_err()
    );
    assert!(
        fixture
            .store
            .publication_destinations(fixture.job, fixture.version)?
            .is_empty()
    );
    drop(response);
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("CREATE TRIGGER sharing_fail_receipt BEFORE UPDATE OF verified_at ON publication_files BEGIN SELECT RAISE(ABORT,'injected sharing interruption'); END;")?;
    assert!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )
        .is_err()
    );
    assert!(before.check().is_err());
    // A new catalog after the interruption must still hide the partial album.
    let hidden = build()?;
    assert_eq!(hidden.counts()?.1, 0);
    assert!(hidden.page(0, 20, None, None)?.is_empty());
    let reopened = Store::open_existing(&fixture.database)?;
    assert!(
        reopened
            .publication_destinations(fixture.job, fixture.version)?
            .iter()
            .all(|target| target.verified_at.is_none())
    );
    drop(reopened);
    raw.execute_batch("DROP TRIGGER sharing_fail_receipt")?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    assert!(hidden.check().is_err());
    let complete = build()?;
    assert_eq!(complete.counts()?.1 as usize, plan.plan.outputs.len() + 1);
    let file = complete
        .page(0, 1, None, None)?
        .pop()
        .ok_or("shared file")?;
    let read = complete.open(&file)?;
    assert_eq!(read.file.metadata()?.len(), file.bytes);
    read.check()?;
    drop(read);
    let cover = album(&fixture, fixture._library.path())?.join("folder.jpg");
    std::fs::remove_file(&cover)?;
    assert!(
        build().is_err(),
        "a missing receipted file must not create a partial shared album"
    );
    assert!(
        !cover.exists(),
        "sharing must not repair or rewrite publication output"
    );
    Ok(())
}

#[test]
fn browser_publication_recovery_requires_exact_retained_journal_and_never_rewrites_partial_files()
-> TestResult {
    use domus::infrastructure::worker::{ResumePublication, resume_publication};
    for partial in [false, true] {
        let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
        let run = normalized(&mut fixture)?;
        let resume = |run, acknowledge| {
            resume_publication(
                &fixture.database,
                fixture.scratch.path(),
                fixture.job,
                fixture.version,
                ResumePublication {
                    normalization_run: run,
                    acknowledge_new_only_publication: acknowledge,
                },
            )
        };
        assert!(resume(run, false).is_err());
        assert!(
            resume(run, true).is_err(),
            "no journal is not recovery authority"
        );
        let raw = Connection::open(&fixture.database)?;
        raw.execute_batch("CREATE TRIGGER fail_recovery_receipt BEFORE UPDATE OF verified_at ON publication_files BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        assert!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run
            )
            .is_err()
        );
        raw.execute_batch("DROP TRIGGER fail_recovery_receipt; UPDATE jobs SET lease_until=0;")?;
        let image = prepared_album(&fixture)?.join("folder.jpg");
        assert!(!album(&fixture, fixture._library.path())?.exists());
        if partial {
            std::fs::write(&image, b"retain partial image")?;
        }
        let before = std::fs::read(&image)?;
        drop(fixture.staging.take());
        let snapshot = serde_json::to_value(fixture.store.transfers(
            fixture.job,
            fixture.version,
            Page::new(0, 200)?,
        )?)?;
        let generation: i64 = raw.query_row(
            "SELECT claim_generation FROM jobs WHERE id=?1",
            [fixture.job],
            |row| row.get(0),
        )?;
        let result = resume_publication(
            &fixture.database,
            fixture.scratch.path(),
            fixture.job,
            fixture.version,
            ResumePublication {
                normalization_run: run + 1,
                acknowledge_new_only_publication: true,
            },
        );
        assert!(result.is_err());
        assert_eq!(
            raw.query_row(
                "SELECT claim_generation FROM jobs WHERE id=?1",
                [fixture.job],
                |row| row.get::<_, i64>(0)
            )?,
            generation
        );
        let result = resume_publication(
            &fixture.database,
            fixture.scratch.path(),
            fixture.job,
            fixture.version,
            ResumePublication {
                normalization_run: run,
                acknowledge_new_only_publication: true,
            },
        );
        if partial {
            assert!(result.is_err());
            assert_eq!(
                fixture.store.job_state(fixture.job)?.as_deref(),
                Some("publishing")
            );
        } else {
            assert_eq!(result?.state, "complete");
            verify_album(&fixture, run, fixture._library.path())?;
        }
        let published_image = album(&fixture, fixture._library.path())?.join("folder.jpg");
        assert_eq!(
            if partial {
                std::fs::read(&image)?
            } else {
                std::fs::read(published_image)?
            },
            before
        );
        assert_eq!(
            serde_json::to_value(fixture.store.transfers(
                fixture.job,
                fixture.version,
                Page::new(0, 200)?
            )?)?,
            snapshot
        );
        assert_eq!(
            raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| row
                .get::<_, i64>(0))?,
            1
        );
    }
    Ok(())
}

fn album(fixture: &Fixture, root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let plan = fixture.store.plan(fixture.job, fixture.version)?;
    Ok(root.join(
        plan.plan.outputs[0]
            .relative_folder
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    ))
}

fn prepared_album(fixture: &Fixture) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let raw = Connection::open(&fixture.database)?;
    let folder: String = raw.query_row(
        "SELECT prepared_folder FROM publication_destinations WHERE job_id=?1",
        [fixture.job],
        |row| row.get(0),
    )?;
    Ok(fixture
        ._library
        .path()
        .join(folder.replace('/', std::path::MAIN_SEPARATOR_STR)))
}

fn verify_album(fixture: &Fixture, run: i64, root: &Path) -> TestResult {
    let folder = album(fixture, root)?;
    let plan = fixture.store.plan(fixture.job, fixture.version)?;
    assert_eq!(
        std::fs::read(folder.join("folder.jpg"))?,
        fixture.cover.bytes()
    );
    assert_eq!(folder.read_dir()?.count(), plan.plan.outputs.len() + 1);
    let raw = Connection::open(&fixture.database)?;
    for (index, output) in plan.plan.outputs.iter().enumerate() {
        let (bytes,sha256):(i64,String)=raw.query_row(
            "SELECT output_bytes,output_sha256 FROM normalization_receipts WHERE run_id=?1 AND track_number=?2",
            params![run,index as i64+1],|row|Ok((row.get(0)?,row.get(1)?)),
        )?;
        let actual = std::fs::read(folder.join(&output.filename))?;
        assert_eq!(actual.len() as i64, bytes);
        assert_eq!(format!("{:x}", Sha256::digest(&actual)), sha256);
    }
    Ok(())
}

#[test]
fn replacement_retains_the_exact_original_folder_and_excludes_it_from_library() -> TestResult {
    let mut fixture = Fixture::build_with_replacement(
        AudioFormat::Flac,
        true,
        super::ReplacementAction::Replace,
    )?;
    let run = normalized(&mut fixture)?;

    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    verify_album(&fixture, run, fixture._library.path())?;

    let raw = Connection::open(&fixture.database)?;
    let retained_relative: String = raw.query_row(
        "SELECT retained_folder FROM publication_replacement_journal",
        [],
        |row| row.get(0),
    )?;
    let retained = fixture
        ._library
        .path()
        .join(retained_relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    assert_eq!(std::fs::read(retained.join("folder.jpg"))?, b"old cover");
    assert_eq!(std::fs::read(retained.join("manual.txt"))?, b"user extra");
    assert_eq!(
        raw.query_row(
            "SELECT state FROM publication_replacement_journal",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "published"
    );

    domus::application::scan_location(&mut fixture.store, fixture.main)?;
    let plan = fixture.store.plan(fixture.job, fixture.version)?;
    let parent = plan.plan.outputs[0]
        .relative_folder
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent);
    let entries = fixture.store.browse(
        fixture.main,
        &domus::domain::LibraryPath::parse(parent)?,
        Page::new(0, 100)?,
    )?;
    assert!(entries.iter().all(|entry| {
        !entry
            .relative_path
            .split('/')
            .any(|part| part.starts_with(".domus-retained-"))
    }));
    assert!(
        domus::application::sharing::inspect_share_folder(
            &fixture.store,
            &domus::application::sharing::InspectShareFolder {
                location_id: fixture.main,
                relative_path: retained_relative,
            },
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn replacement_restore_is_state_bound_and_recovers_after_displacing_the_current_folder()
-> TestResult {
    let mut fixture = Fixture::build_with_replacement(
        AudioFormat::Flac,
        true,
        super::ReplacementAction::Replace,
    )?;
    let run = normalized(&mut fixture)?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    let destination = fixture
        .store
        .publication_destinations(fixture.job, fixture.version)?
        .into_iter()
        .find(|destination| destination.location_id == fixture.main)
        .ok_or("main destination")?;
    let replacement = destination
        .replacement
        .as_ref()
        .ok_or("replacement journal")?;
    let decision = domus::infrastructure::store::AlbumReplacementRestoreDecision {
        expected_preview_sha256: None,
        review_id: replacement.review_id,
        expected_manifest_sha256: destination.manifest_sha256.clone(),
        expected_album_identity: destination
            .album_identity
            .clone()
            .ok_or("published album identity")?,
    };

    let mut stale = decision.clone();
    stale.expected_manifest_sha256 = "0".repeat(64);
    assert!(
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
            stale
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(album(&fixture, fixture._library.path())?.join("folder.jpg"))?,
        fixture.cover.bytes()
    );

    let current = album(&fixture, fixture._library.path())?;
    let temporary = fixture._library.path().join(
        replacement
            .restore_folder
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    let raw = Connection::open(&fixture.database)?;
    raw.execute(
        "UPDATE publication_replacement_journal
         SET state='restore_prepared',restore_started_at=unixepoch()
         WHERE destination_id=?1 AND state='published'",
        [destination.id],
    )?;
    std::fs::rename(&current, &temporary)?;
    raw.execute_batch(
        "DROP TABLE artist_photo_bindings;
         DROP TABLE commons_gate;
         DROP VIEW recovery_inventory;
         DROP INDEX recovery_input_cursor;
         DROP INDEX recovery_normalization_cursor;
         DROP INDEX recovery_pending_cursor;
         DROP INDEX recovery_retained_cursor;
         DROP INDEX recovery_input_job_cursor;
         DROP INDEX recovery_normalization_job_cursor;
         DROP INDEX recovery_pending_job_cursor;
         DROP INDEX recovery_pending_location_cursor;
         DROP INDEX recovery_pending_job_location_cursor;
         DROP INDEX recovery_destination_job_cursor;
         DROP INDEX recovery_destination_location_cursor;
         DROP INDEX recovery_destination_job_location_cursor;
         DROP TABLE recovery_items;
         DROP TABLE recovery_operations;
         DROP TABLE scratch_cleanup_admissions;
         DROP TRIGGER immutable_album_restore_question_binding;
         DROP TRIGGER retained_album_restore_approval;
         DROP TRIGGER immutable_album_restore_approval;
         DROP TABLE album_restore_questions;
         DROP TABLE album_restore_status;
         DROP TABLE album_restore_approvals;
         PRAGMA user_version=27;",
    )?;
    fixture.store = Store::open(&fixture.database)?;
    assert!(
        domus::application::sharing::inspect_share_folder(
            &fixture.store,
            &domus::application::sharing::InspectShareFolder {
                location_id: fixture.main,
                relative_path: replacement.restore_folder.clone(),
            },
        )
        .is_err()
    );
    let restored = domus::application::restore_album_replacement(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        fixture.main,
        decision.clone(),
    )?;
    assert_eq!(
        restored
            .replacement
            .as_ref()
            .ok_or("replacement journal")?
            .state,
        "restored"
    );

    let restored_album = album(&fixture, fixture._library.path())?;
    assert_eq!(
        std::fs::read(restored_album.join("folder.jpg"))?,
        b"old cover"
    );
    assert_eq!(
        std::fs::read(restored_album.join("manual.txt"))?,
        b"user extra"
    );
    let retained = fixture._library.path().join(
        replacement
            .retained_folder
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    assert_eq!(
        std::fs::read(retained.join("folder.jpg"))?,
        fixture.cover.bytes()
    );
    assert!(!temporary.exists());
    let restored_again = domus::application::restore_album_replacement(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        fixture.main,
        decision,
    )?;
    assert_eq!(
        restored_again
            .replacement
            .as_ref()
            .ok_or("replacement journal")?
            .state,
        "restored"
    );
    assert_eq!(
        raw.query_row("SELECT COUNT(*) FROM album_restore_approvals", [], |row| {
            row.get::<_, i64>(0)
        })?,
        0,
    );
    Ok(())
}

#[test]
fn replacement_rejects_changed_original_files_without_displacing_them() -> TestResult {
    let mut fixture = Fixture::build_with_replacement(
        AudioFormat::Flac,
        true,
        super::ReplacementAction::Replace,
    )?;
    let original = album(&fixture, fixture._library.path())?;
    std::fs::write(original.join("manual.txt"), b"changed content")?;
    let run = normalized(&mut fixture)?;

    assert!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(original.join("manual.txt"))?,
        b"changed content"
    );
    assert!(
        std::fs::read_dir(original.parent().ok_or("album parent")?)?
            .filter_map(std::result::Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".domus-retained-"))
    );
    Ok(())
}

fn restore_fixture() -> Result<
    (
        Fixture,
        domus::infrastructure::store::PublicationDestination,
    ),
    Box<dyn std::error::Error>,
> {
    let mut fixture = Fixture::build_with_replacement(
        AudioFormat::Flac,
        true,
        super::ReplacementAction::Replace,
    )?;
    let run = normalized(&mut fixture)?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run,
        )?,
        "complete"
    );
    let destination = fixture
        .store
        .publication_destinations(fixture.job, fixture.version)?
        .into_iter()
        .find(|destination| destination.location_id == fixture.main)
        .ok_or("main destination")?;
    Ok((fixture, destination))
}

fn restore_decision(
    fixture: &mut Fixture,
    destination: &domus::infrastructure::store::PublicationDestination,
) -> Result<domus::infrastructure::store::AlbumReplacementRestoreDecision, Box<dyn std::error::Error>>
{
    let preview = domus::application::preview_album_restore(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        destination.location_id,
    )?;
    assert!(preview.verified, "{:?}", preview.unavailable_reason);
    Ok(
        domus::infrastructure::store::AlbumReplacementRestoreDecision {
            review_id: destination
                .replacement
                .as_ref()
                .ok_or("replacement")?
                .review_id,
            expected_manifest_sha256: destination.manifest_sha256.clone(),
            expected_album_identity: destination.album_identity.clone().ok_or("album identity")?,
            expected_preview_sha256: preview.preview_sha256,
        },
    )
}

#[test]
fn replacement_restore_previews_changed_current_files_and_keeps_them_after_fresh_approval()
-> TestResult {
    use domus::domain::Page;
    use domus::infrastructure::store::QuestionRecovery;
    let (mut fixture, destination) = restore_fixture()?;
    let before = restore_decision(&mut fixture, &destination)?;
    let current = album(&fixture, fixture._library.path())?;
    std::fs::write(current.join("new extra.txt"), b"keep this changed file")?;
    std::fs::write(current.join("folder.jpg"), b"user changed current cover")?;
    assert!(
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
            before,
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(current.join("new extra.txt"))?,
        b"keep this changed file"
    );
    let raw = Connection::open(&fixture.database)?;
    assert_eq!(
        raw.query_row("SELECT COUNT(*) FROM album_restore_approvals", [], |row| {
            row.get::<_, i64>(0)
        })?,
        0
    );
    let mut reopened = Store::open(&fixture.database)?;
    let inbox = reopened.inbox(Page::new(0, 10)?)?;
    assert_eq!(inbox.len(), 1);
    let question = &inbox[0];
    let details = reopened.question_details(question.id)?;
    assert!(details.current);
    assert_eq!(details.job_state, "complete");
    assert!(
        matches!(details.recovery, QuestionRecovery::AlbumRestore { destination_id, .. } if destination_id == destination.id)
    );
    assert!(
        reopened
            .answer(question.id, fixture.version + 1, "Review restore state")
            .is_err()
    );
    reopened.answer(question.id, fixture.version, "Review restore state")?;
    assert_eq!(reopened.inbox(Page::new(0, 10)?)?.len(), 1);
    assert_eq!(
        std::fs::read(current.join("new extra.txt"))?,
        b"keep this changed file"
    );
    let preview = domus::application::preview_album_restore(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        fixture.main,
    )?;
    assert!(preview.verified);
    assert!(
        preview
            .current_files
            .iter()
            .any(|file| file.name == "new extra.txt" && file.bytes == 22)
    );
    assert!(
        preview
            .retained_files
            .iter()
            .any(|file| file.name == "manual.txt")
    );
    let decision = restore_decision(&mut fixture, &destination)?;
    let result = domus::application::restore_album_replacement(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        fixture.main,
        decision,
    )?;
    let replacement = result.replacement.ok_or("replacement")?;
    assert_eq!(replacement.state, "restored");
    let retained = fixture._library.path().join(
        replacement
            .retained_folder
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    assert_eq!(
        std::fs::read(retained.join("new extra.txt"))?,
        b"keep this changed file"
    );
    assert_eq!(
        std::fs::read(retained.join("folder.jpg"))?,
        b"user changed current cover"
    );
    assert_eq!(std::fs::read(current.join("manual.txt"))?, b"user extra");
    assert!(reopened.inbox(Page::new(0, 10)?)?.is_empty());
    assert!(!reopened.question_details(question.id)?.current);
    assert!(
        raw.execute(
            "UPDATE album_restore_approvals SET album_identity='different'",
            []
        )
        .is_err()
    );
    assert!(
        raw.execute("DELETE FROM album_restore_approvals", [])
            .is_err()
    );
    Ok(())
}

#[test]
fn replacement_restore_rejects_old_commands_and_changed_retained_files() -> TestResult {
    let (mut fixture, destination) = restore_fixture()?;
    let mut decision = restore_decision(&mut fixture, &destination)?;
    decision.expected_preview_sha256 = None;
    assert!(
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
            decision,
        )
        .is_err()
    );
    let replacement = destination.replacement.as_ref().ok_or("replacement")?;
    let retained = fixture._library.path().join(
        replacement
            .retained_folder
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    std::fs::write(retained.join("manual.txt"), b"changed original")?;
    let preview = domus::application::preview_album_restore(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        fixture.main,
    )?;
    assert!(!preview.verified);
    assert!(preview.preview_sha256.is_none());
    assert!(preview.unavailable_reason.is_some());
    assert_eq!(
        std::fs::read(retained.join("manual.txt"))?,
        b"changed original"
    );
    assert_eq!(
        std::fs::read(album(&fixture, fixture._library.path())?.join("folder.jpg"))?,
        fixture.cover.bytes()
    );
    Ok(())
}

#[test]
fn replacement_restore_recovers_each_journal_boundary_without_duplicate_effects() -> TestResult {
    for next in [
        "restore_prepared",
        "restore_current_moved",
        "restore_original_visible",
        "restored",
    ] {
        let (mut fixture, destination) = restore_fixture()?;
        let decision = restore_decision(&mut fixture, &destination)?;
        let raw = Connection::open(&fixture.database)?;
        raw.execute_batch(&format!(
            "CREATE TRIGGER interrupt_restore BEFORE UPDATE OF state ON publication_replacement_journal
             WHEN NEW.state='{next}' BEGIN SELECT RAISE(ABORT,'injected restore interruption'); END;"
        ))?;
        for _ in 0..2 {
            assert!(
                domus::application::restore_album_replacement(
                    &mut fixture.store,
                    fixture.job,
                    fixture.version,
                    fixture.main,
                    decision.clone(),
                )
                .is_err()
            );
        }
        assert_eq!(
            raw.query_row(
                "SELECT COUNT(*) FROM album_restore_questions WHERE resolved_at IS NULL",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            1
        );
        raw.execute_batch("DROP TRIGGER interrupt_restore")?;
        fixture.store = Store::open(&fixture.database)?;
        let decision = restore_decision(&mut fixture, &destination)?;
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
            decision,
        )?;
        let completed = restore_decision(&mut fixture, &destination)?;
        for _ in 0..2 {
            domus::application::restore_album_replacement(
                &mut fixture.store,
                fixture.job,
                fixture.version,
                fixture.main,
                completed.clone(),
            )?;
        }
        assert_eq!(
            std::fs::read(album(&fixture, fixture._library.path())?.join("manual.txt"))?,
            b"user extra"
        );
        assert_eq!(
            raw.query_row("SELECT COUNT(*) FROM album_restore_approvals", [], |row| {
                row.get::<_, i64>(0)
            })?,
            1
        );
    }
    Ok(())
}

#[test]
fn replacement_restore_keeps_fixed_approval_when_interrupted_files_change() -> TestResult {
    let (mut fixture, destination) = restore_fixture()?;
    let decision = restore_decision(&mut fixture, &destination)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER interrupt_restore BEFORE UPDATE OF state ON publication_replacement_journal
         WHEN NEW.state='restore_current_moved' BEGIN SELECT RAISE(ABORT,'interrupt'); END;",
    )?;
    assert!(
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
            decision.clone(),
        )
        .is_err()
    );
    raw.execute_batch("DROP TRIGGER interrupt_restore")?;
    let replacement = destination.replacement.as_ref().ok_or("replacement")?;
    let temporary = fixture._library.path().join(
        replacement
            .restore_folder
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    std::fs::write(temporary.join("unexpected.txt"), b"external change")?;
    let preview = domus::application::preview_album_restore(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        fixture.main,
    )?;
    assert!(!preview.verified);
    assert!(
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
            decision,
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(temporary.join("unexpected.txt"))?,
        b"external change"
    );
    assert_eq!(
        raw.query_row("SELECT COUNT(*) FROM album_restore_approvals", [], |row| {
            row.get::<_, i64>(0)
        })?,
        1
    );
    Ok(())
}

#[test]
fn replacement_restore_continues_committed_states_before_the_next_rename() -> TestResult {
    for state in [
        "restore_prepared",
        "restore_current_moved",
        "restore_original_visible",
    ] {
        let (mut fixture, destination) = restore_fixture()?;
        let preview = domus::application::preview_album_restore(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
        )?;
        assert!(preview.verified);
        let replacement = destination.replacement.as_ref().ok_or("replacement")?;
        let native = |relative: &str| {
            fixture
                ._library
                .path()
                .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
        };
        let current = native(&replacement.original_folder);
        let retained = native(&replacement.retained_folder);
        let temporary = native(&replacement.restore_folder);
        let raw = Connection::open(&fixture.database)?;
        raw.execute(
            "INSERT INTO album_restore_approvals
             (destination_id,preview_sha256,album_identity,current_files) VALUES (?1,?2,?3,?4)",
            params![
                destination.id,
                preview.preview_sha256,
                preview.current_album_identity,
                serde_json::to_string(&preview.current_files)?,
            ],
        )?;
        raw.execute(
            "UPDATE publication_replacement_journal SET state='restore_prepared'
             WHERE destination_id=?1",
            [destination.id],
        )?;
        if state != "restore_prepared" {
            std::fs::rename(&current, &temporary)?;
            raw.execute(
                "UPDATE publication_replacement_journal SET state='restore_current_moved'
                 WHERE destination_id=?1",
                [destination.id],
            )?;
        }
        if state == "restore_original_visible" {
            std::fs::rename(&retained, &current)?;
            raw.execute(
                "UPDATE publication_replacement_journal SET state='restore_original_visible'
                 WHERE destination_id=?1",
                [destination.id],
            )?;
        }
        fixture.store = Store::open(&fixture.database)?;
        let decision = restore_decision(&mut fixture, &destination)?;
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
            decision,
        )?;
        assert_eq!(std::fs::read(current.join("manual.txt"))?, b"user extra");
        assert_eq!(
            std::fs::read(retained.join("folder.jpg"))?,
            fixture.cover.bytes()
        );
        assert!(!temporary.exists());
    }
    Ok(())
}

#[test]
fn replacement_restore_reports_offline_destinations_and_continues_them_independently() -> TestResult
{
    let backup = tempdir()?;
    let offline = backup.path().with_extension("restore-offline");
    let mut fixture = Fixture::build_with_backup_replacement(
        AudioFormat::Flac,
        true,
        backup.path(),
        super::ReplacementAction::Replace,
    )?;
    let run = normalized(&mut fixture)?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run,
        )?,
        "complete"
    );
    let destinations = fixture
        .store
        .publication_destinations(fixture.job, fixture.version)?;
    let main = destinations
        .iter()
        .find(|item| item.kind == "main")
        .ok_or("main")?;
    let other = destinations
        .iter()
        .find(|item| item.kind == "backup")
        .ok_or("backup")?;
    let main_decision = restore_decision(&mut fixture, main)?;
    domus::application::restore_album_replacement(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        main.location_id,
        main_decision,
    )?;
    let decision = restore_decision(&mut fixture, other)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER interrupt_restore BEFORE UPDATE OF state ON publication_replacement_journal
         WHEN NEW.state='restore_current_moved' BEGIN SELECT RAISE(ABORT,'interrupt'); END;",
    )?;
    assert!(
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            other.location_id,
            decision.clone(),
        )
        .is_err()
    );
    raw.execute_batch("DROP TRIGGER interrupt_restore")?;
    std::fs::rename(backup.path(), &offline)?;
    let preview = domus::application::preview_album_restore(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        other.location_id,
    );
    let result = domus::application::restore_album_replacement(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        other.location_id,
        decision,
    );
    std::fs::rename(&offline, backup.path())?;
    assert!(!preview?.verified);
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(album(&fixture, fixture._library.path())?.join("manual.txt"))?,
        b"user extra"
    );
    fixture.store = Store::open(&fixture.database)?;
    let next = restore_decision(&mut fixture, other)?;
    let restored = domus::application::restore_album_replacement(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        other.location_id,
        next,
    )?;
    assert_eq!(restored.replacement.ok_or("replacement")?.state, "restored");
    assert_eq!(
        std::fs::read(album(&fixture, backup.path())?.join("manual.txt"))?,
        b"user extra"
    );
    Ok(())
}

#[test]
fn replacement_restore_rejects_replaced_folder_identity_and_nested_entries() -> TestResult {
    for invalid in [
        "retained_identity",
        "nested_current",
        "missing_retained",
        "case_collision",
    ] {
        let (mut fixture, destination) = restore_fixture()?;
        let decision = restore_decision(&mut fixture, &destination)?;
        let replacement = destination.replacement.as_ref().ok_or("replacement")?;
        let retained = fixture._library.path().join(
            replacement
                .retained_folder
                .replace('/', std::path::MAIN_SEPARATOR_STR),
        );
        let current = album(&fixture, fixture._library.path())?;
        match invalid {
            "retained_identity" => {
                std::fs::rename(&retained, retained.with_extension("preserved"))?;
                std::fs::create_dir(&retained)?;
            }
            "nested_current" => std::fs::create_dir(current.join("nested"))?,
            "missing_retained" => std::fs::rename(&retained, retained.with_extension("preserved"))?,
            "case_collision" => {
                // Linux permits both spellings; Windows replaces the same file.
                std::fs::write(current.join("FOLDER.JPG"), b"changed cover")?;
            }
            _ => unreachable!(),
        }
        let preview = domus::application::preview_album_restore(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
        )?;
        if invalid != "case_collision" || cfg!(unix) {
            assert!(!preview.verified);
        }
        assert!(
            domus::application::restore_album_replacement(
                &mut fixture.store,
                fixture.job,
                fixture.version,
                fixture.main,
                decision,
            )
            .is_err()
        );
        assert!(current.exists());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn replacement_restore_rejects_symlinks_in_current_and_retained_versions() -> TestResult {
    for retained_version in [false, true] {
        let (mut fixture, destination) = restore_fixture()?;
        let replacement = destination.replacement.as_ref().ok_or("replacement")?;
        let folder = if retained_version {
            fixture._library.path().join(&replacement.retained_folder)
        } else {
            album(&fixture, fixture._library.path())?
        };
        std::os::unix::fs::symlink(fixture.scratch.path(), folder.join("unsafe"))?;
        let preview = domus::application::preview_album_restore(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
        )?;
        assert!(!preview.verified);
        assert!(preview.preview_sha256.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn replacement_restore_api_requires_guards_and_exact_preview() -> TestResult {
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    let (mut fixture, destination) = restore_fixture()?;
    let router = domus::infrastructure::api::Api::development(
        "https://localhost:8443",
        Store::open(&fixture.database)?,
    )?
    .router();
    let path = format!(
        "/api/jobs/{}/plans/{}/publication/replacement/{}/restore",
        fixture.job, fixture.version, fixture.main
    );
    let peer = ConnectInfo("127.0.0.1:32100".parse::<std::net::SocketAddr>()?);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("host", "localhost:8443")
                .extension(peer)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let preview: domus::infrastructure::store::AlbumRestorePreview =
        serde_json::from_slice(&to_bytes(response.into_body(), 2_000_000).await?)?;
    assert!(preview.verified);
    let decision = restore_decision(&mut fixture, &destination)?;
    let body = serde_json::to_string(&decision)?;
    for (origin, csrf) in [
        (None, None),
        (Some("https://wrong.invalid"), Some("1")),
        (Some("https://localhost:8443"), None),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri(&path)
            .header("host", "localhost:8443")
            .extension(peer)
            .header("content-type", "application/json");
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        if let Some(csrf) = csrf {
            request = request.header("x-domus-csrf", csrf);
        }
        assert_eq!(
            router
                .clone()
                .oneshot(request.body(Body::from(body.clone()))?)
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header("host", "localhost:8443")
                .extension(peer)
                .header("content-type", "application/json")
                .header("origin", "https://localhost:8443")
                .header("x-domus-csrf", "1")
                .body(Body::from(body))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        std::fs::read(album(&fixture, fixture._library.path())?.join("manual.txt"))?,
        b"user extra"
    );
    Ok(())
}

#[test]
fn separate_copy_publishes_to_its_saved_folder_without_changing_original_files() -> TestResult {
    let mut fixture =
        Fixture::build_with_replacement(AudioFormat::Flac, true, super::ReplacementAction::Copy)?;
    let original = album(&fixture, fixture._library.path())?;
    let review = fixture
        .store
        .album_replacement_reviews(fixture.job, fixture.version)?
        .into_iter()
        .find(|review| review.location_id == fixture.main)
        .ok_or("saved replacement review")?;
    let copy_folder = review.output_folder.ok_or("separate output folder")?;
    let run = normalized(&mut fixture)?;

    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    assert_eq!(std::fs::read(original.join("folder.jpg"))?, b"old cover");
    assert_eq!(std::fs::read(original.join("manual.txt"))?, b"user extra");
    let copy = fixture
        ._library
        .path()
        .join(copy_folder.replace('/', std::path::MAIN_SEPARATOR_STR));
    assert_eq!(
        std::fs::read(copy.join("folder.jpg"))?,
        fixture.cover.bytes()
    );
    assert_eq!(
        std::fs::read_dir(&copy)?.count(),
        fixture
            .store
            .plan(fixture.job, fixture.version)?
            .plan
            .outputs
            .len()
            + 1
    );
    Ok(())
}

#[test]
fn replacement_recovers_when_database_receipt_fails_after_the_album_switch() -> TestResult {
    let mut fixture = Fixture::build_with_replacement(
        AudioFormat::Flac,
        true,
        super::ReplacementAction::Replace,
    )?;
    let run = normalized(&mut fixture)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER fail_replacement_receipt
         BEFORE UPDATE OF state ON publication_replacement_journal
         WHEN NEW.state='published'
         BEGIN SELECT RAISE(ABORT,'injected replacement interruption'); END;",
    )?;
    assert!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )
        .is_err()
    );
    verify_album(&fixture, run, fixture._library.path())?;
    let retained_relative: String = raw.query_row(
        "SELECT retained_folder FROM publication_replacement_journal",
        [],
        |row| row.get(0),
    )?;
    let retained = fixture
        ._library
        .path()
        .join(retained_relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    assert_eq!(std::fs::read(retained.join("folder.jpg"))?, b"old cover");
    raw.execute_batch("DROP TRIGGER fail_replacement_receipt")?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    assert_eq!(
        raw.query_row(
            "SELECT state FROM publication_replacement_journal",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "published"
    );
    Ok(())
}

#[test]
fn publication_copies_only_verified_final_files_to_main_and_backup_for_both_formats() -> TestResult
{
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let backup = tempdir()?;
        let mut fixture =
            Fixture::build_with_backup(format, true, true, true, Some(backup.path()))?;
        let run = normalized(&mut fixture)?;
        let transfers = serde_json::to_value(fixture.store.transfers(
            fixture.job,
            fixture.version,
            Page::new(0, 200)?,
        )?)?;
        assert!(
            fixture
                .store
                .advance(&fixture.claim, JobState::Publishing, JobState::Complete)
                .is_err()
        );
        let state = publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run,
        )?;
        assert_eq!(state, "complete");
        verify_album(&fixture, run, fixture._library.path())?;
        verify_album(&fixture, run, backup.path())?;
        let targets = fixture
            .store
            .publication_destinations(fixture.job, fixture.version)?;
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().all(|target| target.verified_at.is_some()
            && target.album_identity.is_some()
            && target.last_error.is_none()));
        assert_eq!(
            serde_json::to_value(fixture.store.transfers(
                fixture.job,
                fixture.version,
                Page::new(0, 200)?
            )?)?,
            transfers
        );
        assert!(
            !fixture
                .scratch
                .path()
                .join(format!("normalization-{run}"))
                .exists()
        );
        assert_eq!(
            fixture
                .store
                .recovery_operation(run * 8 + 1)?
                .ok_or("scratch cleanup")?
                .state,
            "complete"
        );
        assert!(fixture.store.renew_claim(&fixture.claim, 300).is_err());
        let reopened = Store::open_existing(&fixture.database)?;
        assert_eq!(
            reopened.job_state(fixture.job)?.as_deref(),
            Some("complete")
        );
        assert_eq!(
            serde_json::to_value(reopened.publication_destinations(fixture.job, fixture.version)?)?,
            serde_json::to_value(targets)?
        );
    }
    Ok(())
}

#[test]
fn offline_backup_leaves_verified_primary_and_retries_without_normalizing_or_downloading()
-> TestResult {
    let backup = tempdir()?;
    let offline = backup.path().with_extension("offline");
    let mut fixture =
        Fixture::build_with_backup(AudioFormat::Flac, true, true, true, Some(backup.path()))?;
    let run = normalized(&mut fixture)?;
    std::fs::rename(backup.path(), &offline)?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "backup_pending"
    );
    verify_album(&fixture, run, fixture._library.path())?;
    let pending = fixture
        .store
        .publication_destinations(fixture.job, fixture.version)?;
    assert!(
        pending
            .iter()
            .find(|target| target.kind == "backup")
            .ok_or("backup")?
            .last_error
            .is_some()
    );
    let raw = Connection::open(&fixture.database)?;
    let transfers: i64 =
        raw.query_row("SELECT count(*) FROM transfer_files", [], |row| row.get(0))?;
    let runs: i64 = raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| {
        row.get(0)
    })?;
    std::fs::rename(&offline, backup.path())?;
    let reclaimed = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "retry", 300)?
        .ok_or("retry claim")?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &reclaimed.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    verify_album(&fixture, run, backup.path())?;
    assert_eq!(
        raw.query_row("SELECT count(*) FROM transfer_files", [], |row| row
            .get::<_, i64>(0))?,
        transfers
    );
    assert_eq!(
        raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| row
            .get::<_, i64>(0))?,
        runs
    );
    Ok(())
}

#[test]
fn replacement_rejects_changed_backup_content_after_primary_publication() -> TestResult {
    let backup = tempdir()?;
    let mut fixture = Fixture::build_with_backup_replacement(
        AudioFormat::Flac,
        true,
        backup.path(),
        super::ReplacementAction::Replace,
    )?;
    let run = normalized(&mut fixture)?;
    let original_backup = album(&fixture, backup.path())?;
    std::fs::write(original_backup.join("manual.txt"), b"changed backup extra")?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "backup_pending"
    );
    verify_album(&fixture, run, fixture._library.path())?;
    assert_eq!(
        std::fs::read(original_backup.join("manual.txt"))?,
        b"changed backup extra"
    );
    assert!(
        std::fs::read_dir(original_backup.parent().ok_or("backup parent")?)?
            .filter_map(std::result::Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".domus-retained-"))
    );
    let raw = Connection::open(&fixture.database)?;
    let state: String = raw.query_row(
        "SELECT j.state FROM publication_replacement_journal j
         JOIN publication_destinations d ON d.id=j.destination_id
         WHERE d.kind='backup'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(state, "planned");
    Ok(())
}

#[test]
fn replacement_keeps_backup_pending_when_offline_and_reuses_its_saved_decision() -> TestResult {
    let backup = tempdir()?;
    let offline = backup.path().with_extension("offline");
    let mut fixture = Fixture::build_with_backup_replacement(
        AudioFormat::Flac,
        true,
        backup.path(),
        super::ReplacementAction::Replace,
    )?;
    let run = normalized(&mut fixture)?;
    std::fs::rename(backup.path(), &offline)?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "backup_pending"
    );
    verify_album(&fixture, run, fixture._library.path())?;
    assert!(
        domus::application::preview_album_restore(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            fixture.main,
        )
        .is_err()
    );
    std::fs::rename(&offline, backup.path())?;
    let reclaimed = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "replacement-retry", 300)?
        .ok_or("retry claim")?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &reclaimed.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    let current_backup = album(&fixture, backup.path())?;
    assert_eq!(
        std::fs::read(current_backup.join("folder.jpg"))?,
        fixture.cover.bytes()
    );
    let raw = Connection::open(&fixture.database)?;
    let retained: String = raw.query_row(
        "SELECT j.retained_folder
         FROM publication_replacement_journal j
         JOIN publication_destinations d ON d.id=j.destination_id
         WHERE d.kind='backup'",
        [],
        |row| row.get(0),
    )?;
    let retained = backup
        .path()
        .join(retained.replace('/', std::path::MAIN_SEPARATOR_STR));
    assert_eq!(std::fs::read(retained.join("manual.txt"))?, b"user extra");
    Ok(())
}

#[test]
fn existing_unowned_album_and_extra_assets_are_never_adopted_replaced_or_merged() -> TestResult {
    for existing in ["empty", "matching", "different", "extra"] {
        let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
        let run = normalized(&mut fixture)?;
        let target = album(&fixture, fixture._library.path())?;
        std::fs::create_dir_all(&target)?;
        let retained = target.join("folder.jpg");
        if existing != "empty" {
            std::fs::write(
                &retained,
                if existing == "matching" {
                    fixture.cover.bytes()
                } else {
                    b"existing album bytes"
                },
            )?;
        }
        if existing == "extra" {
            std::fs::write(target.join("booklet.pdf"), b"retain old extras")?;
        }
        assert!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run
            )
            .is_err()
        );
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
        assert!(
            fixture
                .store
                .publication_destinations(fixture.job, fixture.version)?[0]
                .album_identity
                .is_none()
        );
        if existing == "matching" {
            assert_eq!(std::fs::read(&retained)?, fixture.cover.bytes());
        }
        if existing == "different" {
            assert_eq!(std::fs::read(&retained)?, b"existing album bytes");
        }
        if existing == "extra" {
            assert_eq!(
                std::fs::read(target.join("booklet.pdf"))?,
                b"retain old extras"
            );
        }
        assert!(target.read_dir()?.count() <= 2);
    }
    Ok(())
}

#[test]
fn synced_unreceipted_file_reconciles_after_reclaim_but_partial_file_is_retained() -> TestResult {
    for partial in [false, true] {
        let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
        let run = normalized(&mut fixture)?;
        let raw = Connection::open(&fixture.database)?;
        raw.execute_batch("CREATE TRIGGER fail_publication_receipt BEFORE UPDATE OF verified_at ON publication_files BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        assert!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run
            )
            .is_err()
        );
        let image = prepared_album(&fixture)?.join("folder.jpg");
        assert!(!album(&fixture, fixture._library.path())?.exists());
        assert_eq!(std::fs::read(&image)?, fixture.cover.bytes());
        let before = if partial {
            std::fs::write(&image, b"partial interrupted image")?;
            std::fs::read(&image)?
        } else {
            fixture.cover.bytes().to_vec()
        };
        raw.execute_batch("DROP TRIGGER fail_publication_receipt; UPDATE jobs SET lease_until=0;")?;
        let mut reopened = Store::open_existing(&fixture.database)?;
        let reclaimed = reopened
            .claim_approved_plan(fixture.job, fixture.version, "restarted", 300)?
            .ok_or("claim")?;
        let result = publish_new_album(
            &mut reopened,
            &reclaimed.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run,
        );
        if partial {
            assert!(result.is_err());
            assert_eq!(
                reopened.job_state(fixture.job)?.as_deref(),
                Some("publishing")
            );
        } else {
            assert_eq!(result?, "complete");
            verify_album(&fixture, run, fixture._library.path())?;
        }
        let published_image = album(&fixture, fixture._library.path())?.join("folder.jpg");
        assert_eq!(
            if partial {
                std::fs::read(image)?
            } else {
                std::fs::read(published_image)?
            },
            before
        );
    }
    Ok(())
}

#[test]
fn unbound_creation_gap_is_abandoned_and_retried_without_adopting_it() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let run = normalized(&mut fixture)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("CREATE TRIGGER fail_publication_binding BEFORE UPDATE OF album_identity ON publication_destinations BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
    assert!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )
        .is_err()
    );
    let abandoned = prepared_album(&fixture)?;
    assert!(abandoned.is_dir());
    assert!(!album(&fixture, fixture._library.path())?.exists());
    raw.execute_batch("DROP TRIGGER fail_publication_binding;")?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    assert!(
        fixture
            .store
            .publication_destinations(fixture.job, fixture.version)?[0]
            .album_identity
            .is_some()
    );
    assert!(abandoned.is_dir());
    verify_album(&fixture, run, fixture._library.path())?;
    Ok(())
}

#[test]
fn late_backup_retry_rejects_changed_primary_and_changed_destination_identity() -> TestResult {
    for fault in ["primary", "backup"] {
        let backup = tempdir()?;
        let offline = backup.path().with_extension("offline");
        let mut fixture =
            Fixture::build_with_backup(AudioFormat::Flac, true, true, true, Some(backup.path()))?;
        let run = normalized(&mut fixture)?;
        std::fs::rename(backup.path(), &offline)?;
        assert_eq!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run
            )?,
            "backup_pending"
        );
        if fault == "primary" {
            std::fs::write(
                album(&fixture, fixture._library.path())?.join("folder.jpg"),
                b"changed primary cover",
            )?;
            std::fs::rename(&offline, backup.path())?;
        } else {
            std::fs::create_dir(backup.path())?;
        }
        let claim = fixture
            .store
            .claim_approved_plan(fixture.job, fixture.version, "retry", 300)?
            .ok_or("retry")?
            .claim;
        let result = publish_new_album(
            &mut fixture.store,
            &claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run,
        );
        if fault == "primary" {
            assert!(result.is_err());
            assert_eq!(
                std::fs::read(album(&fixture, fixture._library.path())?.join("folder.jpg"))?,
                b"changed primary cover"
            );
        } else {
            assert_eq!(result?, "backup_pending");
            assert_eq!(backup.path().read_dir()?.count(), 0);
            std::fs::remove_dir(backup.path())?;
            std::fs::rename(&offline, backup.path())?;
        }
        assert_ne!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("complete")
        );
    }
    Ok(())
}

#[test]
fn publication_journal_is_atomic_immutable_and_rechecks_claim_source_and_observations() -> TestResult
{
    for fault in ["intent", "claim", "source", "observation"] {
        let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
        let run = normalized(&mut fixture)?;
        let raw = Connection::open(&fixture.database)?;
        match fault {
            "intent" => raw.execute_batch("CREATE TRIGGER fail_publication_file BEFORE INSERT ON publication_files BEGIN SELECT RAISE(ABORT,'injected'); END;")?,
            "claim" => {raw.execute("UPDATE jobs SET lease_until=0",[])?;}
            "source" => { std::fs::write(fixture.scratch.path().join(format!("normalization-{run}")).join("folder.jpg"),b"damaged source")?;}
            "observation" => { raw.execute("UPDATE locations SET observed_at=observed_at+1",[])?;}
            _ => unreachable!(),
        }
        assert!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run
            )
            .is_err()
        );
        assert_eq!(
            raw.query_row("SELECT count(*) FROM publication_destinations", [], |row| {
                row.get::<_, i64>(0)
            })?,
            0
        );
        assert_eq!(fixture._library.path().read_dir()?.count(), 0);
    }
    Ok(())
}

#[test]
fn publication_cli_requires_explicit_acknowledgement_and_can_reclaim_completed_staging()
-> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Mp3, true)?;
    let run = normalized(&mut fixture)?;
    let config = fixture._state.path().join("publish.json");
    let mut body = json!({
        "database":fixture.database,"staging_root":fixture.scratch.path(),
        "job_id":fixture.job,"plan_version":fixture.version,"normalization_run":run,
        "acknowledge_new_only_publication":false
    });
    let execute = || {
        Command::new(env!("CARGO_BIN_EXE_domus"))
            .args(["publish-new-once", config.to_str().unwrap_or("")])
            .output()
    };
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    assert!(!execute()?.status.success());
    body["acknowledge_new_only_publication"] = json!(true);
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    assert!(!execute()?.status.success());
    Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
    drop(fixture.staging.take());
    let output = execute()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let outcome: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(outcome["state"], "complete");
    verify_album(&fixture, run, fixture._library.path())?;
    let raw = Connection::open(&fixture.database)?;
    for sql in [
        "UPDATE publication_destinations SET relative_folder='elsewhere'",
        "UPDATE publication_destinations SET album_identity='another'",
        "DELETE FROM publication_destinations",
        "UPDATE publication_files SET sha256=printf('%064d',0)",
        "UPDATE publication_files SET verified_at=NULL",
        "DELETE FROM publication_files",
    ] {
        assert!(raw.execute(sql, []).is_err(), "{sql}");
    }
    let events: i64 = raw.query_row("SELECT count(*) FROM events", [], |row| row.get(0))?;
    let status = Command::new(env!("CARGO_BIN_EXE_domus"))
        .args([
            "publication-status",
            fixture.database.to_str().ok_or("db")?,
            &fixture.job.to_string(),
            &fixture.version.to_string(),
        ])
        .output()?;
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout)?,
        serde_json::to_value(
            fixture
                .store
                .publication_destinations(fixture.job, fixture.version)?
        )?
    );
    assert_eq!(
        raw.query_row("SELECT count(*) FROM events", [], |row| row
            .get::<_, i64>(0))?,
        events
    );
    Ok(())
}

#[test]
fn publication_retry_never_recreates_receipted_missing_files_or_accepts_extras() -> TestResult {
    for fault in ["missing", "extra", "album_identity"] {
        let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
        let run = normalized(&mut fixture)?;
        let raw = Connection::open(&fixture.database)?;
        raw.execute_batch("CREATE TRIGGER fail_publication_track BEFORE UPDATE OF verified_at ON publication_files WHEN OLD.ordinal=1 BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        assert!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run
            )
            .is_err()
        );
        let folder = prepared_album(&fixture)?;
        assert!(!album(&fixture, fixture._library.path())?.exists());
        let receipt: Option<i64> = raw.query_row(
            "SELECT verified_at FROM publication_files WHERE ordinal=0",
            [],
            |row| row.get(0),
        )?;
        assert!(receipt.is_some());
        let moved = folder.with_extension("retained");
        match fault {
            "missing" => std::fs::remove_file(folder.join("folder.jpg"))?,
            "extra" => std::fs::write(folder.join("booklet.pdf"), b"retain this extra")?,
            "album_identity" => {
                std::fs::rename(&folder, &moved)?;
                std::fs::create_dir(&folder)?;
            }
            _ => unreachable!(),
        }
        raw.execute_batch("DROP TRIGGER fail_publication_track;")?;
        assert!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run
            )
            .is_err()
        );
        assert_ne!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("complete")
        );
        match fault {
            "missing" => assert!(!folder.join("folder.jpg").exists()),
            "extra" => assert_eq!(
                std::fs::read(folder.join("booklet.pdf"))?,
                b"retain this extra"
            ),
            "album_identity" => {
                assert_eq!(folder.read_dir()?.count(), 0);
                assert_eq!(
                    std::fs::read(moved.join("folder.jpg"))?,
                    fixture.cover.bytes()
                );
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

#[test]
fn publication_never_creates_parallel_case_aliases_or_follows_parent_links() -> TestResult {
    for fault in ["case", "link"] {
        let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
        let run = normalized(&mut fixture)?;
        let folder = fixture
            .store
            .plan(fixture.job, fixture.version)?
            .plan
            .outputs[0]
            .relative_folder
            .clone();
        let bucket = folder.split('/').next().ok_or("bucket")?;
        let outside = tempdir()?;
        std::fs::write(outside.path().join("retain.txt"), b"outside content")?;
        if fault == "case" {
            assert_ne!(bucket, bucket.to_lowercase());
            std::fs::create_dir(fixture._library.path().join(bucket.to_lowercase()))?;
        } else {
            let link = fixture._library.path().join(bucket);
            #[cfg(windows)]
            {
                let output = Command::new("cmd.exe")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(&link)
                    .arg(outside.path())
                    .output()?;
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(outside.path(), link)?;
        }
        assert!(
            publish_new_album(
                &mut fixture.store,
                &fixture.claim,
                fixture.staging.as_ref().ok_or("staging")?,
                run
            )
            .is_err()
        );
        assert_eq!(outside.path().read_dir()?.count(), 1);
        assert_eq!(
            std::fs::read(outside.path().join("retain.txt"))?,
            b"outside content"
        );
        assert_eq!(fixture._library.path().read_dir()?.count(), 1);
    }
    Ok(())
}

#[test]
fn failed_publication_final_commit_keeps_receipts_and_can_finish_after_restart() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let run = normalized(&mut fixture)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("CREATE TRIGGER fail_publication_complete BEFORE INSERT ON events WHEN NEW.event_type='complete' BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
    assert!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )
        .is_err()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("publishing")
    );
    let saved = fixture
        .store
        .publication_destinations(fixture.job, fixture.version)?;
    assert!(saved[0].verified_at.is_some());
    verify_album(&fixture, run, fixture._library.path())?;
    raw.execute_batch("DROP TRIGGER fail_publication_complete; UPDATE jobs SET lease_until=0;")?;
    let claim = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "retry", 300)?
        .ok_or("claim")?
        .claim;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    assert_eq!(
        serde_json::to_value(saved)?,
        serde_json::to_value(
            fixture
                .store
                .publication_destinations(fixture.job, fixture.version)?
        )?
    );
    Ok(())
}

#[test]
fn publication_rename_receipt_gap_recovers_the_complete_album() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let run = normalized(&mut fixture)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("CREATE TRIGGER fail_publication_visible BEFORE UPDATE OF published_at ON publication_destinations BEGIN SELECT RAISE(ABORT,'injected visibility receipt failure'); END;")?;
    assert!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )
        .is_err()
    );
    assert!(album(&fixture, fixture._library.path())?.exists());
    assert!(!prepared_album(&fixture)?.exists());
    assert!(
        fixture
            .store
            .publication_destinations(fixture.job, fixture.version)?
            .iter()
            .all(|target| target.verified_at.is_none())
    );
    verify_album(&fixture, run, fixture._library.path())?;

    raw.execute_batch("DROP TRIGGER fail_publication_visible; UPDATE jobs SET lease_until=0;")?;
    let claim = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "receipt-retry", 300)?
        .ok_or("claim")?
        .claim;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    assert!(
        fixture
            .store
            .publication_destinations(fixture.job, fixture.version)?
            .iter()
            .all(|target| target.verified_at.is_some())
    );
    verify_album(&fixture, run, fixture._library.path())?;
    Ok(())
}

#[test]
fn schema_fifteen_upgrade_preserves_completed_normalization_and_approval() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let run = normalized(&mut fixture)?;
    let raw = Connection::open(&fixture.database)?;
    let normalization: String = raw.query_row(
        "SELECT cover_sha256 FROM normalization_runs WHERE id=?1",
        [run],
        |row| row.get(0),
    )?;
    raw.execute_batch(include_str!("../support/drop_sharing.sql"))?;
    raw.execute_batch("DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations; DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; PRAGMA user_version=15; UPDATE jobs SET lease_until=0;")?;
    fixture.store = Store::open(&fixture.database)?;
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        30
    );
    assert_eq!(
        raw.query_row(
            "SELECT cover_sha256 FROM normalization_runs WHERE id=?1",
            [run],
            |row| row.get::<_, String>(0)
        )?,
        normalization
    );
    let claim = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "upgraded", 300)?
        .ok_or("claim")?
        .claim;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    verify_album(&fixture, run, fixture._library.path())?;
    Ok(())
}
