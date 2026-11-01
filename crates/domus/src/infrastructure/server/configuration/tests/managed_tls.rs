use super::*;

fn managed_fixture() -> TestResult<Fixture> {
    let mut fixture = Fixture::new(false)?;
    let root = fixture.directory.path();
    for name in ["entra.secret", "dns.secret"] {
        std::fs::write(root.join(name), "fixture-private-secret")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let generated = rcgen::generate_simple_self_signed(vec!["api.home.example".into()])?;
    std::fs::write(root.join("certificate.pem"), generated.cert.pem())?;
    std::fs::write(
        root.join("hidden-key.pem"),
        generated.signing_key.serialize_pem(),
    )?;
    let mut configuration: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    configuration["environment"] = json!("production");
    configuration["origin"] = json!("https://api.home.example:8443");
    for name in ["tenant_id", "object_id", "client_id"] {
        configuration[name] = json!("11111111-1111-1111-1111-111111111111");
    }
    configuration["client_secret_file"] = json!(root.join("entra.secret"));
    configuration["tls_mode"] = json!("azure_dns");
    configuration["managed_tls"] = json!({
        "environment": "staging",
        "state_directory": root.join("certificates"),
        "terms_of_service_agreed": true,
        "dns": {
            "tenant_id": "22222222-2222-2222-2222-222222222222",
            "client_id": "33333333-3333-3333-3333-333333333333",
            "client_secret_file": root.join("dns.secret"),
            "subscription_id": "44444444-4444-4444-4444-444444444444",
            "resource_group": "fixture-dns",
            "zone": "home.example"
        }
    });
    std::fs::write(&fixture.path, serde_json::to_vec(&configuration)?)?;
    fixture.source = ConfigurationService::load(&fixture.path)?.0;
    Ok(fixture)
}

#[test]
fn backend_configuration_certificate_status_uses_active_not_saved_revision() -> TestResult {
    let fixture = managed_fixture()?;
    let active_revision = fixture.source.read()?.active_revision;
    let command = section_command(
        &fixture.source,
        json!([{
            "section":"tls", "tls_mode":"manual", "managed_tls":null,
            "certificate_file":fixture.directory.path().join("certificate.pem"),
            "private_key_file":{"action":"keep"}
        }]),
    )?;
    assert!(fixture.source.save_sections(command)?.errors.is_empty());
    let visible = fixture.source.certificate_status()?;
    assert_eq!(visible.configuration_revision, active_revision);
    assert_ne!(
        visible.configuration_revision,
        fixture.source.read()?.saved_revision
    );
    assert_eq!(
        visible.certificate.ok_or("status is missing")?.phase,
        crate::domain::certificates::CertificatePhase::Uninitialized
    );
    assert!(!fixture.directory.path().join("certificates").exists());
    let reloaded = ConfigurationService::load(&fixture.path)?.0;
    assert!(reloaded.certificate_status()?.certificate.is_none());
    Ok(())
}

#[test]
fn backend_configuration_certificate_status_rejects_an_unconfirmed_listener_selection() -> TestResult
{
    let fixture = managed_fixture()?;
    let managed = fixture
        .source
        .active
        .managed_tls
        .as_ref()
        .ok_or("managed settings are missing")?;
    let binding = managed.binding("api.home.example")?;
    let mut state = crate::infrastructure::certificates::state::StateStore::initialize(
        &managed.state_directory,
        &binding,
        AcmeEnvironment::Staging,
    )?;
    for _ in 0..2 {
        let generated = rcgen::generate_simple_self_signed(vec!["api.home.example".into()])?;
        let certificate = generated.cert.pem();
        let (not_before, not_after) = crate::infrastructure::tls::certificate_validity(
            "api.home.example",
            certificate.as_bytes(),
        )?;
        state.publish_generation(&crate::infrastructure::certificates::state::Generation {
            hostname: "api.home.example".into(),
            environment: AcmeEnvironment::Staging,
            certificate,
            private_key: generated.signing_key.serialize_pem(),
            not_before,
            not_after,
            renew_at: crate::domain::certificates::renewal_time(not_before, not_after, None, 0)?,
            initial_renewal: None,
        })?;
    }
    let original_selection = state.selection(1)?;
    drop(state);
    let path = managed.state_directory.join("lifecycle.json");
    let mut journal: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    journal["active_generation"] = json!(2);
    std::fs::write(&path, serde_json::to_vec(&journal)?)?;
    let before = std::fs::read(&path)?;
    *fixture
        .source
        .certificate_selection()
        .lock()
        .map_err(|_| "fixture selection lock failed")? = Some(original_selection);
    assert_eq!(
        crate::infrastructure::certificates::state::StateStore::status(
            &managed.state_directory,
            &binding,
            AcmeEnvironment::Staging,
        )?
        .active_generation,
        Some(2)
    );
    assert!(matches!(
        fixture.source.certificate_status(),
        Err(Error::Conflict(_))
    ));
    assert_eq!(std::fs::read(&path)?, before);
    journal["active_generation"] = json!(1);
    std::fs::write(&path, serde_json::to_vec(&journal)?)?;
    assert_eq!(
        fixture
            .source
            .certificate_status()?
            .certificate
            .ok_or("status is missing")?
            .active_generation,
        Some(1)
    );
    let first_path = managed.state_directory.join("generation-1.json");
    let first_bytes = std::fs::read(&first_path)?;
    let second_bytes = std::fs::read(managed.state_directory.join("generation-2.json"))?;
    std::fs::write(&first_path, &second_bytes)?;
    use sha2::Digest as _;
    journal["generations"]["1"] = json!(format!("{:x}", sha2::Sha256::digest(&second_bytes)));
    std::fs::write(&path, serde_json::to_vec(&journal)?)?;
    assert!(matches!(
        fixture.source.certificate_status(),
        Err(Error::Conflict(_))
    ));
    std::fs::write(&first_path, &first_bytes)?;
    let retained: crate::infrastructure::certificates::state::Generation =
        serde_json::from_slice(&first_bytes)?;
    std::fs::rename(
        &managed.state_directory,
        fixture.directory.path().join("retained-root"),
    )?;
    let mut replacement = crate::infrastructure::certificates::state::StateStore::initialize(
        &managed.state_directory,
        &binding,
        AcmeEnvironment::Staging,
    )?;
    replacement.publish_generation(&retained)?;
    drop(replacement);
    let mut replaced: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    replaced["active_generation"] = json!(1);
    std::fs::write(&path, serde_json::to_vec(&replaced)?)?;
    assert_eq!(
        crate::infrastructure::certificates::state::StateStore::status(
            &managed.state_directory,
            &binding,
            AcmeEnvironment::Staging,
        )?
        .active_generation,
        Some(1)
    );
    assert!(matches!(
        fixture.source.certificate_status(),
        Err(Error::Conflict(_))
    ));
    Ok(())
}

#[test]
fn backend_configuration_anonymous_certificate_status_does_not_read_inactive_references()
-> TestResult {
    let fixture = managed_fixture()?;
    let mut configuration: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    configuration["environment"] = json!("single_user");
    configuration["origin"] = json!("http://127.0.0.1:8443");
    std::fs::write(&fixture.path, serde_json::to_vec(&configuration)?)?;
    std::fs::remove_file(fixture.directory.path().join("dns.secret"))?;
    std::fs::remove_file(fixture.directory.path().join("entra.secret"))?;
    let source = ConfigurationService::load(&fixture.path)?.0;
    assert!(source.certificate_status()?.certificate.is_none());
    assert!(!fixture.directory.path().join("certificates").exists());
    Ok(())
}

#[test]
fn backend_configuration_certificate_status_reports_runtime_recovery_without_a_journal_write()
-> TestResult {
    let fixture = managed_fixture()?;
    fixture
        .source
        .certificate_recovery()
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(
        fixture
            .source
            .certificate_status()?
            .certificate
            .ok_or("status is missing")?
            .recovery_required
    );
    assert!(!fixture.directory.path().join("certificates").exists());
    Ok(())
}

#[tokio::test]
async fn backend_configuration_certificate_status_reports_owner_restart_failure() -> TestResult {
    let fixture = managed_fixture()?;
    let configuration = fixture.source.active.clone();
    let managed = configuration
        .managed_tls
        .as_ref()
        .ok_or("managed settings are missing")?;
    let hostname = "api.home.example";
    let path = managed.state_directory.join("lifecycle.json");
    let state = crate::infrastructure::certificates::state::StateStore::initialize(
        &managed.state_directory,
        &managed.binding(hostname)?,
        AcmeEnvironment::Staging,
    )?;
    let before = std::fs::read(&path)?;
    let host = super::super::super::Host {
        api: crate::infrastructure::api::Api::development(
            "https://localhost:8443",
            Store::open_current_existing(&configuration.database)?,
        )?,
        tls: Some(crate::infrastructure::tls::configuration(
            hostname,
            &std::fs::read(fixture.directory.path().join("certificate.pem"))?,
            &std::fs::read(fixture.directory.path().join("hidden-key.pem"))?,
        )?),
        configuration,
        hostname: hostname.into(),
        source: std::sync::Arc::new(fixture.source),
        certificate_guard: None,
    };
    assert!(super::super::super::renewal_owner(&host).await.is_err());
    assert!(
        host.source
            .certificate_status()?
            .certificate
            .ok_or("status is missing")?
            .recovery_required,
        "a failed owner restart must not leave healthy runtime status"
    );
    assert_eq!(std::fs::read(&path)?, before);
    drop(state);
    Ok(())
}

#[tokio::test]
async fn backend_configuration_certificate_status_route_is_passive_redacted_and_read_only()
-> TestResult {
    use axum::{
        body::{Body, to_bytes},
        extract::ConnectInfo,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let fixture = managed_fixture()?;
    let revision = fixture.source.read()?.active_revision;
    let path = fixture.directory.path().join("certificates");
    let router = crate::infrastructure::api::Api::development(
        "https://localhost:8443",
        Store::open_current_existing(&fixture.source.active.database)?,
    )?
    .with_system_settings_read_only(true)?
    .with_backend_configuration(fixture.source)?
    .router();
    let mut request = Request::builder()
        .uri("/api/system/certificates/status")
        .header("host", "localhost:8443")
        .body(Body::empty())?;
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let response = router.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 32768).await?;
    let status: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(status["configuration_revision"], revision);
    assert_eq!(status["certificate"]["phase"], "uninitialized");
    assert!(status["certificate"].get("account").is_none());
    assert!(status["certificate"].get("certificate_key").is_none());
    assert!(!String::from_utf8(bytes.to_vec())?.contains("fixture-private-secret"));
    assert!(!path.exists());
    Ok(())
}

#[test]
fn backend_configuration_managed_setup_is_passive_and_secret_references_stay_hidden() -> TestResult
{
    let fixture = managed_fixture()?;
    let directory = fixture.directory.path().join("certificates");
    let command = section_command(
        &fixture.source,
        json!([{
            "section": "music_providers", "services": crate::application::services::ServiceConfiguration::default()
        }]),
    )?;
    assert!(fixture.source.validate_sections(command)?.errors.is_empty());
    assert!(!directory.exists());
    let visible = serde_json::to_string(&fixture.source.read()?)?;
    assert!(!visible.contains("fixture-private-secret"));
    assert!(!visible.contains("dns.secret"));
    assert!(!visible.contains("33333333-3333-3333-3333-333333333333"));
    assert!(visible.contains("\"client_secret_file_configured\":true"));
    assert!(super::super::super::PreparedServer::load(&fixture.path).is_err());
    assert!(!directory.exists());
    Ok(())
}

#[test]
fn backend_configuration_mode_changes_retain_inactive_managed_settings() -> TestResult {
    let fixture = managed_fixture()?;
    let previous: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    let command = section_command(
        &fixture.source,
        json!([{
            "section": "tls", "tls_mode": "manual", "managed_tls": null,
            "certificate_file": fixture.directory.path().join("certificate.pem"),
            "private_key_file": { "action": "keep" }
        }]),
    )?;
    let result = fixture.source.save_sections(command)?;
    assert!(result.errors.is_empty());
    let saved: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    assert_eq!(saved["managed_tls"], previous["managed_tls"]);
    assert_eq!(saved["tls_mode"], "manual");
    assert_eq!(saved["client_secret_file"], previous["client_secret_file"]);
    assert_eq!(
        fixture.source.read()?.changed_sections,
        vec![ConfigurationSection::Tls]
    );
    assert!(!fixture.directory.path().join("certificates").exists());
    Ok(())
}

#[test]
fn backend_configuration_certificate_root_separation_keeps_siblings_and_offline_roots() -> TestResult
{
    let fixture = managed_fixture()?;
    let root = fixture.directory.path();
    let store = Store::open_current_existing(&root.join("state.db"))?;
    let media = root.join("media");
    std::fs::create_dir(&media)?;
    crate::application::add_location(&store, &media, "main")?;
    store.inspect_certificate_root(&root.join("certificates"))?;
    assert!(
        store
            .inspect_certificate_root(&media.join("certificates"))
            .is_err()
    );
    std::fs::remove_dir(&media)?;
    store.inspect_certificate_root(&root.join("certificates"))?;
    assert!(!root.join("certificates").exists());
    Ok(())
}

#[test]
fn backend_configuration_bound_certificate_state_excludes_new_media_roots() -> TestResult {
    let fixture = managed_fixture()?;
    let root = fixture.directory.path();
    let store = Store::open_current_existing(&root.join("state.db"))?;
    let state = crate::infrastructure::certificates::state::StateStore::initialize(
        &root.join("certificates"),
        &"a".repeat(64),
        AcmeEnvironment::Staging,
    )?;
    state.bind_database_root(&store)?;
    let media = root.join("certificates").join("media");
    std::fs::create_dir(&media)?;
    assert!(crate::application::add_location(&store, &media, "main").is_err());
    store.inspect_certificate_root(&root.join("certificates"))?;
    Ok(())
}
