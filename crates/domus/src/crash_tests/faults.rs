#[test]
fn cross_process_faults_keep_evidence_and_native_ownership() -> TestResult {
    let case = prepare(Action::Normalize, "database_locked", AudioFormat::Flac)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let mut child = OwnedChild::start(
        &case.descriptor,
        "hold_database",
        Some(listener.local_addr()?),
    )?;
    let channel = child.ready(&listener, "database_locked")?;
    let before = case.fixture.store.last_event_id()?;
    let start = Instant::now();
    assert!(matches!(
        case.fixture.store.renew_claim(&case.fixture.claim, 300),
        Err(crate::Error::Database(rusqlite::Error::SqliteFailure(error, _))) if error.code == rusqlite::ErrorCode::DatabaseBusy
    ));
    assert!(start.elapsed() < Duration::from_secs(15));
    assert_eq!(case.fixture.store.last_event_id()?, before);
    child.terminate()?;
    drop(channel);
    case.fixture.store.renew_claim(&case.fixture.claim, 300)?;
    integrity(&case.fixture.database)?;

    let mut case = prepare(Action::Normalize, "staging_locked", AudioFormat::Flac)?;
    let marker = case.fixture.scratch.path().join("owner.lock");
    let marker_identity = native_file_identity(&File::open(&marker)?)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let mut child = OwnedChild::start(
        &case.descriptor,
        "hold_staging",
        Some(listener.local_addr()?),
    )?;
    let channel = child.ready(&listener, "staging_locked")?;
    assert!(StagingRoot::open(&mut case.fixture.store, case.fixture.scratch.path()).is_err());
    child.terminate()?;
    drop(channel);
    let _staging = StagingRoot::open(&mut case.fixture.store, case.fixture.scratch.path())?;
    drop(_staging);
    assert_eq!(native_file_identity(&File::open(marker)?)?, marker_identity);
    Ok(())
}

#[test]
fn cross_process_simulated_disk_full_preserves_partial_publication() -> TestResult {
    let case = prepare(Action::Publish, "publication_writing", AudioFormat::Mp3)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let mut child = OwnedChild::start(&case.descriptor, "disk_full", Some(listener.local_addr()?))?;
    child.finish()?;
    let before = inventory(case.fixture._library.path())?;
    assert!(
        before.values().any(|file| file.bytes == 65_536),
        "fixture did not retain a partial output"
    );
    let mut retry = OwnedChild::start(&case.descriptor, "partial_publication", None)?;
    retry.finish()?;
    assert_eq!(inventory(case.fixture._library.path())?, before);
    integrity(&case.fixture.database)
}

#[test]
fn sqlite_full_keeps_failed_admission_atomic() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    fixture.store.limit_crash_fixture_pages()?;
    let raw = Connection::open(&fixture.database)?;
    let mut full = false;
    for _ in 0..1024 {
        let before: i64 = raw.query_row("SELECT count(*) FROM jobs", [], |row| row.get(0))?;
        match fixture.store.enqueue(1) {
            Ok(_) => {}
            Err(crate::Error::Database(rusqlite::Error::SqliteFailure(error, _)))
                if error.code == rusqlite::ErrorCode::DiskFull =>
            {
                assert_eq!(
                    raw.query_row("SELECT count(*) FROM jobs", [], |row| row.get::<_, i64>(0))?,
                    before
                );
                full = true;
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    assert!(full, "SQLite fixture did not reach its page limit");
    integrity(&fixture.database)
}

#[test]
fn cross_process_unavailable_backup_remains_pending_until_same_root_returns() -> TestResult {
    let owner = tempdir()?;
    let backup = owner.path().join("backup");
    let offline = owner.path().join("offline");
    std::fs::create_dir(&backup)?;
    let mut fixture =
        Fixture::build_with_backup(AudioFormat::Flac, true, true, true, Some(&backup))?;
    let run = normalize_album(
        &mut fixture.store,
        &fixture.claim,
        fixture.staging.as_ref().ok_or("staging")?,
        &fixture.cover,
    )?;
    let case = case_from_fixture(fixture, Action::Publish, "unused", Some(run))?;
    let source_before = inventory(case.fixture.scratch.path())?;
    let root_identity = crate::infrastructure::filesystem::native_root_identity(&backup)?;
    std::fs::rename(&backup, &offline)?;
    let mut pending = OwnedChild::start(&case.descriptor, "backup_pending", None)?;
    let outcome = pending.finish();
    // Restore the owned fixture path even when the child reports a failure.
    std::fs::rename(&offline, &backup)?;
    outcome?;
    assert_eq!(
        case.fixture.store.job_state(case.fixture.job)?.as_deref(),
        Some("backup_pending")
    );
    assert_eq!(inventory(case.fixture.scratch.path())?, source_before);
    let primary = inventory(case.fixture._library.path())?;
    assert_eq!(
        crate::infrastructure::filesystem::native_root_identity(&backup)?,
        root_identity
    );
    let mut complete = OwnedChild::start(&case.descriptor, "backup_complete", None)?;
    complete.finish()?;
    assert_eq!(
        case.fixture.store.job_state(case.fixture.job)?.as_deref(),
        Some("complete")
    );
    assert_preserved(&primary, &inventory(case.fixture._library.path())?, false);
    integrity(&case.fixture.database)
}

#[cfg(windows)]
#[test]
fn cross_process_windows_file_sharing_denial_keeps_sealed_input() -> TestResult {
    let mut case = prepare(Action::Normalize, "file_locked", AudioFormat::Flac)?;
    let before = inventory(case.fixture.scratch.path())?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let mut child = OwnedChild::start(&case.descriptor, "hold_file", Some(listener.local_addr()?))?;
    let channel = child.ready(&listener, "file_locked")?;
    let staging = StagingRoot::open(&mut case.fixture.store, case.fixture.scratch.path())?;
    let transfer = case
        .fixture
        .store
        .transfers(case.fixture.job, case.fixture.version, Page::new(0, 1)?)?
        .pop()
        .ok_or("received transfer")?;
    assert!(staging.received(&transfer).is_err());
    child.terminate()?;
    drop(channel);
    staging.received(&transfer)?;
    drop(staging);
    assert_eq!(inventory(case.fixture.scratch.path())?, before);
    Ok(())
}

#[test]
fn cross_process_recovery_rejects_changed_approved_content() -> TestResult {
    for (action, point, mode, prefix) in [
        (
            Action::Restore,
            "restore_current_renamed",
            "reject_restore",
            ".domus-restore-",
        ),
        (
            Action::Delete,
            "deletion_file_removed",
            "reject_delete",
            ".domus-retained-",
        ),
    ] {
        let case = prepare(action, point, AudioFormat::Flac)?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let mut child = OwnedChild::start(&case.descriptor, "crash", Some(listener.local_addr()?))?;
        let channel = child.ready(&listener, point)?;
        child.terminate()?;
        drop(channel);
        let files = inventory(case.fixture._library.path())?;
        let changed = files
            .values()
            .find(|file| {
                file.path
                    .components()
                    .any(|part| part.as_os_str().to_string_lossy().starts_with(prefix))
            })
            .ok_or("remaining approved file")?;
        std::fs::write(
            case.fixture._library.path().join(&changed.path),
            b"changed generated evidence",
        )?;
        let before = inventory(case.fixture._library.path())?;
        let mut retry = OwnedChild::start(&case.descriptor, mode, None)?;
        retry.finish()?;
        assert_eq!(inventory(case.fixture._library.path())?, before);
        integrity(&case.fixture.database)?;
    }
    Ok(())
}
