use super::*;
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn fixture() -> Result<(tempfile::TempDir, Store, i64, i64)> {
    let directory = crate::infrastructure::permissions::private_test_directory()?;
    let primary = directory.path().join("primary");
    let backup = directory.path().join("backup");
    std::fs::create_dir(&primary)?;
    std::fs::create_dir(&backup)?;
    let store = Store::open(&directory.path().join("state.db"))?;
    let main = crate::application::add_location(&store, &primary, "main")?;
    let copy = crate::application::add_location(&store, &backup, "backup")?;
    Ok((directory, store, main, copy))
}

#[test]
fn scan_checks_backups_without_changing_media() -> TestResult {
    let (directory, mut store, main, backup) = fixture()?;
    for (folder, name, bytes) in [
        ("primary", "same.txt", "same"),
        ("backup", "same.txt", "same"),
        ("primary", "missing.txt", "missing"),
        ("primary", "size.txt", "longer"),
        ("backup", "size.txt", "short"),
        ("backup", "extra.txt", "extra"),
    ] {
        std::fs::write(directory.path().join(folder).join(name), bytes)?;
    }
    crate::application::scan_library(&mut store, main)?;
    assert_eq!(store.location(main)?.coverage, "complete");
    assert_eq!(store.location(backup)?.coverage, "complete");
    let first = store.compare_library_backup(backup, "", 2)?;
    assert_eq!(first.differences.len(), 2);
    assert_eq!(first.differences[0].path, "extra.txt");
    assert_eq!(first.differences[0].difference, "only_in_backup");
    assert_eq!(first.differences[1].difference, "missing_in_backup");
    let next = store.compare_library_backup(backup, &first.differences[1].path, 2)?;
    assert_eq!(next.differences.len(), 1);
    assert_eq!(next.differences[0].difference, "file_size_differs");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("backup").join("size.txt"))?,
        "short"
    );
    assert!(!directory.path().join("backup").join("missing.txt").exists());
    assert!(store.compare_library_backup(main, "", 50).is_err());
    assert!(crate::application::scan_library(&mut store, backup).is_err());
    std::fs::rename(
        directory.path().join("backup"),
        directory.path().join("offline-backup"),
    )?;
    assert!(crate::application::scan_library(&mut store, main).is_err());
    assert_eq!(store.location(main)?.coverage, "complete");
    assert_eq!(store.location(backup)?.coverage, "incomplete");
    let incomplete = store.compare_library_backup(backup, "", 50)?;
    assert!(incomplete.differences.is_empty());
    assert_eq!(incomplete.backup.coverage, "incomplete");
    Ok(())
}

#[test]
fn schedule_is_versioned_guarded_and_idempotent() -> TestResult {
    let (_directory, mut store, main, _backup) = fixture()?;
    assert!(store.library_scan_policy()?.interval_minutes.is_none());
    assert!(!store.library_scan_due()?);
    let policy = SaveLibraryScanPolicy {
        expected_version: 1,
        interval_minutes: Some(1440),
    };
    assert_eq!(store.save_library_scan_policy(&policy)?.version, 2);
    assert!(store.save_library_scan_policy(&policy).is_err());
    assert!(!store.library_scan_due()?);
    let binding = "a".repeat(64);
    store
        .connection
        .execute("UPDATE library_scan_schedule SET next_due=0", [])?;
    assert!(
        store
            .schedule_library_scan(64, &binding, Err(Error::Conflict("read-only".into())))
            .is_err()
    );
    assert!(store.worker_requests(Page::new(0, 100)?)?.is_empty());
    assert!(
        store
            .library_scan_policy()?
            .last_error
            .is_some_and(|error| error.contains("read-only"))
    );
    store
        .connection
        .execute("UPDATE library_scan_schedule SET next_due=0", [])?;
    store.schedule_library_scan(64, &binding, Ok(()))?;
    let requests = store.worker_requests(Page::new(0, 100)?)?;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].operation,
        WorkerOperation::ScanLibrary { location_id: main }
    );
    store
        .connection
        .execute("UPDATE library_scan_schedule SET next_due=0", [])?;
    store.schedule_library_scan(64, &binding, Ok(()))?;
    assert_eq!(store.worker_requests(Page::new(0, 100)?)?.len(), 1);
    let disabled = store.save_library_scan_policy(&SaveLibraryScanPolicy {
        expected_version: 2,
        interval_minutes: None,
    })?;
    assert!(disabled.next_due.is_none());
    assert!(!store.library_scan_due()?);
    assert!(
        store
            .connection
            .execute(
                "UPDATE library_scan_settings SET interval_minutes=60 WHERE version=2",
                [],
            )
            .is_err()
    );
    assert!(
        store
            .connection
            .execute("DELETE FROM library_scan_settings WHERE version=2", [],)
            .is_err()
    );
    assert_eq!(
        store.connection.query_row(
            "SELECT interval_minutes FROM library_scan_settings WHERE version=2",
            [],
            |row| row.get::<_, i64>(0),
        )?,
        1440
    );
    Ok(())
}

#[test]
fn setup_distinguishes_missing_policy_and_offline_primary() -> TestResult {
    let (directory, mut store, main, backup) = fixture()?;
    let initial = store.library_setup()?;
    assert_eq!(initial.primary_id, Some(main));
    assert!(initial.primary_error.is_none());
    assert!(!initial.output_configured);
    store.save_settings(None, &crate::domain::OutputSettings::default())?;
    assert!(store.library_setup()?.output_configured);
    std::fs::rename(
        directory.path().join("primary"),
        directory.path().join("offline-primary"),
    )?;
    let offline = store.library_setup()?;
    assert_eq!(offline.primary_id, Some(main));
    assert!(offline.primary_error.is_some());
    assert!(offline.output_configured);
    assert!(
        store
            .register_selected_library_folder("unselected", false, "backup")
            .is_err()
    );
    assert!(
        store
            .compare_library_backup(backup, "../escape", 50)
            .is_err()
    );
    Ok(())
}
