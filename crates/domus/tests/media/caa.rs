use super::{AudioFormat, Connection, Fixture, Page, TestResult, cover_source, params};
use domus::infrastructure::store::Store;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::process::Command;

const RELEASE: &str = super::super::RELEASE;
const IMAGE: &str = "9007199254740995";

#[test]
fn schema_thirteen_upgrade_preserves_legacy_previews_without_fabricated_evidence() -> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    let candidate = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        IMAGE,
        &cover_source()?,
        None,
    )?;
    fixture.store.approve_artwork(
        fixture.job,
        fixture.version,
        candidate.id,
        &candidate.sha256,
    )?;
    let jpeg = fixture
        .store
        .artwork_jpeg(fixture.job, fixture.version, candidate.id)?;
    drop(fixture.store);
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(include_str!("../support/drop_sharing.sql"))?;
    raw.execute_batch("DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations;")?;
    raw.execute_batch(
        "DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; PRAGMA user_version=13;",
    )?;
    let reopened = Store::open_existing(&fixture.database)?;
    assert_eq!(
        reopened
            .approved_artwork(fixture.job, fixture.version)?
            .bytes(),
        jpeg.bytes()
    );
    assert!(
        reopened
            .artwork_caa_evidence(fixture.job, fixture.version, candidate.id)?
            .is_none()
    );
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        31
    );
    Ok(())
}

fn cached_cover(fixture: &Fixture, bytes: &[u8]) -> TestResult {
    let raw = serde_json::to_vec(&json!({
        "release":format!("https://musicbrainz.org/release/{RELEASE}"),
        "images":[{
            "id":IMAGE,"edit":321,"types":["Front"],"front":true,"back":false,
            "approved":false,"comment":"Generated fixture; not a rights grant",
            "image":format!("https://coverartarchive.org/release/{RELEASE}/{IMAGE}.png")
        }]
    }))?;
    let listing = format!("https://coverartarchive.org/release/{RELEASE}/");
    let image = format!("https://coverartarchive.org/release/{RELEASE}/{IMAGE}");
    let raw_db = Connection::open(&fixture.database)?;
    for (key, body, sources) in [
        (listing.clone(), raw, vec![listing]),
        (
            format!("{image}#mbid-{RELEASE}-{IMAGE}.png"),
            bytes.to_vec(),
            vec![
                image,
                format!("https://archive.org/download/mbid-{RELEASE}/mbid-{RELEASE}-{IMAGE}.png"),
            ],
        ),
    ] {
        raw_db.execute(
            "INSERT INTO caa_cache(cache_key,body,sha256,sources,fetched_at,expires_at,accessed_at_ms)
             VALUES (?1,?2,?3,?4,unixepoch(),unixepoch()+86400,unixepoch()*1000)
             ON CONFLICT(cache_key) DO UPDATE SET body=excluded.body,sha256=excluded.sha256,
             sources=excluded.sources,fetched_at=excluded.fetched_at,expires_at=excluded.expires_at",
            params![key,body,&Sha256::digest(&body)[..],serde_json::to_string(&sources)?],
        )?;
    }

    Ok(())
}

#[test]
fn shared_cache_caa_preview_retains_approval_and_normalization_after_complete_cache_eviction()
-> TestResult {
    use domus::application::services::cache::{Cache, CacheKey};
    use domus::infrastructure::cache::{CacheLimits, FileCache};
    use std::sync::Arc;
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, true, false)?;
    cached_cover(&fixture, &cover_source()?)?;
    let root = super::private::tempdir()?;
    let cache = Arc::new(FileCache::open(
        &mut fixture.store,
        root.path(),
        CacheLimits::default(),
    )?);
    let raw = Connection::open(&fixture.database)?;
    let mut statement =
        raw.prepare("SELECT cache_key,body,sources,fetched_at,expires_at FROM caa_cache")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    for row in rows {
        let (key, body, sources, fetched, expires) = row?;
        let mut envelope = b"DOMCAA1\n".to_vec();
        envelope.extend_from_slice(&(sources.len() as u32).to_le_bytes());
        envelope.extend_from_slice(sources.as_bytes());
        envelope.extend_from_slice(&body);
        cache.put(
            &CacheKey::new("cover-art-archive", "responses-v1", &key)?,
            &envelope,
            u64::try_from(fetched)?,
            u64::try_from(expires)?,
        )?;
    }
    drop(statement);
    raw.execute("DELETE FROM caa_cache", [])?;
    fixture.store = fixture.store.with_cache(cache.clone());
    let candidate = fixture.store.prepare_cached_caa_artwork(
        fixture.job,
        fixture.version,
        IMAGE,
        None,
        true,
    )?;
    assert!(candidate.approved_at.is_none());
    let duplicate = fixture.store.prepare_cached_caa_artwork(
        fixture.job,
        fixture.version,
        IMAGE,
        None,
        true,
    )?;
    assert_eq!(duplicate.id, candidate.id);
    let evidence = serde_json::to_value(fixture.store.artwork_caa_evidence(
        fixture.job,
        fixture.version,
        candidate.id,
    )?)?;
    assert_eq!(cache.evict_expired(u64::MAX, 64)?, 3);
    assert!(
        fixture
            .store
            .prepare_cached_caa_artwork(fixture.job, fixture.version, IMAGE, None, true)
            .is_err()
    );
    fixture.store.approve_artwork(
        fixture.job,
        fixture.version,
        candidate.id,
        &candidate.sha256,
    )?;
    let retained = Store::open_current_existing(&fixture.database)?;
    let cover = retained.approved_artwork(fixture.job, fixture.version)?;
    assert_eq!(
        format!("{:x}", Sha256::digest(cover.bytes())),
        candidate.sha256
    );
    assert_eq!(
        serde_json::to_value(retained.artwork_caa_evidence(
            fixture.job,
            fixture.version,
            candidate.id
        )?)?,
        evidence
    );
    let staging = fixture.staging.as_ref().ok_or("staging")?;
    domus::application::normalize_album(&mut fixture.store, &fixture.claim, staging, &cover)?;
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("publishing")
    );
    assert_eq!(
        raw.query_row(
            "SELECT generation FROM catalog_gate WHERE id=1",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    Ok(())
}

#[test]
fn cached_caa_preparation_resolves_a_saved_permission_question_with_frozen_evidence() -> TestResult
{
    use domus::domain::ArtworkIssue;
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    cached_cover(&fixture, &cover_source()?)?;
    Connection::open(&fixture.database)?
        .execute("UPDATE jobs SET lease_until=0 WHERE id=?1", [fixture.job])?;
    let result = fixture.store.prepare_cached_caa_artwork_or_ask(
        fixture.job,
        fixture.version,
        IMAGE,
        None,
        false,
    );
    let Err(domus::Error::ArtworkRequired(id)) = result else {
        return Err("expected permission question".into());
    };
    assert_eq!(
        fixture.store.artwork_decision(id)?.issue,
        ArtworkIssue::Permission
    );
    let candidate = fixture.store.prepare_cached_caa_artwork_or_ask(
        fixture.job,
        fixture.version,
        IMAGE,
        None,
        true,
    )?;
    assert!(
        fixture
            .store
            .artwork_caa_evidence(fixture.job, fixture.version, candidate.id)?
            .is_some()
    );
    assert!(
        fixture
            .store
            .answer(id, fixture.version, "Use approved cover")
            .is_err()
    );
    fixture.store.approve_artwork(
        fixture.job,
        fixture.version,
        candidate.id,
        &candidate.sha256,
    )?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute("DELETE FROM caa_cache", [])?;
    let mut reopened = Store::open_existing(&fixture.database)?;
    reopened.answer(id, fixture.version, "Use approved cover")?;
    assert_eq!(reopened.resume_artwork(id, fixture.version)?, "validating");
    assert_eq!(
        reopened.artwork_decision(id)?.sha256.as_deref(),
        Some(candidate.sha256.as_str())
    );
    assert_eq!(
        raw.query_row(
            "SELECT generation FROM catalog_gate WHERE id=1",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    assert!(
        raw.execute(
            "UPDATE artwork_question_resolutions SET sha256=printf('%064d',0)",
            []
        )
        .is_err()
    );
    assert!(
        raw.execute("UPDATE artwork_questions SET issue='missing'", [])
            .is_err()
    );
    Ok(())
}

#[test]
fn caa_preview_freezes_evidence_survives_eviction_and_requires_separate_approval() -> TestResult {
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let mut fixture = Fixture::build_inner(format, true, false)?;
        let input = cover_source()?;
        cached_cover(&fixture, &input)?;
        let candidate = fixture.store.prepare_cached_caa_artwork(
            fixture.job,
            fixture.version,
            IMAGE,
            None,
            true,
        )?;
        assert!(candidate.approved_at.is_none());
        let evidence = fixture
            .store
            .artwork_caa_evidence(fixture.job, fixture.version, candidate.id)?
            .ok_or("missing evidence")?;
        assert_eq!(evidence.release_id, RELEASE);
        assert_eq!(evidence.image.id, IMAGE);
        assert_eq!(evidence.image.edit_id, "321");
        assert!(!evidence.image.provider_approved);
        assert!(evidence.permitted_use_acknowledged);
        assert_eq!(evidence.image_sha256, candidate.input_sha256);
        assert_eq!(evidence.image_sources.len(), 2);
        assert!(
            fixture
                .store
                .approved_artwork(fixture.job, fixture.version)
                .is_err()
        );
        let duplicate = fixture.store.prepare_cached_caa_artwork(
            fixture.job,
            fixture.version,
            IMAGE,
            None,
            true,
        )?;
        assert_eq!(duplicate.id, candidate.id);
        let raw = Connection::open(&fixture.database)?;
        assert_eq!(
            raw.query_row("SELECT count(*) FROM artwork_caa_evidence", [], |row| row
                .get::<_, i64>(
                0
            ))?,
            1
        );
        for sql in [
            "UPDATE artwork_caa_evidence SET permitted_use_acknowledged=1",
            "DELETE FROM artwork_caa_evidence",
        ] {
            assert!(raw.execute(sql, []).is_err());
        }
        raw.execute("DELETE FROM caa_cache", [])?;
        let mut reopened = Store::open_existing(&fixture.database)?;
        assert_eq!(
            serde_json::to_value(reopened.artwork_caa_evidence(
                fixture.job,
                fixture.version,
                candidate.id
            )?)?,
            serde_json::to_value(Some(evidence))?
        );
        reopened.approve_artwork(
            fixture.job,
            fixture.version,
            candidate.id,
            &candidate.sha256,
        )?;
        let cover = reopened.approved_artwork(fixture.job, fixture.version)?;
        let staging = fixture.staging.as_ref().ok_or("staging")?;
        domus::application::normalize_album(&mut fixture.store, &fixture.claim, staging, &cover)?;
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
    }
    Ok(())
}

#[test]
fn caa_preview_rejects_missing_stale_corrupt_unlisted_or_unpermitted_inputs_without_rows()
-> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    assert!(
        fixture
            .store
            .prepare_cached_caa_artwork(fixture.job, fixture.version, IMAGE, None, true)
            .is_err()
    );
    for sql in [
        "UPDATE caa_cache SET fetched_at=unixepoch()-86401,expires_at=unixepoch()-1",
        "UPDATE caa_cache SET sha256=zeroblob(32)",
        "UPDATE caa_cache SET sources='[\"https://example.invalid/\"]'",
    ] {
        cached_cover(&fixture, &cover_source()?)?;
        Connection::open(&fixture.database)?.execute(sql, [])?;
        assert!(
            fixture
                .store
                .prepare_cached_caa_artwork(fixture.job, fixture.version, IMAGE, None, true)
                .is_err()
        );
    }
    cached_cover(&fixture, &cover_source()?)?;
    assert!(
        fixture
            .store
            .prepare_cached_caa_artwork(fixture.job, fixture.version, "123", None, true)
            .is_err()
    );
    assert!(
        fixture
            .store
            .prepare_cached_caa_artwork(fixture.job, fixture.version, IMAGE, None, false)
            .is_err()
    );
    assert!(
        fixture
            .store
            .prepare_cached_caa_artwork(fixture.job, fixture.version + 1, IMAGE, None, true)
            .is_err()
    );
    cached_cover(&fixture, b"not an image")?;
    assert!(
        fixture
            .store
            .prepare_cached_caa_artwork(fixture.job, fixture.version, IMAGE, None, true)
            .is_err()
    );
    assert!(
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    Ok(())
}

#[test]
fn caa_evidence_and_preview_rollback_together_and_retained_corruption_stops_reads() -> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    cached_cover(&fixture, &cover_source()?)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER fail_caa BEFORE INSERT ON artwork_caa_evidence BEGIN SELECT RAISE(ABORT,'injected'); END;"
    )?;
    assert!(
        fixture
            .store
            .prepare_cached_caa_artwork(fixture.job, fixture.version, IMAGE, None, true)
            .is_err()
    );
    assert!(
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM events WHERE event_type='artwork_prepared'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    raw.execute_batch("DROP TRIGGER fail_caa;")?;
    let candidate = fixture.store.prepare_cached_caa_artwork(
        fixture.job,
        fixture.version,
        IMAGE,
        None,
        true,
    )?;
    raw.execute_batch("DROP TRIGGER immutable_caa_evidence; UPDATE artwork_caa_evidence SET listing_sha256=zeroblob(32);")?;
    assert!(
        fixture
            .store
            .artwork_caa_evidence(fixture.job, fixture.version, candidate.id)
            .is_err()
    );
    assert!(
        fixture
            .store
            .artwork_jpeg(fixture.job, fixture.version, candidate.id)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approve_artwork(
                fixture.job,
                fixture.version,
                candidate.id,
                &candidate.sha256
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn caa_cli_uses_only_cached_evidence_and_never_infers_approval() -> TestResult {
    let fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    cached_cover(&fixture, &cover_source()?)?;
    let job = fixture.job.to_string();
    let version = fixture.version.to_string();
    let args = [
        "artwork-caa-prepare",
        fixture.database.to_str().ok_or("database path")?,
        &job,
        &version,
        IMAGE,
        "null",
    ];
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_domus"))
            .args(args)
            .output()
    };
    let host = fixture._state.path().join("host.json");
    let request = fixture._state.path().join("request.json");
    std::fs::write(
        &host,
        serde_json::to_vec(&json!({"database":fixture.database}))?,
    )?;
    std::fs::write(
        &request,
        serde_json::to_vec(&json!({
            "operation":"artwork_fetch","provider":"cover-art-archive",
            "release":{"provider":"musicbrainz","id":RELEASE},"image_id":IMAGE,
        }))?,
    )?;
    let raw = Connection::open(&fixture.database)?;
    let generation: i64 = raw.query_row(
        "SELECT generation FROM catalog_gate WHERE id=1",
        [],
        |row| row.get(0),
    )?;
    let fetched = run(&[
        "service-call",
        host.to_str().ok_or("host path")?,
        request.to_str().ok_or("request path")?,
    ])?;
    assert!(
        fetched.status.success(),
        "{}",
        String::from_utf8_lossy(&fetched.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&fetched.stdout)?;
    assert_eq!(result["operation"], "artwork_fetched");
    assert_eq!(result["image"]["bytes"], cover_source()?.len());
    assert_eq!(
        result["image"]["sha256"],
        format!("{:x}", Sha256::digest(cover_source()?))
    );
    assert!(result["image"].get("content").is_none());
    assert_eq!(
        raw.query_row(
            "SELECT generation FROM catalog_gate WHERE id=1",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        generation
    );
    assert!(!run(&args)?.status.success());
    let output = run(&[args.as_slice(), &["--permitted-caa-cover"]].concat())?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let candidate: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(candidate["approved_at"].is_null());
    let id = candidate["id"].as_i64().ok_or("candidate ID")?.to_string();
    let output = run(&["artwork-evidence", args[1], &job, &version, &id])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let evidence: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(evidence["release_id"], RELEASE);
    assert_eq!(evidence["image"]["approved"], serde_json::Value::Null);
    assert_eq!(evidence["image"]["provider_approved"], false);
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)
            .is_err()
    );
    Ok(())
}
