use super::*;

fn sharing() -> Value {
    json!({
        "policy": crate::domain::sharing::SharePolicy::default(),
        "account": null,
        "listen": null
    })
}

#[test]
fn backend_configuration_sharing_is_separate_and_passive() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    let sections = json!([{"section":"music_sharing","sharing":sharing()}]);
    let checked = fixture
        .source
        .validate_sections(section_command(&fixture.source, sections.clone())?)?;
    assert!(
        checked.errors.is_empty(),
        "{}",
        serde_json::to_string(&checked.errors)?
    );
    assert_eq!(std::fs::read(&fixture.path)?, before);
    let result = fixture
        .source
        .save_sections(section_command(&fixture.source, sections)?)?;
    assert!(result.errors.is_empty());
    let state = result.state.ok_or("missing saved state")?;
    assert!(state.active.production_worker.is_none());
    assert_eq!(
        state
            .saved
            .production_worker
            .as_ref()
            .and_then(|worker| worker.sharing.as_ref())
            .ok_or("missing sharing")?
            .policy
            .access,
        crate::domain::sharing::ShareAccess::None
    );
    assert!(
        state
            .changed_sections
            .contains(&ConfigurationSection::MusicSharing)
    );
    let sections = json!([{
        "section":"music_acquisition", "local_acquisition":null,
        "production_worker":{"queue_capacity":7,"acquisition":null}
    }]);
    let state = fixture
        .source
        .save_sections(section_command(&fixture.source, sections)?)?
        .state
        .ok_or("missing saved state")?;
    let worker = state.saved.production_worker.ok_or("missing worker")?;
    assert_eq!(worker.queue_capacity, 7);
    assert!(worker.sharing.is_some());
    let state = fixture
        .source
        .save_sections(section_command(
            &fixture.source,
            json!([{"section":"music_sharing","sharing":null}]),
        )?)?
        .state
        .ok_or("missing saved state")?;
    let worker = state.saved.production_worker.ok_or("missing worker")?;
    assert_eq!(worker.queue_capacity, 7);
    assert!(worker.sharing.is_none());
    let store = Store::open_read_current(&fixture.source.active.database)?;
    assert!(!store.worker_status(7)?.online);
    assert!(
        store
            .worker_requests(crate::domain::Page::new(0, 10)?)?
            .is_empty()
    );
    Ok(())
}

#[test]
fn backend_configuration_sharing_limits_and_read_only_are_enforced() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    for (field, value) in [
        ("upload_slots", 0),
        ("peer_connections", 25),
        ("operation_seconds", 3601),
        ("buffer_bytes", 4095),
    ] {
        let mut sharing = sharing();
        sharing["policy"]["limits"][field] = json!(value);
        let result = fixture.source.save_sections(section_command(
            &fixture.source,
            json!([{"section":"music_sharing","sharing":sharing}]),
        )?)?;
        assert!(!result.errors.is_empty(), "{field} accepted");
        assert!(
            result.errors[0]
                .field
                .starts_with("production_worker.sharing")
        );
        assert_eq!(std::fs::read(&fixture.path)?, before);
    }
    let read_only = Fixture::new(true)?;
    assert!(
        read_only
            .source
            .save_sections(section_command(
                &read_only.source,
                json!([{"section":"music_sharing","sharing":sharing()}])
            )?)
            .is_err()
    );
    assert!(
        section_command(
            &fixture.source,
            json!([{
                "section":"music_acquisition","local_acquisition":null,
                "production_worker":{"queue_capacity":64,"acquisition":null,"sharing":sharing()}
            }])
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn backend_configuration_sharing_only_account_stays_hidden_and_keeps_references() -> TestResult {
    let fixture = Fixture::new(false)?;
    let credentials = fixture.directory.path().join("hidden-sharing-account.json");
    std::fs::write(
        &credentials,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut sharing = sharing();
    sharing["policy"]["access"] = json!("all");
    sharing["policy"]["roots"] = json!([{"binding_id":99,"alias":"Music"}]);
    sharing["account"] = json!({
        "credentials_file":{"action":"replace","value":credentials},
        "server":"8.8.8.8:2242","acknowledge_plaintext_login":true
    });
    let result = fixture.source.save_sections(section_command(
        &fixture.source,
        json!([{"section":"music_sharing","sharing":sharing.clone()}]),
    )?)?;
    assert!(
        result.errors.is_empty(),
        "{}",
        serde_json::to_string(&result.errors)?
    );
    let rendered = serde_json::to_string(&result)?;
    assert!(!rendered.contains("hidden-sharing-account"));
    assert!(!rendered.contains("fixture-password"));
    let state = result.state.ok_or("missing state")?;
    assert!(
        state
            .saved
            .production_worker
            .as_ref()
            .ok_or("worker missing")?
            .acquisition
            .is_none()
    );
    sharing["account"]["credentials_file"] = json!({"action":"keep"});
    let result = fixture.source.validate_sections(section_command(
        &fixture.source,
        json!([{"section":"music_sharing","sharing":sharing.clone()}]),
    )?)?;
    assert!(result.errors.is_empty());
    let saved = fixture.source.read()?;
    let mut full = ConfigurationService::draft_from(saved.saved);
    full.browser_assets = None;
    let result = fixture.source.save(BackendConfigurationCommand {
        expected_revision: saved.saved_revision,
        draft: full,
    })?;
    assert!(result.errors.is_empty());
    let persisted: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    assert_eq!(
        persisted["production_worker"]["sharing"]["account"]["credentials_file"],
        json!(credentials)
    );
    sharing["account"]["credentials_file"] = json!({"action":"clear"});
    let result = fixture.source.validate_sections(section_command(
        &fixture.source,
        json!([{"section":"music_sharing","sharing":sharing}]),
    )?)?;
    assert_eq!(
        result.errors[0].field,
        "production_worker.sharing.account.credentials_file"
    );
    assert!(credentials.exists());
    Ok(())
}

#[test]
fn backend_configuration_sharing_rejects_conflicting_accounts_before_file_checks() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    let mut value = sharing();
    value["account"] = json!({
        "credentials_file":{"action":"replace","value":fixture.directory.path().join("unopened-account.json")},
        "server":"8.8.4.4:2242","acknowledge_plaintext_login":true
    });
    let result = fixture.source.save_sections(section_command(&fixture.source, json!([
        {"section":"music_sharing","sharing":value},
        {"section":"music_acquisition","local_acquisition":null,"production_worker":{
            "queue_capacity":64,"acquisition":{
                "staging_root":fixture.directory.path().join("unopened-staging"),
                "credentials_file":{"action":"replace","value":fixture.directory.path().join("unopened-account.json")},
                "server":"8.8.8.8:2242","acknowledge_plaintext_login":true
            }
        }}
    ]))?)?;
    assert_eq!(result.errors[0].field, "production_worker.sharing.account");
    assert_eq!(std::fs::read(&fixture.path)?, before);
    assert!(
        !fixture
            .directory
            .path()
            .join("unopened-account.json")
            .exists()
    );
    assert!(!fixture.directory.path().join("unopened-staging").exists());
    Ok(())
}
