use super::*;
use crate::infrastructure::server::configuration::SelectPrimaryLibrary;
use crate::infrastructure::store::StoreOpenMode;

fn approve(source: &ConfigurationService, path: &Path) -> TestResult<i64> {
    let change = source.approve_primary_library(selection(source, path, false)?)?;
    assert_eq!(change.state, "approved");
    Ok(change.id)
}

fn selection(
    source: &ConfigurationService,
    path: &Path,
    backup: bool,
) -> TestResult<SelectPrimaryLibrary> {
    let preview = source.inspect_primary_library(path.to_str().ok_or("fixture path")?, backup)?;
    Ok(SelectPrimaryLibrary {
        path: preview.path,
        expected_revision: preview.expected_revision,
        previous_location_id: preview.previous_location_id,
        review_token: preview.review_token,
        confirm_separation: true,
        backup: backup.then_some(true),
    })
}

#[test]
fn primary_library_switch_retains_old_identity_and_media_and_survives_restart() -> TestResult {
    let fixture = Fixture::new(false)?;
    let first = crate::infrastructure::permissions::private_test_directory()?;
    let second = crate::infrastructure::permissions::private_test_directory()?;
    std::fs::write(first.path().join("first.txt"), "first original")?;
    std::fs::write(second.path().join("second.txt"), "second original")?;
    let original: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    let first_change = approve(&fixture.source, first.path())?;
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&fixture.path)?)?,
        original
    );
    let source = fixture.source.activate_stopped_primary(first_change)?;
    let mut store = source.open_store(StoreOpenMode::Existing)?;
    let first_id = store
        .primary_library_selection()?
        .ok_or("missing first primary")?;
    crate::application::scan_library(&mut store, first_id)?;
    let first_location = store.location(first_id)?;
    drop(store);
    assert_eq!(
        source
            .open_store(StoreOpenMode::ReadCurrent)?
            .primary_library_change(first_change)?
            .state,
        "active"
    );
    drop(source);
    let source = ConfigurationService::open(&fixture.path)?;
    assert!(source.validate_saved()?.errors.is_empty());
    let second_change = approve(&source, second.path())?;
    let source = source.activate_stopped_primary(second_change)?;
    let store = source.open_store(StoreOpenMode::ReadCurrent)?;
    let locations = store.locations(crate::domain::Page::new(0, 100)?)?;
    assert_eq!(locations.len(), 1);
    assert_ne!(locations[0].id, first_id);
    assert_eq!(locations[0].kind, "main");
    assert_eq!(locations[0].coverage, "unknown");
    let retained = store.location(first_id)?;
    assert_eq!(retained.root, first_location.root);
    assert_eq!(retained.native_identity, first_location.native_identity);
    assert_eq!(retained.coverage, "complete");
    assert_eq!(
        std::fs::read_to_string(first.path().join("first.txt"))?,
        "first original"
    );
    assert_eq!(
        std::fs::read_to_string(second.path().join("second.txt"))?,
        "second original"
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    for (key, value) in original.as_object().ok_or("fixture object")? {
        if key != "storage" {
            assert_eq!(saved[key], *value, "unrelated host setting {key} changed");
        }
    }
    assert!(source.require_configured_primary(&store).is_ok());
    drop(store);
    drop(source);
    let source = ConfigurationService::open(&fixture.path)?;
    let mut store = source.open_store(StoreOpenMode::Existing)?;
    source.activate_configured_primary(&mut store)?;
    assert!(source.require_configured_primary(&store).is_ok());
    Ok(())
}

#[test]
fn primary_library_preview_rejects_overlap_stale_evidence_and_read_only() -> TestResult {
    let fixture = Fixture::new(false)?;
    let media = crate::infrastructure::permissions::private_test_directory()?;
    assert!(
        fixture
            .source
            .inspect_primary_library(fixture.directory.path().to_str().ok_or("path")?, false)
            .is_err()
    );
    let preview = fixture
        .source
        .inspect_primary_library(media.path().to_str().ok_or("path")?, false)?;
    assert!(
        fixture
            .source
            .approve_primary_library(SelectPrimaryLibrary {
                path: preview.path.clone(),
                expected_revision: preview.expected_revision.clone(),
                previous_location_id: preview.previous_location_id,
                review_token: preview.review_token.clone(),
                confirm_separation: false,
                backup: None,
            })
            .is_err()
    );
    assert!(
        fixture
            .source
            .approve_primary_library(SelectPrimaryLibrary {
                path: preview.path,
                expected_revision: preview.expected_revision,
                previous_location_id: preview.previous_location_id,
                review_token: "0".repeat(64),
                confirm_separation: true,
                backup: None,
            })
            .is_err()
    );
    let read_only = Fixture::new(true)?;
    assert!(
        read_only
            .source
            .inspect_primary_library(media.path().to_str().ok_or("path")?, false)
            .is_err()
    );
    assert!(
        fixture
            .source
            .open_store(StoreOpenMode::ReadCurrent)?
            .pending_primary_library_change()?
            .is_none()
    );
    Ok(())
}

#[test]
fn backup_library_recovers_published_approval_without_changing_primary() -> TestResult {
    let fixture = Fixture::new(false)?;
    let primary = crate::infrastructure::permissions::private_test_directory()?;
    let first = crate::infrastructure::permissions::private_test_directory()?;
    let second = crate::infrastructure::permissions::private_test_directory()?;
    assert!(selection(&fixture.source, first.path(), true).is_err());
    let id = approve(&fixture.source, primary.path())?;
    let source = fixture.source.activate_stopped_primary(id)?;
    let mut store = source.open_store(StoreOpenMode::Existing)?;
    let primary_id = store
        .primary_library_selection()?
        .ok_or("missing primary")?;
    crate::application::scan_library(&mut store, primary_id)?;
    let schedule = serde_json::to_value(store.save_library_scan_policy(
        &crate::domain::library::SaveLibraryScanPolicy {
            expected_version: store.library_scan_policy()?.version,
            interval_minutes: Some(60),
        },
    )?)?;
    let primary_location = serde_json::to_value(store.location(primary_id)?)?;
    drop(store);

    let nested = primary.path().join("nested");
    std::fs::create_dir(&nested)?;
    assert!(selection(&source, &nested, true).is_err());
    let mut wrong_role = selection(&source, first.path(), false)?;
    wrong_role.backup = Some(true);
    assert!(source.approve_primary_library(wrong_role).is_err());

    let stale = selection(&source, second.path(), true)?;
    let id = source
        .approve_primary_library(selection(&source, first.path(), true)?)?
        .id;
    let source = source.activate_stopped_primary(id)?;
    assert!(source.approve_primary_library(stale).is_err());
    assert!(selection(&source, first.path(), true).is_err());
    let store = source.open_store(StoreOpenMode::ReadCurrent)?;
    let first_id = store
        .primary_library_change(id)?
        .location_id
        .ok_or("missing backup")?;
    let first_location = serde_json::to_value(store.location(first_id)?)?;
    drop(store);

    std::fs::write(second.path().join("original.txt"), "unchanged")?;
    let id = source
        .approve_primary_library(selection(&source, second.path(), true)?)?
        .id;
    let prepared = source.prepare_primary_activation(id)?;
    drop(source);
    let candidate = prepared.validate()?;
    prepared.publish(&candidate)?;
    drop(candidate);
    let source = ConfigurationService::open(&fixture.path)?;
    let mut store = source.open_store(StoreOpenMode::Existing)?;
    assert!(source.require_configured_primary(&store).is_err());
    assert_eq!(store.locations(crate::domain::Page::new(0, 100)?)?.len(), 2);
    source.activate_configured_primary(&mut store)?;
    source.activate_configured_primary(&mut store)?;
    source.require_configured_primary(&store)?;
    assert_eq!(store.primary_library_selection()?, Some(primary_id));
    assert_eq!(
        serde_json::to_value(store.location(primary_id)?)?,
        primary_location
    );
    assert_eq!(
        serde_json::to_value(store.location(first_id)?)?,
        first_location
    );
    assert_eq!(
        serde_json::to_value(store.library_scan_policy()?)?,
        schedule
    );
    let applied = store.primary_library_change(id)?;
    assert_eq!(applied.state, "active");
    let backup = store.location(applied.location_id.ok_or("missing second backup")?)?;
    assert_eq!(backup.kind, "backup");
    assert_eq!(backup.coverage, "unknown");
    assert_eq!(store.locations(crate::domain::Page::new(0, 100)?)?.len(), 3);
    assert_eq!(
        std::fs::read_to_string(second.path().join("original.txt"))?,
        "unchanged"
    );
    Ok(())
}

#[test]
fn primary_library_activation_rechecks_replaced_folders_before_json_publication() -> TestResult {
    let fixture = Fixture::new(false)?;
    let media_parent = crate::infrastructure::permissions::private_test_directory()?;
    let media = media_parent.path().join("media");
    std::fs::create_dir(&media)?;
    let change = approve(&fixture.source, &media)?;
    let original = std::fs::read(&fixture.path)?;
    std::fs::rename(&media, media_parent.path().join("original"))?;
    std::fs::create_dir(&media)?;
    let source = fixture.source.activate_stopped_primary(change)?;
    assert_eq!(std::fs::read(&fixture.path)?, original);
    let store = source.open_store(StoreOpenMode::ReadCurrent)?;
    assert_eq!(store.primary_library_change(change)?.state, "failed");
    assert!(store.primary_library_selection()?.is_none());
    Ok(())
}

#[test]
fn primary_library_activation_refuses_unfinished_work_without_changing_json() -> TestResult {
    let fixture = Fixture::new(false)?;
    let media = crate::infrastructure::permissions::private_test_directory()?;
    let change = approve(&fixture.source, media.path())?;
    let mut store = fixture.source.open_store(StoreOpenMode::Existing)?;
    store.enqueue(1)?;
    drop(store);
    let original = std::fs::read(&fixture.path)?;
    let source = fixture.source.activate_stopped_primary(change)?;
    assert_eq!(std::fs::read(&fixture.path)?, original);
    assert_eq!(
        source
            .open_store(StoreOpenMode::ReadCurrent)?
            .primary_library_change(change)?
            .state,
        "failed"
    );
    Ok(())
}
