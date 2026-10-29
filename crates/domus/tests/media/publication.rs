use super::*;
use domus::application::{normalize_album, publish_new_album};
use serde_json::json;
use std::path::Path;
use std::process::Command;

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
        let image = album(&fixture, fixture._library.path())?.join("folder.jpg");
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
        assert_eq!(std::fs::read(&image)?, before);
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

fn verify_album(fixture: &Fixture, run: i64, root: &Path) -> TestResult {
    let folder = album(fixture, root)?;
    let plan = fixture.store.plan(fixture.job, fixture.version)?;
    assert_eq!(
        std::fs::read(folder.join("folder.jpg"))?,
        fixture.cover.bytes()
    );
    assert_eq!(folder.read_dir()?.count(), plan.plan.outputs.len() + 1);
    for (index, output) in plan.plan.outputs.iter().enumerate() {
        let staged = fixture
            .scratch
            .path()
            .join(format!("normalization-{run}"))
            .join(format!(
                "track-{:04}.{}",
                index + 1,
                plan.plan.format.extension()
            ));
        assert_eq!(
            std::fs::read(folder.join(&output.filename))?,
            std::fs::read(staged)?
        );
    }
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
            fixture
                .scratch
                .path()
                .join(format!("normalization-{run}"))
                .is_dir()
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
        let image = album(&fixture, fixture._library.path())?.join("folder.jpg");
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
        assert_eq!(std::fs::read(image)?, before);
    }
    Ok(())
}

#[test]
fn creation_binding_gap_is_retained_and_never_recovered_by_guessing_ownership() -> TestResult {
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
    assert_eq!(
        album(&fixture, fixture._library.path())?
            .read_dir()?
            .count(),
        0
    );
    raw.execute_batch("DROP TRIGGER fail_publication_binding;")?;
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
            .publication_destinations(fixture.job, fixture.version)?[0]
            .album_identity
            .is_none()
    );
    assert_eq!(
        album(&fixture, fixture._library.path())?
            .read_dir()?
            .count(),
        0
    );
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
        let folder = album(&fixture, fixture._library.path())?;
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
        25
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
