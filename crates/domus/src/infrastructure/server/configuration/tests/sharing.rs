use super::*;

fn sharing() -> Value {
    json!({"policy": crate::domain::sharing::SharePolicy::default(), "account":null, "listen":null})
}

#[test]
fn backend_configuration_sharing_is_separate_and_passive() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    let sections = json!([{"section":"music_sharing","sharing":sharing()}]);
    let command = section_command(&fixture.source, sections.clone())?;
    let proposed = fixture
        .source
        .proposed_update(ConfigurationUpdate::Sections(command.sections), &before)
        .map_err(configuration_error)?;
    let worker = proposed
        .production_worker
        .ok_or("missing proposed worker")?;
    assert!(worker.acquisition.is_none());
    assert_eq!(
        worker.sharing.ok_or("missing sharing")?.policy.access,
        crate::domain::sharing::ShareAccess::None
    );
    let checked = fixture
        .source
        .validate_sections(section_command(&fixture.source, sections)?)?;
    assert!(checked.errors.is_empty() && checked.state.is_none());
    assert_eq!(std::fs::read(&fixture.path)?, before);
    assert!(fixture.source.read()?.saved.production_worker.is_none());
    let store = Store::open_read_current(&fixture.source.active.database)?;
    assert!(!store.worker_status(64)?.online);
    assert!(
        store
            .worker_requests(crate::domain::Page::new(0, 10)?)?
            .is_empty()
    );
    Ok(())
}

#[test]
fn backend_configuration_sharing_validation_keeps_limits_and_never_writes() -> TestResult {
    for read_only in [false, true] {
        let fixture = Fixture::new(read_only)?;
        let before = std::fs::read(&fixture.path)?;
        for (field, value) in [
            ("upload_slots", 0),
            ("peer_connections", 25),
            ("operation_seconds", 3601),
            ("buffer_bytes", 4095),
        ] {
            let mut value_to_check = sharing();
            value_to_check["policy"]["limits"][field] = json!(value);
            let result = fixture.source.validate_sections(section_command(
                &fixture.source,
                json!([{"section":"music_sharing","sharing":value_to_check}]),
            )?)?;
            assert!(!result.errors.is_empty(), "{field} accepted");
            assert!(
                result.errors[0]
                    .field
                    .starts_with("production_worker.sharing")
            );
            assert_eq!(std::fs::read(&fixture.path)?, before);
        }
    }
    Ok(())
}

#[test]
fn backend_configuration_sharing_account_references_stay_hidden() -> TestResult {
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
    let mut value = sharing();
    value["policy"]["access"] = json!("all");
    value["policy"]["roots"] = json!([{"binding_id":99,"alias":"Music"}]);
    value["account"] = json!({"credentials_file":credentials,"server":"8.8.8.8:2242","acknowledge_plaintext_login":true});
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["production_worker"] = json!({"queue_capacity":64,"acquisition":null,"sharing":value});
    let bytes = serde_json::to_vec(&document)?;
    std::fs::write(&fixture.path, &bytes)?;
    let visible = serde_json::to_string(&fixture.source.read()?)?;
    assert!(!visible.contains("hidden-sharing-account"));
    assert!(!visible.contains("fixture-password"));
    value["account"]["credentials_file"] = json!({"action":"keep"});
    let checked = fixture.source.validate_sections(section_command(
        &fixture.source,
        json!([{"section":"music_sharing","sharing":value.clone()}]),
    )?)?;
    assert!(checked.errors.is_empty());
    value["account"]["credentials_file"] = json!({"action":"clear"});
    let checked = fixture.source.validate_sections(section_command(
        &fixture.source,
        json!([{"section":"music_sharing","sharing":value}]),
    )?)?;
    assert_eq!(
        checked.errors[0].field,
        "production_worker.sharing.account.credentials_file"
    );
    assert_eq!(std::fs::read(&fixture.path)?, bytes);
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
    let result = fixture.source.validate_sections(section_command(&fixture.source, json!([
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
