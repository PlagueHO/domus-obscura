use domus::Error;
use domus::domain::OutputSettings;
use domus::infrastructure::catalog::{CatalogQuery, retry_after};
use domus::infrastructure::store::Store;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[path = "catalog/filters.rs"]
mod filters;
#[path = "catalog/ownership.rs"]
mod ownership;
const ARTIST: &str = "10000000-0000-4000-8000-000000000001";
const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
const GROUP: &str = "30000000-0000-4000-8000-000000000001";

fn credit(name: &str) -> Value {
    json!({"artist": {"id": ARTIST, "name": name}, "name": name, "joinphrase": ""})
}

fn release() -> Value {
    json!({
        "id": RELEASE, "title": "Fixture: Exact Edition", "artist-credit": [credit("The Fixture")],
        "date": "2024-02", "country": null, "disambiguation": "two discs", "status": "Official",
        "release-group": {"id": GROUP,"primary-type":"Album","secondary-types":[]},
        "media": [
            {"position": 2, "format": "CD", "title": null, "track-count": 1, "tracks": [
                {"id": "40000000-0000-4000-8000-000000000002", "position": 1, "number": "B1",
                 "title": "Second <Track>", "length": null, "artist-credit": [credit("Guest")],
                 "recording": {"id": "50000000-0000-4000-8000-000000000002", "video": false}}
            ]},
            {"position": 1, "format": "CD", "title": "First", "track-count": 1, "tracks": [
                {"id": "40000000-0000-4000-8000-000000000001", "position": 1, "number": "A1",
                 "title": "First / Track", "length": 123456, "artist-credit": [credit("The Fixture")],
                 "recording": {"id": "50000000-0000-4000-8000-000000000001", "video": false}}
            ]}
        ]
    })
}

#[test]
fn shared_service_cli_uses_cached_provider_data_without_job_or_approval_effects() -> TestResult {
    use sha2::{Digest, Sha256};
    use std::process::Command;
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.db");
    let configuration = state.path().join("host.json");
    let request = state.path().join("request.json");
    drop(Store::open(&database)?);
    let raw = rusqlite::Connection::open(&database)?;
    for (query, data) in [
        (CatalogQuery::release(RELEASE)?, release()),
        (
            CatalogQuery::artists("Fixture", 0, 1)?,
            json!({"offset":0,"count":1,"artists":[{"id":ARTIST,"name":"The Fixture"}]}),
        ),
    ] {
        let body = serde_json::to_vec(&data)?;
        raw.execute(
            "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
             VALUES (?1,?2,?3,unixepoch(),unixepoch()+86400,unixepoch()*1000)",
            rusqlite::params![query.source(), body, Sha256::digest(&body).to_vec()],
        )?;
    }
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_domus"))
            .arg("service-call")
            .arg(&configuration)
            .arg(&request)
            .output()
    };
    std::fs::write(
        &configuration,
        serde_json::to_vec(&json!({"database":database}))?,
    )?;
    for (command, response_operation) in [
        (
            json!({"operation":"metadata","release":{"provider":"musicbrainz","id":RELEASE}}),
            "metadata",
        ),
        (
            json!({"operation":"discover","request":{"query":{"operation":"artists","text":"Fixture","offset":0,"limit":1}}}),
            "discovery",
        ),
    ] {
        std::fs::write(&request, serde_json::to_vec(&command)?)?;
        let output = run()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(result["operation"], response_operation);
        if response_operation == "metadata" {
            assert_eq!(result["release"]["entity"]["provider"], "musicbrainz");
            assert_eq!(result["release"]["tracks"][0]["title"], "First / Track");
            assert_eq!(result["release"]["tracks"][1]["disc"], 2);
        } else {
            assert_eq!(
                result["report"]["results"][0]["page"]["items"][0]["entity"]["id"],
                ARTIST
            );
            assert_eq!(result["report"]["failures"], json!([]));
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_domus"))
        .arg("catalog-release")
        .arg(&database)
        .arg(RELEASE)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(result["data"]["summary"]["id"], RELEASE);
    assert_eq!(result["cached"], true);
    std::fs::write(
        &configuration,
        serde_json::to_vec(&json!({"database":database,"services":{"discovery":[]}}))?,
    )?;
    let disabled = run()?;
    assert!(!disabled.status.success());
    assert!(String::from_utf8(disabled.stderr)?.contains("enabled discovery providers"));
    let effects: i64 = raw.query_row(
        "SELECT (SELECT COUNT(*) FROM jobs) + (SELECT COUNT(*) FROM acquisition_sources)
         + (SELECT COUNT(*) FROM transfer_files) + (SELECT COUNT(*) FROM events)",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(effects, 0);
    Ok(())
}

#[test]
fn catalog_plan_cli_uses_only_fresh_verified_cache_and_never_approves_or_starts_work() -> TestResult
{
    use sha2::{Digest, Sha256};
    use std::process::Command;
    let directory = tempfile::tempdir()?;
    let media = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    let request_path = directory.path().join("request.json");
    let mut store = Store::open(&database)?;
    let location = domus::application::add_location(&store, media.path(), "main")?;
    domus::application::scan_location(&mut store, location)?;
    let settings = store.save_settings(None, &OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let request = json!({
        "job_id":job,"expected_version":1,"settings_version":settings,
        "format":"mp3","locations":[location],"accepts_incomplete_coverage":false,
        "bucket":null
    });
    std::fs::write(&request_path, serde_json::to_vec(&request)?)?;
    let run = || -> std::io::Result<std::process::Output> {
        Command::new(env!("CARGO_BIN_EXE_domus"))
            .arg("catalog-plan-prepare")
            .arg(&database)
            .arg(RELEASE)
            .arg(&request_path)
            .output()
    };
    assert!(!run()?.status.success());
    assert!(store.plan(job, 2).is_err());
    let query = CatalogQuery::release(RELEASE)?;
    let body = serde_json::to_vec(&release())?;
    let raw = rusqlite::Connection::open(&database)?;
    // A retained provider response is an isolated test fixture, not live proof.
    raw.execute(
        "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
         VALUES (?1,?2,?3,unixepoch(),unixepoch()+86400,unixepoch()*1000)",
        rusqlite::params![query.source(), body, Sha256::digest(&body).to_vec()],
    )?;
    raw.execute(
        "UPDATE catalog_cache SET fetched_at=unixepoch()-1000, expires_at=unixepoch()-1",
        [],
    )?;
    assert!(!run()?.status.success());
    assert!(store.plan(job, 2).is_err());
    raw.execute(
        "UPDATE catalog_cache SET fetched_at=unixepoch(), expires_at=unixepoch()+86400, payload_sha256=zeroblob(32)",
        [],
    )?;
    assert!(!run()?.status.success());
    assert!(store.plan(job, 2).is_err());
    raw.execute(
        "UPDATE catalog_cache SET payload_sha256=?1",
        [Sha256::digest(&body).to_vec()],
    )?;
    let mut forged = request.clone();
    forged["edition"] = json!({"album":"Untrusted override"});
    std::fs::write(&request_path, serde_json::to_vec(&forged)?)?;
    assert!(!run()?.status.success());
    assert!(store.plan(job, 2).is_err());
    std::fs::write(&request_path, serde_json::to_vec(&request)?)?;
    let output = run()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: domus::infrastructure::store::StoredPlan = serde_json::from_slice(&output.stdout)?;
    assert_eq!(plan.approved_at, None);
    assert_eq!(plan.plan.edition.release_id, RELEASE);
    assert_eq!(plan.plan.edition.album, "Fixture: Exact Edition");
    assert_eq!(plan.plan.edition.tracks[0].title, "First / Track");
    assert_eq!(plan.plan.edition.tracks[1].disc, Some(2));
    assert_eq!(
        plan.plan.outputs[1].filename,
        "Guest - Fixture_ Exact Edition - 02 - Second _Track_.mp3"
    );
    assert!(plan.plan.catalog.ok_or("missing catalog evidence")?.cached);
    assert!(!run()?.status.success());
    assert_eq!(store.plan(job, 2)?.approved_at, None);
    assert_eq!(std::fs::read_dir(media.path())?.count(), 0);
    let effects: i64 = raw.query_row(
        "SELECT (SELECT COUNT(*) FROM acquisition_sources) + (SELECT COUNT(*) FROM transfer_files)",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(effects, 0);
    Ok(())
}

#[test]
fn catalog_artist_search_uses_name_alias_and_sort_name_default_fields() -> TestResult {
    for text in ["The Ramones", "Ramones", "  The Ramones  "] {
        let query = CatalogQuery::artists(text, 0, 24)?;
        let url = reqwest::Url::parse(query.source())?;
        let expression = url
            .query_pairs()
            .find(|(key, _)| key == "query")
            .ok_or("missing search query")?
            .1
            .into_owned();
        assert_eq!(expression, format!("\"{}\"", text.trim()));
        assert!(!expression.contains("artist:"));
    }
    Ok(())
}

#[test]
fn catalog_search_is_literal_bounded_and_fixed_authority() -> TestResult {
    let query = CatalogQuery::artists("A+B / \" OR name:*", 25, 25)?;
    let url = reqwest::Url::parse(query.source())?;
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("musicbrainz.org"));
    let pairs = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        pairs.get("query").map(|value| value.as_ref()),
        Some("\"A\\+B \\/ \\\" OR name\\:\\*\"")
    );
    assert!(CatalogQuery::artists("", 0, 25).is_err());
    assert!(CatalogQuery::artists("x\nname:y", 0, 25).is_err());
    assert!(CatalogQuery::artists(&"x".repeat(513), 0, 25).is_err());
    assert!(CatalogQuery::artists("valid", 0, 0).is_err());
    assert!(CatalogQuery::artists("valid", 0, 26).is_err());
    assert!(CatalogQuery::release("../release").is_err());
    assert!(CatalogQuery::release("abcdef12-1234-4abc-8def-123456789abc").is_ok());
    assert!(CatalogQuery::release("ABCDEF12-1234-4ABC-8DEF-123456789ABC").is_err());
    let query = CatalogQuery::release(RELEASE)?;
    let url = reqwest::Url::parse(query.source())?;
    assert_eq!(
        url.query_pairs()
            .find(|(name, _)| name == "inc")
            .map(|(_, value)| value.into_owned()),
        Some("recordings artist-credits release-groups".into())
    );
    Ok(())
}

#[test]
fn catalog_pages_preserve_labels_and_reject_inconsistent_results() -> TestResult {
    let query = CatalogQuery::artists("Fixture", 0, 2)?;
    let page = json!({"offset":0,"count":1,"artists":[{"id":ARTIST,"name":"Fixture <&>","sort-name":null}]});
    let parsed = query.parse_artists(&serde_json::to_vec(&page)?)?;
    assert_eq!(parsed.items[0].name, "Fixture <&>");
    assert_eq!(parsed.items[0].country, None);
    for bad in [
        json!({"offset":1,"count":1,"artists":[]}),
        json!({"offset":0,"count":1,"artists":[]}),
        json!({"offset":0,"count":0,"artists":[{"id":ARTIST,"name":"Fixture"}]}),
        json!({"offset":0,"count":2,"artists":[{"id":ARTIST,"name":"A"},{"id":ARTIST,"name":"B"}]}),
        json!({"offset":0,"count":1,"artists":[{"id":"../escape","name":"A"}]}),
    ] {
        assert!(query.parse_artists(&serde_json::to_vec(&bad)?).is_err());
    }
    let page = json!({"release-offset":0,"release-count":1,"releases":[release()]});
    let parsed =
        CatalogQuery::releases(ARTIST, 0, 25)?.parse_releases(&serde_json::to_vec(&page)?)?;
    assert_eq!(parsed.items[0].id, RELEASE);
    assert_eq!(parsed.items[0].date.as_deref(), Some("2024-02"));
    assert!(
        query
            .parse_artists(&vec![b' '; 2 * 1024 * 1024 + 1])
            .is_err()
    );
    Ok(())
}

#[test]
fn catalog_exact_release_keeps_ids_credits_order_and_optional_values() -> TestResult {
    let query = CatalogQuery::release(RELEASE)?;
    let mut raw = release();
    raw["media"][1]["tracks"][0]["artist-credit"] = json!([
        {"artist":{"id":ARTIST,"name":"Canonical"},"name":"Credited","joinphrase":" & "},
        {"artist":{"id":GROUP,"name":"Guest"},"joinphrase":""}
    ]);
    let parsed = query.parse_release(&serde_json::to_vec(&raw)?)?;
    assert_eq!(parsed.release_group_id, GROUP);
    assert_eq!(parsed.media[0].tracks[0].number, "A1");
    assert_ne!(
        parsed.media[0].tracks[0].track_id,
        parsed.media[0].tracks[0].recording_id
    );
    assert_eq!(parsed.media[1].tracks[0].duration_ms, None);
    let selection = parsed.selection(None)?;
    assert_eq!(selection.tracks[0].artist, "Credited & Guest");
    assert_eq!(selection.tracks[1].artist, "Guest");
    assert_eq!(selection.tracks[1].disc, Some(2));
    assert_eq!(selection.tracks[1].disc_track, Some(1));
    let outputs = selection.preview(&OutputSettings::default())?;
    assert!(outputs[1].filename.contains("02"));
    assert_eq!(selection.album, "Fixture: Exact Edition");
    assert_eq!(selection.tracks[0].title, "First / Track");
    assert!(
        CatalogQuery::release(GROUP)?
            .parse_release(&serde_json::to_vec(&raw)?)
            .is_err()
    );
    Ok(())
}

#[test]
fn catalog_incomplete_mappings_and_invalid_dates_are_not_silently_used() -> TestResult {
    let query = CatalogQuery::release(RELEASE)?;
    for date in [
        "2023-02-29",
        "2024-13",
        "2024-01-00",
        "2024-2",
        "0000",
        "2024-01-01T00:00",
    ] {
        let mut raw = release();
        raw["date"] = json!(date);
        assert!(query.parse_release(&serde_json::to_vec(&raw)?).is_err());
    }
    for date in ["2024", "2024-02", "2024-02-29"] {
        let mut raw = release();
        raw["date"] = json!(date);
        query.parse_release(&serde_json::to_vec(&raw)?)?;
    }
    let mut raw = release();
    raw["media"][0]["track-count"] = json!(2);
    assert!(query.parse_release(&serde_json::to_vec(&raw)?).is_err());
    let mut raw = release();
    raw["media"][0]["tracks"][0]["position"] = json!(2);
    assert!(query.parse_release(&serde_json::to_vec(&raw)?).is_err());
    let mut raw = release();
    raw["media"][0]["tracks"][0]
        .as_object_mut()
        .ok_or("track missing")?
        .remove("artist-credit");
    assert!(query.parse_release(&serde_json::to_vec(&raw)?).is_err());
    raw["media"][0]["tracks"][0]["recording"]["artist-credit"] =
        json!([credit("Recording Credit")]);
    assert_eq!(
        query.parse_release(&serde_json::to_vec(&raw)?)?.media[1].tracks[0].credits[0].name,
        "Recording Credit"
    );
    for field in ["pregap", "data-tracks"] {
        let mut raw = release();
        raw["media"][0][field] = if field == "pregap" {
            json!({"title":"Hidden"})
        } else {
            json!([{"title":"Data"}])
        };
        assert!(
            query
                .parse_release(&serde_json::to_vec(&raw)?)?
                .selection(None)
                .is_err()
        );
    }
    for video in [json!(true), Value::Null] {
        let mut raw = release();
        raw["media"][0]["tracks"][0]["recording"]["video"] = video;
        assert!(
            query
                .parse_release(&serde_json::to_vec(&raw)?)?
                .selection(None)
                .is_err()
        );
    }
    let mut raw = release();
    raw["media"][0]["data-track-count"] = json!(1);
    assert!(
        query
            .parse_release(&serde_json::to_vec(&raw)?)?
            .selection(None)
            .is_err()
    );
    Ok(())
}

#[test]
fn catalog_rate_lease_is_durable_fenced_and_cannot_be_skipped_on_restart() -> TestResult {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    let mut first = Store::open(&database)?;
    let mut second = Store::open_existing(&database)?;
    let old = first.claim_catalog_request()?;
    assert!(first.resume_catalog_requests(true).is_err());
    assert!(matches!(
        second.claim_catalog_request(),
        Err(Error::Catalog { .. })
    ));
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute(
        "UPDATE catalog_gate SET lease_until_ms = 0, next_request_ms = 0",
        [],
    )?;
    assert!(matches!(
        second.claim_catalog_request(),
        Err(Error::Catalog { .. })
    ));
    connection.execute("UPDATE catalog_gate SET generation=generation+1", [])?;
    assert!(matches!(
        first.complete_catalog_request(old, 0, None),
        Err(Error::Conflict(_))
    ));
    let current = second.claim_catalog_request()?;
    let completion = second.complete_catalog_request(current, 120, None)?;
    assert_eq!(completion.retry_after_seconds, 120);
    drop(first);
    drop(second);
    let mut reopened = Store::open_existing(&database)?;
    assert!(matches!(
        reopened.claim_catalog_request(),
        Err(Error::Catalog {
            retry_after_seconds: 119..=120,
            ..
        })
    ));
    Ok(())
}

#[test]
fn catalog_concurrent_connections_receive_one_network_permit() -> TestResult {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    Store::open(&database)?;
    let stores = (0..8)
        .map(|_| Store::open_existing(&database))
        .collect::<domus::Result<Vec<_>>>()?;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(stores.len()));
    let results = std::thread::scope(|scope| {
        let handles = stores
            .into_iter()
            .map(|mut store| {
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    store.claim_catalog_request()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().map_err(|_| "catalog fixture thread failed"))
            .collect::<Result<Vec<_>, _>>()
    })?;
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    for result in results.into_iter().filter(Result::is_err) {
        assert!(matches!(result, Err(Error::Catalog { .. })));
    }
    Ok(())
}

#[test]
fn catalog_retry_after_accepts_delta_and_http_date_without_rounding_down() -> TestResult {
    let now =
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_millis(500);
    assert_eq!(retry_after("120", now)?, 120);
    assert_eq!(retry_after(" \t120\t ", now)?, 120);
    assert_eq!(retry_after(&(u32::MAX - 1).to_string(), now)?, u32::MAX - 1);
    for value in [
        u32::MAX.to_string(),
        "9".repeat(129),
        "120, 600".into(),
        "-1".into(),
        "invalid".into(),
    ] {
        assert!(matches!(
            retry_after(&value, now),
            Err(Error::Catalog {
                retry_after_seconds: u32::MAX,
                ..
            })
        ));
    }
    let date = httpdate::fmt_http_date(now + Duration::from_millis(1500));
    assert_eq!(retry_after(&date, now)?, 2);
    let unsupported_date = httpdate::fmt_http_date(now + Duration::from_secs(u64::from(u32::MAX)));
    assert!(matches!(
        retry_after(&unsupported_date, now),
        Err(Error::Catalog {
            retry_after_seconds: u32::MAX,
            ..
        })
    ));
    assert_eq!(
        retry_after(&httpdate::fmt_http_date(now - Duration::from_secs(20)), now)?,
        0
    );
    assert!(retry_after("-1", now).is_err());
    assert!(retry_after("999999999999999999999", now).is_err());
    assert!(retry_after("private upstream description", now).is_err());
    Ok(())
}
