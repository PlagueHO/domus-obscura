use std::process::Command;
use tempfile::tempdir;

#[path = "cli/sources.rs"]
mod source_tests;
mod support;

fn run(args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_domus"))
        .args(args)
        .output()
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
        serde_json::to_vec(&json!({
            "database":database,"services":{"discovery":[],"metadata":[],"artwork":[],"acquisition":[]}
        }))?,
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
    let path = database.to_str().ok_or("database path")?;
    for arguments in [
        vec!["job-control", path, "1", "1", "pause"],
        vec!["job-control", path, "1", "1", "restart"],
        vec!["job-control-status", path, "1"],
        vec!["job-control-continue", path, "1", "1"],
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
        vec!["job-control", path, "1", "1", "cancel"],
        vec!["job-control-status", path, "1"],
        vec!["job-control-continue", path, "1", "1"],
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
    let path = database.to_str().ok_or("database path")?;
    let output = run(&["job-control", path, &job, "1", "pause"])?;
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
    let status = run(&["job-control-status", path, &id])?;
    assert!(status.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout)?,
        control
    );
    assert!(
        !run(&["job-control-continue", path, &id, "1"])?
            .status
            .success()
    );
    assert!(
        run(&["question-answer", path, &question, "1", "Resume"])?
            .status
            .success()
    );
    let continuation = run(&["job-control-continue", path, &id, "1"])?;
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
    let missing = run(&["worker-check", config])?;
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
    let old = run(&["worker-check", config])?;
    assert!(!old.status.success());
    assert!(old.stdout.is_empty());
    let error = String::from_utf8(old.stderr)?;
    assert!(error.contains("upgrade separately"), "{error}");
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
    let database = state.path().join("state.db");
    let database = database.to_str().ok_or("non-Unicode database")?;
    let root = media.path().to_str().ok_or("non-Unicode media")?;
    std::fs::write(
        media.path().join("folder.jpg"),
        b"unchanged image observation",
    )?;
    assert!(run(&["init", database])?.status.success());
    let output = run(&["location-add", database, root, "main"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let location = String::from_utf8(output.stdout)?.trim().to_owned();
    assert!(run(&["scan", database, &location])?.status.success());
    let details = run(&["location", database, &location])?;
    assert!(details.status.success());
    let details: serde_json::Value = serde_json::from_slice(&details.stdout)?;
    assert_eq!(details["coverage"], "complete");
    let browse = run(&["browse", database, &location, "", "0", "20"])?;
    assert!(browse.status.success());
    let entries: serde_json::Value = serde_json::from_slice(&browse.stdout)?;
    assert_eq!(entries[0]["relative_path"], "folder.jpg");
    let invalid = run(&["browse", database, &location, "../secret", "0", "20"])?;
    assert!(!invalid.status.success());
    assert!(!invalid.stderr.is_empty());
    assert_eq!(
        std::fs::read(media.path().join("folder.jpg"))?,
        b"unchanged image observation"
    );
    Ok(())
}

#[test]
fn local_cli_explicitly_binds_legacy_media_and_staging_without_changing_files()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempdir()?;
    let media = tempdir()?;
    let staging = tempdir()?;
    let database = state.path().join("state.db");
    let database_text = database.to_str().ok_or("non-Unicode database")?;
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
    raw.execute_batch("DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots; PRAGMA user_version = 12;")?;
    drop(raw);
    let location = location.to_string();
    let identity = run(&["root-identity", root])?;
    assert!(identity.status.success());
    assert_eq!(String::from_utf8(identity.stdout)?.trim(), expected);
    assert!(!run(&["scan", database_text, &location])?.status.success());
    assert!(
        !run(&["location-bind", database_text, &location, "wrong"])?
            .status
            .success()
    );
    let bound = run(&["location-bind", database_text, &location, &expected])?;
    assert!(bound.status.success(), "{:?}", bound.stderr);
    assert!(!run(&["scan", database_text, &location])?.status.success());
    let staging_identity = run(&["root-identity", staging_root])?;
    assert!(staging_identity.status.success());
    let staging_identity = String::from_utf8(staging_identity.stdout)?;
    assert!(
        !run(&["staging-bind", database_text, staging_root, "wrong"])?
            .status
            .success()
    );
    let bound = run(&[
        "staging-bind",
        database_text,
        staging_root,
        staging_identity.trim(),
    ])?;
    assert!(bound.status.success(), "{:?}", bound.stderr);
    assert!(run(&["scan", database_text, &location])?.status.success());
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
    let configuration = state.path().join("worker.json");
    let database = state.path().join("missing.db");
    let body = serde_json::json!({
        "database":database,"staging_root":state.path(),
        "credentials_file":state.path().join("secret.json"),
        "server":"127.0.0.1:2242","job_id":1,"plan_version":1,
        "acknowledge_plaintext_login":false
    });
    std::fs::write(&configuration, serde_json::to_vec(&body)?)?;
    let output = run(&[
        "worker-once",
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
        let output = run(&["publish-new-once", config.to_str().ok_or("config")?])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8(output.stderr)?.contains("never-echo"));
        assert!(!database.exists());
        assert!(!staging.exists());
    }
    std::fs::write(&config, serde_json::to_vec(&valid)?)?;
    assert!(
        !run(&["publish-new-once", config.to_str().ok_or("config")?])?
            .status
            .success()
    );
    assert!(
        !run(&[
            "publication-status",
            database.to_str().ok_or("db")?,
            "1",
            "2"
        ])?
        .status
        .success()
    );
    assert!(!database.exists());
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch("PRAGMA user_version=15; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES ('unchanged');")?;
    for output in [
        run(&["publish-new-once", config.to_str().ok_or("config")?])?,
        run(&[
            "publication-status",
            database.to_str().ok_or("db")?,
            "1",
            "2",
        ])?,
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
    let output = run(&[
        "browse",
        database.to_str().ok_or("non-Unicode path")?,
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
    let database = state.path().join("state.db");
    let database = database.to_str().ok_or("non-Unicode database")?;
    assert!(run(&["init", database])?.status.success());
    let absent = run(&["settings", database])?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&absent.stdout)?,
        serde_json::Value::Null
    );
    let settings_file = state.path().join("settings.json");
    std::fs::write(
        &settings_file,
        serde_json::to_vec(&domus::domain::OutputSettings::default())?,
    )?;
    let settings_file = settings_file.to_str().ok_or("non-Unicode settings file")?;
    assert!(
        run(&["settings-save", database, "0", settings_file])?
            .status
            .success()
    );
    assert!(
        !run(&["settings-save", database, "0", settings_file])?
            .status
            .success()
    );
    let location = run(&[
        "location-add",
        database,
        media.path().to_str().ok_or("non-Unicode root")?,
        "main",
    ])?;
    assert!(location.status.success());
    let location = String::from_utf8(location.stdout)?.trim().to_owned();
    assert!(run(&["scan", database, &location])?.status.success());
    let job = run(&["job-create", database])?;
    assert!(job.status.success());
    let job = String::from_utf8(job.stdout)?.trim().to_owned();
    let request_file = state.path().join("request.json");
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
        database,
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
    let approved = run(&["plan-approve", database, &job, "2"])?;
    assert!(approved.status.success());
    assert!(String::from_utf8(approved.stdout)?.contains("No transfer or file operation started"));
    let saved = run(&["plan", database, &job, "2"])?;
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
    let database = state.path().join("state.db");
    let database = database.to_str().ok_or("non-Unicode database")?;
    assert!(run(&["init", database])?.status.success());
    let file = state.path().join("large-settings.json");
    std::fs::write(&file, vec![b' '; 16385])?;
    let output = run(&[
        "settings-save",
        database,
        "0",
        file.to_str().ok_or("non-Unicode file")?,
    ])?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("JSON input exceeds 16384 bytes"));
    Ok(())
}
