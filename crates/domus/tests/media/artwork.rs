use super::{
    AudioFormat, Connection, Fixture, Page, PlanRequest, TestResult, cover_source, params,
};
use domus::application::normalize_album;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::io::Cursor;

fn other_cover() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(20, 40, Rgb([240, 100, 10])))
        .write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

#[test]
fn permitted_local_cover_preserves_provenance_approval_and_real_album_normalization() -> TestResult
{
    use sha2::{Digest, Sha256};
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let mut fixture = Fixture::build_inner(format, true, false)?;
        let input = cover_source()?;
        let path = fixture.scratch.path().join("local-cover.png");
        std::fs::write(&path, &input)?;
        let candidate =
            fixture
                .store
                .prepare_local_artwork(fixture.job, fixture.version, &path, None, true)?;
        assert_eq!(
            candidate.source,
            format!(
                "urn:domus:local-artwork:sha256:{:x}",
                Sha256::digest(&input),
            )
        );
        assert_eq!(
            candidate.input_sha256,
            format!("{:x}", Sha256::digest(&input))
        );
        assert!(candidate.approved_at.is_none());
        assert!(!candidate.source.contains("coverartarchive"));
        assert!(
            !candidate
                .source
                .contains(&path.to_string_lossy().to_string())
        );
        assert!(
            fixture
                .store
                .approved_artwork(fixture.job, fixture.version)
                .is_err()
        );
        let duplicate =
            fixture
                .store
                .prepare_local_artwork(fixture.job, fixture.version, &path, None, true)?;
        assert_eq!(duplicate.id, candidate.id);
        let provider =
            fixture
                .store
                .prepare_artwork(fixture.job, fixture.version, "123", &input, None)?;
        assert_ne!(provider.id, candidate.id);
        assert_eq!(provider.sha256, candidate.sha256);
        assert!(
            fixture
                .store
                .approve_artwork(fixture.job, fixture.version, candidate.id, &"0".repeat(64),)
                .is_err()
        );
        fixture.store.approve_artwork(
            fixture.job,
            fixture.version,
            candidate.id,
            &candidate.sha256,
        )?;
        let cover = fixture
            .store
            .approved_artwork(fixture.job, fixture.version)?;
        let staging = fixture.staging.as_ref().ok_or("missing staging")?;
        normalize_album(&mut fixture.store, &fixture.claim, staging, &cover)?;
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
        assert_eq!(std::fs::read(&path)?, input);
        let reopened = domus::infrastructure::store::Store::open_existing(&fixture.database)?;
        assert_eq!(
            reopened
                .approved_artwork(fixture.job, fixture.version)?
                .bytes(),
            cover.bytes()
        );
    }
    Ok(())
}

#[test]
fn local_artwork_rejects_missing_permission_invalid_files_and_stale_plans_without_candidates()
-> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    let path = fixture.scratch.path().join("local-cover.png");
    std::fs::write(&path, cover_source()?)?;
    assert!(
        fixture
            .store
            .prepare_local_artwork(fixture.job, fixture.version, &path, None, false,)
            .is_err()
    );
    assert!(
        fixture
            .store
            .prepare_local_artwork(fixture.job, fixture.version + 1, &path, None, true,)
            .is_err()
    );
    assert!(
        fixture
            .store
            .prepare_local_artwork(
                fixture.job,
                fixture.version,
                std::path::Path::new("relative.png"),
                None,
                true,
            )
            .is_err()
    );
    for bytes in [b"invalid image".to_vec(), vec![0; 8 * 1024 * 1024 + 1]] {
        std::fs::write(&path, bytes)?;
        assert!(
            fixture
                .store
                .prepare_local_artwork(fixture.job, fixture.version, &path, None, true,)
                .is_err()
        );
    }
    assert!(
        fixture
            .store
            .prepare_artwork(
                fixture.job,
                fixture.version,
                "local",
                &cover_source()?,
                None,
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(0, 200)?,)?
            .is_empty()
    );
    Ok(())
}

#[test]
fn local_artwork_cli_requires_permission_and_separate_exact_hash_approval() -> TestResult {
    use std::process::Command;
    let fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    let image = fixture.scratch.path().join("permitted.png");
    std::fs::write(&image, cover_source()?)?;
    let database = fixture.database.to_str().ok_or("database path")?;
    let job = fixture.job.to_string();
    let version = fixture.version.to_string();
    let args = [
        "artwork-local-prepare",
        database,
        &job,
        &version,
        image.to_str().ok_or("image path")?,
        "null",
    ];
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_domus"))
            .args(args)
            .output()
    };
    assert!(!run(&args)?.status.success());
    let output = run(&[args.as_slice(), &["--permitted-local-cover"]].concat())?;
    assert!(output.status.success(), "{:?}", output.stderr);
    let candidate: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(candidate["approved_at"].is_null());
    let id = candidate["id"].as_i64().ok_or("candidate ID")?.to_string();
    let hash = candidate["sha256"].as_str().ok_or("candidate hash")?;
    let preview = run(&["artwork-preview", database, &job, &version, &id])?;
    assert!(preview.status.success(), "{:?}", preview.stderr);
    assert!(preview.stderr.is_empty());
    let retained = fixture.store.artwork_jpeg(
        fixture.job,
        fixture.version,
        candidate["id"].as_i64().ok_or("candidate ID")?,
    )?;
    assert_eq!(preview.stdout, retained.bytes());
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)
            .is_err()
    );
    let missing = run(&["artwork-preview", database, &job, &version, "999999"])?;
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(
        !run(&[
            "artwork-approve",
            database,
            &job,
            &version,
            &id,
            &"0".repeat(64)
        ])?
        .status
        .success()
    );
    let approved = run(&["artwork-approve", database, &job, &version, &id, hash])?;
    assert!(approved.status.success(), "{:?}", approved.stderr);
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)
            .is_ok()
    );
    Ok(())
}

#[test]
fn artwork_requires_explicit_exact_preview_approval_before_any_normalization_effect() -> TestResult
{
    let mut fixture = Fixture::build_inner(AudioFormat::Mp3, true, false)?;
    let staging = fixture.staging.as_ref().ok_or("missing staging")?;
    assert!(normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover).is_err());
    assert!(fixture.store.normalization_run(&fixture.claim)?.is_none());
    assert!(
        fixture
            .scratch
            .path()
            .read_dir()?
            .all(|entry| entry.is_ok_and(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with("normalization-")))
    );
    let first = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123",
        &cover_source()?,
        None,
    )?;
    let second = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "456",
        &other_cover()?,
        None,
    )?;
    assert!(
        fixture.store.jobs(Page::new(0, 10)?)?[0]
            .artwork_candidate_id
            .is_none()
    );
    assert_eq!(
        first.source,
        format!("https://coverartarchive.org/release/{}/123", super::RELEASE)
    );
    assert!(first.approved_at.is_none());
    assert_eq!((first.width, first.height), fixture.cover.dimensions());
    let bytes = fixture
        .store
        .artwork_jpeg(fixture.job, fixture.version, first.id)?;
    assert_eq!(bytes.bytes(), fixture.cover.bytes());
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approve_artwork(fixture.job, fixture.version, first.id, &second.sha256)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approve_artwork(fixture.job + 1, fixture.version, first.id, &first.sha256)
            .is_err()
    );
    assert!(
        fixture
            .store
            .begin_normalization(&fixture.claim, &fixture.cover)
            .is_err()
    );
    fixture
        .store
        .approve_artwork(fixture.job, fixture.version, first.id, &first.sha256)?;
    assert_eq!(
        fixture.store.jobs(Page::new(0, 10)?)?[0].artwork_candidate_id,
        Some(first.id)
    );
    fixture
        .store
        .approve_artwork(fixture.job, fixture.version, first.id, &first.sha256)?;
    assert!(
        fixture
            .store
            .approve_artwork(fixture.job, fixture.version, second.id, &second.sha256)
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)?
            .bytes(),
        fixture.cover.bytes()
    );
    let selected =
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(0, 1)?)?;
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, first.id);
    assert!(selected[0].approved_at.is_some());
    assert_eq!(
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(first.id, 1)?)?[0]
            .id,
        second.id
    );
    let wrong = fixture
        .store
        .artwork_jpeg(fixture.job, fixture.version, second.id)?;
    assert!(
        fixture
            .store
            .begin_normalization(&fixture.claim, &wrong)
            .is_err()
    );
    normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover)?;
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("publishing")
    );
    let raw = Connection::open(&fixture.database)?;
    let approvals: i64 = raw.query_row(
        "SELECT count(*) FROM events WHERE event_type = 'artwork_approved'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(approvals, 1);
    Ok(())
}

#[test]
fn artwork_preparation_is_bounded_idempotent_and_rejects_invalid_sources_without_rows() -> TestResult
{
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    for id in ["", "0", "01", "../file", "12.jpg", "123456789012345678901"] {
        assert!(
            fixture
                .store
                .prepare_artwork(fixture.job, fixture.version, id, &cover_source()?, None)
                .is_err()
        );
    }
    for bytes in [Vec::new(), vec![0; 8 * 1024 * 1024 + 1]] {
        assert!(
            fixture
                .store
                .prepare_artwork(fixture.job, fixture.version, "123", &bytes, None)
                .is_err()
        );
    }
    assert!(
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    let first = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123",
        &cover_source()?,
        None,
    )?;
    for approved in [false, true] {
        if approved {
            fixture
                .store
                .approve_artwork(fixture.job, fixture.version, first.id, &first.sha256)?;
        }
        let same = fixture.store.prepare_artwork(
            fixture.job,
            fixture.version,
            "123",
            &cover_source()?,
            None,
        )?;
        assert_eq!(same.id, first.id);
    }
    assert!(
        fixture
            .store
            .prepare_artwork(fixture.job, fixture.version, "456", &other_cover()?, None)
            .is_err()
    );
    let raw = Connection::open(&fixture.database)?;
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM events WHERE event_type = 'artwork_prepared'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        1
    );
    for sql in [
        "UPDATE artwork_candidates SET width = 1",
        "DELETE FROM artwork_candidates",
        "UPDATE artwork_approvals SET approved_at = 0",
        "DELETE FROM artwork_approvals",
    ] {
        assert!(raw.execute(sql, []).is_err());
    }
    Ok(())
}

#[test]
fn artwork_writes_and_approvals_roll_back_with_events() -> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Mp3, false, false)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("CREATE TRIGGER fail_artwork BEFORE INSERT ON events WHEN NEW.event_type = 'artwork_prepared' BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
    assert!(
        fixture
            .store
            .prepare_artwork(fixture.job, fixture.version, "123", &cover_source()?, None)
            .is_err()
    );
    assert!(
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    raw.execute_batch("DROP TRIGGER fail_artwork;")?;
    let cover = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123",
        &cover_source()?,
        None,
    )?;
    raw.execute_batch("CREATE TRIGGER fail_artwork BEFORE INSERT ON events WHEN NEW.event_type = 'artwork_approved' BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
    assert!(
        fixture
            .store
            .approve_artwork(fixture.job, fixture.version, cover.id, &cover.sha256)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)
            .is_err()
    );
    raw.execute_batch("DROP TRIGGER fail_artwork;")?;
    fixture
        .store
        .approve_artwork(fixture.job, fixture.version, cover.id, &cover.sha256)?;
    drop(fixture.store);
    fixture.store = super::Store::open_existing(&fixture.database)?;
    assert_eq!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)?
            .bytes(),
        fixture.cover.bytes()
    );
    raw.execute_batch("DROP TRIGGER immutable_artwork_candidate; UPDATE artwork_candidates SET jpeg = zeroblob(length(jpeg));")?;
    assert!(
        fixture
            .store
            .artwork_jpeg(fixture.job, fixture.version, cover.id)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)
            .is_err()
    );
    assert!(
        fixture
            .store
            .prepare_artwork(fixture.job, fixture.version, "123", &cover_source()?, None)
            .is_err()
    );
    Ok(())
}

#[test]
fn artwork_approval_never_carries_over_to_a_revised_plan() -> TestResult {
    let mut fixture = Fixture::new(AudioFormat::Mp3)?;
    let selected = fixture
        .store
        .artwork_candidates(fixture.job, fixture.version, Page::new(0, 1)?)?
        .remove(0);
    assert_eq!(
        fixture.store.jobs(Page::new(0, 10)?)?[0].artwork_candidate_id,
        Some(selected.id)
    );
    let raw = Connection::open(&fixture.database)?;
    raw.execute(
        "UPDATE jobs SET state = 'awaiting_input', owner = NULL, lease_until = NULL WHERE id = ?1",
        [fixture.job],
    )?;
    let old = fixture.store.plan(fixture.job, fixture.version)?;
    let revised = fixture.store.prepare_catalog_plan(
        PlanRequest {
            job_id: fixture.job,
            expected_version: fixture.version,
            settings_version: old.settings_version,
            format: Some(old.plan.format),
            edition: old.plan.edition,
            locations: vec![fixture.main],
            accepts_incomplete_coverage: false,
        },
        old.plan.catalog.ok_or("missing catalog")?,
    )?;
    assert!(
        fixture.store.jobs(Page::new(0, 10)?)?[0]
            .artwork_candidate_id
            .is_none()
    );
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, revised.version)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approve_artwork(fixture.job, revised.version, selected.id, &selected.sha256)
            .is_err()
    );
    assert!(
        fixture
            .store
            .begin_normalization(&fixture.claim, &fixture.cover)
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .artwork_jpeg(fixture.job, fixture.version, selected.id)?
            .bytes(),
        fixture.cover.bytes()
    );
    Ok(())
}

#[test]
fn artwork_approval_rechecks_destination_observations() -> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Mp3, false, false)?;
    let cover = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123",
        &cover_source()?,
        None,
    )?;
    domus::application::scan_location(&mut fixture.store, fixture.main)?;
    assert!(
        fixture
            .store
            .approve_artwork(fixture.job, fixture.version, cover.id, &cover.sha256)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)
            .is_err()
    );
    assert!(
        fixture
            .store
            .prepare_artwork(fixture.job, fixture.version, "456", &other_cover()?, None)
            .is_err()
    );
    assert!(
        fixture
            .store
            .begin_normalization(&fixture.claim, &fixture.cover)
            .is_err()
    );
    Ok(())
}

#[test]
fn artwork_keeps_explicit_transparency_choice_and_exact_jpeg_across_restart() -> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        20,
        40,
        image::Rgba([255, 0, 0, 128]),
    ))
    .write_to(&mut png, ImageFormat::Png)?;
    assert!(
        fixture
            .store
            .prepare_artwork(fixture.job, fixture.version, "123", png.get_ref(), None)
            .is_err()
    );
    assert!(
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    let background = Some([0, 0, 255]);
    let selected = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123",
        png.get_ref(),
        background,
    )?;
    assert_eq!(selected.background, background);
    let bytes = fixture
        .store
        .artwork_jpeg(fixture.job, fixture.version, selected.id)?
        .bytes()
        .to_vec();
    fixture
        .store
        .approve_artwork(fixture.job, fixture.version, selected.id, &selected.sha256)?;
    drop(fixture.store);
    fixture.store = super::Store::open_existing(&fixture.database)?;
    assert_eq!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)?
            .bytes(),
        bytes
    );
    assert_eq!(
        fixture
            .store
            .artwork_candidates(fixture.job, fixture.version, Page::new(0, 1)?)?[0]
            .background,
        background
    );
    Ok(())
}

#[test]
fn schema_eleven_upgrade_retains_receipts_but_requires_approval_of_the_exact_old_cover()
-> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let run = fixture
        .store
        .begin_normalization(&fixture.claim, &fixture.cover)?;
    let work = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 1)?)?
        .remove(0);
    let proof = fixture.proof(&work)?;
    fixture
        .store
        .record_normalized_audio(&fixture.claim, run.id, 1, &proof)?;
    for item in fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(1, 200)?)?
    {
        let proof = fixture.proof(&item)?;
        fixture
            .store
            .record_normalized_audio(&fixture.claim, run.id, item.track_number, &proof)?;
    }
    fixture.store.record_normalized_cover(
        &fixture.claim,
        run.id,
        run.cover_bytes,
        &run.cover_sha256,
    )?;
    drop(fixture.store);
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations; DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; DROP TABLE artwork_caa_evidence; DROP TABLE caa_cache; DROP TABLE filesystem_roots; DROP TABLE artwork_approvals; DROP TABLE artwork_candidates; PRAGMA user_version = 11;",
    )?;
    fixture.store = super::Store::open_existing(&fixture.database)?;
    fixture.bind_roots()?;
    let staging = fixture.staging.as_ref().ok_or("missing staging")?;
    assert!(normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover).is_err());
    assert!(!fixture.scratch.path().join(&run.staging_directory).exists());
    assert!(
        fixture
            .store
            .finish_normalization(&fixture.claim, run.id)
            .is_err()
    );
    let retained = fixture
        .store
        .normalization_work(&fixture.claim, run.id, Page::new(0, 1)?)?
        .remove(0);
    assert_eq!(
        retained.receipt.ok_or("missing retained receipt")?.sha256,
        proof.sha256
    );
    let wrong = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "456",
        &other_cover()?,
        None,
    )?;
    assert!(
        fixture
            .store
            .approve_artwork(fixture.job, fixture.version, wrong.id, &wrong.sha256)
            .is_err()
    );
    let correct = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123",
        &cover_source()?,
        None,
    )?;
    fixture
        .store
        .approve_artwork(fixture.job, fixture.version, correct.id, &correct.sha256)?;
    assert_eq!(
        fixture
            .store
            .approved_artwork(fixture.job, fixture.version)?
            .bytes(),
        fixture.cover.bytes()
    );
    let counts: i64 = raw.query_row(
        "SELECT count(*) FROM normalization_receipts WHERE run_id = ?1",
        params![run.id],
        |row| row.get(0),
    )?;
    assert_eq!(counts, 3);
    fixture.store.finish_normalization(&fixture.claim, run.id)?;
    Ok(())
}
