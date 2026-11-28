use super::*;
use crate::infrastructure::server::configuration::SaveSoulseekAccount;

fn save(source: &ConfigurationService) -> TestResult<i64> {
    Ok(source
        .save_soulseek_account(SaveSoulseekAccount {
            expected_revision: source.active_revision().into(),
            username: "generated-test-user".into(),
            password: "generated-test-password".into(),
        })?
        .id)
}

#[test]
fn backend_configuration_soulseek_account_is_private_versioned_and_offline() -> TestResult {
    let fixture = Fixture::new(false)?;
    let original: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    let original_services = serde_json::to_value(&fixture.source.active.services)?;
    let id = save(&fixture.source)?;
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&fixture.path)?)?,
        original
    );
    let store = fixture.source.open_store(StoreOpenMode::ReadCurrent)?;
    let response = serde_json::to_string(&store.soulseek_account_change(id)?)?;
    assert!(!response.contains("generated-test"));
    assert_eq!(store.soulseek_account_change(id)?.state, "approved");
    drop(store);
    let source = fixture.source.activate_stopped_soulseek_account(id)?;
    let store = source.open_store(StoreOpenMode::ReadCurrent)?;
    assert_eq!(store.soulseek_account_change(id)?.state, "active");
    let account = source.active.soulseek_account.as_ref().ok_or("account")?;
    let credentials = crate::infrastructure::worker::read_credentials(
        &store.source(),
        &account.credentials_file,
    )?;
    assert_eq!(credentials.username, "generated-test-user");
    assert_eq!(credentials.password, "generated-test-password");
    let file = account.credentials_file.clone();
    let old_bytes = std::fs::read(&file)?;
    let saved: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    for (key, value) in original.as_object().ok_or("object")? {
        assert_eq!(saved[key], *value, "unrelated host setting {key} changed");
    }
    assert!(source.active.local_acquisition.is_none());
    assert!(source.active.production_worker.is_none());
    assert_eq!(
        serde_json::to_value(&source.active.services)?,
        original_services
    );
    assert!(source.require_configured_soulseek_account(&store).is_ok());
    drop(store);
    let next = save(&source)?;
    let source = source.activate_stopped_soulseek_account(next)?;
    assert_ne!(next, id);
    assert_eq!(std::fs::read(&file)?, old_bytes);
    assert!(source.validate_saved()?.errors.is_empty());
    let database = source.active.database.clone();
    drop(source);
    let database_bytes = std::fs::read(database)?;
    for secret in [
        b"generated-test-user".as_slice(),
        b"generated-test-password".as_slice(),
    ] {
        assert!(
            !database_bytes
                .windows(secret.len())
                .any(|window| window == secret)
        );
    }
    Ok(())
}

#[test]
fn backend_configuration_soulseek_account_rejects_invalid_stale_and_read_only_input() -> TestResult
{
    let fixture = Fixture::new(false)?;
    for (username, password, revision) in [
        (
            "invalid\nuser".to_owned(),
            "password".to_owned(),
            fixture.source.active_revision().to_owned(),
        ),
        (
            "valid".to_owned(),
            "bad\npassword".to_owned(),
            fixture.source.active_revision().to_owned(),
        ),
        (
            "valid".to_owned(),
            "é".repeat(65),
            fixture.source.active_revision().to_owned(),
        ),
        ("valid".to_owned(), "password".to_owned(), "0".repeat(64)),
    ] {
        assert!(
            fixture
                .source
                .save_soulseek_account(SaveSoulseekAccount {
                    username,
                    password,
                    expected_revision: revision,
                })
                .is_err()
        );
    }
    assert!(
        fixture
            .source
            .open_store(StoreOpenMode::ReadCurrent)?
            .pending_soulseek_account_change()?
            .is_none()
    );
    assert!(save(&Fixture::new(true)?.source).is_err());
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["system_settings_read_only"] = true.into();
    std::fs::write(&fixture.path, serde_json::to_vec(&document)?)?;
    assert!(save(&fixture.source).is_err());
    Ok(())
}

#[test]
fn backend_configuration_soulseek_account_replacement_collision_and_pending_changes_fail_safely()
-> TestResult {
    let fixture = Fixture::new(false)?;
    let id = save(&fixture.source)?;
    assert!(save(&fixture.source).is_err());
    let primary = crate::infrastructure::permissions::private_test_directory()?;
    let preview = fixture
        .source
        .inspect_primary_library(primary.path().to_str().ok_or("path")?, false)?;
    assert!(
        fixture
            .source
            .approve_primary_library(SelectPrimaryLibrary {
                path: preview.path,
                expected_revision: preview.expected_revision,
                previous_location_id: preview.previous_location_id,
                review_token: preview.review_token,
                confirm_separation: true,
                backup: None,
            })
            .is_err()
    );
    let path = fixture
        .directory
        .path()
        .join(format!(".domus-soulseek-{id}.credentials.json"));
    let moved = fixture.directory.path().join("retained-account.json");
    std::fs::rename(&path, &moved)?;
    std::fs::write(&path, std::fs::read(&moved)?)?;
    let original = std::fs::read(&fixture.path)?;
    let source = fixture.source.activate_stopped_soulseek_account(id)?;
    assert_eq!(std::fs::read(&fixture.path)?, original);
    assert_eq!(
        source
            .open_store(StoreOpenMode::ReadCurrent)?
            .soulseek_account_change(id)?
            .state,
        "failed"
    );
    let collision = fixture
        .directory
        .path()
        .join(format!(".domus-soulseek-{}.credentials.json", id + 1));
    std::fs::write(&collision, "keep existing evidence")?;
    assert!(save(&source).is_err());
    assert_eq!(
        std::fs::read_to_string(&collision)?,
        "keep existing evidence"
    );
    assert!(
        source
            .open_store(StoreOpenMode::ReadCurrent)?
            .pending_soulseek_account_change()?
            .is_none()
    );
    Ok(())
}

#[test]
fn backend_configuration_soulseek_account_cancel_and_recovery_require_exact_activation()
-> TestResult {
    let fixture = Fixture::new(false)?;
    let id = save(&fixture.source)?;
    let mut store = fixture.source.open_store(StoreOpenMode::Existing)?;
    assert!(
        store
            .begin_soulseek_account(id + 1, &"1".repeat(64))
            .is_err()
    );
    assert_eq!(store.pending_soulseek_account_change()?, Some(id));
    assert_eq!(store.cancel_soulseek_account(id)?.state, "failed");
    assert!(store.cancel_soulseek_account(id).is_err());
    let preparing = store.reserve_soulseek_account(fixture.source.active_revision())?;
    assert_eq!(store.cancel_soulseek_account(preparing)?.state, "failed");
    drop(store);
    let id = save(&fixture.source)?;
    let mut store = fixture.source.open_store(StoreOpenMode::Existing)?;
    store.begin_soulseek_account(id, &"1".repeat(64))?;
    assert!(store.cancel_soulseek_account(id).is_err());
    assert!(
        store
            .complete_soulseek_account(id, &"2".repeat(64))
            .is_err()
    );
    assert_eq!(store.soulseek_account_change(id)?.state, "activating");
    Ok(())
}

#[test]
fn backend_configuration_soulseek_account_resolves_only_omitted_operator_references() -> TestResult
{
    let fixture = Fixture::new(false)?;
    let id = save(&fixture.source)?;
    let source = fixture.source.activate_stopped_soulseek_account(id)?;
    let selected = source
        .active
        .soulseek_account
        .as_ref()
        .ok_or("account")?
        .credentials_file
        .clone();
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["local_acquisition"] = json!({
        "staging_root":"staging", "server":"208.76.170.51:2242",
        "acknowledge_plaintext_login":true
    });
    std::fs::write(&fixture.path, serde_json::to_vec(&document)?)?;
    let loaded = ConfigurationService::open(&fixture.path)?;
    assert_eq!(
        loaded
            .active
            .local_acquisition
            .as_ref()
            .ok_or("acquisition")?
            .credentials_file,
        selected
    );
    assert!(save(&loaded).is_err());
    document["local_acquisition"]["credentials_file"] = "".into();
    std::fs::write(&fixture.path, serde_json::to_vec(&document)?)?;
    assert!(ConfigurationService::open(&fixture.path).is_err());
    document["local_acquisition"]["credentials_file"] = "operator-account.json".into();
    std::fs::write(&fixture.path, serde_json::to_vec(&document)?)?;
    let loaded = ConfigurationService::open(&fixture.path)?;
    assert_eq!(
        loaded
            .active
            .local_acquisition
            .as_ref()
            .ok_or("acquisition")?
            .credentials_file,
        fixture.directory.path().join("operator-account.json")
    );
    Ok(())
}

#[test]
fn backend_configuration_soulseek_account_recovers_only_the_published_candidate() -> TestResult {
    let fixture = Fixture::new(false)?;
    let id = save(&fixture.source)?;
    let prepared = fixture.source.prepare_soulseek_account(id)?;
    let path = fixture.path.clone();
    drop(fixture.source);
    let candidate = prepared.validate()?;
    prepared.publish(&candidate)?;
    assert_eq!(
        candidate
            .open_store(StoreOpenMode::ReadCurrent)?
            .soulseek_account_change(id)?
            .state,
        "activating"
    );
    drop(candidate);
    let published = std::fs::read(&path)?;
    let mut altered: Value = serde_json::from_slice(&published)?;
    altered["origin"] = "https://localhost:9443".into();
    altered["listen"] = "127.0.0.1:9443".into();
    std::fs::write(&path, serde_json::to_vec(&altered)?)?;
    let source = ConfigurationService::open(&path)?;
    let store = source.open_store(StoreOpenMode::ReadCurrent)?;
    assert!(source.activate_configured_soulseek_account(&store).is_err());
    assert_eq!(store.soulseek_account_change(id)?.state, "activating");
    drop(store);
    drop(source);
    std::fs::write(&path, published)?;
    let source = ConfigurationService::open(&path)?;
    let store = source.open_store(StoreOpenMode::Existing)?;
    source.activate_configured_soulseek_account(&store)?;
    assert_eq!(store.soulseek_account_change(id)?.state, "active");
    drop(store);
    // Passive status must not read a credential file or test a connection.
    let selected = source.active.soulseek_account.as_ref().ok_or("account")?;
    std::fs::remove_file(&selected.credentials_file)?;
    assert!(source.soulseek_account_status()?.configured);
    Ok(())
}

#[test]
fn backend_configuration_soulseek_account_bounds_interrupted_history() -> TestResult {
    let fixture = Fixture::new(false)?;
    let mut store = fixture.source.open_store(StoreOpenMode::Existing)?;
    for _ in 0..100 {
        let id = store.reserve_soulseek_account(fixture.source.active_revision())?;
        store.cancel_soulseek_account(id)?;
    }
    assert!(
        store
            .reserve_soulseek_account(fixture.source.active_revision())
            .is_err()
    );
    assert!(store.pending_soulseek_account_change()?.is_none());
    Ok(())
}

#[test]
fn backend_configuration_soulseek_account_stale_host_never_restarts_a_different_configuration()
-> TestResult {
    let fixture = Fixture::new(false)?;
    let id = save(&fixture.source)?;
    let mut changed: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    changed["origin"] = "https://localhost:9443".into();
    changed["listen"] = "127.0.0.1:9443".into();
    let bytes = serde_json::to_vec(&changed)?;
    std::fs::write(&fixture.path, &bytes)?;
    assert!(
        fixture
            .source
            .activate_stopped_soulseek_account(id)
            .is_err()
    );
    assert_eq!(std::fs::read(&fixture.path)?, bytes);
    let source = ConfigurationService::open(&fixture.path)?;
    assert_eq!(
        source
            .open_store(StoreOpenMode::ReadCurrent)?
            .soulseek_account_change(id)?
            .state,
        "failed"
    );
    assert!(source.active.soulseek_account.is_none());
    Ok(())
}
