use super::{AudioFormat, Fixture, Page, Sha256, StagingRoot, Store, TestResult};
use domus::application::normalize_album;
use sha2::Digest;
use std::io::Read;

#[test]
fn staged_transfer_and_normalization_use_private_controlled_names_and_verified_disk_bytes()
-> TestResult {
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let mut fixture = Fixture::build(format, true)?;
        let root = fixture.staging.as_ref().ok_or("missing staging")?;
        let run = fixture
            .store
            .begin_normalization(&fixture.claim, &fixture.cover)?;
        let directory = root.create_normalization(&run)?;
        let work = fixture
            .store
            .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
        for item in work {
            let mut input = root.open_input(&item)?;
            let mut copied = Vec::new();
            input.read_to_end(&mut copied)?;
            assert_eq!(copied, fixture.source);
            let evidence = directory.normalize(&item, &mut input, &fixture.cover)?;
            let output = std::fs::read(
                fixture
                    .scratch
                    .path()
                    .join(&run.staging_directory)
                    .join(&item.staging_name),
            )?;
            assert_eq!(output.len() as u64, evidence.bytes);
            assert_eq!(format!("{:x}", Sha256::digest(&output)), evidence.sha256);
            fixture.store.record_normalized_audio(
                &fixture.claim,
                run.id,
                item.track_number,
                &evidence,
            )?;
            assert_eq!(
                directory.normalize(&item, &mut input, &fixture.cover)?,
                evidence
            );
            assert_eq!(
                std::fs::read(
                    fixture
                        .scratch
                        .path()
                        .join(&run.staging_directory)
                        .join(&item.staging_name)
                )?,
                output
            );
        }
        directory.write_cover(&fixture.cover)?;
        assert_eq!(
            std::fs::read(
                fixture
                    .scratch
                    .path()
                    .join(&run.staging_directory)
                    .join("folder.jpg")
            )?,
            fixture.cover.bytes()
        );
        fixture.store.record_normalized_cover(
            &fixture.claim,
            run.id,
            run.cover_bytes,
            &run.cover_sha256,
        )?;
        fixture.store.finish_normalization(&fixture.claim, run.id)?;
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
        assert!(!fixture.scratch.path().join("Peer").exists());
    }
    Ok(())
}

#[test]
fn staging_root_is_reserved_against_browsing_and_has_one_database_and_one_live_owner() -> TestResult
{
    let mut fixture = Fixture::build(AudioFormat::Mp3, true)?;
    assert!(StagingRoot::open(&mut fixture.store, fixture.scratch.path()).is_err());
    let root = std::fs::canonicalize(fixture.scratch.path())?;
    assert!(
        fixture
            .store
            .add_location(root.to_str().ok_or("invalid test path")?, "main")
            .is_err()
    );
    let child = fixture.scratch.path().join("child");
    std::fs::create_dir(&child)?;
    let child = std::fs::canonicalize(child)?;
    assert!(
        fixture
            .store
            .add_location(child.to_str().ok_or("invalid test path")?, "backup")
            .is_err()
    );
    drop(fixture.staging.take());
    let before = std::fs::read(fixture.scratch.path().join("owner.lock"))?;
    let other = tempfile::tempdir()?;
    let mut other_store = Store::open(&other.path().join("other.db"))?;
    assert!(StagingRoot::open(&mut other_store, fixture.scratch.path()).is_err());
    assert_eq!(
        std::fs::read(fixture.scratch.path().join("owner.lock"))?,
        before
    );
    fixture.staging = Some(StagingRoot::open(
        &mut fixture.store,
        fixture.scratch.path(),
    )?);
    drop(fixture.staging.take());
    std::fs::remove_file(fixture.scratch.path().join("owner.lock"))?;
    assert!(StagingRoot::open(&mut fixture.store, fixture.scratch.path()).is_err());
    assert!(!fixture.scratch.path().join("owner.lock").exists());
    Ok(())
}

#[test]
fn staging_rejects_changed_input_and_never_changes_existing_output_bytes() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Mp3, true)?;
    let root = fixture.staging.as_ref().ok_or("missing staging")?;
    let run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let directory = root.create_normalization(&run)?;
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
    let path = fixture
        .scratch
        .path()
        .join(format!("transfer-{}.input", work[0].transfer_id));
    let mut changed = fixture.source.clone();
    changed[0] ^= 1;
    std::fs::write(&path, &changed)?;
    assert!(root.open_input(&work[0]).is_err());
    std::fs::write(&path, &fixture.source)?;
    let target = fixture
        .scratch
        .path()
        .join(&run.staging_directory)
        .join(&work[0].staging_name);
    std::fs::write(&target, b"retained partial output")?;
    let mut input = root.open_input(&work[0])?;
    assert!(
        directory
            .normalize(&work[0], &mut input, &fixture.cover)
            .is_err()
    );
    assert_eq!(std::fs::read(target)?, b"retained partial output");
    assert_eq!(
        fixture
            .store
            .normalization_run(&fixture.claim)?
            .ok_or("missing run")?
            .verified_files,
        0
    );
    Ok(())
}

#[test]
fn normalization_use_case_recovers_synced_unreceipted_outputs_after_reopen() -> TestResult {
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let mut fixture = Fixture::build(format, true)?;
        let run = fixture
            .store
            .begin_normalization(&fixture.claim, &fixture.cover)?;
        {
            let root = fixture.staging.as_ref().ok_or("missing staging")?;
            let directory = root.create_normalization(&run)?;
            let work =
                fixture
                    .store
                    .normalization_work(&fixture.claim, run.id, Page::new(0, 200)?)?;
            for item in work.iter().take(2) {
                let mut input = root.open_input(item)?;
                let evidence = directory.normalize(item, &mut input, &fixture.cover)?;
                if item.track_number == 2 {
                    fixture
                        .store
                        .record_normalized_audio(&fixture.claim, run.id, 2, &evidence)?;
                }
            }
            directory.write_cover(&fixture.cover)?;
            if format == AudioFormat::Flac {
                fixture.store.record_normalized_cover(
                    &fixture.claim,
                    run.id,
                    run.cover_bytes,
                    &run.cover_sha256,
                )?;
            }
        }
        let before = std::fs::read(
            fixture
                .scratch
                .path()
                .join(&run.staging_directory)
                .join(format!("track-0001.{}", format.extension())),
        )?;
        drop(fixture.staging.take());
        drop(fixture.store);
        fixture.store = Store::open_existing(&fixture.database)?;
        fixture.staging = Some(StagingRoot::open(
            &mut fixture.store,
            fixture.scratch.path(),
        )?);
        let root = fixture.staging.as_ref().ok_or("missing resumed staging")?;
        assert_eq!(
            normalize_album(&mut fixture.store, &fixture.claim, root, &fixture.cover)?,
            run.id
        );
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
        assert_eq!(
            std::fs::read(
                fixture
                    .scratch
                    .path()
                    .join(&run.staging_directory)
                    .join(format!("track-0001.{}", format.extension()))
            )?,
            before
        );
        let raw = rusqlite::Connection::open(&fixture.database)?;
        let receipts: i64 = raw.query_row(
            "SELECT count(*) FROM normalization_receipts WHERE run_id = ?1",
            [run.id],
            |row| row.get(0),
        )?;
        assert_eq!(receipts, 3);
    }
    Ok(())
}

#[test]
fn normalization_use_case_starts_fresh_and_rejects_corrupt_receipted_files_without_overwrite()
-> TestResult {
    let mut fresh = Fixture::build(AudioFormat::Mp3, true)?;
    let root = fresh.staging.as_ref().ok_or("missing staging")?;
    normalize_album(&mut fresh.store, &fresh.claim, root, &fresh.cover)?;
    assert_eq!(
        fresh.store.job_state(fresh.job)?.as_deref(),
        Some("publishing")
    );

    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let root = fixture.staging.as_ref().ok_or("missing staging")?;
    let run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let directory = root.create_normalization(&run)?;
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 1)?)?;
    let mut input = root.open_input(&work[0])?;
    let evidence = directory.normalize(&work[0], &mut input, &fixture.cover)?;
    fixture
        .store
        .record_normalized_audio(&fixture.claim, run.id, 1, &evidence)?;
    let path = fixture
        .scratch
        .path()
        .join(&run.staging_directory)
        .join(&work[0].staging_name);
    let mut corrupted = std::fs::read(&path)?;
    corrupted[0] ^= 1;
    std::fs::write(&path, &corrupted)?;
    assert!(normalize_album(&mut fixture.store, &fixture.claim, root, &fixture.cover).is_err());
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("normalizing")
    );
    assert_eq!(std::fs::read(path)?, corrupted);
    assert!(
        !fixture
            .scratch
            .path()
            .join(&run.staging_directory)
            .join("folder.jpg")
            .exists()
    );
    Ok(())
}

#[test]
fn failed_transfer_streams_never_get_a_sealed_receipt_or_become_reconcilable() -> TestResult {
    let fixture = Fixture::build(AudioFormat::Mp3, true)?;
    let root = fixture.staging.as_ref().ok_or("missing staging")?;
    let mut transfer = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?
        .pop()
        .ok_or("missing transfer")?;
    transfer.state = "active".into();
    transfer.payload_sha256 = None;
    for (index, bytes) in [
        fixture.source[..fixture.source.len() - 1].to_vec(),
        [fixture.source.as_slice(), b"extra"].concat(),
    ]
    .iter()
    .enumerate()
    {
        transfer.id = 10_000 + index as i64;
        transfer.remote_path = "Z:\\not-a-local-output\\track.mp3".into();
        assert!(
            root.receive(&transfer, &mut std::io::Cursor::new(bytes))
                .is_err()
        );
        assert!(root.received(&transfer).is_err());
        assert!(
            !fixture
                .scratch
                .path()
                .join(format!("transfer-{}.receipt", transfer.id))
                .exists()
        );
    }
    transfer.id = 20_000;
    let received = root.receive(&transfer, &mut std::io::Cursor::new(&fixture.source))?;
    let hash = *received.sha256();
    drop(received);
    let reconciled = root.received(&transfer)?;
    assert_eq!(reconciled.sha256(), &hash);
    assert_eq!(reconciled.byte_length(), fixture.source.len() as u64);
    Ok(())
}

#[test]
fn staging_rejects_media_roots_unowned_content_and_wrong_database_use_before_normalization()
-> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Mp3, true)?;
    assert!(StagingRoot::open(&mut fixture.store, fixture._library.path()).is_err());
    assert!(!fixture._library.path().join("owner.lock").exists());
    let foreign = tempfile::tempdir()?;
    std::fs::write(foreign.path().join("existing.txt"), b"unchanged")?;
    assert!(StagingRoot::open(&mut fixture.store, foreign.path()).is_err());
    assert_eq!(
        std::fs::read(foreign.path().join("existing.txt"))?,
        b"unchanged"
    );
    assert!(!foreign.path().join("owner.lock").exists());
    let mut other = Fixture::new(AudioFormat::Mp3)?;
    let root = fixture.staging.as_ref().ok_or("missing staging")?;
    assert!(normalize_album(&mut other.store, &other.claim, root, &other.cover).is_err());
    assert_eq!(
        other.store.job_state(other.job)?.as_deref(),
        Some("validating")
    );
    Ok(())
}

#[test]
fn index_change_after_first_receipt_stops_the_next_filesystem_effect() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Mp3, true)?;
    let raw = rusqlite::Connection::open(&fixture.database)?;
    raw.execute_batch(&format!(
        "CREATE TRIGGER refresh_fixture AFTER INSERT ON normalization_receipts
         BEGIN UPDATE locations SET coverage = 'incomplete' WHERE id = {}; END;",
        fixture.main,
    ))?;
    let root = fixture.staging.as_ref().ok_or("missing staging")?;
    assert!(normalize_album(&mut fixture.store, &fixture.claim, root, &fixture.cover).is_err());
    let run = fixture
        .store
        .normalization_run(&fixture.claim)?
        .ok_or("missing run")?;
    assert_eq!(run.verified_files, 1);
    let directory = fixture.scratch.path().join(&run.staging_directory);
    assert!(directory.join("track-0001.mp3").exists());
    assert!(!directory.join("track-0002.mp3").exists());
    assert!(!directory.join("track-0003.mp3").exists());
    assert!(!directory.join("folder.jpg").exists());
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("normalizing")
    );
    Ok(())
}

#[test]
fn expired_worker_claim_cannot_create_a_normalization_directory() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Mp3, true)?;
    let raw = rusqlite::Connection::open(&fixture.database)?;
    raw.execute(
        "UPDATE jobs SET lease_until = unixepoch() - 1 WHERE id = ?1",
        [fixture.job],
    )?;
    let root = fixture.staging.as_ref().ok_or("missing staging")?;
    assert!(normalize_album(&mut fixture.store, &fixture.claim, root, &fixture.cover).is_err());
    let runs: i64 = raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| {
        row.get(0)
    })?;
    assert_eq!(runs, 0);
    assert!(!fixture.scratch.path().join("normalization-1").exists());
    let new = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "worker", 300)?
        .ok_or("missing reclaimed worker")?;
    normalize_album(&mut fixture.store, &new.claim, root, &fixture.cover)?;
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("publishing")
    );
    Ok(())
}
