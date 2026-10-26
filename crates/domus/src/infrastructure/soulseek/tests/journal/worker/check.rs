use super::*;
use crate::infrastructure::worker::{PreparedFolder, PreparedSearch};

#[test]
fn reviewed_worker_selection_requires_saved_approval_and_records_exact_candidate_without_network()
-> TestResult {
    use crate::domain::sources::{SourceFileMetadata, SourceFolder};
    let mut fixture = Fixture::build(AudioFormat::Flac, 1000, 3, true, false)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute("UPDATE jobs SET lease_until=0", [])?;
    let plan = fixture.store.plan(fixture.job, fixture.version)?;
    let listing = SourceFolder {
        peer: "fixture-peer".into(),
        folder: "Peer\\Album".into(),
        directories: 1,
        files: plan
            .plan
            .edition
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| SourceFileMetadata {
                remote_path: format!("Peer\\Album\\{:02} - {}.flac", index + 1, track.title),
                expected_bytes: 1000,
                extension: "flac".into(),
                bitrate_kbps: None,
                duration_seconds: None,
                vbr: None,
                sample_rate: Some(44100),
                bit_depth: Some(16),
            })
            .collect(),
    };
    let candidate =
        fixture
            .store
            .prepare_source_candidate(fixture.job, fixture.version, listing)?;
    let scratch = tempdir()?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("reviewed-worker.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let mut body = configuration(&fixture, scratch.path(), &secret);
    body["source_candidate"] = json!(candidate.id);
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    let before = local_state(&raw)?;
    assert!(PreparedWorker::check(&config).is_err());
    assert!(PreparedWorker::load(&config).is_err());
    assert_eq!(local_state(&raw)?, before);
    fixture.store.approve_source_candidate(
        candidate.id,
        fixture.version,
        &candidate.review_sha256,
        true,
    )?;
    let before = local_state(&raw)?;
    let report = PreparedWorker::check(&config)?;
    assert!(report.source_will_be_recorded);
    assert!(!report.network_checked && !report.claim_acquired && !report.staging_checked);
    assert_eq!(local_state(&raw)?, before);
    for value in [json!(0), json!(candidate.id + 1)] {
        body["source_candidate"] = value;
        std::fs::write(&config, serde_json::to_vec(&body)?)?;
        assert!(PreparedWorker::check(&config).is_err());
        assert_eq!(local_state(&raw)?, before);
    }
    body["source_candidate"] = json!(candidate.id);
    body["source"] = source_selection();
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    assert!(PreparedWorker::check(&config).is_err());
    assert_eq!(local_state(&raw)?, before);
    body.as_object_mut()
        .ok_or("configuration")?
        .remove("source");
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    drop(PreparedWorker::load(&config)?);
    assert_eq!(
        fixture
            .store
            .acquisition_source_candidate(fixture.job, fixture.version)?,
        Some(candidate.id)
    );
    let files = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    assert_eq!(files.len(), 3);
    assert!(
        files
            .iter()
            .all(|file| file.state == "planned" && file.peer == "fixture-peer")
    );
    raw.execute("UPDATE jobs SET lease_until=0", [])?;
    assert!(PreparedWorker::check(&config).is_err());
    body.as_object_mut()
        .ok_or("configuration")?
        .remove("source_candidate");
    std::fs::write(&config, serde_json::to_vec(&body)?)?;
    assert!(!PreparedWorker::check(&config)?.source_will_be_recorded);
    drop(PreparedWorker::load(&config)?);
    assert_eq!(
        fixture
            .store
            .acquisition_source_candidate(fixture.job, fixture.version)?,
        Some(candidate.id)
    );
    Ok(())
}

#[test]
fn search_preparation_uses_exact_cached_release_private_credentials_and_no_job_effects()
-> TestResult {
    let fixture = Fixture::build(AudioFormat::Flac, 1000, 3, true, false)?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("search.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let original = json!({
        "database":fixture.database,"credentials_file":secret,
        "server":"8.8.8.8:2242","release_id":"20000000-0000-4000-8000-000000000001",
        "acknowledge_plaintext_login":true,
    });
    let raw = Connection::open(&fixture.database)?;
    let before = local_state(&raw)?;
    std::fs::write(&config, serde_json::to_vec(&original)?)?;
    drop(PreparedSearch::load(&config)?);
    for (key, value) in [
        ("services", json!({"acquisition":[]})),
        ("services", json!({"acquisition":["unavailable"]})),
        ("acknowledge_plaintext_login", json!(false)),
        ("server", json!("127.0.0.1:2242")),
        ("release_id", json!("20000000-0000-4000-8000-000000000002")),
        ("release_id", json!("invalid")),
        ("database", json!("relative.db")),
        ("unexpected", json!(true)),
    ] {
        let mut config_body = original.clone();
        config_body[key] = value;
        std::fs::write(&config, serde_json::to_vec(&config_body)?)?;
        assert!(PreparedSearch::load(&config).is_err());
        assert_eq!(local_state(&raw)?, before);
    }
    assert_eq!(local_state(&raw)?, before);
    Ok(())
}

#[test]
fn folder_preparation_requires_exact_cached_release_selected_peer_and_no_local_effects()
-> TestResult {
    let fixture = Fixture::build(AudioFormat::Flac, 1000, 3, true, false)?;
    let secret = fixture._state.path().join("credentials.json");
    let config = fixture._state.path().join("folder.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let original = json!({
        "database":fixture.database,"credentials_file":secret,"server":"8.8.8.8:2242",
        "release_id":"20000000-0000-4000-8000-000000000001","acknowledge_plaintext_login":true,
        "folder":{"peer":"fixture-peer","remote_folder":"Peer\\Album","format":"flac"},
    });
    let raw = Connection::open(&fixture.database)?;
    let before = local_state(&raw)?;
    std::fs::write(&config, serde_json::to_vec(&original)?)?;
    drop(PreparedFolder::load(&config)?);
    assert!(PreparedSearch::load(&config).is_err());
    for (key, value) in [
        ("services", json!({"acquisition":[]})),
        ("services", json!({"acquisition":["other"]})),
        ("acknowledge_plaintext_login", json!(false)),
        ("server", json!("127.0.0.1:2242")),
        ("release_id", json!("20000000-0000-4000-8000-000000000002")),
        ("database", json!("relative.db")),
        ("folder", json!(null)),
        (
            "folder",
            json!({"peer":"fixture-peer","remote_folder":"Peer\\..\\Album","format":"flac"}),
        ),
        (
            "folder",
            json!({"peer":"bad\npeer","remote_folder":"Peer\\Album","format":"flac"}),
        ),
        (
            "folder",
            json!({"peer":"fixture-peer","remote_folder":"Peer\\Album","format":"wav"}),
        ),
        (
            "folder",
            json!({"peer":"fixture-peer","remote_folder":"Peer\\Album","format":"flac","approve":true}),
        ),
    ] {
        let mut body = original.clone();
        body[key] = value;
        std::fs::write(&config, serde_json::to_vec(&body)?)?;
        assert!(PreparedFolder::load(&config).is_err());
        assert_eq!(local_state(&raw)?, before);
    }
    std::fs::write(&config, serde_json::to_vec(&original)?)?;
    raw.execute(
        "UPDATE catalog_cache SET expires_at=fetched_at-86399,fetched_at=fetched_at-86400",
        [],
    )?;
    assert!(PreparedFolder::load(&config).is_err());
    assert_eq!(local_state(&raw)?, before);
    Ok(())
}

fn local_state(raw: &Connection) -> rusqlite::Result<String> {
    raw.query_row(
        "SELECT json_object(
          'jobs',(SELECT json_group_array(json_object(
            'id',id,'state',state,'owner',owner,'lease',lease_until,'generation',claim_generation
          )) FROM jobs),
          'sources',(SELECT count(*) FROM acquisition_sources),
          'files',(SELECT count(*) FROM transfer_files),
          'events',(SELECT count(*) FROM events),
          'roots',(SELECT count(*) FROM private_roots),
          'native_roots',(SELECT count(*) FROM filesystem_roots))",
        [],
        |row| row.get(0),
    )
}

#[test]
fn worker_check_reads_new_and_committed_sources_without_claim_network_or_staging_effects()
-> TestResult {
    for selected in [false, true] {
        let fixture = Fixture::build(AudioFormat::Flac, 1000, 3, true, selected)?;
        let scratch = tempdir()?;
        let secret = fixture._state.path().join("credentials.json");
        let config = fixture._state.path().join("worker.json");
        credentials(
            &secret,
            br#"{"username":"fixture-user","password":"fixture-password"}"#,
        )?;
        let mut body = configuration(&fixture, scratch.path(), &secret);
        body["export_for_playback"] = json!(true);
        if !selected {
            body["source"] = source_selection();
        }
        std::fs::write(&config, serde_json::to_vec(&body)?)?;
        let original_config = std::fs::read(&config)?;
        let original_secret = std::fs::read(&secret)?;
        let raw = Connection::open(&fixture.database)?;
        let before = local_state(&raw)?;
        for _ in 0..2 {
            let report = serde_json::to_value(PreparedWorker::check(&config)?)?;
            assert_eq!(
                report,
                json!({
                    "job_id":fixture.job, "plan_version":fixture.version,
                    "job_state":if selected {"downloading"} else {"queued"},
                    "tracks":3, "received_tracks":0,
                    "source_will_be_recorded":!selected, "export_for_playback":true,
                    "network_checked":false, "staging_checked":false, "claim_acquired":false,
                })
            );
            let output = report.to_string();
            assert!(!output.contains("fixture-password"));
            assert!(!output.contains("fixture-user"));
            assert!(!output.contains("fixture-peer"));
        }
        assert_eq!(local_state(&raw)?, before);
        assert_eq!(scratch.path().read_dir()?.count(), 0);
        assert_eq!(std::fs::read(&config)?, original_config);
        assert_eq!(std::fs::read(&secret)?, original_secret);
        fixture.store.renew_claim(&fixture.claim, 300)?;
    }
    Ok(())
}

#[test]
fn worker_check_refuses_uncertainty_stale_observations_and_schema_without_mutation() -> TestResult {
    for fault in ["active", "submitting", "observations", "schema", "control"] {
        let mut fixture = Fixture::album(AudioFormat::Flac, 1000, 3, true)?;
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
        match fault {
            "active" | "submitting" => {
                fixture.store.start_transfer(&fixture.claim, fixture.file)?;
                if fault == "active" {
                    fixture
                        .store
                        .accept_transfer(&fixture.claim, fixture.file, 17)?;
                }
            }
            "observations" => {
                raw.execute("UPDATE locations SET coverage='incomplete'", [])?;
            }
            "schema" => raw.pragma_update(None, "user_version", 12)?,
            "control" => {
                fixture.store.request_job_control(
                    fixture.job,
                    fixture.version,
                    crate::infrastructure::store::JobControlAction::Pause,
                )?;
            }
            _ => unreachable!(),
        }
        let before = local_state(&raw)?;
        assert!(PreparedWorker::check(&config).is_err(), "fault: {fault}");
        assert_eq!(local_state(&raw)?, before);
        assert_eq!(scratch.path().read_dir()?.count(), 0);
        if fault == "schema" {
            assert_eq!(
                raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
                12
            );
        }
    }
    Ok(())
}

#[test]
fn worker_check_does_not_create_a_missing_database() -> TestResult {
    let state = tempdir()?;
    let database = state.path().join("missing.db");
    let config = state.path().join("worker.json");
    let secret = state.path().join("credentials.json");
    credentials(
        &secret,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "database":database,"staging_root":state.path().join("scratch"),
            "credentials_file":secret,"server":"8.8.8.8:2242",
            "job_id":1,"plan_version":2,"acknowledge_plaintext_login":true,
        }))?,
    )?;
    assert!(PreparedWorker::check(&config).is_err());
    assert!(!database.exists());
    assert!(!state.path().join("scratch").exists());
    Ok(())
}

#[test]
fn diagnostics_snapshot_rejects_writes_and_keeps_one_consistent_database_view() -> TestResult {
    let fixture = Fixture::build(AudioFormat::Flac, 1000, 3, true, false)?;
    let raw = Connection::open(&fixture.database)?;
    let mut snapshot = Store::open_read_snapshot(&fixture.database)?;
    assert_eq!(snapshot.job_state(fixture.job)?.as_deref(), Some("queued"));
    let before = local_state(&raw)?;
    assert!(snapshot.enqueue(1).is_err());
    assert_eq!(local_state(&raw)?, before);
    raw.execute(
        "UPDATE jobs SET state='searching' WHERE id=?1",
        [fixture.job],
    )?;
    assert_eq!(snapshot.job_state(fixture.job)?.as_deref(), Some("queued"));
    let newer = Store::open_read_snapshot(&fixture.database)?;
    assert_eq!(newer.job_state(fixture.job)?.as_deref(), Some("searching"));
    Ok(())
}
