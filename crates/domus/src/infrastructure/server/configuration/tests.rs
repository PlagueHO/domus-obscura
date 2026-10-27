use super::*;
use serde_json::{Value, json};

mod worker;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn section_command(
    source: &ConfigurationService,
    sections: Value,
) -> TestResult<ConfigurationSectionCommand> {
    Ok(serde_json::from_value(
        json!({"expected_revision": source.read()?.saved_revision, "sections": sections}),
    )?)
}

#[test]
fn backend_configuration_section_saves_preserve_other_values_and_detect_stale_edits() -> TestResult
{
    let fixture = Fixture::new(false)?;
    let before: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    let stale = section_command(
        &fixture.source,
        json!([{"section":"music_providers", "services": {
            "discovery":[], "metadata":[], "artwork":[], "acquisition":[]
        }}]),
    )?;
    let general = json!([{"section":"general", "environment":"development",
        "listen":"127.0.0.1:9443", "origin":"https://localhost:9443", "browser_assets":null}]);
    let command = section_command(&fixture.source, general.clone())?;
    assert!(fixture.source.validate_sections(command)?.errors.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&fixture.path)?)?,
        before
    );
    let state = fixture
        .source
        .save_sections(section_command(&fixture.source, general)?)?
        .state
        .ok_or("missing saved state")?;
    assert_eq!(state.changed_sections, vec![ConfigurationSection::General]);
    assert_eq!(state.active.origin, "https://localhost:8443");
    assert_eq!(state.saved.origin, "https://localhost:9443");
    let saved: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    for field in [
        "database",
        "certificate_file",
        "private_key_file",
        "system_settings_read_only",
    ] {
        assert_eq!(
            before[field], saved[field],
            "{field} changed in another section"
        );
    }
    assert!(fixture.source.save_sections(stale).is_err());
    let state = fixture
        .source
        .save_sections(section_command(
            &fixture.source,
            json!([{"section":"music_providers", "services":{
                "discovery":[], "metadata":[], "artwork":[], "acquisition":[]
            }}]),
        )?)?
        .state
        .ok_or("missing saved state")?;
    assert_eq!(state.saved.origin, "https://localhost:9443");
    assert!(state.saved.services.discovery.is_empty());
    assert_eq!(
        state.changed_sections,
        vec![
            ConfigurationSection::General,
            ConfigurationSection::MusicProviders
        ]
    );
    Ok(())
}

#[test]
fn backend_configuration_section_batches_are_atomic_and_reject_invalid_shapes() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    let providers = json!({"section":"music_providers", "services":{"discovery":[], "metadata":[], "artwork":[], "acquisition":[]}});
    for sections in [
        json!([]),
        json!([providers.clone(), providers.clone()]),
        json!([providers.clone(), {"section":"tls", "certificate_file":"relative", "private_key_file":{"action":"keep"}}]),
    ] {
        let result = fixture
            .source
            .save_sections(section_command(&fixture.source, sections)?)?;
        assert!(!result.errors.is_empty());
        assert!(result.state.is_none());
        assert_eq!(std::fs::read(&fixture.path)?, before);
    }
    for invalid in [
        json!([{"section":"movies"}]),
        json!([{"section":"music_acquisition", "local_acquisition":null, "system_settings_read_only":false}]),
    ] {
        assert!(section_command(&fixture.source, invalid).is_err());
    }
    let result = fixture.source.save_sections(section_command(
        &fixture.source,
        json!([
            providers, {"section":"music_acquisition", "local_acquisition":null},
            {"section":"storage", "database":fixture.source.read()?.saved.database, "cache":null}
        ]),
    )?)?;
    assert!(result.errors.is_empty());
    let state = result.state.ok_or("missing saved state")?;
    assert!(state.saved.cache.is_none());
    assert!(state.saved.local_acquisition.is_none());
    Ok(())
}

#[test]
fn backend_configuration_unselected_general_section_keeps_its_original_origin() -> TestResult {
    let fixture = Fixture::new(false)?;
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    saved["origin"] = json!("https://localhost:8443/");
    let bytes = serde_json::to_vec(&saved)?;
    std::fs::write(&fixture.path, &bytes)?;
    let (source, _, _) = ConfigurationService::load(&fixture.path)?;
    assert!(source.read()?.changed_sections.is_empty());
    let result = source.save_sections(section_command(&source, json!([{
        "section":"music_providers", "services":{"discovery":[], "metadata":[], "artwork":[], "acquisition":[]}
    }]))?)?;
    assert!(result.errors.is_empty());
    let after: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    assert_eq!(after["origin"], saved["origin"]);
    assert_eq!(
        source.read()?.changed_sections,
        vec![ConfigurationSection::MusicProviders]
    );
    Ok(())
}

#[test]
fn backend_configuration_sections_redact_changed_hidden_references_and_keep_read_only() -> TestResult
{
    let fixture = Fixture::new(false)?;
    let key = fixture.directory.path().join("replacement-hidden-key.pem");
    std::fs::copy(fixture.directory.path().join("hidden-key.pem"), &key)?;
    let result = fixture.source.save_sections(section_command(
        &fixture.source,
        json!([{
            "section":"tls", "certificate_file":fixture.source.read()?.saved.certificate_file,
            "private_key_file":{"action":"replace","value":key}
        }]),
    )?)?;
    let state = result.state.ok_or("missing saved state")?;
    assert_eq!(state.changed_sections, vec![ConfigurationSection::Tls]);
    assert!(state.active.private_key_file_configured && state.saved.private_key_file_configured);
    assert!(!serde_json::to_string(&state)?.contains("replacement-hidden"));
    let restricted = Fixture::new(true)?;
    assert!(
        restricted
            .source
            .save_sections(section_command(
                &restricted.source,
                json!([{"section":"music_acquisition", "local_acquisition":null}])
            )?)
            .is_err()
    );
    Ok(())
}

#[test]
fn backend_configuration_reload_is_explicit_revision_bound_and_gates_saves() -> TestResult {
    let fixture = Fixture::new(false)?;
    let revision = fixture.source.read()?.saved_revision;
    let command = || ConfigurationReloadCommand {
        expected_revision: revision.clone(),
    };
    assert!(fixture.source.request_reload(command()).is_err());
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    fixture.source.attach_reload(sender)?;
    assert!(matches!(
        fixture.source.request_reload(command())?,
        ConfigurationReloadState::Pending { .. }
    ));
    assert_eq!(receiver.try_recv()?, revision);
    assert!(fixture.source.request_reload(command()).is_err());
    assert!(fixture.source.require_editable().is_err());
    assert!(
        fixture
            .source
            .save_sections(section_command(
                &fixture.source,
                json!([{"section":"music_acquisition", "local_acquisition":null}])
            )?)
            .is_err()
    );
    let candidate = fixture.source.reload_candidate(&revision)?;
    candidate.verify_reload(&revision)?;
    candidate.finish_reload(revision.clone(), Ok(()))?;
    assert!(matches!(
        fixture.source.read()?.reload,
        ConfigurationReloadState::Active { .. }
    ));
    assert!(fixture.source.require_editable().is_ok());
    let mut changed: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    changed["origin"] = json!("https://localhost:9443");
    std::fs::write(&fixture.path, serde_json::to_vec(&changed)?)?;
    assert!(fixture.source.request_reload(command()).is_err());
    assert!(candidate.verify_reload(&revision).is_err());
    fixture.source.finish_reload(
        revision,
        Err(Error::Internal("injected preparation failure")),
    )?;
    assert!(matches!(
        fixture.source.read()?.reload,
        ConfigurationReloadState::Failed { .. }
    ));
    assert_eq!(
        fixture.source.read()?.active.origin,
        "https://localhost:8443"
    );
    let restricted = Fixture::new(true)?;
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    restricted.source.attach_reload(sender)?;
    assert!(
        restricted
            .source
            .request_reload(ConfigurationReloadCommand {
                expected_revision: restricted.source.read()?.saved_revision,
            })
            .is_err()
    );
    Ok(())
}

struct Fixture {
    directory: tempfile::TempDir,
    path: PathBuf,
    source: ConfigurationService,
}

#[tokio::test]
async fn backend_configuration_https_reload_hands_over_listener_and_keeps_host_on_failure()
-> TestResult {
    crate::infrastructure::tls::initialize_crypto()?;
    use std::net::TcpListener;
    use std::time::Duration;
    let fixture = Fixture::new(false)?;
    let reservation = TcpListener::bind(("127.0.0.1", 0))?;
    let address = reservation.local_addr()?;
    drop(reservation);
    let origin = format!("https://localhost:{}", address.port());
    let mut initial: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    initial["listen"] = json!(address.to_string());
    initial["origin"] = json!(origin);
    std::fs::write(&fixture.path, serde_json::to_vec(&initial)?)?;
    let prepared = super::super::PreparedServer::load(&fixture.path)?;
    let host = tokio::spawn(prepared.run());
    struct StopOnDrop(tokio::task::AbortHandle);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _stop = StopOnDrop(host.abort_handle());
    let client = reqwest::Client::builder()
        .tls_certs_only([reqwest::Certificate::from_pem(&std::fs::read(
            fixture.directory.path().join("certificate.pem"),
        )?)?])
        .resolve("localhost", address)
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()?;
    let url = format!("{origin}/api/system/configuration");
    let mut initial_state = None;
    for _ in 0..80 {
        if let Ok(response) = client.get(&url).send().await {
            initial_state = Some(serde_json::from_slice::<Value>(&response.bytes().await?)?);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let initial_state = initial_state.ok_or("API did not start")?;
    let sections = json!({"expected_revision":initial_state["saved_revision"], "sections":[{
        "section":"music_providers", "services":{"discovery":[], "metadata":[], "artwork":[], "acquisition":[]}
    }]});
    let response = client
        .post(format!("{url}/sections/save"))
        .header("Origin", &origin)
        .header("x-domus-csrf", "1")
        .header("Content-Type", "application/json")
        .body(sections.to_string())
        .send()
        .await?;
    assert!(response.status().is_success());
    let saved: Value = serde_json::from_slice(&response.bytes().await?)?;
    let revision = saved["state"]["saved_revision"].clone();
    assert_eq!(
        saved["state"]["active_revision"],
        initial_state["active_revision"]
    );
    for guarded in [
        client
            .post(format!("{url}/reload"))
            .header("Content-Type", "application/json")
            .body(json!({"expected_revision":revision}).to_string()),
        client
            .post(format!("{url}/reload"))
            .header("Origin", "https://untrusted.invalid")
            .header("x-domus-csrf", "1")
            .header("Content-Type", "application/json")
            .body(json!({"expected_revision":revision}).to_string()),
    ] {
        assert_eq!(
            guarded.send().await?.status(),
            reqwest::StatusCode::FORBIDDEN
        );
    }
    let accepted = client
        .post(format!("{url}/reload"))
        .header("Origin", &origin)
        .header("x-domus-csrf", "1")
        .header("Content-Type", "application/json")
        .body(json!({"expected_revision":revision}).to_string())
        .send()
        .await?
        .error_for_status()?;
    let accepted: Value = serde_json::from_slice(&accepted.bytes().await?)?;
    assert_eq!(accepted["status"], "pending");
    let mut activated = false;
    for _ in 0..100 {
        if let Ok(response) = client.get(&url).send().await {
            let state: Value = serde_json::from_slice(&response.bytes().await?)?;
            if state["active_revision"] == revision {
                assert_eq!(state["restart_required"], false);
                assert_eq!(state["reload"]["status"], "active");
                activated = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(activated, "saved revision did not become active");

    let occupied = TcpListener::bind(("127.0.0.1", 0))?;
    let occupied_address = occupied.local_addr()?;
    let saved = client.post(format!("{url}/sections/save")).header("Origin", &origin).header("x-domus-csrf", "1").header("Content-Type", "application/json").body(json!({
        "expected_revision":revision, "sections":[{"section":"general", "environment":"development",
        "listen":occupied_address.to_string(), "origin":format!("https://localhost:{}", occupied_address.port()), "browser_assets":null}]
    }).to_string()).send().await?.error_for_status()?;
    let saved: Value = serde_json::from_slice(&saved.bytes().await?)?;
    let next_revision = saved["state"]["saved_revision"].clone();
    assert!(next_revision.is_string());
    client
        .post(format!("{url}/reload"))
        .header("Origin", &origin)
        .header("x-domus-csrf", "1")
        .header("Content-Type", "application/json")
        .body(json!({"expected_revision":next_revision}).to_string())
        .send()
        .await?
        .error_for_status()?;
    let mut failed = false;
    for _ in 0..100 {
        let response = client.get(&url).send().await?.error_for_status()?;
        let state: Value = serde_json::from_slice(&response.bytes().await?)?;
        if state["reload"]["status"] == "failed" {
            assert_eq!(state["active_revision"], revision);
            assert_eq!(state["active"]["origin"], origin);
            assert_eq!(state["restart_required"], true);
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        failed,
        "occupied replacement port was not reported as a reload failure"
    );
    assert!(
        !host.is_finished(),
        "failed reload stopped the previous host"
    );
    drop(occupied);
    client
        .post(format!("{url}/reload"))
        .header("Origin", &origin)
        .header("x-domus-csrf", "1")
        .header("Content-Type", "application/json")
        .body(json!({"expected_revision":next_revision}).to_string())
        .send()
        .await?
        .error_for_status()?;
    let next_origin = format!("https://localhost:{}", occupied_address.port());
    let next_url = format!("{next_origin}/api/system/configuration");
    let mut moved = false;
    for _ in 0..100 {
        if let Ok(response) = client.get(&next_url).send().await {
            let state: Value = serde_json::from_slice(&response.bytes().await?)?;
            if state["active_revision"] == next_revision {
                assert_eq!(state["active"]["origin"], next_origin);
                assert_eq!(state["restart_required"], false);
                moved = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(moved, "reload did not activate the new listener address");
    assert!(
        client.get(&url).send().await.is_err(),
        "old listener remained active"
    );
    Ok(())
}

impl Fixture {
    fn new(read_only: bool) -> TestResult<Self> {
        let directory = crate::infrastructure::permissions::private_test_directory()?;
        let root = directory.path();
        let database = root.join("state.db");
        drop(Store::open(&database)?);
        let key = rcgen::KeyPair::generate()?;
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()])?;
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        std::fs::write(
            root.join("certificate.pem"),
            params.self_signed(&key)?.pem(),
        )?;
        std::fs::write(root.join("hidden-key.pem"), key.serialize_pem())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                root.join("hidden-key.pem"),
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
        let path = root.join("configuration.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "environment": "development", "system_settings_read_only": read_only,
                "database": database, "listen": "127.0.0.1:8443",
                "origin": "https://localhost:8443", "certificate_file": root.join("certificate.pem"),
                "private_key_file": root.join("hidden-key.pem")
            }))?,
        )?;
        let (source, _, _) = ConfigurationService::load(&path)?;
        Ok(Self {
            directory,
            path,
            source,
        })
    }

    fn command(&self) -> TestResult<Value> {
        let state = self.source.read()?;
        let mut draft = serde_json::to_value(state.saved)?;
        let object = draft
            .as_object_mut()
            .ok_or("configuration was not an object")?;
        object.remove("system_settings_read_only");
        for field in [
            "tenant_id",
            "object_id",
            "client_id",
            "client_secret_file",
            "private_key_file",
        ] {
            object.remove(&format!("{field}_configured"));
            object.insert(field.into(), json!({"action":"keep"}));
        }
        Ok(json!({"expected_revision": state.saved_revision, "draft": draft}))
    }
}

#[test]
fn backend_configuration_service_validates_and_saves_without_activation() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    let mut command = fixture.command()?;
    command["draft"]["listen"] = json!("127.0.0.1:9443");
    command["draft"]["origin"] = json!("https://localhost:9443");
    let validated = fixture
        .source
        .validate(serde_json::from_value(command.clone())?)?;
    assert!(validated.errors.is_empty());
    assert!(validated.state.is_none());
    assert_eq!(std::fs::read(&fixture.path)?, before);
    assert!(
        !fixture
            .directory
            .path()
            .join(".configuration.json.lock")
            .exists()
    );
    let saved = fixture
        .source
        .save(serde_json::from_value(command.clone())?)?;
    assert!(saved.errors.is_empty());
    let state = saved.state.ok_or("save had no state")?;
    assert!(state.restart_required);
    assert!(!state.saved_requires_validation);
    assert_eq!(state.active.origin, "https://localhost:8443");
    assert_eq!(state.saved.origin, "https://localhost:9443");
    let json = String::from_utf8(serde_json::to_vec(&state)?)?;
    assert!(!json.contains("hidden-key"));
    let actual: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    assert_eq!(
        actual["private_key_file"],
        fixture
            .directory
            .path()
            .join("hidden-key.pem")
            .to_string_lossy()
            .as_ref()
    );
    assert!(
        fixture
            .source
            .save(serde_json::from_value(command)?)
            .is_err()
    );
    Ok(())
}

#[test]
fn backend_configuration_service_rejects_invalid_edits_and_retains_file() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    for (field, value) in [
        ("origin", json!("http://localhost:8443")),
        ("listen", json!("0.0.0.0:8443")),
        ("private_key_file", json!({"action":"clear"})),
        (
            "tenant_id",
            json!({"action":"replace","value":"must-not-leak"}),
        ),
        (
            "database",
            json!(fixture.directory.path().join("missing.db")),
        ),
        (
            "cache",
            json!({"root":fixture.directory.path().join("missing-cache"), "item_bytes":1, "total_bytes":512,"entries":1}),
        ),
    ] {
        let mut command = fixture.command()?;
        command["draft"][field] = value;
        let result = fixture.source.save(serde_json::from_value(command)?)?;
        assert!(!result.errors.is_empty(), "accepted {field}");
        assert!(!serde_json::to_string(&result)?.contains("must-not-leak"));
        assert_eq!(std::fs::read(&fixture.path)?, before);
    }
    assert!(!fixture.directory.path().join("missing.db").exists());
    assert!(!fixture.directory.path().join("missing-cache").exists());
    Ok(())
}

#[test]
fn backend_configuration_service_enforces_active_and_saved_read_only() -> TestResult {
    let fixture = Fixture::new(true)?;
    let command = fixture.command()?;
    assert!(
        fixture
            .source
            .save(serde_json::from_value(command)?)
            .is_err()
    );
    let fixture = Fixture::new(false)?;
    let command = fixture.command()?;
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    saved["system_settings_read_only"] = json!(true);
    std::fs::write(&fixture.path, serde_json::to_vec(&saved)?)?;
    let mut command = command;
    command["expected_revision"] = json!(fixture.source.read()?.saved_revision);
    assert!(
        fixture
            .source
            .save(serde_json::from_value(command)?)
            .is_err()
    );
    let mut command = fixture.command()?;
    command["draft"]["system_settings_read_only"] = json!(false);
    assert!(serde_json::from_value::<BackendConfigurationCommand>(command).is_err());
    Ok(())
}

#[test]
fn backend_configuration_service_keeps_blank_replacement_and_detects_external_changes() -> TestResult
{
    let fixture = Fixture::new(false)?;
    let mut command = fixture.command()?;
    command["draft"]["private_key_file"] = json!({"action":"replace","value":""});
    assert!(
        fixture
            .source
            .save(serde_json::from_value(command)?)?
            .errors
            .is_empty()
    );
    let mut bytes = std::fs::read(&fixture.path)?;
    bytes.push(b' ');
    std::fs::write(&fixture.path, &bytes)?;
    assert!(fixture.source.read()?.saved_requires_validation);
    std::fs::write(&fixture.path, b"{broken")?;
    assert!(fixture.source.read().is_err());
    std::fs::write(&fixture.path, vec![b' '; MAXIMUM as usize + 1])?;
    assert!(fixture.source.read().is_err());
    Ok(())
}

#[test]
fn backend_configuration_persistence_failures_preserve_previous_bytes_or_report_commit()
-> TestResult {
    for point in [
        SaveFailure::Write,
        SaveFailure::Sync,
        SaveFailure::Replace,
        SaveFailure::AfterReplace,
    ] {
        let fixture = Fixture::new(false)?;
        let before = std::fs::read(&fixture.path)?;
        let mut command = fixture.command()?;
        command["draft"]["services"]["artwork"] = json!([]);
        *fixture.source.failure.lock().map_err(|_| "failure lock")? = Some(point);
        let error = fixture
            .source
            .save(serde_json::from_value(command)?)
            .err()
            .ok_or("failure was ignored")?;
        let actual = std::fs::read(&fixture.path)?;
        if point == SaveFailure::AfterReplace {
            assert_ne!(actual, before);
            assert!(error.to_string().contains("was replaced"));
            assert!(fixture.source.read()?.saved_requires_validation);
        } else {
            assert_eq!(actual, before);
        }
        assert!(fixture.directory.path().read_dir()?.all(|entry| {
            entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().ends_with(".pending"))
        }));
    }
    Ok(())
}

#[test]
fn backend_configuration_optional_groups_and_secret_replacements_are_validated() -> TestResult {
    let fixture = Fixture::new(false)?;
    let cache_root = crate::infrastructure::permissions::private_test_directory()?;
    let staging_root = crate::infrastructure::permissions::private_test_directory()?;
    let browser = fixture.directory.path().join("browser");
    std::fs::create_dir(&browser)?;
    std::fs::create_dir(browser.join("assets"))?;
    std::fs::write(browser.join("index.html"), "<html></html>")?;
    let credentials = fixture.directory.path().join("hidden-credentials.json");
    std::fs::write(
        &credentials,
        br#"{"username":"fixture-user","password":"fixture-only"}"#,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut store = Store::open_read_current(&fixture.source.active.database)?;
    drop(store);
    store = Store::open_current_existing(&fixture.source.active.database)?;
    drop(crate::infrastructure::cache::FileCache::open(
        &mut store,
        cache_root.path(),
        Default::default(),
    )?);
    drop(crate::infrastructure::staging::StagingRoot::open(
        &mut store,
        staging_root.path(),
    )?);
    let mut command = fixture.command()?;
    command["draft"]["browser_assets"] = json!(browser);
    command["draft"]["cache"] = json!({
        "root":cache_root.path(),"item_bytes":9437184,"total_bytes":134217728,"entries":64
    });
    command["draft"]["local_acquisition"] = json!({
        "staging_root":staging_root.path(),
        "credentials_file":{"action":"replace","value":credentials},
        "server":"8.8.8.8:2242","acknowledge_plaintext_login":true
    });
    for (pointer, field) in [
        ("/draft/browser_assets", "browser_assets"),
        ("/draft/cache/root", "cache.root"),
        (
            "/draft/local_acquisition/staging_root",
            "local_acquisition.staging_root",
        ),
        (
            "/draft/local_acquisition/credentials_file/value",
            "local_acquisition.credentials_file",
        ),
    ] {
        let mut invalid = command.clone();
        *invalid
            .pointer_mut(pointer)
            .ok_or("missing optional field")? =
            json!(fixture.directory.path().join("unsafe:stream"));
        let result = fixture.source.validate(serde_json::from_value(invalid)?)?;
        assert_eq!(result.errors[0].field, field);
    }
    let result = fixture.source.save(serde_json::from_value(command)?)?;
    assert!(
        result.errors.is_empty(),
        "{}",
        serde_json::to_string(&result.errors)?
    );
    let rendered = serde_json::to_string(&result)?;
    assert!(!rendered.contains("hidden-credentials"));
    assert!(!rendered.contains("fixture-only"));
    assert!(
        result
            .state
            .ok_or("missing saved state")?
            .saved
            .local_acquisition
            .is_some()
    );
    let mut command = fixture.command()?;
    // Public summaries do not contain write-only update instructions.
    command["draft"]["local_acquisition"]["credentials_file"] = json!({"action":"keep"});
    command["draft"]["local_acquisition"]
        .as_object_mut()
        .ok_or("local object")?
        .remove("credentials_file_configured");
    assert!(
        fixture
            .source
            .validate(serde_json::from_value(command.clone())?)?
            .errors
            .is_empty()
    );
    command["draft"]["local_acquisition"] = Value::Null;
    command["draft"]["cache"] = Value::Null;
    command["draft"]["browser_assets"] = Value::Null;
    assert!(
        fixture
            .source
            .save(serde_json::from_value(command)?)?
            .errors
            .is_empty()
    );
    assert!(cache_root.path().join("owner.lock").exists());
    assert!(credentials.exists());
    Ok(())
}

#[test]
fn backend_configuration_production_references_are_write_only_and_can_be_cleared_explicitly()
-> TestResult {
    let fixture = Fixture::new(false)?;
    let secret = fixture.directory.path().join("hidden-secret.txt");
    std::fs::write(&secret, "fixture-only-secret")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut command = fixture.command()?;
    command["draft"]["environment"] = json!("production");
    for field in ["tenant_id", "object_id", "client_id"] {
        command["draft"][field] =
            json!({"action":"replace","value":"10000000-0000-4000-8000-000000000001"});
    }
    command["draft"]["client_secret_file"] = json!({"action":"replace","value":secret});
    let result = fixture.source.save(serde_json::from_value(command)?)?;
    assert!(result.errors.is_empty());
    let rendered = serde_json::to_string(&result)?;
    for hidden in [
        "hidden-secret",
        "fixture-only-secret",
        "10000000-0000-4000-8000-000000000001",
    ] {
        assert!(!rendered.contains(hidden));
    }
    let mut command = fixture.command()?;
    command["draft"]["environment"] = json!("development");
    for field in ["tenant_id", "object_id", "client_id", "client_secret_file"] {
        command["draft"][field] = json!({"action":"clear"});
    }
    assert!(
        fixture
            .source
            .save(serde_json::from_value(command)?)?
            .errors
            .is_empty()
    );
    Ok(())
}

#[test]
fn backend_configuration_validation_rejects_old_schema_without_migration() -> TestResult {
    let fixture = Fixture::new(false)?;
    let path = fixture.directory.path().join("old.db");
    let database = rusqlite::Connection::open(&path)?;
    database.pragma_update(None, "user_version", 1)?;
    let mut command = fixture.command()?;
    command["draft"]["database"] = json!(path);
    let result = fixture.source.validate(serde_json::from_value(command)?)?;
    assert_eq!(result.errors[0].field, "database");
    assert_eq!(
        database.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))?,
        1
    );
    let count: i64 = database.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(count, 0);
    Ok(())
}

#[test]
fn backend_configuration_process_writer() -> TestResult {
    let Some(path) = std::env::var_os("DOMUS_CONFIGURATION_TEST_FILE") else {
        return Ok(());
    };
    let (source, _, _) = ConfigurationService::load(Path::new(&path))?;
    if std::env::var_os("DOMUS_CONFIGURATION_TEST_CRASH").is_some() {
        *source.failure.lock().map_err(|_| "failure lock")? = Some(SaveFailure::CrashAfterSync);
    }
    let request = std::env::var("DOMUS_CONFIGURATION_TEST_COMMAND")?;
    match source.save(serde_json::from_str(&request)?) {
        Ok(result) if result.errors.is_empty() => println!("CONFIGURATION_SAVED"),
        Err(Error::Busy(_) | Error::Conflict(_)) => println!("CONFIGURATION_CONFLICT"),
        Ok(_) => return Err("child validation failed".into()),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[test]
fn backend_configuration_competing_processes_cannot_overwrite_the_same_revision() -> TestResult {
    let fixture = Fixture::new(false)?;
    let mut command = fixture.command()?;
    command["draft"]["services"]["artwork"] = json!([]);
    let spawn = || -> TestResult<std::process::Child> {
        Ok(std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "infrastructure::server::configuration::tests::backend_configuration_process_writer", "--nocapture"])
            .env("DOMUS_CONFIGURATION_TEST_FILE", &fixture.path)
            .env("DOMUS_CONFIGURATION_TEST_COMMAND", serde_json::to_string(&command)?)
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
            .spawn()?)
    };
    let first = spawn()?;
    let second = spawn()?;
    let results = [first.wait_with_output()?, second.wait_with_output()?];
    let mut saved = 0;
    for output in results {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if String::from_utf8_lossy(&output.stdout).contains("CONFIGURATION_SAVED") {
            saved += 1;
        }
    }
    assert_eq!(saved, 1);
    Ok(())
}

#[test]
fn backend_configuration_interrupted_writer_preserves_the_old_file_and_releases_its_lock()
-> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    let mut command = fixture.command()?;
    command["draft"]["services"]["artwork"] = json!([]);
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "infrastructure::server::configuration::tests::backend_configuration_process_writer",
            "--nocapture",
        ])
        .env("DOMUS_CONFIGURATION_TEST_FILE", &fixture.path)
        .env(
            "DOMUS_CONFIGURATION_TEST_COMMAND",
            serde_json::to_string(&command)?,
        )
        .env("DOMUS_CONFIGURATION_TEST_CRASH", "1")
        .output()?;
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(std::fs::read(&fixture.path)?, before);
    let pending = fixture
        .directory
        .path()
        .read_dir()?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".pending"))
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 1);
    let pending_bytes = std::fs::read(pending[0].path())?;
    // A pending file is never adopted or overwritten by the next attempt.
    assert!(
        fixture
            .source
            .save(serde_json::from_value(command.clone())?)
            .is_err()
    );
    assert_eq!(std::fs::read(pending[0].path())?, pending_bytes);
    command["draft"]["services"]["metadata"] = json!([]);
    assert!(
        fixture
            .source
            .save(serde_json::from_value(command)?)?
            .errors
            .is_empty()
    );
    assert_eq!(std::fs::read(pending[0].path())?, pending_bytes);
    Ok(())
}

#[cfg(windows)]
#[test]
fn backend_configuration_rejects_alternate_streams_and_directory_reparse_points() -> TestResult {
    let fixture = Fixture::new(false)?;
    let bytes = std::fs::read(&fixture.path)?;
    let stream = fixture.path.with_file_name("configuration.json:stream");
    std::fs::write(&stream, &bytes)?;
    assert!(ConfigurationService::load(&stream).is_err());
    let alias = fixture.directory.path().join("alias");
    let linked = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(fixture.directory.path())
        .output()?;
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    assert!(ConfigurationService::load(&alias.join("configuration.json")).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn backend_configuration_rejects_symlinks_and_parent_identity_changes() -> TestResult {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new(false)?;
    let alias = fixture.directory.path().join("alias");
    symlink(&fixture.path, &alias)?;
    assert!(ConfigurationService::load(&alias).is_err());
    let parent = crate::infrastructure::permissions::private_test_directory()?;
    let directory = parent.path().join("source");
    std::fs::create_dir(&directory)?;
    let path = directory.join("configuration.json");
    let bytes = std::fs::read(&fixture.path)?;
    std::fs::write(&path, &bytes)?;
    let (source, _, _) = ConfigurationService::load(&path)?;
    std::fs::rename(&directory, parent.path().join("moved"))?;
    std::fs::create_dir(&directory)?;
    std::fs::write(&path, &bytes)?;
    assert!(source.read().is_err());
    Ok(())
}

#[tokio::test]
async fn backend_configuration_routes_require_guards_and_reject_read_only_or_stale_writes()
-> TestResult {
    use crate::infrastructure::api::Api;
    use axum::{
        body::{Body, to_bytes},
        extract::ConnectInfo,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    async fn call(
        router: &axum::Router,
        route: &str,
        body: Value,
        guarded: bool,
    ) -> TestResult<axum::response::Response> {
        let mut request = Request::builder()
            .method("POST")
            .uri(route)
            .header("host", "localhost:8443")
            .header("content-type", "application/json");
        if guarded {
            request = request
                .header("origin", "https://localhost:8443")
                .header("x-domus-csrf", "1");
        }
        let mut request = request.body(Body::from(serde_json::to_vec(&body)?))?;
        request
            .extensions_mut()
            .insert(ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                12345,
            ))));
        Ok(router.clone().oneshot(request).await?)
    }
    for read_only in [true, false] {
        let fixture = Fixture::new(read_only)?;
        let mut command = fixture.command()?;
        command["draft"]["services"]["metadata"] = json!([]);
        let router = Api::development(
            "https://localhost:8443",
            Store::open_current_existing(&fixture.source.active.database)?,
        )?
        .with_system_settings_read_only(read_only)?
        .with_backend_configuration(fixture.source)?
        .router();
        for route in [
            "/api/system/configuration/save",
            "/api/system/configuration/validate",
        ] {
            assert_eq!(
                call(&router, route, command.clone(), false).await?.status(),
                StatusCode::FORBIDDEN
            );
        }
        let response = call(
            &router,
            "/api/system/configuration/validate",
            command.clone(),
            true,
        )
        .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let validated: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 32768).await?)?;
        assert_eq!(validated["errors"], json!([]));
        assert!(validated["state"].is_null());
        let before = std::fs::read(&fixture.path)?;
        let response = call(
            &router,
            "/api/system/configuration/save",
            command.clone(),
            true,
        )
        .await?;
        if read_only {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(std::fs::read(&fixture.path)?, before);
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), 32768).await?;
            assert!(!String::from_utf8_lossy(&bytes).contains("hidden-key"));
            let saved: Value = serde_json::from_slice(&bytes)?;
            assert_eq!(
                saved["state"]["active"]["services"]["metadata"],
                json!(["musicbrainz"])
            );
            assert_eq!(saved["state"]["saved"]["services"]["metadata"], json!([]));
            assert_eq!(saved["state"]["restart_required"], true);
            assert_eq!(
                call(
                    &router,
                    "/api/system/configuration/save",
                    command.clone(),
                    true
                )
                .await?
                .status(),
                StatusCode::CONFLICT
            );
            command["draft"]["system_settings_read_only"] = json!(false);
            let rejected = call(&router, "/api/system/configuration/save", command, true).await?;
            assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let error = to_bytes(rejected.into_body(), 1024).await?;
            assert_eq!(
                serde_json::from_slice::<Value>(&error)?["error"],
                "backend configuration request is invalid"
            );
        }
    }
    Ok(())
}
