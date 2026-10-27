use domus::application::{add_location, check_location_availability, scan_location};
use domus::domain::{LibraryPath, Page};
use domus::infrastructure::filesystem::native_root_identity;
use domus::infrastructure::staging::StagingRoot;
use domus::infrastructure::store::Store;
#[path = "support/private.rs"]
mod private;
use private::tempdir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn root_availability_checks_preserve_cached_coverage_when_missing_or_replaced() -> TestResult {
    let state = tempdir()?;
    let scratch = tempdir()?;
    let root = scratch.path().join("root");
    let retained = scratch.path().join("retained");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("original.mp3"), b"generated sentinel")?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let id = add_location(&store, &root, "main")?;
    scan_location(&mut store, id)?;
    let location = serde_json::to_value(store.location(id)?)?;
    let entries =
        serde_json::to_value(store.browse(id, &LibraryPath::parse("")?, Page::new(0, 50)?)?)?;
    let raw = rusqlite::Connection::open(&database)?;
    let changes =
        || -> rusqlite::Result<i64> { raw.query_row("PRAGMA data_version", [], |row| row.get(0)) };
    let before = changes()?;
    check_location_availability(&store, id)?;
    assert!(check_location_availability(&store, i64::MAX).is_err());
    std::fs::rename(&root, &retained)?;
    assert!(matches!(
        check_location_availability(&store, id),
        Err(domus::Error::Io(_))
    ));
    std::fs::create_dir(&root)?;
    assert!(matches!(
        check_location_availability(&store, id),
        Err(domus::Error::Conflict(_))
    ));
    assert_eq!(std::fs::read_dir(&root)?.count(), 0);
    std::fs::remove_dir(&root)?;
    std::fs::rename(&retained, &root)?;
    check_location_availability(&store, id)?;
    assert_eq!(
        changes()?,
        before,
        "availability must not write the database"
    );
    assert_eq!(serde_json::to_value(store.location(id)?)?, location);
    assert_eq!(
        serde_json::to_value(store.browse(id, &LibraryPath::parse("")?, Page::new(0, 50)?)?)?,
        entries,
    );
    assert_eq!(
        std::fs::read(root.join("original.mp3"))?,
        b"generated sentinel"
    );
    Ok(())
}

#[test]
fn native_identity_is_stable_across_reopen_and_rename_but_not_a_different_directory() -> TestResult
{
    let scratch = tempdir()?;
    let root = scratch.path().join("root");
    let renamed = scratch.path().join("renamed");
    std::fs::create_dir(&root)?;
    let first = native_root_identity(&root)?;
    assert_eq!(native_root_identity(&root)?, first);
    #[cfg(windows)]
    {
        assert!(first.starts_with("win128:"));
        assert_eq!(first.len(), 7 + 16 + 1 + 32);
    }
    #[cfg(unix)]
    assert!(first.starts_with("unix:"));
    std::fs::rename(&root, &renamed)?;
    assert_eq!(native_root_identity(&renamed)?, first);
    std::fs::create_dir(&root)?;
    assert_ne!(native_root_identity(&root)?, first);
    assert!(native_root_identity(&root.join("missing")).is_err());
    std::fs::write(root.join("not-a-directory"), b"unchanged")?;
    assert!(native_root_identity(&root.join("not-a-directory")).is_err());
    Ok(())
}

#[test]
fn replaced_media_root_never_scans_the_replacement_or_erases_cached_presence() -> TestResult {
    let state = tempdir()?;
    let scratch = tempdir()?;
    let root = scratch.path().join("root");
    let old = scratch.path().join("old");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("original.mp3"), b"original bytes")?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let location = add_location(&store, &root, "main")?;
    let identity = store
        .location(location)?
        .native_identity
        .ok_or("missing native binding")?;
    scan_location(&mut store, location)?;
    drop(store);
    std::fs::rename(&root, &old)?;
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("replacement.mp3"), b"replacement sentinel")?;
    let replacement = native_root_identity(&root)?;
    assert_ne!(identity, replacement);
    let mut store = Store::open_existing(&database)?;
    assert!(scan_location(&mut store, location).is_err());
    let rows = store.browse(location, &LibraryPath::parse("")?, Page::new(0, 200)?)?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].relative_path, "original.mp3");
    assert!(rows[0].present);
    assert_eq!(store.location(location)?.coverage, "incomplete");
    assert!(store.bind_location(location, &replacement).is_err());
    assert!(store.bind_location(location, &identity).is_err());
    assert_eq!(std::fs::read(old.join("original.mp3"))?, b"original bytes");
    assert_eq!(
        std::fs::read(root.join("replacement.mp3"))?,
        b"replacement sentinel"
    );
    std::fs::remove_file(root.join("replacement.mp3"))?;
    std::fs::remove_dir(&root)?;
    std::fs::rename(&old, &root)?;
    scan_location(&mut store, location)?;
    assert_eq!(store.location(location)?.coverage, "complete");
    Ok(())
}

#[test]
fn schema_twelve_upgrade_requires_explicit_native_binding_without_losing_observations() -> TestResult
{
    let state = tempdir()?;
    let media = tempdir()?;
    let another = tempdir()?;
    let database = state.path().join("state.db");
    std::fs::write(media.path().join("track.flac"), b"untouched")?;
    let mut store = Store::open(&database)?;
    let location = add_location(&store, media.path(), "main")?;
    let expected = store
        .location(location)?
        .native_identity
        .ok_or("missing identity")?;
    scan_location(&mut store, location)?;
    drop(store);
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch("DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations; DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots; PRAGMA user_version = 12;")?;
    let mut store = Store::open_existing(&database)?;
    assert!(store.location(location)?.native_identity.is_none());
    let before_check = serde_json::to_value(store.location(location)?)?;
    assert!(check_location_availability(&store, location).is_err());
    assert_eq!(
        serde_json::to_value(store.location(location)?)?,
        before_check
    );
    assert!(scan_location(&mut store, location).is_err());
    assert!(add_location(&store, another.path(), "backup").is_err());
    assert!(
        store
            .bind_location(location, "wrong native identity")
            .is_err()
    );
    assert!(store.location(location)?.native_identity.is_none());
    let rows = store.browse(location, &LibraryPath::parse("")?, Page::new(0, 200)?)?;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].present);
    store.bind_location(location, &expected)?;
    store.bind_location(location, &expected)?;
    assert_eq!(
        store.location(location)?.native_identity.as_deref(),
        Some(expected.as_str())
    );
    scan_location(&mut store, location)?;
    add_location(&store, another.path(), "backup")?;
    assert!(
        raw.execute("UPDATE filesystem_roots SET identity = 'changed'", [])
            .is_err()
    );
    assert!(raw.execute("DELETE FROM filesystem_roots", []).is_err());
    assert_eq!(
        std::fs::read(media.path().join("track.flac"))?,
        b"untouched"
    );
    Ok(())
}

#[test]
fn copied_owner_marker_does_not_authorize_a_replacement_staging_directory() -> TestResult {
    let state = tempdir()?;
    let scratch = tempdir()?;
    let root = scratch.path().join("root");
    let old = scratch.path().join("old");
    std::fs::create_dir(&root)?;
    private::directory(&root)?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let staging = StagingRoot::open(&mut store, &root)?;
    let expected = native_root_identity(&root)?;
    drop(staging);
    std::fs::rename(&root, &old)?;
    std::fs::create_dir(&root)?;
    private::directory(&root)?;
    let marker = std::fs::read(old.join("owner.lock"))?;
    std::fs::write(root.join("owner.lock"), &marker)?;
    let replacement = native_root_identity(&root)?;
    assert_ne!(expected, replacement);
    assert!(matches!(
        StagingRoot::open(&mut store, &root),
        Err(domus::Error::Conflict(_))
    ));
    assert!(matches!(
        store.bind_staging_root(&root, &replacement),
        Err(domus::Error::Conflict(_))
    ));
    assert_eq!(std::fs::read(root.join("owner.lock"))?, marker);
    assert_eq!(std::fs::read_dir(&root)?.count(), 1);
    assert_eq!(std::fs::read(old.join("owner.lock"))?, marker);
    Ok(())
}

#[test]
fn legacy_staging_binding_checks_exact_identity_before_any_marker_change() -> TestResult {
    let state = tempdir()?;
    let media = tempdir()?;
    let scratch = tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let main = add_location(&store, media.path(), "main")?;
    let expected_media = native_root_identity(media.path())?;
    drop(StagingRoot::open(&mut store, scratch.path())?);
    let expected_staging = native_root_identity(scratch.path())?;
    let marker = std::fs::read(scratch.path().join("owner.lock"))?;
    drop(store);
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch("DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations; DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots; PRAGMA user_version = 12;")?;
    let mut store = Store::open_existing(&database)?;
    assert!(StagingRoot::open(&mut store, scratch.path()).is_err());
    assert!(store.bind_staging_root(scratch.path(), "wrong").is_err());
    assert_eq!(std::fs::read(scratch.path().join("owner.lock"))?, marker);
    store.bind_location(main, &expected_media)?;
    store.bind_staging_root(scratch.path(), &expected_staging)?;
    drop(StagingRoot::open(&mut store, scratch.path())?);
    assert_eq!(std::fs::read(scratch.path().join("owner.lock"))?, marker);
    Ok(())
}
