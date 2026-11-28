use super::*;
use serde_json::{Value, json};

mod artist_images;
mod folder_browser;
mod managed_tls;
mod primary_library;
mod sharing;
mod soulseek_account;
mod tls_crash;
mod worker;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    source: ConfigurationService,
    path: PathBuf,
    directory: tempfile::TempDir,
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
                "storage": StorageConfiguration::inspect_initial(root, &crate::domain::LibraryPath::parse("")?)?,
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

    fn select_root(
        &mut self,
        root_id: &str,
        role: crate::infrastructure::filesystem::StorageRole,
        root: &Path,
    ) -> TestResult {
        let storage = crate::infrastructure::filesystem::fixture_storage(
            self.directory.path(),
            &crate::domain::LibraryPath::parse("")?,
            &[(root_id, role, root)],
        )?;
        let mut document: Value = serde_json::from_slice(&std::fs::read(&self.path)?)?;
        document["storage"] = serde_json::to_value(storage)?;
        std::fs::write(&self.path, serde_json::to_vec(&document)?)?;
        self.source = ConfigurationService::load(&self.path)?.0;
        Ok(())
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

fn section_command(
    source: &ConfigurationService,
    sections: Value,
) -> TestResult<ConfigurationSectionCommand> {
    Ok(serde_json::from_value(
        json!({"expected_revision": source.read()?.saved_revision, "sections": sections}),
    )?)
}

#[test]
fn backend_configuration_relative_paths_use_the_configuration_directory() -> TestResult {
    let fixture = Fixture::new(false)?;
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["database"] = json!("state.db");
    document["certificate_file"] = json!("certificate.pem");
    document["private_key_file"] = json!("hidden-key.pem");
    let bytes = serde_json::to_vec(&document)?;
    std::fs::write(&fixture.path, &bytes)?;
    let (source, active, _) = ConfigurationService::load(&fixture.path)?;
    assert_eq!(active.database, fixture.directory.path().join("state.db"));
    assert_eq!(
        active.certificate_file,
        Some(fixture.directory.path().join("certificate.pem"))
    );
    assert_eq!(
        active.private_key_file,
        Some(fixture.directory.path().join("hidden-key.pem"))
    );
    let state = source.read()?;
    assert_eq!(state.saved.database, "state.db");
    assert!(!state.restart_required);
    assert!(state.changed_sections.is_empty());
    assert!(
        source
            .validate_sections(section_command(
                &source,
                json!([{"section":"storage", "database":"state.db", "cache":null}]),
            )?)?
            .errors
            .is_empty()
    );
    assert_eq!(std::fs::read(&fixture.path)?, bytes);
    Ok(())
}

#[test]
fn backend_configuration_optional_role_defaults_use_the_configuration_directory() -> TestResult {
    let fixture = Fixture::new(false)?;
    let root = fixture.directory.path();
    for worker in [false, true] {
        let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
        let acquisition = json!({
            "credentials_file":"secrets/soulseek.json",
            "server":"208.76.170.51:2242",
            "acknowledge_plaintext_login":false
        });
        document["cache"] = json!({});
        document["local_acquisition"] = if worker {
            Value::Null
        } else {
            acquisition.clone()
        };
        document["production_worker"] = if worker {
            json!({"queue_capacity":64,"acquisition":acquisition})
        } else {
            Value::Null
        };
        document["managed_tls"] = json!({
            "environment":"staging", "terms_of_service_agreed":false,
            "dns":{
                "tenant_id":"generated-tenant", "client_id":"generated-client",
                "client_secret_file":"secrets/azure.txt", "subscription_id":"generated-subscription",
                "resource_group":"generated-group", "zone":"generated.invalid"
            }
        });
        let bytes = serde_json::to_vec(&document)?;
        std::fs::write(&fixture.path, &bytes)?;
        let (_, active, _) = ConfigurationService::load(&fixture.path)?;
        assert_eq!(
            active.cache.ok_or("cache missing")?.root,
            root.join("cache")
        );
        let acquisition = if worker {
            active
                .production_worker
                .ok_or("worker missing")?
                .acquisition
        } else {
            active.local_acquisition
        }
        .ok_or("acquisition missing")?;
        assert_eq!(acquisition.staging_root, root.join("staging"));
        assert_eq!(
            acquisition.credentials_file,
            root.join("secrets").join("soulseek.json")
        );
        let managed = active.managed_tls.ok_or("managed TLS missing")?;
        assert_eq!(managed.state_directory, root.join("certificates"));
        assert_eq!(
            managed.dns.client_secret_file,
            root.join("secrets").join("azure.txt")
        );
        assert_eq!(std::fs::read(&fixture.path)?, bytes);
    }
    for name in ["cache", "staging", "certificates", "secrets"] {
        assert!(!root.join(name).exists());
    }
    Ok(())
}

#[test]
fn backend_configuration_absolute_overrides_require_an_explicit_new_application_binding()
-> TestResult {
    use crate::infrastructure::filesystem::{RootRevision, TopologyAssurance};
    let fixture = Fixture::new(false)?;
    let other = crate::infrastructure::permissions::private_test_directory()?;
    let path = other.path().join("config.json");
    let mut bytes = std::fs::read(&fixture.path)?;
    std::fs::write(&path, &bytes)?;
    assert!(ConfigurationService::load(&path).is_err());
    let mut document: Value = serde_json::from_slice(&bytes)?;
    let mut storage = StorageConfiguration::inspect_initial(
        other.path(),
        &crate::domain::LibraryPath::parse("")?,
    )?;
    storage
        .providers
        .push(crate::infrastructure::filesystem::ProviderMapping {
            scope: "state".into(),
            boundary: fixture.directory.path().to_owned(),
        });
    let state = storage
        .roots
        .iter_mut()
        .find(|root| root.role == crate::infrastructure::filesystem::StorageRole::State)
        .ok_or("state root missing")?;
    state.provider_scope = "state".into();
    state.expected = crate::infrastructure::filesystem::DirectoryExpectation::inspect(
        fixture.directory.path(),
        &crate::domain::LibraryPath::parse("")?,
    )?
    .0;
    document["storage"] = serde_json::to_value(&storage)?;
    std::fs::write(&path, serde_json::to_vec(&document)?)?;
    assert!(matches!(
        ConfigurationService::load(&path),
        Err(Error::Conflict(message)) if message.contains("topology assurance")
    ));
    storage.topology_assurances.push(TopologyAssurance {
        left: RootRevision {
            root_id: "application".into(),
            binding_revision: 1,
        },
        right: RootRevision {
            root_id: "state".into(),
            binding_revision: 1,
        },
        selection_digest: storage.selection_digest()?,
    });
    document["storage"] = serde_json::to_value(storage)?;
    bytes = serde_json::to_vec(&document)?;
    std::fs::write(&path, &bytes)?;
    let (_, active, _) = ConfigurationService::load(&path)?;
    assert_eq!(active.database, fixture.directory.path().join("state.db"));
    assert_eq!(
        active.private_key_file,
        Some(fixture.directory.path().join("hidden-key.pem"))
    );
    assert_eq!(std::fs::read(&path)?, bytes);
    Ok(())
}

#[test]
fn backend_configuration_missing_storage_refuses_startup_without_state_effects() -> TestResult {
    let fixture = Fixture::new(true)?;
    let root = fixture.directory.path();
    let before = std::fs::read(root.join("state.db"))?;
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document
        .as_object_mut()
        .ok_or("configuration object")?
        .remove("storage");
    let bytes = serde_json::to_vec(&document)?;
    std::fs::write(&fixture.path, &bytes)?;
    let names = || -> std::io::Result<std::collections::BTreeSet<std::ffi::OsString>> {
        std::fs::read_dir(root)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    };
    let entries = names()?;
    assert!(matches!(
        ConfigurationService::load(&fixture.path),
        Err(Error::Conflict(message)) if message.contains("storage selection is missing")
    ));
    assert!(crate::infrastructure::server::PreparedServer::load(&fixture.path).is_err());
    assert_eq!(names()?, entries);
    assert_eq!(std::fs::read(root.join("state.db"))?, before);
    assert_eq!(std::fs::read(&fixture.path)?, bytes);
    Ok(())
}

#[test]
fn backend_configuration_empty_paths_do_not_select_the_configuration_directory() -> TestResult {
    let fixture = Fixture::new(false)?;
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["database"] = json!("");
    let bytes = serde_json::to_vec(&document)?;
    std::fs::write(&fixture.path, &bytes)?;
    assert!(ConfigurationService::load(&fixture.path).is_err());
    assert_eq!(std::fs::read(&fixture.path)?, bytes);
    Ok(())
}

#[cfg(windows)]
#[test]
fn backend_configuration_rooted_paths_without_a_drive_remain_invalid() -> TestResult {
    let fixture = Fixture::new(false)?;
    let database = fixture.directory.path().join("state.db");
    let prefix = database
        .components()
        .next()
        .ok_or("missing generated drive prefix")?;
    let rooted = database.strip_prefix(prefix.as_os_str())?;
    assert!(rooted.has_root());
    assert!(!rooted.is_absolute());
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["database"] = json!(rooted);
    let bytes = serde_json::to_vec(&document)?;
    std::fs::write(&fixture.path, &bytes)?;
    assert!(ConfigurationService::load(&fixture.path).is_err());
    assert_eq!(std::fs::read(&fixture.path)?, bytes);
    Ok(())
}

#[test]
fn backend_configuration_inspection_keeps_active_and_saved_state_separate() -> TestResult {
    let fixture = Fixture::new(false)?;
    let original = fixture.source.read()?;
    let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    document["origin"] = json!("https://localhost:9443");
    document["listen"] = json!("127.0.0.1:9443");
    let bytes = serde_json::to_vec(&document)?;
    std::fs::write(&fixture.path, &bytes)?;
    let state = fixture.source.read()?;
    assert_eq!(state.active_revision, original.active_revision);
    assert_ne!(state.saved_revision, original.saved_revision);
    assert_eq!(state.active.origin, original.active.origin);
    assert_eq!(state.saved.origin, "https://localhost:9443");
    assert!(state.restart_required && state.saved_requires_validation);
    assert_eq!(state.changed_sections, vec![ConfigurationSection::General]);
    let validated = fixture.source.validate_sections(section_command(
        &fixture.source,
        json!([{"section":"music_providers", "services": ServiceConfiguration::default()}]),
    )?)?;
    assert!(validated.errors.is_empty() && validated.state.is_none());
    assert!(fixture.source.read()?.saved_requires_validation);
    assert_eq!(std::fs::read(&fixture.path)?, bytes);
    assert!(!serde_json::to_string(&state)?.contains("hidden-key"));
    Ok(())
}

#[test]
fn backend_configuration_validation_is_passive_revision_bound_and_redacted() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    assert!(fixture.source.validate_saved()?.errors.is_empty());
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
            json!({"root":fixture.directory.path().join("missing-cache"),"item_bytes":1,"total_bytes":512,"entries":1}),
        ),
    ] {
        let mut command = fixture.command()?;
        command["draft"][field] = value;
        let result = fixture.source.validate(serde_json::from_value(command)?)?;
        assert!(!result.errors.is_empty(), "accepted {field}");
        assert!(!serde_json::to_string(&result)?.contains("must-not-leak"));
        assert!(result.state.is_none());
        assert_eq!(std::fs::read(&fixture.path)?, before);
    }
    let command = fixture.command()?;
    let mut changed = before.clone();
    changed.push(b' ');
    std::fs::write(&fixture.path, &changed)?;
    assert!(
        fixture
            .source
            .validate(serde_json::from_value(command)?)
            .is_err()
    );
    for name in [".configuration.json.lock", "missing.db", "missing-cache"] {
        assert!(!fixture.directory.path().join(name).exists());
    }
    std::fs::write(&fixture.path, b"{broken")?;
    assert!(fixture.source.read().is_err());
    std::fs::write(&fixture.path, vec![b' '; MAXIMUM as usize + 1])?;
    assert!(fixture.source.read().is_err());
    assert!(fixture.source.validate_saved().is_err());
    Ok(())
}

#[test]
fn backend_configuration_section_validation_rejects_invalid_batches_without_writes() -> TestResult {
    let fixture = Fixture::new(false)?;
    let before = std::fs::read(&fixture.path)?;
    let providers =
        json!({"section":"music_providers", "services":ServiceConfiguration::default()});
    for sections in [
        json!([]),
        json!([providers.clone(), providers]),
        json!([{"section":"tls","certificate_file":null,"private_key_file":{"action":"clear"}}]),
    ] {
        assert!(
            !fixture
                .source
                .validate_sections(section_command(&fixture.source, sections)?)?
                .errors
                .is_empty()
        );
        assert_eq!(std::fs::read(&fixture.path)?, before);
    }
    assert!(section_command(&fixture.source, json!([{"section":"movies"}])).is_err());
    let mut command = fixture.command()?;
    command["draft"]["system_settings_read_only"] = json!(false);
    assert!(serde_json::from_value::<BackendConfigurationCommand>(command).is_err());
    Ok(())
}

#[test]
fn backend_configuration_sqlite_policy_keeps_active_and_saved_read_only_guards() -> TestResult {
    for (active_read_only, saved_read_only) in [(true, false), (false, true), (false, false)] {
        let fixture = Fixture::new(active_read_only)?;
        let mut document: Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
        document["system_settings_read_only"] = json!(saved_read_only);
        let bytes = serde_json::to_vec(&document)?;
        std::fs::write(&fixture.path, &bytes)?;
        assert_eq!(
            fixture.source.require_editable().is_ok(),
            !active_read_only && !saved_read_only
        );
        assert!(
            fixture
                .source
                .validate(serde_json::from_value(fixture.command()?)?)?
                .errors
                .is_empty()
        );
        assert_eq!(std::fs::read(&fixture.path)?, bytes);
    }
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
    let fixture = Fixture::new(false)?;
    let alias = fixture.directory.path().join("alias");
    std::os::unix::fs::symlink(&fixture.path, &alias)?;
    assert!(ConfigurationService::load(&alias).is_err());
    let parent = crate::infrastructure::permissions::private_test_directory()?;
    let directory = parent.path().join("source");
    std::fs::create_dir(&directory)?;
    crate::infrastructure::private_fixture::directory(&directory)?;
    drop(Store::open(&directory.join("state.db"))?);
    let path = directory.join("configuration.json");
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&fixture.path)?)?;
    value["database"] = serde_json::json!(directory.join("state.db"));
    value["storage"] = serde_json::to_value(StorageConfiguration::inspect_initial(
        &directory,
        &crate::domain::LibraryPath::parse("")?,
    )?)?;
    let bytes = serde_json::to_vec(&value)?;
    std::fs::write(&path, &bytes)?;
    let (source, _, _) = ConfigurationService::load(&path)?;
    std::fs::rename(&directory, parent.path().join("moved"))?;
    std::fs::create_dir(&directory)?;
    std::fs::write(&path, &bytes)?;
    assert!(source.read().is_err());
    Ok(())
}

#[tokio::test]
async fn backend_configuration_routes_refuse_host_writes_and_reload_under_both_policies()
-> TestResult {
    use crate::infrastructure::api::Api;
    use axum::{
        body::{Body, to_bytes},
        extract::ConnectInfo,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    for read_only in [true, false] {
        let fixture = Fixture::new(read_only)?;
        let command = fixture.command()?;
        let before = std::fs::read(&fixture.path)?;
        let source = Arc::new(fixture.source);
        let router = Api::development(
            "https://localhost:8443",
            Store::open_current_existing(&source.active.database)?,
        )?
        .with_system_settings_read_only(read_only)?
        .with_shared_configuration(source.clone())?
        .router();
        for route in ["save", "sections/save", "reload", "validate"] {
            for guarded in [false, true] {
                let mut request = Request::builder()
                    .method("POST")
                    .uri(format!("/api/system/configuration/{route}"))
                    .header("host", "localhost:8443")
                    .header("content-type", "application/json");
                if guarded {
                    request = request
                        .header("origin", "https://localhost:8443")
                        .header("x-domus-csrf", "1");
                }
                let mut request = request.body(Body::from(serde_json::to_vec(&command)?))?;
                request
                    .extensions_mut()
                    .insert(ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        12345,
                    ))));
                let response = router.clone().oneshot(request).await?;
                assert_eq!(
                    response.status(),
                    if !guarded {
                        StatusCode::FORBIDDEN
                    } else if route == "validate" {
                        StatusCode::OK
                    } else {
                        StatusCode::GONE
                    }
                );
                let body: Value =
                    serde_json::from_slice(&to_bytes(response.into_body(), 32768).await?)?;
                if guarded && route == "validate" {
                    assert_eq!(body["errors"], json!([]));
                    assert!(body["state"].is_null());
                } else if guarded {
                    assert!(
                        body["error"]
                            .as_str()
                            .ok_or("missing refusal")?
                            .contains("operator-managed")
                    );
                }
                assert_eq!(std::fs::read(&fixture.path)?, before);
                assert_eq!(
                    source.read()?.active_revision,
                    source.read()?.saved_revision
                );
            }
        }
        assert!(
            !fixture
                .directory
                .path()
                .join(".configuration.json.lock")
                .exists()
        );
        assert!(fixture.directory.path().read_dir()?.all(|entry| {
            entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().ends_with(".pending"))
        }));
    }
    Ok(())
}
