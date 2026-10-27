use super::*;
use crate::infrastructure::worker::PreparedWorker;
use std::path::Path;

mod check;

#[test]
fn worker_acquisition_and_scan_claims_are_independent_and_waiting_work_stays_idle() -> TestResult {
    use crate::infrastructure::production_worker::WorkerOperation;
    let mut fixture = Fixture::album(AudioFormat::Flac, 1000, 1, true)?;
    let binding = "a".repeat(64);
    let request = fixture.store.enqueue_worker(
        WorkerOperation::Acquire {
            job_id: fixture.job,
            plan_version: fixture.version,
            source_candidate: None,
            acknowledge_new_only_publication: true,
        },
        64,
        &binding,
    )?;
    fixture.store.enqueue_worker(
        WorkerOperation::Scan {
            location_id: fixture.location,
        },
        64,
        &binding,
    )?;
    let host = fixture.store.start_worker_host("fixture", &binding)?;
    let acquisition = fixture
        .store
        .claim_worker_request(&host, "acquisition")?
        .ok_or("missing acquisition lane")?;
    let scan = fixture
        .store
        .claim_worker_request(&host, "scan")?
        .ok_or("missing independent scan lane")?;
    assert_eq!(fixture.store.worker_status(64)?.active, 2);
    fixture
        .store
        .finish_worker_request(&acquisition, "waiting", Some("saved decision"))?;
    fixture
        .store
        .finish_worker_request(&scan, "complete", None)?;
    assert!(
        fixture
            .store
            .claim_worker_request(&host, "acquisition")?
            .is_none()
    );
    assert_eq!(fixture.store.worker_status(64)?.unfinished, 1);
    fixture.store.retry_worker(request.id)?;
    assert!(
        fixture
            .store
            .claim_worker_request(&host, "acquisition")?
            .is_some()
    );
    Ok(())
}

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
    use crate::infrastructure::production_worker::WorkerOperation;
    let retained = WorkerOperation::Acquire {
        job_id: fixture.job,
        plan_version: fixture.version,
        source_candidate: None,
        acknowledge_new_only_publication: true,
    };
    assert!(
        fixture
            .store
            .publication_destinations(fixture.job, fixture.version)?
            .is_empty()
    );
    retained.validate_recovery(&fixture.store)?;
    assert!(
        WorkerOperation::Publish {
            job_id: fixture.job,
            plan_version: fixture.version,
            normalization_run: outcome.normalization_run,
            acknowledge_new_only_publication: true,
        }
        .validate(&fixture.store)
        .is_err(),
        "an explicit publication recovery still needs its journal"
    );
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    assert!(matches!(
        crate::infrastructure::worker::publish_worker(
            &fixture.database,
            scratch.path(),
            fixture.job,
            fixture.version,
            outcome.normalization_run,
            stop.clone(),
        ),
        Err(Error::WorkerStopping)
    ));
    assert!(
        fixture
            .store
            .publication_destinations(fixture.job, fixture.version)?
            .is_empty()
    );
    stop.store(false, std::sync::atomic::Ordering::Release);
    fixture.store = fixture.store.with_worker_stop(stop.clone());
    let publication_claim = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "copy-stop-fixture", 300)?
        .ok_or("missing publication claim")?
        .claim;
    {
        use crate::infrastructure::{publication::PublicationDirectory, staging::StagingRoot};
        let staging = StagingRoot::open(&mut fixture.store, scratch.path())?;
        let (normalized, _, _) = fixture
            .store
            .publication_source(&publication_claim, outcome.normalization_run)?;
        let source = staging.open_normalization(&normalized)?;
        let target = fixture
            .store
            .begin_publication(&publication_claim, outcome.normalization_run)?
            .pop()
            .ok_or("missing publication target")?;
        let directory = PublicationDirectory::open(&fixture.store, &target)?;
        fixture.store.bind_publication_album(
            &publication_claim,
            target.id,
            directory.identity(),
        )?;
        let target = fixture
            .store
            .check_publication(&publication_claim, target.id)?;
        let file = fixture
            .store
            .publication_files(&publication_claim, target.id)?
            .into_iter()
            .next()
            .ok_or("missing cover publication")?;
        assert_eq!(file.ordinal, 0);
        let mut input = source.publication_input(None, &|| Ok(()))?;
        let checks = std::cell::Cell::new(0);
        directory.copy(&file, &mut input, &|| {
            checks.set(checks.get() + 1);
            if checks.get() == 3 {
                stop.store(true, std::sync::atomic::Ordering::Release);
            }
            fixture
                .store
                .renew_claim(&publication_claim, 300)
                .map_err(std::io::Error::other)?;
            staging
                .check_store(&fixture.store)
                .map_err(std::io::Error::other)?;
            directory
                .check(&fixture.store, &target)
                .map_err(std::io::Error::other)
        })?;
        assert!(
            stop.load(std::sync::atomic::Ordering::Acquire),
            "stop must arrive during copy"
        );
        fixture
            .store
            .record_publication_file(&publication_claim, target.id, &file)?;
        assert!(
            fixture
                .store
                .publication_files(&publication_claim, target.id)?[0]
                .verified_at
                .is_some()
        );
        assert!(matches!(
            fixture.store.check_worker_stop(),
            Err(Error::WorkerStopping)
        ));
    }
    fixture.store.release_job_claim(&publication_claim)?;
    stop.store(false, std::sync::atomic::Ordering::Release);
    let published = crate::infrastructure::worker::publish_worker(
        &fixture.database,
        scratch.path(),
        fixture.job,
        fixture.version,
        outcome.normalization_run,
        stop,
    )?;
    assert_eq!(published.state, "complete");
    assert!(
        published
            .destinations
            .iter()
            .all(|target| target.verified_at.is_some())
    );
    Ok(())
}

#[test]
fn worker_account_ownership_excludes_search_and_folder_commands_until_drop() -> TestResult {
    use crate::infrastructure::worker::{PreparedFolder, PreparedSearch};
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
    let search = fixture._state.path().join("search.json");
    let folder = fixture._state.path().join("folder.json");
    let mut body = json!({
        "database":fixture.database,"credentials_file":secret,"server":"8.8.8.8:2242",
        "release_id":"20000000-0000-4000-8000-000000000001","acknowledge_plaintext_login":true
    });
    std::fs::write(&search, serde_json::to_vec(&body)?)?;
    body["folder"] = json!({"peer":"fixture-peer","remote_folder":"Peer\\Album","format":"flac"});
    std::fs::write(&folder, serde_json::to_vec(&body)?)?;
    assert!(matches!(PreparedSearch::load(&search), Err(Error::Busy(_))));
    assert!(matches!(PreparedFolder::load(&folder), Err(Error::Busy(_))));
    drop(worker);
    drop(PreparedSearch::load(&search)?);
    drop(PreparedFolder::load(&folder)?);
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
