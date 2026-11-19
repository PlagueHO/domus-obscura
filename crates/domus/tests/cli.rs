use std::process::Command;
#[path = "support/private.rs"]
mod private;
use private::tempdir;

#[path = "cli/cache.rs"]
mod cache_tests;
#[path = "cli/sources.rs"]
mod source_tests;
#[path = "support/storage.rs"]
mod storage_fixture;
mod support;
use storage_fixture::select_host_roots;

fn run(args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_domus"))
        .args(args)
        .output()
}

fn selected_host(
    parent: &std::path::Path,
    mut value: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    value["environment"] = serde_json::json!("single_user");
    value["listen"] = serde_json::json!("127.0.0.1:8080");
    value["origin"] = serde_json::json!("http://127.0.0.1:8080");
    value["system_settings_read_only"] = serde_json::json!(false);
    value["storage"] = serde_json::to_value(
        domus::infrastructure::filesystem::StorageConfiguration::inspect_initial(
            parent,
            &domus::domain::LibraryPath::parse("")?,
        )?,
    )?;
    Ok(value)
}

fn write_host(
    parent: &std::path::Path,
    database: &std::path::Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let path = parent.join("host.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&selected_host(
            parent,
            serde_json::json!({"database":database}),
        )?)?,
    )?;
    Ok(path)
}

fn initialized_storage(
    parent: &std::path::Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf), Box<dyn std::error::Error>> {
    let root = parent.join("application");
    let output = run(&[
        "init",
        "--storage-root",
        root.to_str().ok_or("storage path")?,
    ])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let host = root.join("config.json");
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&host)?)?;
    value["system_settings_read_only"] = serde_json::json!(false);
    std::fs::write(&host, serde_json::to_vec(&value)?)?;
    Ok((root.join("state").join("state.db"), host))
}

#[cfg(windows)]
#[test]
fn local_setup_script_selects_generated_media_and_staging_before_registration()
-> Result<(), Box<dyn std::error::Error>> {
    use domus::infrastructure::filesystem::{StorageConfiguration, StorageRole};

    let fixture = tempdir()?;
    let repository = fixture.path().join("repository");
    let scripts = repository.join("scripts");
    let binaries = repository.join("target").join("debug");
    let assets = repository.join("frontend").join("dist-app");
    for path in [&scripts, &binaries, &assets] {
        std::fs::create_dir_all(path)?;
    }
    let script = scripts.join("local-setup.ps1");
    std::fs::write(&script, include_str!("../../../scripts/local-setup.ps1"))?;
    std::fs::copy(env!("CARGO_BIN_EXE_domus"), binaries.join("domus.exe"))?;
    std::fs::write(assets.join("index.html"), b"generated setup test assets")?;
    let root = fixture.path().join("fresh setup");
    let setup = || {
        Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .arg("-Directory")
            .arg(&root)
            .output()
    };
    let output = setup()?;
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let application = root.join("application");
    let host = application.join("config.json");
    let configuration = std::fs::read(&host)?;
    let value: serde_json::Value = serde_json::from_slice(&configuration)?;
    let storage: StorageConfiguration = serde_json::from_value(value["storage"].clone())?;
    let media = storage
        .roots
        .iter()
        .find(|root| root.role == StorageRole::Media)
        .ok_or("media root was not selected")?;
    let media_provider = storage
        .providers
        .iter()
        .find(|provider| provider.scope == media.provider_scope)
        .ok_or("media provider was not selected")?;
    assert_eq!(
        std::fs::canonicalize(&media_provider.boundary)?,
        std::fs::canonicalize(root.join("library"))?
    );
    assert!(
        storage
            .roots
            .iter()
            .any(|root| root.role == StorageRole::Staging)
    );
    let digest = storage.selection_digest()?;
    for private in storage
        .roots
        .iter()
        .filter(|root| root.role != StorageRole::Media)
    {
        assert!(storage.topology_assurances.iter().any(|assurance| {
            assurance.selection_digest == digest
                && ((assurance.left.root_id == private.root_id
                    && assurance.left.binding_revision == private.binding_revision
                    && assurance.right.root_id == media.root_id
                    && assurance.right.binding_revision == media.binding_revision)
                    || (assurance.right.root_id == private.root_id
                        && assurance.right.binding_revision == private.binding_revision
                        && assurance.left.root_id == media.root_id
                        && assurance.left.binding_revision == media.binding_revision))
        }));
    }
    assert!(
        !application
            .join("secrets")
            .join("setup-storage-selection.json")
            .exists()
    );
    let output = run(&["location", "--host", host.to_str().ok_or("host path")?, "1"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run(&["settings", "--host", host.to_str().ok_or("host path")?])?;
    assert!(output.status.success());
    assert!(!serde_json::from_slice::<serde_json::Value>(&output.stdout)?.is_null());

    let database = application.join("state").join("state.db");
    let state = std::fs::read(&database)?;
    let sentinel = root.join("library").join("keep.txt");
    std::fs::write(&sentinel, b"preserve generated existing media")?;
    assert!(!setup()?.status.success());
    assert_eq!(std::fs::read(&host)?, configuration);
    assert_eq!(std::fs::read(&database)?, state);
    assert_eq!(
        std::fs::read(&sentinel)?,
        b"preserve generated existing media"
    );
    Ok(())
}

#[test]
fn fresh_cli_setup_preserves_existing_configuration_and_orphaned_storage()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempdir()?;
    let root = fixture.path().join("application");
    let arguments = [
        "init",
        "--storage-root",
        root.to_str().ok_or("storage path")?,
    ];
    let output = run(&arguments)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let host = root.join("config.json");
    let database = root.join("state").join("state.db");
    let configuration = std::fs::read(&host)?;
    let state = std::fs::read(&database)?;
    let value: serde_json::Value = serde_json::from_slice(&configuration)?;
    assert_eq!(value["environment"], "single_user");
    assert_eq!(value["system_settings_read_only"], true);
    assert_eq!(value["services"]["acquisition"], serde_json::json!([]));
    assert!(value["storage"].is_object());
    assert!(!run(&arguments)?.status.success());
    assert_eq!(std::fs::read(&host)?, configuration);
    assert_eq!(std::fs::read(&database)?, state);
    let orphaned = fixture.path().join("orphaned");
    std::fs::create_dir(&orphaned)?;
    std::fs::write(orphaned.join("state.db"), b"retained evidence")?;
    assert!(
        !run(&[
            "init",
            "--storage-root",
            orphaned.to_str().ok_or("storage path")?
        ])?
        .status
        .success()
    );
    assert_eq!(
        std::fs::read(orphaned.join("state.db"))?,
        b"retained evidence"
    );
    assert_eq!(std::fs::read_dir(&orphaned)?.count(), 1);
    Ok(())
}

#[test]
fn state_cli_requires_host_and_preserves_read_only_system_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempdir()?;
    let (database, host) = initialized_storage(fixture.path())?;
    let mut configuration: serde_json::Value = serde_json::from_slice(&std::fs::read(&host)?)?;
    configuration["system_settings_read_only"] = serde_json::json!(true);
    let configuration = serde_json::to_vec(&configuration)?;
    std::fs::write(&host, &configuration)?;
    let before = std::fs::read(&database)?;
    for command in ["settings", "job-create", "init"] {
        let output = run(&[command, database.to_str().ok_or("database path")?])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
    }
    let settings = fixture.path().join("settings.json");
    std::fs::write(
        &settings,
        serde_json::to_vec(&domus::domain::OutputSettings::default())?,
    )?;
    let output = run(&[
        "settings-save",
        "--host",
        host.to_str().ok_or("host path")?,
        "0",
        settings.to_str().ok_or("settings path")?,
    ])?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("system settings are read-only"));
    assert_eq!(std::fs::read(&database)?, before);
    assert_eq!(std::fs::read(&host)?, configuration);
    let output = run(&["settings", "--host", host.to_str().ok_or("host path")?])?;
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout)?,
        serde_json::Value::Null
    );
    assert_eq!(std::fs::read(&database)?, before);
    Ok(())
}

#[test]
fn serve_storage_override_rejects_relative_paths_without_creating_state()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempdir()?;
    let output = Command::new(env!("CARGO_BIN_EXE_domus"))
        .current_dir(fixture.path())
        .args(["serve", "--storage-root", "relative-storage"])
        .output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("absolute paths"));
    assert_eq!(std::fs::read_dir(fixture.path())?.count(), 0);
    Ok(())
}

#[test]
fn job_history_never_creates_or_upgrades_state() -> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let database = state.path().join("history.db");
    let host = write_host(state.path(), &database)?;
    let path = host.to_str().ok_or("host path")?;
    let missing = run(&["job-history", "--host", path, "1", "2", "0", "50"])?;
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(!database.exists());
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch(
        "PRAGMA user_version=19; CREATE TABLE sentinel(value TEXT);
         INSERT INTO sentinel VALUES ('retained');",
    )?;
    let old = run(&["job-history", "--host", path, "1", "2", "0", "50"])?;
    assert!(!old.status.success());
    assert!(old.stdout.is_empty());
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        19
    );
    assert_eq!(
        raw.query_row("SELECT value FROM sentinel", [], |row| row
            .get::<_, String>(0))?,
        "retained"
    );
    for (after, limit) in [("-1", "50"), ("0", "201"), ("0", "0")] {
        let invalid = run(&["job-history", "--host", path, "1", "2", after, limit])?;
        assert!(!invalid.status.success());
        assert!(invalid.stdout.is_empty());
    }
    Ok(())
}

#[test]
fn mcp_startup_never_creates_or_upgrades_state_or_echoes_private_configuration()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let database = state.path().join("missing.db");
    let configuration = state.path().join("host.json");
    let path = configuration.to_str().ok_or("host path")?;
    std::fs::write(
        &configuration,
        serde_json::to_vec(&serde_json::json!({"database":database,"credentials":"never-echo"}))?,
    )?;
    let invalid = run(&["mcp-stdio", path])?;
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(!String::from_utf8(invalid.stderr)?.contains("never-echo"));
    assert!(!database.exists());
    std::fs::write(
        &configuration,
        serde_json::to_vec(&serde_json::json!({"database":database}))?,
    )?;
    let missing = run(&["mcp-stdio", path])?;
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(!database.exists());
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch("PRAGMA user_version=16; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES ('unchanged');")?;
    let old = run(&["mcp-stdio", path])?;
    assert!(!old.status.success());
    assert!(old.stdout.is_empty());
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        16
    );
    assert_eq!(
        raw.query_row("SELECT value FROM sentinel", [], |row| row
            .get::<_, String>(0))?,
        "unchanged"
    );
    Ok(())
}

#[test]
fn service_and_mcp_require_external_selection_before_state_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let database = state.path().join("state.db");
    drop(domus::infrastructure::store::Store::open(&database)?);
    let configuration = state.path().join("host.json");
    let request = state.path().join("request.json");
    std::fs::write(
        &configuration,
        serde_json::to_vec(&serde_json::json!({
            "environment":"single_user", "database":database,
            "listen":"127.0.0.1:8080", "origin":"http://127.0.0.1:8080"
        }))?,
    )?;
    std::fs::write(
        &request,
        br#"{"operation":"metadata","release":{"provider":"musicbrainz","id":"20000000-0000-4000-8000-000000000001"}}"#,
    )?;
    let before = std::fs::read(&database)?;
    let entries = std::fs::read_dir(state.path())?.count();
    for arguments in [
        vec!["mcp-stdio", configuration.to_str().ok_or("host path")?],
        vec![
            "service-call",
            configuration.to_str().ok_or("host path")?,
            request.to_str().ok_or("request path")?,
        ],
    ] {
        let output = run(&arguments)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("storage selection is missing"));
        assert_eq!(std::fs::read(&database)?, before);
        assert_eq!(std::fs::read_dir(state.path())?.count(), entries);
    }
    Ok(())
}

#[test]
fn mcp_stdio_uses_shared_services_without_network_or_media_authority()
-> Result<(), Box<dyn std::error::Error>> {
    use serde_json::{Value, json};
    use std::io::Write;
    use std::process::Stdio;
    let state = tempdir()?;
    let database = state.path().join("state.db");
    drop(domus::infrastructure::store::Store::open(&database)?);
    let configuration = state.path().join("host.json");
    std::fs::write(
        &configuration,
        serde_json::to_vec(&selected_host(
            state.path(),
            json!({
                "database":database,"services":{"discovery":[],"metadata":[],"artwork":[],"acquisition":[]}
            }),
        )?)?,
    )?;
    let messages = [
        json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}
        }}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":"read","method":"tools/call","params":{
            "name":"release_metadata","arguments":{"release":{"provider":"musicbrainz","id":"fixture"}}
        }}),
    ];
    for modern in [false, true] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_domus"))
            .args(["mcp-stdio", configuration.to_str().ok_or("host path")?])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        {
            let mut input = child.stdin.take().ok_or("child stdin")?;
            for mut message in messages.iter().cloned() {
                if modern {
                    if message["method"] == "initialize"
                        || message["method"] == "notifications/initialized"
                    {
                        continue;
                    }
                    message["params"]["_meta"] = json!({
                        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities":{},
                        "io.modelcontextprotocol/clientInfo":{"name":"administrator","version":"1"}
                    });
                }
                writeln!(input, "{message}")?;
            }
        }
        let output = child.wait_with_output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let replies: Vec<Value> = output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice)
            .collect::<Result<_, _>>()?;
        let list = if modern { 0 } else { 1 };
        let read = list + 1;
        assert_eq!(replies.len(), list + 2);
        if !modern {
            assert_eq!(replies[0]["id"], "init");
        }
        assert_eq!(
            replies[list]["result"]["tools"]
                .as_array()
                .ok_or("tools")?
                .len(),
            4
        );
        assert_eq!(replies[read]["id"], "read");
        assert_eq!(replies[read]["result"]["isError"], true);
        assert_eq!(replies[read]["result"].get("resultType").is_some(), modern);
        assert!(
            replies[read]["result"]["content"][0]["text"]
                .as_str()
                .ok_or("tool error")?
                .contains("disabled")
        );
        assert!(
            !replies
                .iter()
                .any(|reply| reply.to_string().contains("host.json")
                    || reply.to_string().contains("state.db"))
        );
    }
    let raw = rusqlite::Connection::open(database)?;
    assert_eq!(
        raw.query_row("SELECT count(*) FROM jobs", [], |row| row.get::<_, i64>(0))?,
        0
    );
    Ok(())
}

#[test]
fn job_controls_cli_requires_current_state_and_preserves_legacy_databases()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let database = state.path().join("state.db");
    let host = write_host(state.path(), &database)?;
    let path = host.to_str().ok_or("host path")?;
    for arguments in [
        vec!["job-control", "--host", path, "1", "1", "pause"],
        vec!["job-control", "--host", path, "1", "1", "restart"],
        vec!["job-control-status", "--host", path, "1"],
        vec!["job-control-continue", "--host", path, "1", "1"],
    ] {
        let output = run(&arguments)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
        assert!(!database.exists());
    }
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch("PRAGMA user_version=16; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES ('unchanged');")?;
    for arguments in [
        vec!["job-control", "--host", path, "1", "1", "cancel"],
        vec!["job-control-status", "--host", path, "1"],
        vec!["job-control-continue", "--host", path, "1", "1"],
    ] {
        let output = run(&arguments)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        16
    );
    assert_eq!(
        raw.query_row("SELECT value FROM sentinel", [], |row| row
            .get::<_, String>(0))?,
        "unchanged"
    );
    drop(raw);
    Ok(())
}

#[test]
fn job_controls_cli_pauses_answers_and_continues_without_claiming()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let database = state.path().join("state.db");
    let mut store = domus::infrastructure::store::Store::open(&database)?;
    let job = store.enqueue(1)?.to_string();
    drop(store);
    let host = write_host(state.path(), &database)?;
    let path = host.to_str().ok_or("host path")?;
    let output = run(&["job-control", "--host", path, &job, "1", "pause"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let control: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let id = control["id"].as_i64().ok_or("control ID")?.to_string();
    let question = control["question_id"]
        .as_i64()
        .ok_or("question ID")?
        .to_string();
    let status = run(&["job-control-status", "--host", path, &id])?;
    assert!(status.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout)?,
        control
    );
    assert!(
        !run(&["job-control-continue", "--host", path, &id, "1"])?
            .status
            .success()
    );
    assert!(
        run(&["question-answer", "--host", path, &question, "1", "Resume"])?
            .status
            .success()
    );
    let continuation = run(&["job-control-continue", "--host", path, &id, "1"])?;
    assert!(continuation.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&continuation.stdout)?,
        serde_json::json!("queued")
    );
    let raw = rusqlite::Connection::open(&database)?;
    assert_eq!(
        raw.query_row("SELECT state,owner IS NULL FROM jobs", [], |row| Ok((
            row.get::<_, String>(0)?,
            row.get::<_, bool>(1)?
        )))?,
        ("queued".into(), true)
    );
    Ok(())
}

#[test]
fn service_cli_rejects_invalid_hosts_and_tool_inputs_without_creating_state()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let config = state.path().join("host.json");
    let request = state.path().join("request.json");
    let database = state.path().join("missing.db");
    let valid_request = serde_json::json!({"operation":"metadata","release":{"provider":"musicbrainz","id":"invalid"}});
    std::fs::write(&request, serde_json::to_vec(&valid_request)?)?;
    for settings in [
        serde_json::json!({"database":database,"services":{"metadata":["spotify"]}}),
        serde_json::json!({"database":"relative.db"}),
        serde_json::json!({"database":database,"password":"never-echo"}),
    ] {
        std::fs::write(&config, serde_json::to_vec(&settings)?)?;
        let output = run(&[
            "service-call",
            config.to_str().ok_or("config path")?,
            request.to_str().ok_or("request path")?,
        ])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8(output.stderr)?.contains("never-echo"));
        assert!(!database.exists());
    }
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({"database":database}))?,
    )?;
    for command in [
        serde_json::json!({"operation":"download","url":"never-echo"}),
        serde_json::json!({"operation":"metadata","release":{"provider":"musicbrainz","id":"valid"},"database":"never-echo"}),
    ] {
        std::fs::write(&request, serde_json::to_vec(&command)?)?;
        let output = run(&[
            "service-call",
            config.to_str().ok_or("config path")?,
            request.to_str().ok_or("request path")?,
        ])?;
        assert!(!output.status.success());
        assert!(!String::from_utf8(output.stderr)?.contains("never-echo"));
        assert!(!database.exists());
    }
    Ok(())
}

#[test]
fn discovery_cli_requires_existing_state_and_valid_inputs_before_network()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempdir()?;
    let database = root.path().join("missing.db");
    let database = database.to_str().ok_or("database path")?;
    let output = run(&["catalog-release", database, "invalid-release"])?;
    assert!(!output.status.success());
    assert!(!std::path::Path::new(database).exists());
    let config = root.path().join("search.json");
    std::fs::write(
        &config,
        br#"{"username":"never-echo","password":"never-echo"}"#,
    )?;
    let output = run(&["source-search", config.to_str().ok_or("config path")?])?;
    assert!(!output.status.success());
    assert!(!String::from_utf8(output.stderr)?.contains("never-echo"));
    assert!(output.stdout.is_empty());
    let output = run(&["source-folder", config.to_str().ok_or("config path")?])?;
    assert!(!output.status.success());
    assert!(!String::from_utf8(output.stderr)?.contains("never-echo"));
    assert!(output.stdout.is_empty());
    assert!(!std::path::Path::new(database).exists());
    Ok(())
}

#[test]
fn worker_check_cli_rejects_missing_database_and_never_upgrades_old_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let database = state.path().join("state.db");
    let host = write_host(state.path(), &database)?;
    let host = host.to_str().ok_or("host path")?;
    let config = state.path().join("worker.json");
    let secret = state.path().join("credentials.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "database":database, "staging_root":state.path().join("scratch"),
            "credentials_file":secret,"server":"8.8.8.8:2242","job_id":1,
            "plan_version":2,"acknowledge_plaintext_login":true,
        }))?,
    )?;
    let config = config.to_str().ok_or("config path")?;
    let missing = run(&["worker-check", "--host", host, config])?;
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(!missing.stderr.is_empty());
    assert!(!database.exists());
    assert!(!secret.exists());
    std::fs::write(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch("PRAGMA user_version=12; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES ('unchanged');")?;
    let old = run(&["worker-check", "--host", host, config])?;
    assert!(!old.status.success());
    assert!(old.stdout.is_empty());
    let error = String::from_utf8(old.stderr)?;
    assert!(error.contains("explicit fresh setup"), "{error}");
    assert!(!state.path().join("sqlite-tmp").exists());
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        12
    );
    assert_eq!(
        raw.query_row("SELECT value FROM sentinel", [], |row| row
            .get::<_, String>(0))?,
        "unchanged"
    );
    assert!(!state.path().join("scratch").exists());
    Ok(())
}

#[test]
fn local_cli_can_initialize_scan_and_report_coverage() -> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let media = tempdir()?;
    let (_database, host) = initialized_storage(state.path())?;
    select_host_roots(
        &host,
        &[(
            "media",
            domus::infrastructure::filesystem::StorageRole::Media,
            media.path(),
        )],
    )?;
    let host = host.to_str().ok_or("host path")?;
    let root = media.path().to_str().ok_or("non-Unicode media")?;
    std::fs::write(
        media.path().join("folder.jpg"),
        b"unchanged image observation",
    )?;
    let output = run(&["location-add", "--host", host, root, "main"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let location = String::from_utf8(output.stdout)?.trim().to_owned();
    assert!(run(&["scan", "--host", host, &location])?.status.success());
    let details = run(&["location", "--host", host, &location])?;
    assert!(details.status.success());
    let details: serde_json::Value = serde_json::from_slice(&details.stdout)?;
    assert_eq!(details["coverage"], "complete");
    let browse = run(&["browse", "--host", host, &location, "", "0", "20"])?;
    assert!(browse.status.success());
    let entries: serde_json::Value = serde_json::from_slice(&browse.stdout)?;
    assert_eq!(entries[0]["relative_path"], "folder.jpg");
    let invalid = run(&["browse", "--host", host, &location, "../secret", "0", "20"])?;
    assert!(!invalid.status.success());
    assert!(!invalid.stderr.is_empty());
    assert_eq!(
        std::fs::read(media.path().join("folder.jpg"))?,
        b"unchanged image observation"
    );
    Ok(())
}

#[test]
fn local_cli_refuses_legacy_media_and_staging_binding_without_upgrading_state()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let media = tempdir()?;
    let staging = tempdir()?;
    let database = state.path().join("state.db");
    let host = write_host(state.path(), &database)?;
    let host = host.to_str().ok_or("host path")?;
    let root = media.path().to_str().ok_or("non-Unicode media")?;
    let staging_root = staging.path().to_str().ok_or("non-Unicode staging")?;
    let mut store = domus::infrastructure::store::Store::open(&database)?;
    let location = domus::application::add_location(&store, media.path(), "main")?;
    let expected = store
        .location(location)?
        .native_identity
        .ok_or("missing native identity")?;
    drop(domus::infrastructure::staging::StagingRoot::open(
        &mut store,
        staging.path(),
    )?);
    let marker = std::fs::read(staging.path().join("owner.lock"))?;
    drop(store);
    std::fs::write(media.path().join("sentinel.txt"), b"unchanged")?;
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch(include_str!("support/drop_sharing.sql"))?;
    raw.execute_batch("DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations; DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots; PRAGMA user_version = 12;")?;
    drop(raw);
    let location = location.to_string();
    let identity = run(&["root-identity", root])?;
    assert!(identity.status.success());
    assert_eq!(String::from_utf8(identity.stdout)?.trim(), expected);
    assert!(!run(&["scan", "--host", host, &location])?.status.success());
    assert!(
        !run(&["location-bind", "--host", host, &location, "wrong"])?
            .status
            .success()
    );
    let bound = run(&["location-bind", "--host", host, &location, &expected])?;
    assert!(!bound.status.success());
    assert!(String::from_utf8_lossy(&bound.stderr).contains("explicit fresh setup"));
    assert!(!run(&["scan", "--host", host, &location])?.status.success());
    let staging_identity = run(&["root-identity", staging_root])?;
    assert!(staging_identity.status.success());
    let staging_identity = String::from_utf8(staging_identity.stdout)?;
    assert!(
        !run(&["staging-bind", "--host", host, staging_root, "wrong"])?
            .status
            .success()
    );
    let bound = run(&[
        "staging-bind",
        "--host",
        host,
        staging_root,
        staging_identity.trim(),
    ])?;
    assert!(!bound.status.success());
    assert!(String::from_utf8_lossy(&bound.stderr).contains("explicit fresh setup"));
    assert!(!run(&["scan", "--host", host, &location])?.status.success());
    assert_eq!(
        rusqlite::Connection::open(&database)?
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        12
    );
    assert_eq!(
        std::fs::read(media.path().join("sentinel.txt"))?,
        b"unchanged"
    );
    assert_eq!(std::fs::read_dir(media.path())?.count(), 1);
    assert_eq!(std::fs::read(staging.path().join("owner.lock"))?, marker);
    assert_eq!(std::fs::read_dir(staging.path())?.count(), 1);
    Ok(())
}

#[test]
fn invalid_usage_fails_explicitly() -> Result<(), Box<dyn std::error::Error>> {
    let output = run(&["serve-without-auth"])?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("Usage:"));
    Ok(())
}

#[test]
fn worker_cli_rejects_invalid_configuration_without_network_or_database_creation()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let fixture_database = state.path().join("fixture.db");
    drop(domus::infrastructure::store::Store::open(
        &fixture_database,
    )?);
    let configuration = state.path().join("worker.json");
    let database = state.path().join("missing.db");
    let host = write_host(state.path(), &fixture_database)?;
    let body = serde_json::json!({
        "database":database,"staging_root":state.path(),
        "credentials_file":state.path().join("secret.json"),
        "server":"127.0.0.1:2242","job_id":1,"plan_version":1,
        "acknowledge_plaintext_login":false
    });
    std::fs::write(&configuration, serde_json::to_vec(&body)?)?;
    let output = run(&[
        "worker-once",
        "--host",
        host.to_str().ok_or("host path")?,
        configuration.to_str().ok_or("non-Unicode configuration")?,
    ])?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)?.contains("plaintext-login acknowledgement"));
    assert!(!database.exists());
    Ok(())
}

#[test]
fn publication_cli_rejects_invalid_inputs_missing_state_and_old_schema_without_side_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let database = state.path().join("missing.db");
    let host = write_host(state.path(), &database)?;
    let host = host.to_str().ok_or("host path")?;
    let config = state.path().join("publication.json");
    let staging = state.path().join("scratch");
    let valid = serde_json::json!({
        "database":database,"staging_root":staging,"job_id":1,"plan_version":2,
        "normalization_run":1,"acknowledge_new_only_publication":true
    });
    for (key, value) in [
        ("acknowledge_new_only_publication", serde_json::json!(false)),
        ("database", serde_json::json!("relative.db")),
        ("staging_root", serde_json::json!("relative")),
        ("job_id", serde_json::json!(0)),
        ("plan_version", serde_json::json!(-1)),
        ("normalization_run", serde_json::json!(0)),
        ("password", serde_json::json!("never-echo")),
    ] {
        let mut body = valid.clone();
        body[key] = value;
        std::fs::write(&config, serde_json::to_vec(&body)?)?;
        let output = run(&[
            "publish-new-once",
            "--host",
            host,
            config.to_str().ok_or("config")?,
        ])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8(output.stderr)?.contains("never-echo"));
        assert!(!database.exists());
        assert!(!staging.exists());
    }
    std::fs::write(&config, serde_json::to_vec(&valid)?)?;
    assert!(
        !run(&[
            "publish-new-once",
            "--host",
            host,
            config.to_str().ok_or("config")?
        ])?
        .status
        .success()
    );
    assert!(
        !run(&["publication-status", "--host", host, "1", "2"])?
            .status
            .success()
    );
    assert!(!database.exists());
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch("PRAGMA user_version=15; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES ('unchanged');")?;
    for output in [
        run(&[
            "publish-new-once",
            "--host",
            host,
            config.to_str().ok_or("config")?,
        ])?,
        run(&["publication-status", "--host", host, "1", "2"])?,
    ] {
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        15
    );
    assert_eq!(
        raw.query_row("SELECT value FROM sentinel", [], |row| row
            .get::<_, String>(0))?,
        "unchanged"
    );
    assert!(!staging.exists());
    Ok(())
}

#[test]
fn a_read_command_does_not_create_a_misspelled_database() -> Result<(), Box<dyn std::error::Error>>
{
    let state = tempdir()?;
    let database = state.path().join("missing.db");
    let host = write_host(state.path(), &database)?;
    let output = run(&[
        "browse",
        "--host",
        host.to_str().ok_or("host path")?,
        "1",
        "",
        "0",
        "20",
    ])?;
    assert!(!output.status.success());
    assert!(!database.exists());
    Ok(())
}

#[test]
fn local_cli_can_save_preview_and_explicitly_approve_without_starting_work()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let media = tempdir()?;
    let (database, host) = initialized_storage(state.path())?;
    select_host_roots(
        &host,
        &[(
            "media",
            domus::infrastructure::filesystem::StorageRole::Media,
            media.path(),
        )],
    )?;
    let application = host.parent().ok_or("application directory")?;
    let host = host.to_str().ok_or("host path")?;
    let database = database.to_str().ok_or("non-Unicode database")?;
    let absent = run(&["settings", "--host", host])?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&absent.stdout)?,
        serde_json::Value::Null
    );
    let settings_file = application.join("settings.json");
    std::fs::write(
        &settings_file,
        serde_json::to_vec(&domus::domain::OutputSettings::default())?,
    )?;
    let settings_file = settings_file.to_str().ok_or("non-Unicode settings file")?;
    assert!(
        run(&["settings-save", "--host", host, "0", settings_file])?
            .status
            .success()
    );
    assert!(
        !run(&["settings-save", "--host", host, "0", settings_file])?
            .status
            .success()
    );
    let location = run(&[
        "location-add",
        "--host",
        host,
        media.path().to_str().ok_or("non-Unicode root")?,
        "main",
    ])?;
    assert!(location.status.success());
    let location = String::from_utf8(location.stdout)?.trim().to_owned();
    assert!(run(&["scan", "--host", host, &location])?.status.success());
    let job = run(&["job-create", "--host", host])?;
    assert!(job.status.success());
    let job = String::from_utf8(job.stdout)?.trim().to_owned();
    let request_file = application.join("request.json");
    std::fs::write(
        &request_file,
        serde_json::to_vec(&serde_json::json!({
            "job_id": job.parse::<i64>()?, "expected_version": 1, "settings_version": 1,
            "edition": {
                "release_id": "12345678-1234-4321-9876-123456789abc",
                "album_artist": "The Example", "album": "A Fixture", "bucket": null,
                "tracks": [{"artist": "The Example", "title": "A Track", "disc": 1, "disc_track": 1}]
            },
            "locations": [location.parse::<i64>()?], "accepts_incomplete_coverage": false
        }))?,
    )?;
    let prepared = run(&[
        "plan-prepare",
        "--host",
        host,
        request_file.to_str().ok_or("non-Unicode request")?,
    ])?;
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let prepared: serde_json::Value = serde_json::from_slice(&prepared.stdout)?;
    assert_eq!(prepared["version"], 2);
    assert!(prepared["approved_at"].is_null());
    let approved = run(&["plan-approve", "--host", host, &job, "2"])?;
    assert!(approved.status.success());
    assert!(String::from_utf8(approved.stdout)?.contains("No transfer or file operation started"));
    let saved = run(&["plan", "--host", host, &job, "2"])?;
    assert!(saved.status.success());
    let saved: serde_json::Value = serde_json::from_slice(&saved.stdout)?;
    assert!(saved["approved_at"].is_i64());
    assert_eq!(std::fs::read_dir(media.path())?.count(), 0);
    assert_eq!(
        domus::infrastructure::store::Store::open_existing(std::path::Path::new(database))?
            .job_state(job.parse()?)?
            .as_deref(),
        Some("queued")
    );
    Ok(())
}

#[test]
fn local_json_input_is_bounded_before_parsing() -> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let (_database, host) = initialized_storage(state.path())?;
    let file = host
        .parent()
        .ok_or("application directory")?
        .join("large-settings.json");
    let host = host.to_str().ok_or("host path")?;
    std::fs::write(&file, vec![b' '; 16385])?;
    let output = run(&[
        "settings-save",
        "--host",
        host,
        "0",
        file.to_str().ok_or("non-Unicode file")?,
    ])?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("exceeds its size limit"));
    std::fs::write(&file, br#""never-echo-private-input""#)?;
    let output = run(&[
        "settings-save",
        "--host",
        host,
        "0",
        file.to_str().ok_or("non-Unicode file")?,
    ])?;
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr)?;
    assert!(error.contains("invalid JSON input"));
    assert!(!error.contains("never-echo-private-input"));
    Ok(())
}
