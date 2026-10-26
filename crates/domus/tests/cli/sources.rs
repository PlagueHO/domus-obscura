use super::*;
use domus::application::{add_location, scan_location};
use domus::domain::{EditionSelection, OutputSettings, PlannedTrack};
use domus::infrastructure::store::{PlanRequest, Store};
use serde_json::{Value, json};
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn source_review_cli_imports_only_local_listing_and_requires_separate_exact_approval() -> TestResult
{
    let state = tempdir()?;
    let media = tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;
    store.save_settings(None, &OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let catalog = support::catalog_snapshot(
        &database,
        &EditionSelection {
            release_id: "12345678-1234-4321-9876-123456789abc".into(),
            album_artist: "Generated".into(),
            album: "Generated Album".into(),
            bucket: None,
            tracks: vec![PlannedTrack {
                artist: "Generated".into(),
                title: "First".into(),
                disc: None,
                disc_track: None,
            }],
        },
    )?;
    let plan = store.prepare_catalog_plan(
        PlanRequest {
            job_id: job,
            expected_version: 1,
            settings_version: 1,
            format: None,
            edition: catalog.data.selection(None)?,
            locations: vec![location],
            accepts_incomplete_coverage: false,
        },
        catalog,
    )?;
    store.approve_plan(job, plan.version)?;
    let db = database.to_str().ok_or("database")?;
    let job = job.to_string();
    let version = plan.version.to_string();
    let listing = state.path().join("listing.json");
    let path = listing.to_str().ok_or("listing path")?;
    let folder = json!({"peer":"generated-peer","folder":"Peer\\Album","directories":1,"files":[{
        "remote_path":"Peer\\Album\\First.mp3","expected_bytes":1000,"extension":"mp3",
        "bitrate_kbps":320,"vbr":false
    }]});
    for body in [
        json!({"listing":folder}),
        json!({"secret":"never-echo-private"}),
        json!({"folder":"never-echo-private"}),
    ] {
        std::fs::write(&listing, serde_json::to_vec(&body)?)?;
        let output = run(&["source-candidate-prepare", db, &job, &version, path])?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8(output.stderr)?.contains("never-echo-private"));
    }
    std::fs::write(&listing, serde_json::to_vec(&folder)?)?;
    let output = run(&["source-candidate-prepare", db, &job, &version, path])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let candidate: Value = serde_json::from_slice(&output.stdout)?;
    let id = candidate["id"].as_i64().ok_or("candidate")?.to_string();
    let hash = candidate["review_sha256"].as_str().ok_or("review digest")?;
    assert!(candidate["approved_at"].is_null());
    let output = run(&["source-candidate", db, &id])?;
    assert!(output.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout)?, candidate);
    let flag = "--reviewed-exact-edition-and-permitted-download";
    let invalid = run(&["source-candidate-approve", db, &id, &version, hash])?;
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    let invalid = run(&[
        "source-candidate-approve",
        db,
        &id,
        &version,
        &"0".repeat(64),
        flag,
    ])?;
    assert!(!invalid.status.success());
    let output = run(&["source-candidate-approve", db, &id, &version, hash, flag])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let approved: Value = serde_json::from_slice(&output.stdout)?;
    assert!(approved["approved_at"].is_number());
    assert_eq!(approved["assessment"], candidate["assessment"]);
    let raw = rusqlite::Connection::open(&database)?;
    assert_eq!(raw.query_row("SELECT (SELECT count(*) FROM transfer_files)+(SELECT count(*) FROM acquisition_sources)+(SELECT count(*) FROM jobs WHERE owner IS NOT NULL)",[],|row|row.get::<_,i64>(0))?,0);
    assert_eq!(std::fs::read_dir(media.path())?.count(), 0);
    let prohibited = media.path().join("listing.json");
    std::fs::write(&prohibited, serde_json::to_vec(&folder)?)?;
    let invalid = run(&[
        "source-candidate-prepare",
        db,
        &job,
        &version,
        prohibited.to_str().ok_or("media input")?,
    ])?;
    assert!(!invalid.status.success());
    Ok(())
}

#[test]
fn source_review_cli_never_creates_or_migrates_state_or_echoes_input() -> TestResult {
    let state = tempdir()?;
    let database = state.path().join("missing.db");
    let listing = state.path().join("listing.json");
    std::fs::write(&listing, br#"{"private":"never-echo-private"}"#)?;
    let db = database.to_str().ok_or("database")?;
    let path = listing.to_str().ok_or("input")?;
    for old in [false, true] {
        let raw = if old {
            let raw = rusqlite::Connection::open(&database)?;
            raw.execute_batch("PRAGMA user_version=17; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES ('unchanged');")?;
            Some(raw)
        } else {
            None
        };
        for args in [
            vec!["source-candidate-prepare", db, "1", "2", path],
            vec!["source-candidate", db, "1"],
            vec![
                "source-candidate-approve",
                db,
                "1",
                "2",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "--reviewed-exact-edition-and-permitted-download",
            ],
        ] {
            let output = run(&args)?;
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8(output.stderr)?.contains("never-echo-private"));
        }
        if let Some(raw) = raw {
            assert_eq!(
                raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
                17
            );
            assert_eq!(
                raw.query_row("SELECT value FROM sentinel", [], |row| row
                    .get::<_, String>(0))?,
                "unchanged"
            );
        } else {
            assert!(!database.exists());
        }
    }
    Ok(())
}
