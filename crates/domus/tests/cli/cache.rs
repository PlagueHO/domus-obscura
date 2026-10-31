use super::*;
use domus::application::services::cache::{Cache, CacheKey};
use domus::application::{add_location, scan_location};
use domus::domain::{EditionSelection, OutputSettings, PlannedTrack};
use domus::infrastructure::cache::{CacheLimits, FileCache};
use domus::infrastructure::catalog::CatalogQuery;
use domus::infrastructure::store::Store;
use domus::infrastructure::worker::{PreparedFolder, PreparedSearch};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::PathBuf;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const RELEASE: &str = "20000000-0000-4000-8000-000000000001";

struct Fixture {
    state: tempfile::TempDir,
    _media: tempfile::TempDir,
    _files: tempfile::TempDir,
    database: PathBuf,
    host: PathBuf,
    root: PathBuf,
    request: PathBuf,
    source: PathBuf,
    cache: FileCache,
    key: CacheKey,
    body: Vec<u8>,
    now: u64,
    job: i64,
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let state = tempdir()?;
        let media = tempdir()?;
        let files = tempdir()?;
        let root = files.path().join("cache");
        std::fs::create_dir(&root)?;
        private::directory(&root)?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let location = add_location(&store, media.path(), "main")?;
        scan_location(&mut store, location)?;
        store.save_settings(None, &OutputSettings::default())?;
        let job = store.enqueue(1)?;
        support::catalog_snapshot(
            &database,
            &EditionSelection {
                release_id: RELEASE.into(),
                album_artist: "Generated artist".into(),
                album: "Legacy album".into(),
                bucket: None,
                tracks: vec![PlannedTrack {
                    artist: "Generated artist".into(),
                    title: "Generated track".into(),
                    disc: None,
                    disc_track: None,
                }],
            },
        )?;
        let raw = Connection::open(&database)?;
        let legacy: Vec<u8> =
            raw.query_row("SELECT body FROM catalog_cache", [], |row| row.get(0))?;
        let mut value: Value = serde_json::from_slice(&legacy)?;
        value["title"] = json!("File album");
        let body = serde_json::to_vec(&value)?;
        let now =
            u64::try_from(raw.query_row("SELECT unixepoch()", [], |row| row.get::<_, i64>(0))?)?;
        raw.execute(
            "UPDATE catalog_gate SET delay_seconds=4294967295,next_request_ms=9223372036854775807 WHERE id=1",
            [],
        )?;
        let cache = FileCache::open(&mut store, &root, CacheLimits::default())?;
        let key = CacheKey::new(
            "musicbrainz",
            "responses-v1",
            CatalogQuery::release(RELEASE)?.source(),
        )?;
        cache.put(&key, &body, now, now + 86400)?;
        let host = state.path().join("host.json");
        std::fs::write(
            &host,
            serde_json::to_vec(&json!({"database":database,"cache":{"root":root}}))?,
        )?;
        let request = state.path().join("plan.json");
        std::fs::write(
            &request,
            serde_json::to_vec(&json!({
                "job_id":job,"expected_version":1,"settings_version":1,"locations":[location],
                "accepts_incomplete_coverage":false,"bucket":null
            }))?,
        )?;
        let credentials = state.path().join("credentials.json");
        std::fs::write(
            &credentials,
            br#"{"username":"generated-peer","password":"generated-fixture"}"#,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
        }
        let source = state.path().join("source.json");
        std::fs::write(
            &source,
            serde_json::to_vec(&json!({
                "database":database,"credentials_file":credentials,"server":"8.8.8.8:2242",
                "release_id":RELEASE,"acknowledge_plaintext_login":true
            }))?,
        )?;
        Ok(Self {
            state,
            _media: media,
            _files: files,
            database,
            host,
            root,
            request,
            source,
            cache,
            key,
            body,
            now,
            job,
        })
    }

    fn cli(&self, command: &str, rest: &[&str]) -> TestResult<std::process::Output> {
        let mut arguments = vec![command, "--host", self.host.to_str().ok_or("host path")?];
        arguments.extend_from_slice(rest);
        Ok(run(&arguments)?)
    }

    fn plan(&self) -> TestResult<Value> {
        let output = self.cli(
            "catalog-plan-prepare",
            &[RELEASE, self.request.to_str().ok_or("request")?],
        )?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn gate(&self) -> TestResult<(i64, i64, i64)> {
        Ok(Connection::open(&self.database)?.query_row(
            "SELECT generation,delay_seconds,failures FROM catalog_gate WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?)
    }

    fn entry(&self) -> PathBuf {
        let digest: String = self
            .key
            .digest()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.root.join(format!("{digest}.entry"))
    }

    fn set_host(&self, value: Value) -> TestResult {
        std::fs::write(&self.host, serde_json::to_vec(&value)?)?;
        Ok(())
    }
}

fn failed(output: std::process::Output, message: &str) -> TestResult {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr)?;
    assert!(
        error.contains(message),
        "expected {message:?}, got {error:?}"
    );
    Ok(())
}

#[test]
fn configured_cli_catalog_hit_and_plan_use_only_selected_cache_during_pause() -> TestResult {
    let fixture = Fixture::new()?;
    let gate = fixture.gate()?;
    let raw = Connection::open(&fixture.database)?;
    let accessed_at: i64 =
        raw.query_row("SELECT accessed_at_ms FROM catalog_cache", [], |row| {
            row.get(0)
        })?;
    let output = fixture.cli("catalog-release", &[RELEASE])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let release: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(release["data"]["summary"]["title"], "File album");
    assert_eq!(release["cached"], true);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM job_plans", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    assert_eq!(
        raw.query_row("SELECT accessed_at_ms FROM catalog_cache", [], |row| row
            .get::<_, i64>(0))?,
        accessed_at
    );
    let plan = fixture.plan()?;
    assert_eq!(plan["plan"]["edition"]["album"], "File album");
    assert_eq!(fixture.gate()?, gate);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM catalog_cache", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    let legacy = run(&[
        "catalog-release",
        fixture.database.to_str().ok_or("database")?,
        RELEASE,
    ])?;
    assert!(legacy.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&legacy.stdout)?["data"]["summary"]["title"],
        "Legacy album"
    );
    Ok(())
}

#[test]
fn configured_cli_miss_expiry_and_corruption_never_use_valid_sqlite_rows_or_clear_pause()
-> TestResult {
    let fixture = Fixture::new()?;
    let gate = fixture.gate()?;
    fixture.cache.invalidate(&fixture.key)?;
    failed(fixture.cli("catalog-release", &[RELEASE])?, "paused")?;
    failed(
        fixture.cli(
            "catalog-plan-prepare",
            &[RELEASE, fixture.request.to_str().ok_or("request")?],
        )?,
        "load the exact release",
    )?;
    fixture.cache.put(
        &fixture.key,
        &fixture.body,
        fixture.now - 86401,
        fixture.now - 1,
    )?;
    failed(fixture.cli("catalog-release", &[RELEASE])?, "paused")?;
    fixture.cache.put(
        &fixture.key,
        &fixture.body,
        fixture.now,
        fixture.now + 86400,
    )?;
    let mut bytes = std::fs::read(fixture.entry())?;
    *bytes.last_mut().ok_or("entry bytes")? ^= 1;
    std::fs::write(fixture.entry(), bytes)?;
    failed(fixture.cli("catalog-release", &[RELEASE])?, "checksum")?;
    failed(
        fixture.cli(
            "catalog-plan-prepare",
            &[RELEASE, fixture.request.to_str().ok_or("request")?],
        )?,
        "checksum",
    )?;
    assert_eq!(fixture.gate()?, gate);
    assert_eq!(
        Connection::open(&fixture.database)?.query_row(
            "SELECT count(*) FROM job_plans",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    Ok(())
}

#[test]
fn configured_cli_omitted_cache_uses_legacy_and_disabled_providers_stay_disabled() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.set_host(json!({"database":fixture.database}))?;
    let output = fixture.cli("catalog-release", &[RELEASE])?;
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?["data"]["summary"]["title"],
        "Legacy album"
    );
    fixture.set_host(
        json!({"database":fixture.database,"cache":{"root":fixture.root},
        "services":{"metadata":[],"artwork":[],"acquisition":[]}}),
    )?;
    failed(fixture.cli("catalog-release", &[RELEASE])?, "disabled")?;
    failed(
        fixture.cli(
            "catalog-plan-prepare",
            &[RELEASE, fixture.request.to_str().ok_or("request")?],
        )?,
        "disabled",
    )?;
    failed(
        fixture.cli(
            "artwork-caa-prepare",
            &["1", "2", "1", "null", "--permitted-caa-cover"],
        )?,
        "disabled",
    )?;
    assert!(PreparedSearch::load_with_host(&fixture.source, &fixture.host).is_err());
    Ok(())
}

#[test]
fn configured_cli_storage_failure_and_miss_preserve_long_provider_cooldown() -> TestResult {
    let fixture = Fixture::new()?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute(
        "UPDATE catalog_gate SET delay_seconds=120,failures=3,next_request_ms=unixepoch()*1000+120000 WHERE id=1",
        [],
    )?;
    let gate = fixture.gate()?;
    assert!(fixture.cli("catalog-release", &[RELEASE])?.status.success());
    fixture.cache.invalidate(&fixture.key)?;
    failed(
        fixture.cli("catalog-release", &[RELEASE])?,
        "admission wait ended",
    )?;
    std::fs::create_dir(fixture.entry())?;
    let output = fixture.cli("catalog-release", &[RELEASE])?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8(output.stderr)?.contains("admission"));
    assert_eq!(fixture.gate()?, gate);
    assert!(fixture.entry().is_dir());
    Ok(())
}

#[test]
fn configured_cli_cache_open_never_binds_repairs_or_cleans_read_state() -> TestResult {
    let fixture = Fixture::new()?;
    let raw = Connection::open(&fixture.database)?;
    let before: i64 = raw.query_row("SELECT count(*) FROM private_roots", [], |row| row.get(0))?;
    let pending = fixture.root.join("pending.cache");
    std::fs::write(&pending, b"retained pending bytes")?;
    assert!(fixture.cli("catalog-release", &[RELEASE])?.status.success());
    assert_eq!(std::fs::read(&pending)?, b"retained pending bytes");
    let unbound = fixture._files.path().join("unbound");
    std::fs::create_dir(&unbound)?;
    private::directory(&unbound)?;
    fixture.set_host(json!({"database":fixture.database,"cache":{"root":unbound}}))?;
    failed(fixture.cli("catalog-release", &[RELEASE])?, "binding")?;
    assert_eq!(std::fs::read_dir(&unbound)?.count(), 0);
    std::fs::copy(fixture.root.join("owner.lock"), unbound.join("owner.lock"))?;
    failed(fixture.cli("catalog-release", &[RELEASE])?, "binding")?;
    assert_eq!(
        raw.query_row("SELECT count(*) FROM private_roots", [], |row| row
            .get::<_, i64>(0))?,
        before
    );
    let missing = fixture._files.path().join("missing");
    fixture.set_host(json!({"database":fixture.database,"cache":{"root":missing}}))?;
    assert!(!fixture.cli("catalog-release", &[RELEASE])?.status.success());
    assert!(!missing.exists());
    fixture.set_host(json!({"database":fixture.database,"cache":{"root":fixture.root}}))?;
    drop(fixture.cache);
    std::fs::remove_file(fixture.root.join("owner.lock"))?;
    failed(
        run(&[
            "catalog-release",
            "--host",
            fixture.host.to_str().ok_or("host")?,
            RELEASE,
        ])?,
        "marker is missing",
    )?;
    assert!(!fixture.root.join("owner.lock").exists());
    Ok(())
}

#[test]
fn configured_cli_rejects_database_alias_inside_bound_cache() -> TestResult {
    let fixture = Fixture::new()?;
    let alias = fixture.root.join("state.db");
    std::fs::hard_link(&fixture.database, &alias)?;
    let database_bytes = std::fs::read(&alias)?;
    fixture.set_host(json!({"database":alias,"cache":{"root":fixture.root}}))?;
    failed(
        fixture.cli("catalog-release", &[RELEASE])?,
        "application database",
    )?;
    assert_eq!(std::fs::read(&alias)?, database_bytes);
    assert_eq!(fixture.gate()?, (0, i64::from(u32::MAX), 0));
    Ok(())
}

#[test]
fn configured_cli_missing_and_legacy_databases_are_never_created_or_migrated() -> TestResult {
    let fixture = Fixture::new()?;
    let missing = fixture.state.path().join("missing.db");
    fixture.set_host(json!({"database":missing,"cache":{"root":fixture.root}}))?;
    assert!(!fixture.cli("catalog-release", &[RELEASE])?.status.success());
    assert!(!missing.exists());
    let old = Connection::open(&missing)?;
    old.execute_batch("PRAGMA user_version=20; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('retained');")?;
    for (command, args) in [
        ("catalog-release", vec![RELEASE]),
        (
            "catalog-plan-prepare",
            vec![RELEASE, fixture.request.to_str().ok_or("request")?],
        ),
        (
            "artwork-caa-prepare",
            vec!["1", "2", "1", "null", "--permitted-caa-cover"],
        ),
    ] {
        assert!(!fixture.cli(command, &args)?.status.success());
    }
    assert_eq!(
        old.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        20
    );
    assert_eq!(
        old.query_row("SELECT value FROM sentinel", [], |row| row
            .get::<_, String>(0))?,
        "retained"
    );
    Ok(())
}

#[test]
fn configured_cli_protects_host_requests_and_credentials_from_media_and_cache_overlap() -> TestResult
{
    let fixture = Fixture::new()?;
    let copied_host = fixture.root.join("host.json");
    std::fs::copy(&fixture.host, &copied_host)?;
    failed(
        run(&[
            "catalog-release",
            "--host",
            copied_host.to_str().ok_or("host")?,
            RELEASE,
        ])?,
        "overlap",
    )?;
    std::fs::remove_file(&copied_host)?;
    let copied_request = fixture._media.path().join("request.json");
    std::fs::copy(&fixture.request, &copied_request)?;
    failed(
        fixture.cli(
            "catalog-plan-prepare",
            &[RELEASE, copied_request.to_str().ok_or("request")?],
        )?,
        "outside all media roots",
    )?;
    let cache_request = fixture.root.join("request.json");
    std::fs::copy(&fixture.request, &cache_request)?;
    failed(
        fixture.cli(
            "catalog-plan-prepare",
            &[RELEASE, cache_request.to_str().ok_or("request")?],
        )?,
        "overlap",
    )?;
    std::fs::remove_file(cache_request)?;
    let credentials = fixture.root.join("credentials.json");
    std::fs::write(&credentials, b"never-echo-private-credentials")?;
    let mut source: Value = serde_json::from_slice(&std::fs::read(&fixture.source)?)?;
    source["credentials_file"] = json!(credentials);
    std::fs::write(&fixture.source, serde_json::to_vec(&source)?)?;
    failed(
        fixture.cli("source-search", &[fixture.source.to_str().ok_or("source")?])?,
        "overlap",
    )?;
    assert_eq!(
        std::fs::read(credentials)?,
        b"never-echo-private-credentials"
    );
    Ok(())
}

#[test]
fn configured_source_loading_uses_shared_metadata_without_network_or_approval() -> TestResult {
    let fixture = Fixture::new()?;
    let gate = fixture.gate()?;
    drop(PreparedSearch::load_with_host(
        &fixture.source,
        &fixture.host,
    )?);
    let mut source: Value = serde_json::from_slice(&std::fs::read(&fixture.source)?)?;
    source["folder"] =
        json!({"peer":"generated-peer","remote_folder":"Peer\\Album","format":"mp3"});
    std::fs::write(&fixture.source, serde_json::to_vec(&source)?)?;
    drop(PreparedFolder::load_with_host(
        &fixture.source,
        &fixture.host,
    )?);
    fixture.cache.invalidate(&fixture.key)?;
    assert!(
        matches!(PreparedFolder::load_with_host(&fixture.source, &fixture.host),
        Err(domus::Error::Conflict(message)) if message.contains("selected catalog cache"))
    );
    fixture.cache.put(
        &fixture.key,
        &fixture.body,
        fixture.now - 86401,
        fixture.now - 1,
    )?;
    assert!(
        matches!(PreparedFolder::load_with_host(&fixture.source, &fixture.host),
        Err(domus::Error::Conflict(message)) if message.contains("selected catalog cache"))
    );
    fixture.cache.put(
        &fixture.key,
        b"invalid generated metadata",
        fixture.now,
        fixture.now + 86400,
    )?;
    assert!(PreparedFolder::load_with_host(&fixture.source, &fixture.host).is_err());
    fixture.cache.put(
        &fixture.key,
        &fixture.body,
        fixture.now,
        fixture.now + 86400,
    )?;
    let mut bytes = std::fs::read(fixture.entry())?;
    *bytes.last_mut().ok_or("entry bytes")? ^= 1;
    std::fs::write(fixture.entry(), bytes)?;
    assert!(
        matches!(PreparedFolder::load_with_host(&fixture.source, &fixture.host),
        Err(domus::Error::Conflict(message)) if message.contains("checksum"))
    );
    assert_eq!(fixture.gate()?, gate);
    let raw = Connection::open(&fixture.database)?;
    assert_eq!(
        raw.query_row("SELECT count(*) FROM job_plans", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM jobs WHERE owner IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    assert_eq!(std::fs::read_dir(fixture._media.path())?.count(), 0);
    Ok(())
}

#[test]
fn configured_source_mismatch_and_disabled_selection_fail_before_credentials() -> TestResult {
    let fixture = Fixture::new()?;
    let other = fixture.state.path().join("other.db");
    drop(Store::open(&other)?);
    let mut source: Value = serde_json::from_slice(&std::fs::read(&fixture.source)?)?;
    let alias = fixture.state.path().join("alias.db");
    std::fs::hard_link(&fixture.database, &alias)?;
    source["database"] = json!(alias);
    std::fs::write(&fixture.source, serde_json::to_vec(&source)?)?;
    drop(PreparedSearch::load_with_host(
        &fixture.source,
        &fixture.host,
    )?);
    source["database"] = json!(other);
    source["credentials_file"] = json!(fixture.state.path().join("missing-credentials"));
    std::fs::write(&fixture.source, serde_json::to_vec(&source)?)?;
    failed(
        fixture.cli("source-search", &[fixture.source.to_str().ok_or("source")?])?,
        "same database",
    )?;
    source["services"] = json!({"acquisition":[]});
    std::fs::write(&fixture.source, serde_json::to_vec(&source)?)?;
    failed(
        fixture.cli("source-search", &[fixture.source.to_str().ok_or("source")?])?,
        "disabled",
    )?;
    source["services"] = json!({});
    std::fs::write(&fixture.source, serde_json::to_vec(&source)?)?;
    fixture.set_host(json!({"database":fixture.database,"services":{"acquisition":[]}}))?;
    failed(
        fixture.cli("source-search", &[fixture.source.to_str().ok_or("source")?])?,
        "disabled",
    )?;
    Ok(())
}

#[test]
fn configured_caa_preparation_freezes_evidence_and_survives_cache_eviction() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.plan()?;
    let mut store = Store::open_current_existing(&fixture.database)?;
    store.approve_plan(fixture.job, 2)?;
    let listing_source = format!("https://coverartarchive.org/release/{RELEASE}/");
    let image_source = format!("{listing_source}1");
    let listing = serde_json::to_vec(&json!({
        "release":format!("https://musicbrainz.org/release/{RELEASE}"),
        "images":[{"id":"1","edit":1,"types":["Front"],"front":true,"back":false,
            "approved":false,"comment":"","image":format!("{image_source}.png")}]
    }))?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(32, 16).write_to(&mut png, image::ImageFormat::Png)?;
    let mut keys = vec![fixture.key.clone()];
    for (key, body, source) in [
        (listing_source.clone(), listing, listing_source),
        (
            format!("{image_source}#mbid-{RELEASE}-1.png"),
            png.into_inner(),
            image_source,
        ),
    ] {
        let sources = serde_json::to_vec(&vec![source])?;
        let mut bytes = b"DOMCAA1\n".to_vec();
        bytes.extend_from_slice(&(sources.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&sources);
        bytes.extend_from_slice(&body);
        let key = CacheKey::new("cover-art-archive", "responses-v1", &key)?;
        fixture
            .cache
            .put(&key, &bytes, fixture.now, fixture.now + 86400)?;
        keys.push(key);
    }
    let job = fixture.job.to_string();
    let args = [&*job, "2", "1", "null", "--permitted-caa-cover"];
    let image_key = keys.last().ok_or("image key")?;
    fixture.cache.invalidate(image_key)?;
    failed(fixture.cli("artwork-caa-prepare", &args)?, "fetch")?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(32, 16).write_to(&mut png, image::ImageFormat::Png)?;
    let sources = serde_json::to_vec(&vec![format!(
        "https://coverartarchive.org/release/{RELEASE}/1"
    )])?;
    let mut envelope = b"DOMCAA1\n".to_vec();
    envelope.extend_from_slice(&(sources.len() as u32).to_le_bytes());
    envelope.extend_from_slice(&sources);
    envelope.extend_from_slice(&png.into_inner());
    fixture
        .cache
        .put(image_key, &envelope, fixture.now - 86401, fixture.now - 1)?;
    failed(fixture.cli("artwork-caa-prepare", &args)?, "fetch")?;
    fixture.cache.put(
        image_key,
        b"corrupt generated envelope",
        fixture.now,
        fixture.now + 86400,
    )?;
    failed(fixture.cli("artwork-caa-prepare", &args)?, "invalid")?;
    assert_eq!(
        Connection::open(&fixture.database)?.query_row(
            "SELECT count(*) FROM questions",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    fixture
        .cache
        .put(image_key, &envelope, fixture.now, fixture.now + 86400)?;
    let output = fixture.cli("artwork-caa-prepare", &args)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let candidate: Value = serde_json::from_slice(&output.stdout)?;
    let id = candidate["id"].as_i64().ok_or("candidate")?;
    assert!(candidate["approved_at"].is_null());
    let hash = candidate["sha256"].as_str().ok_or("preview hash")?;
    store.approve_artwork(fixture.job, 2, id, hash)?;
    for key in keys {
        fixture.cache.invalidate(&key)?;
    }
    assert!(store.approved_artwork(fixture.job, 2).is_ok());
    let evidence = run(&[
        "artwork-evidence",
        fixture.database.to_str().ok_or("db")?,
        &job,
        "2",
        &id.to_string(),
    ])?;
    assert!(evidence.status.success());
    let output = fixture.cli("artwork-caa-prepare", &args)?;
    failed(output, "fresh exact-release CAA listing")?;
    assert!(store.approved_artwork(fixture.job, 2).is_ok());
    assert_eq!(
        Connection::open(&fixture.database)?.query_row(
            "SELECT count(*) FROM caa_cache",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    assert_eq!(fixture.gate()?, (0, i64::from(u32::MAX), 0));
    Ok(())
}
