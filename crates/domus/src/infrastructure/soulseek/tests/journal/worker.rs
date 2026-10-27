use super::*;
use crate::infrastructure::worker::PreparedWorker;
use std::path::Path;

mod check;

fn credentials(path: &Path, body: &[u8]) -> TestResult {
    std::fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn configuration(fixture: &Fixture, root: &Path, secret: &Path) -> serde_json::Value {
    json!({
        "database":fixture.database,
        "staging_root":root,
        "credentials_file":secret,
        "server":"8.8.8.8:2242",
        "job_id":fixture.job,
        "plan_version":fixture.version,
        "acknowledge_plaintext_login":true
    })
}

fn source_selection() -> serde_json::Value {
    json!({
        "release_id":"20000000-0000-4000-8000-000000000001",
        "peer":"fixture-peer",
        "acknowledge_exact_edition_and_permitted_download":true,
        "files": (1..=3).map(|track| json!({
            "track_number":track, "remote_path":format!("Peer\\Album\\{track}.flac"),
            "expected_bytes":1000,
        })).collect::<Vec<_>>(),
    })
}

#[test]
fn worker_journals_an_explicit_complete_source_before_network_and_never_replaces_it() -> TestResult
{
    let fixture = Fixture::build(AudioFormat::Flac, 1000, 3, true, false)?;
    let scratch = tempdir()?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("worker.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let mut body = configuration(&fixture, scratch.path(), &secret);
    body["source"] = source_selection();
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute("UPDATE jobs SET lease_until=0", [])?;
    let worker = PreparedWorker::load(&config)?;
    let files = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    assert_eq!(files.len(), 3);
    for (index, file) in files.iter().enumerate() {
        assert_eq!(usize::from(file.track_number), index + 1);
        assert_eq!(file.remote_path, format!("Peer\\Album\\{}.flac", index + 1));
        assert_eq!(file.peer, "fixture-peer");
        assert_eq!(file.expected_bytes, 1000);
        assert_eq!(file.state, "planned");
        assert_eq!(file.remote_token, None);
    }
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("downloading")
    );
    drop(worker);
    raw.execute("UPDATE jobs SET lease_until=0", [])?;
    body["source"]["peer"] = json!("different-peer");
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    assert!(PreparedWorker::load(&config).is_err());
    body.as_object_mut()
        .ok_or("config object")?
        .remove("source");
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    let worker = PreparedWorker::load(&config)?;
    drop(worker);
    let events: i64 = raw.query_row(
        "SELECT count(*) FROM events WHERE event_type='acquisition_prepared'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(events, 1);
    assert_eq!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?,)?[0]
            .peer,
        "fixture-peer"
    );
    Ok(())
}

#[test]
fn invalid_source_selection_leaves_queued_job_claim_and_staging_unchanged() -> TestResult {
    for fault in [
        "permission",
        "release",
        "missing",
        "order",
        "duplicate",
        "format",
        "size",
        "path",
        "peer",
        "unknown",
    ] {
        let fixture = Fixture::build(AudioFormat::Flac, 1000, 3, true, false)?;
        let scratch = tempdir()?;
        let secret = fixture._state.path().join("credentials.json");
        let config = fixture._state.path().join("worker.json");
        credentials(
            &secret,
            br#"{"username":"fixture-user","password":"fixture-password"}"#,
        )?;
        let mut selected = source_selection();
        match fault {
            "permission" => {
                selected["acknowledge_exact_edition_and_permitted_download"] = json!(false)
            }
            "release" => selected["release_id"] = json!("20000000-0000-4000-8000-000000000002"),
            "missing" => {
                selected["files"].as_array_mut().ok_or("files array")?.pop();
            }
            "order" => selected["files"][0]["track_number"] = json!(2),
            "duplicate" => {
                selected["files"][1]["remote_path"] = selected["files"][0]["remote_path"].clone()
            }
            "format" => selected["files"][0]["remote_path"] = json!("Peer\\Album\\1.mp3"),
            "size" => selected["files"][0]["expected_bytes"] = json!(0),
            "path" => selected["files"][0]["remote_path"] = json!("Peer\\..\\secret.flac"),
            "peer" => selected["peer"] = json!(""),
            "unknown" => selected["unexpected"] = json!(true),
            _ => unreachable!(),
        }
        let mut body = configuration(&fixture, scratch.path(), &secret);
        body["source"] = selected;
        std::fs::write(&config, serde_json::to_vec(&body)?)?;
        let raw = Connection::open(&fixture.database)?;
        raw.execute("UPDATE jobs SET lease_until=0", [])?;
        let generation: i64 =
            raw.query_row("SELECT claim_generation FROM jobs", [], |row| row.get(0))?;
        assert!(PreparedWorker::load(&config).is_err(), "fault: {fault}");
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("queued")
        );
        assert!(
            fixture
                .store
                .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
                .is_empty()
        );
        assert_eq!(
            raw.query_row("SELECT claim_generation FROM jobs", [], |row| row
                .get::<_, i64>(0))?,
            generation
        );
        assert_eq!(scratch.path().read_dir()?.count(), 0);
    }
    Ok(())
}

#[test]
fn missing_worker_cover_pauses_before_claim_source_or_staging_effects() -> TestResult {
    let fixture = Fixture::build(AudioFormat::Flac, 1000, 3, false, false)?;
    let scratch = tempdir()?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("worker.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let mut body = configuration(&fixture, scratch.path(), &secret);
    body["source"] = source_selection();
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute("UPDATE jobs SET lease_until=0", [])?;
    let error = match PreparedWorker::load(&config) {
        Err(error) => error,
        Ok(_) => return Err("missing cover must pause".into()),
    };
    let crate::Error::ArtworkRequired(id) = error else {
        return Err("expected saved question".into());
    };
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("awaiting_input")
    );
    assert_eq!(fixture.store.artwork_decision(id)?.paused_state, "queued");
    assert!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    assert!(raw.query_row(
        "SELECT owner IS NULL AND lease_until IS NULL FROM jobs",
        [],
        |row| row.get::<_, bool>(0)
    )?);
    assert_eq!(scratch.path().read_dir()?.count(), 0);
    Ok(())
}

#[test]
fn source_event_failure_rolls_back_all_files_and_can_resume_before_network() -> TestResult {
    let fixture = Fixture::build(AudioFormat::Flac, 1000, 3, true, false)?;
    let scratch = tempdir()?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("worker.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let mut body = configuration(&fixture, scratch.path(), &secret);
    body["source"] = source_selection();
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "UPDATE jobs SET lease_until=0;
         CREATE TRIGGER fail_selection BEFORE INSERT ON events
         WHEN NEW.event_type='acquisition_prepared' BEGIN SELECT RAISE(ABORT,'injected'); END;",
    )?;
    assert!(PreparedWorker::load(&config).is_err());
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("searching")
    );
    assert!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    raw.execute_batch("DROP TRIGGER fail_selection; UPDATE jobs SET lease_until=0;")?;
    let worker = PreparedWorker::load(&config)?;
    drop(worker);
    assert_eq!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
            .len(),
        3
    );
    Ok(())
}

#[test]
fn worker_loads_an_approved_selected_album_without_opening_any_network_socket() -> TestResult {
    let fixture = Fixture::album(AudioFormat::Flac, 1000, 3, true)?;
    let scratch = tempdir()?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("worker.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    std::fs::write(
        &config,
        serde_json::to_vec(&configuration(&fixture, scratch.path(), &secret))?,
    )?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute("UPDATE jobs SET lease_until=0", [])?;
    let worker = PreparedWorker::load(&config)?;
    let files = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    assert!(
        files
            .iter()
            .all(|file| file.state == "planned" && file.remote_token.is_none())
    );
    let live: bool = raw.query_row(
        "SELECT lease_until > unixepoch() FROM jobs WHERE id=?1",
        [fixture.job],
        |row| row.get(0),
    )?;
    assert!(live);
    assert!(PreparedWorker::load(&config).is_err());
    drop(worker);
    Ok(())
}

#[test]
fn worker_configuration_rejects_missing_acknowledgement_private_servers_and_bad_ids() -> TestResult
{
    let fixture = Fixture::album(AudioFormat::Flac, 1000, 1, true)?;
    let scratch = tempdir()?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("worker.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let original = configuration(&fixture, scratch.path(), &secret);
    for (field, value) in [
        ("services", json!({"acquisition":[]})),
        ("services", json!({"acquisition":["unavailable"]})),
        ("services", json!({"artwork":["unavailable"]})),
        ("acknowledge_plaintext_login", json!(false)),
        ("server", json!("127.0.0.1:2242")),
        ("server", json!("10.0.0.1:2242")),
        ("server", json!("8.8.8.8:0")),
        ("server", json!("[::1]:2242")),
        ("job_id", json!(0)),
        ("plan_version", json!(-1)),
        ("database", json!("relative.db")),
        ("credentials_file", json!("relative.json")),
        ("unknown", json!("fixture-secret-should-not-be-echoed")),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        std::fs::write(&config, serde_json::to_vec(&changed)?)?;
        let error = PreparedWorker::load(&config)
            .err()
            .ok_or("invalid worker config accepted")?;
        assert!(
            !error
                .to_string()
                .contains("fixture-secret-should-not-be-echoed")
        );
    }
    Ok(())
}

#[test]
fn worker_rejects_credentials_in_media_roots_or_unreadable_private_credentials() -> TestResult {
    for fault in [
        "media",
        "json",
        "username",
        "password",
        "oversize",
        "unapproved",
    ] {
        let fixture = Fixture::album(AudioFormat::Flac, 1000, 1, fault != "unapproved")?;
        let scratch = tempdir()?;
        let secret = if fault == "media" {
            fixture._library.path().join("credentials.json")
        } else {
            fixture._state.path().join("credentials.json")
        };
        let config = fixture._state.path().join("worker.json");
        let body = match fault {
            "json" => br#"{"password":"fixture-secret","unexpected":"fixture-secret"}"#.to_vec(),
            "username" => br#"{"username":"","password":"fixture-secret"}"#.to_vec(),
            "password" => br#"{"username":"fixture-user","password":""}"#.to_vec(),
            "oversize" => vec![b'x'; 4097],
            _ => br#"{"username":"fixture-user","password":"fixture-secret"}"#.to_vec(),
        };
        credentials(&secret, &body)?;
        std::fs::write(
            &config,
            serde_json::to_vec(&configuration(&fixture, scratch.path(), &secret))?,
        )?;
        Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
        let error = PreparedWorker::load(&config)
            .err()
            .ok_or("invalid worker setup accepted")?;
        assert!(!error.to_string().contains("fixture-secret"));
        let files = fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?;
        assert_eq!(files[0].state, "planned");
    }
    Ok(())
}

#[test]
fn worker_loads_uncertainty_for_reconciliation_and_sealed_input_needs_no_login() -> TestResult {
    let source = media::flac();
    let mut fixture = Fixture::album(AudioFormat::Flac, source.len() as i64, 1, true)?;
    let scratch = tempdir()?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("worker.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    std::fs::write(
        &config,
        serde_json::to_vec(&configuration(&fixture, scratch.path(), &secret))?,
    )?;
    fixture.store.start_transfer(&fixture.claim, fixture.file)?;
    fixture
        .store
        .accept_transfer(&fixture.claim, fixture.file, 17)?;
    let staging =
        crate::infrastructure::staging::StagingRoot::open(&mut fixture.store, scratch.path())?;
    let transfer = fixture
        .store
        .active_transfer(&fixture.claim, fixture.file)?;
    staging.receive(&transfer, &mut std::io::Cursor::new(source))?;
    drop(staging);
    Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
    let worker = PreparedWorker::load(&config)?;
    assert_eq!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?[0]
            .state,
        "active"
    );
    let outcome = worker.run_with_test_login(&mut |_, _, _| {
        Err(Error::Internal(
            "sealed input recovery attempted a network login",
        ))
    })?;
    assert!(outcome.normalization_run > 0);
    assert!(outcome.publication_pending);
    assert!(
        fixture
            .store
            .transfer_summary(fixture.job, fixture.version)?
            .all_received
    );
    Ok(())
}

#[test]
fn worker_reconnect_attempts_are_bounded_and_terminal_login_errors_are_not_retried() -> TestResult {
    for transient in [false, true] {
        let fixture = Fixture::album(AudioFormat::Flac, 1000, 1, true)?;
        let scratch = tempdir()?;
        let secret = fixture._state.path().join("credentials.json");
        let config = fixture._state.path().join("worker.json");
        credentials(
            &secret,
            br#"{"username":"fixture-user","password":"fixture-password"}"#,
        )?;
        std::fs::write(
            &config,
            serde_json::to_vec(&configuration(&fixture, scratch.path(), &secret))?,
        )?;
        Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
        let worker = PreparedWorker::load(&config)?;
        let mut attempts = 0;
        let error = worker
            .run_with_test_login(&mut |_, username, _| {
                attempts += 1;
                assert_eq!(username, "fixture-user");
                if transient {
                    Err(Error::SoulseekTransport(
                        std::io::ErrorKind::ConnectionReset.into(),
                    ))
                } else {
                    Err(Error::Remote("Soulseek rejected the account password"))
                }
            })
            .err()
            .ok_or("worker unexpectedly succeeded")?;
        assert_eq!(attempts, if transient { 4 } else { 1 });
        if transient {
            assert!(error.to_string().contains("reconnect budget exhausted"));
        }
        let transfers =
            fixture
                .store
                .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
        assert!(transfers.iter().all(|transfer| transfer.state == "planned"));
    }
    Ok(())
}
