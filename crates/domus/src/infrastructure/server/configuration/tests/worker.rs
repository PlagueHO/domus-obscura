use super::*;

#[test]
fn backend_configuration_worker_capacity_validation_does_not_save_or_start_work() -> TestResult {
    let fixture = Fixture::new(false)?;
    for capacity in [0, 1025] {
        let command = section_command(
            &fixture.source,
            json!([{
                "section":"music_acquisition","local_acquisition":null,
                "production_worker":{"queue_capacity":capacity,"acquisition":null}
            }]),
        )?;
        assert_eq!(
            fixture.source.validate_sections(command)?.errors[0].field,
            "production_worker.queue_capacity"
        );
    }
    let command = section_command(
        &fixture.source,
        json!([{
            "section":"music_acquisition","local_acquisition":null,
            "production_worker":{"queue_capacity":3,"acquisition":null}
        }]),
    )?;
    let result = fixture.source.validate_sections(command)?;
    assert!(result.errors.is_empty());
    assert!(result.state.is_none());
    let state = fixture.source.read()?;
    assert!(state.active.production_worker.is_none());
    assert!(state.saved.production_worker.is_none());
    assert!(!state.restart_required);
    let store = Store::open_read_current(&fixture.source.active.database)?;
    assert!(!store.worker_status(3)?.online);
    assert!(
        store
            .worker_requests(crate::domain::Page::new(0, 10)?)?
            .is_empty()
    );
    Ok(())
}

#[test]
fn backend_configuration_worker_credentials_stay_redacted_and_local_mode_stays_separate()
-> TestResult {
    let mut fixture = Fixture::new(false)?;
    let staging = crate::infrastructure::permissions::private_test_directory()?;
    fixture.select_root(
        "staging",
        crate::infrastructure::filesystem::StorageRole::Staging,
        staging.path(),
    )?;
    let credentials = fixture
        .directory
        .path()
        .join("hidden-worker-credentials.json");
    std::fs::write(
        &credentials,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut store = fixture.source.open_store(StoreOpenMode::Current)?;
    drop(crate::infrastructure::staging::StagingRoot::open(
        &mut store,
        staging.path(),
    )?);
    let acquisition = json!({
        "staging_root":staging.path(),"credentials_file":{"action":"replace","value":credentials},
        "server":"8.8.8.8:2242","acknowledge_plaintext_login":true
    });
    let mut command = fixture.command()?;
    command["draft"]["production_worker"] = json!({"queue_capacity":64,"acquisition":acquisition});
    let mut both = command.clone();
    both["draft"]["local_acquisition"] = acquisition;
    assert_eq!(
        fixture
            .source
            .validate(serde_json::from_value(both)?)?
            .errors[0]
            .field,
        "production_worker"
    );
    let saved = fixture.source.validate(serde_json::from_value(command)?)?;
    assert!(
        saved.errors.is_empty(),
        "{}",
        serde_json::to_string(&saved.errors)?
    );
    let rendered = serde_json::to_string(&saved)?;
    assert!(!rendered.contains("hidden-worker-credentials"));
    assert!(!rendered.contains("fixture-password"));
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["production_worker"] = json!({"queue_capacity":64,"acquisition":{
        "staging_root":staging.path(),"credentials_file":credentials,
        "server":"8.8.8.8:2242","acknowledge_plaintext_login":true
    }});
    std::fs::write(&fixture.path, serde_json::to_vec(&document)?)?;
    let mut keep = fixture.command()?;
    keep["draft"]["production_worker"]["acquisition"]["credentials_file"] =
        json!({"action":"keep"});
    keep["draft"]["production_worker"]["acquisition"]
        .as_object_mut()
        .ok_or("missing acquisition")?
        .remove("credentials_file_configured");
    assert!(
        fixture
            .source
            .validate(serde_json::from_value(keep)?)?
            .errors
            .is_empty()
    );
    Ok(())
}
