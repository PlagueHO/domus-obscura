use super::*;

#[test]
fn backend_configuration_worker_capacity_is_validated_and_save_does_not_start_work() -> TestResult {
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
    let result = fixture.source.save_sections(command)?;
    assert!(result.errors.is_empty());
    let state = result.state.ok_or("missing saved worker state")?;
    assert!(state.active.production_worker.is_none());
    assert_eq!(
        state
            .saved
            .production_worker
            .ok_or("missing saved worker configuration")?
            .queue_capacity,
        3
    );
    assert!(state.restart_required);
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
    let fixture = Fixture::new(false)?;
    let staging = crate::infrastructure::permissions::private_test_directory()?;
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
    let mut store = Store::open_current_existing(&fixture.source.active.database)?;
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
    let saved = fixture.source.save(serde_json::from_value(command)?)?;
    assert!(
        saved.errors.is_empty(),
        "{}",
        serde_json::to_string(&saved.errors)?
    );
    let rendered = serde_json::to_string(&saved)?;
    assert!(!rendered.contains("hidden-worker-credentials"));
    assert!(!rendered.contains("fixture-password"));
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
