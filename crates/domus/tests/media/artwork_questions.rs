use super::*;
use domus::Error;
use domus::domain::ArtworkIssue;
use serde_json::json;
use std::process::Command;

const USE_COVER: &str = "Use approved cover";

fn release_owner(fixture: &Fixture) -> TestResult {
    Connection::open(&fixture.database)?
        .execute("UPDATE jobs SET lease_until=0 WHERE id=?1", [fixture.job])?;
    Ok(())
}

#[test]
fn pending_artwork_issue_updates_without_losing_the_original_pause_boundary() -> TestResult {
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    release_owner(&fixture)?;
    let mut input = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 128])))
        .write_to(&mut input, ImageFormat::Png)?;
    let path = fixture._state.path().join("transparent.png");
    std::fs::write(&path, input.get_ref())?;
    let result = fixture.store.prepare_local_artwork_or_ask(
        fixture.job,
        fixture.version,
        &path,
        None,
        false,
    );
    let Err(Error::ArtworkRequired(id)) = result else {
        return Err("missing permission question".into());
    };
    assert_eq!(
        fixture.store.artwork_decision(id)?.issue,
        ArtworkIssue::Permission
    );
    let result =
        fixture
            .store
            .prepare_local_artwork_or_ask(fixture.job, fixture.version, &path, None, true);
    assert!(matches!(result,Err(Error::ArtworkRequired(same)) if same==id));
    let mut reopened = Store::open_existing(&fixture.database)?;
    assert_eq!(
        reopened.artwork_decision(id)?.issue,
        ArtworkIssue::Background
    );
    assert!(reopened.question(id)?.prompt.contains("background color"));
    assert_eq!(reopened.artwork_decision(id)?.paused_state, "validating");
    assert_eq!(
        reopened
            .check_artwork(fixture.job, fixture.version)?
            .ok_or("question")?
            .issue,
        ArtworkIssue::Background
    );
    assert_eq!(reopened.questions(Page::new(0, 20)?)?.len(), 1);
    let events = reopened.last_event_id()?;
    assert!(
        matches!(reopened.prepare_local_artwork_or_ask(fixture.job,fixture.version,&path,None,true),Err(Error::ArtworkRequired(same)) if same==id)
    );
    assert_eq!(reopened.last_event_id()?, events);
    Ok(())
}

#[test]
fn artwork_resume_rechecks_destinations_and_the_exact_saved_jpeg() -> TestResult {
    for fault in ["observation", "jpeg"] {
        let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
        let decision = fixture
            .store
            .ask_artwork(&fixture.claim, ArtworkIssue::Missing)?;
        let candidate = fixture.store.prepare_artwork(
            fixture.job,
            fixture.version,
            "123",
            &cover_source()?,
            None,
        )?;
        fixture.store.approve_artwork(
            fixture.job,
            fixture.version,
            candidate.id,
            &candidate.sha256,
        )?;
        fixture
            .store
            .answer(decision.question_id, fixture.version, USE_COVER)?;
        let raw = Connection::open(&fixture.database)?;
        if fault == "observation" {
            raw.execute(
                "UPDATE locations SET observed_at=observed_at+1 WHERE id=?1",
                [fixture.main],
            )?;
        } else {
            let triggers: Vec<String> = raw.prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name='artwork_candidates'")?
                .query_map([],|row| row.get(0))?.collect::<Result<_,_>>()?;
            for name in triggers {
                raw.execute_batch(&format!("DROP TRIGGER \"{}\";", name.replace('"', "\"\"")))?;
            }
            raw.execute(
                "UPDATE artwork_candidates SET jpeg=zeroblob(length(jpeg)) WHERE id=?1",
                [candidate.id],
            )?;
        }
        assert!(
            fixture
                .store
                .resume_artwork(decision.question_id, fixture.version)
                .is_err()
        );
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("awaiting_input")
        );
        assert!(
            fixture
                .store
                .artwork_decision(decision.question_id)?
                .resumed_at
                .is_none()
        );
    }
    Ok(())
}

fn run(args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_domus"))
        .args(args)
        .output()
}

#[test]
fn artwork_questions_survive_restart_and_resume_real_normalization_without_redownload() -> TestResult
{
    for (format, paused_state) in [
        (AudioFormat::Mp3, "validating"),
        (AudioFormat::Flac, "validating"),
        (AudioFormat::Mp3, "downloading"),
        (AudioFormat::Flac, "downloading"),
    ] {
        let mut fixture = Fixture::build_inner(format, true, false)?;
        if paused_state == "downloading" {
            Connection::open(&fixture.database)?.execute(
                "UPDATE jobs SET state='downloading' WHERE id=?1",
                [fixture.job],
            )?;
        }
        let receipts = serde_json::to_value(fixture.store.transfers(
            fixture.job,
            fixture.version,
            Page::new(0, 200)?,
        )?)?;
        assert!(
            fixture
                .store
                .check_artwork(fixture.job, fixture.version)
                .is_err()
        );
        let decision = fixture
            .store
            .ask_artwork(&fixture.claim, ArtworkIssue::Missing)?;
        assert_eq!(decision.paused_state, paused_state);
        assert!(fixture.store.renew_claim(&fixture.claim, 300).is_err());
        assert!(fixture.store.claim(fixture.job, "other", 300)?.is_none());
        let mut store = Store::open_existing(&fixture.database)?;
        let duplicate = store
            .check_artwork(fixture.job, fixture.version)?
            .ok_or("missing question")?;
        assert_eq!(duplicate.question_id, decision.question_id);
        assert_eq!(store.questions(Page::new(0, 20)?)?.len(), 1);
        assert!(
            store
                .answer(decision.question_id, fixture.version, USE_COVER)
                .is_err()
        );
        let path = fixture._state.path().join("cover.png");
        std::fs::write(&path, cover_source()?)?;
        let candidate =
            store.prepare_local_artwork_or_ask(fixture.job, fixture.version, &path, None, true)?;
        assert!(
            store
                .answer(decision.question_id, fixture.version, USE_COVER)
                .is_err()
        );
        assert!(
            store
                .resume_artwork(decision.question_id, fixture.version)
                .is_err()
        );
        store.approve_artwork(
            fixture.job,
            fixture.version,
            candidate.id,
            &candidate.sha256,
        )?;
        assert!(
            store
                .answer(decision.question_id, fixture.version + 1, USE_COVER)
                .is_err()
        );
        assert!(
            store
                .answer(decision.question_id, fixture.version, "Not offered")
                .is_err()
        );
        store.answer(decision.question_id, fixture.version, USE_COVER)?;
        assert!(
            store
                .answer(decision.question_id, fixture.version, USE_COVER)
                .is_err()
        );
        assert_eq!(
            store.job_state(fixture.job)?.as_deref(),
            Some("awaiting_input")
        );
        assert!(store.claim(fixture.job, "other", 300)?.is_none());
        assert!(
            store
                .claim_approved_plan(fixture.job, fixture.version, "other", 300)?
                .is_none()
        );
        let resolved = store.artwork_decision(decision.question_id)?;
        assert_eq!(resolved.candidate_id, Some(candidate.id));
        assert_eq!(resolved.sha256.as_deref(), Some(candidate.sha256.as_str()));
        drop(store);
        let mut store = Store::open_existing(&fixture.database)?;
        assert!(
            store
                .resume_artwork(decision.question_id, fixture.version + 1)
                .is_err()
        );
        assert_eq!(
            store.resume_artwork(decision.question_id, fixture.version)?,
            paused_state
        );
        assert!(
            store
                .resume_artwork(decision.question_id, fixture.version)
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(store.transfers(
                fixture.job,
                fixture.version,
                Page::new(0, 200)?
            )?)?,
            receipts
        );
        // The real command must finish entirely from sealed local inputs. Its
        // configured public peer is not a fixture server and must not be used.
        let secret = fixture._state.path().join("credentials.json");
        std::fs::write(
            &secret,
            br#"{"username":"fixture-user","password":"fixture-password"}"#,
        )?;
        let config = fixture._state.path().join("worker.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database":fixture.database,"staging_root":fixture.scratch.path(),"credentials_file":secret,
                "server":"8.8.8.8:2242","job_id":fixture.job,"plan_version":fixture.version,
                "acknowledge_plaintext_login":true
            }))?,
        )?;
        drop(fixture.staging.take());
        let checked = run(&["worker-check", config.to_str().ok_or("config")?])?;
        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
        let output = run(&["worker-once", config.to_str().ok_or("config")?])?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(store.job_state(fixture.job)?.as_deref(), Some("publishing"));
        assert_eq!(
            serde_json::to_value(store.transfers(
                fixture.job,
                fixture.version,
                Page::new(0, 200)?
            )?)?,
            receipts
        );
        assert!(
            store
                .artwork_decision(decision.question_id)?
                .resumed_at
                .is_some()
        );
        assert!(store.renew_claim(&fixture.claim, 300).is_err());
    }
    Ok(())
}

#[test]
fn artwork_preparation_pauses_for_permission_background_and_unsupported_input() -> TestResult {
    use image::{DynamicImage, ImageEncoder, ImageFormat, Rgba, RgbaImage};
    for issue in [
        ArtworkIssue::Permission,
        ArtworkIssue::Background,
        ArtworkIssue::Unsupported,
        ArtworkIssue::Animated,
        ArtworkIssue::ColorProfile,
    ] {
        let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
        release_owner(&fixture)?;
        let path = fixture._state.path().join("cover.png");
        let bytes = if issue == ArtworkIssue::Unsupported {
            b"GIF89a".to_vec()
        } else if issue == ArtworkIssue::Animated {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(1, 0)?;
            let mut writer = encoder.write_header()?;
            writer.write_image_data(&[10, 20, 30, 255])?;
            writer.finish()?;
            bytes
        } else if issue == ArtworkIssue::ColorProfile {
            let mut bytes = Vec::new();
            let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
            encoder.set_icc_profile(vec![0; 128])?;
            encoder.write_image(&[10, 20, 30], 1, 1, image::ExtendedColorType::Rgb8)?;
            bytes
        } else {
            let mut png = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([0, 255, 0, 128])))
                .write_to(&mut png, ImageFormat::Png)?;
            png.into_inner()
        };
        std::fs::write(&path, &bytes)?;
        let result = fixture.store.prepare_local_artwork_or_ask(
            fixture.job,
            fixture.version,
            &path,
            None,
            issue != ArtworkIssue::Permission,
        );
        let Err(Error::ArtworkRequired(id)) = result else {
            return Err("expected saved artwork question".into());
        };
        assert_eq!(fixture.store.artwork_decision(id)?.issue, issue);
        assert!(
            fixture
                .store
                .artwork_candidates(fixture.job, fixture.version, Page::new(0, 10)?)?
                .is_empty()
        );
        assert_eq!(std::fs::read(&path)?, bytes);
        assert!(
            fixture
                .store
                .answer(id, fixture.version, USE_COVER)
                .is_err()
        );
        if issue == ArtworkIssue::Background {
            let candidate = fixture.store.prepare_local_artwork_or_ask(
                fixture.job,
                fixture.version,
                &path,
                Some([255, 255, 255]),
                true,
            )?;
            assert_eq!(candidate.background, Some([255, 255, 255]));
            assert!(candidate.approved_at.is_none());
        }
    }
    Ok(())
}

#[test]
fn worker_missing_cover_saves_one_question_and_cli_resolves_it_without_starting_work() -> TestResult
{
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, true, false)?;
    release_owner(&fixture)?;
    let credentials = fixture._state.path().join("credentials.json");
    std::fs::write(
        &credentials,
        br#"{"username":"fixture-user","password":"fixture-password"}"#,
    )?;
    let configuration = fixture._state.path().join("worker.json");
    std::fs::write(
        &configuration,
        serde_json::to_vec(&json!({
            "database":fixture.database,"staging_root":fixture.scratch.path(),"credentials_file":credentials,
            "server":"8.8.8.8:2242","job_id":fixture.job,"plan_version":fixture.version,
            "acknowledge_plaintext_login":true
        }))?,
    )?;
    let configuration = configuration.to_str().ok_or("config")?;
    assert!(!run(&["worker-check", configuration])?.status.success());
    assert!(fixture.store.questions(Page::new(0, 20)?)?.is_empty());
    let output = run(&["worker-once", configuration])?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("question"));
    let question = fixture
        .store
        .questions(Page::new(0, 20)?)?
        .pop()
        .ok_or("question")?;
    let db = fixture.database.to_str().ok_or("db")?;
    let job = fixture.job.to_string();
    let version = fixture.version.to_string();
    let id = question.id.to_string();
    let checked = run(&["artwork-check", db, &job, &version])?;
    assert!(checked.status.success());
    let checked: serde_json::Value = serde_json::from_slice(&checked.stdout)?;
    assert_eq!(checked["question_id"], question.id);
    let details = run(&["artwork-question", db, &id])?;
    assert!(details.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&details.stdout)?["issue"],
        "missing"
    );
    assert!(
        !run(&["question-answer", db, &id, &version, USE_COVER])?
            .status
            .success()
    );
    let image = fixture._state.path().join("permitted-cover.png");
    std::fs::write(&image, cover_source()?)?;
    let prepared = run(&[
        "artwork-local-prepare",
        db,
        &job,
        &version,
        image.to_str().ok_or("cover path")?,
        "null",
        "--permitted-local-cover",
    ])?;
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let prepared: serde_json::Value = serde_json::from_slice(&prepared.stdout)?;
    assert!(
        !run(&["question-answer", db, &id, &version, USE_COVER])?
            .status
            .success()
    );
    let candidate = prepared["id"].as_i64().ok_or("candidate")?.to_string();
    assert!(
        run(&[
            "artwork-approve",
            db,
            &job,
            &version,
            &candidate,
            prepared["sha256"].as_str().ok_or("hash")?
        ])?
        .status
        .success()
    );
    assert!(
        run(&["question-answer", db, &id, &version, USE_COVER])?
            .status
            .success()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("awaiting_input")
    );
    assert!(
        run(&["artwork-resume", db, &id, &version])?
            .status
            .success()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("validating")
    );
    let raw = Connection::open(&fixture.database)?;
    assert_eq!(
        raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    assert!(
        fixture
            .store
            .artwork_decision(question.id)?
            .resumed_at
            .is_some()
    );
    // Keep the existing staging lock alive: no command above opens it.
    assert!(fixture.staging.take().is_some());
    Ok(())
}

#[test]
fn artwork_decisions_reject_live_owners_uncertain_transfers_and_stale_plans() -> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    assert!(
        fixture
            .store
            .check_artwork(fixture.job, fixture.version)
            .is_err()
    );
    assert!(
        fixture
            .store
            .check_artwork(fixture.job, fixture.version + 1)
            .is_err()
    );
    assert!(fixture.store.questions(Page::new(0, 20)?)?.is_empty());
    release_owner(&fixture)?;
    assert!(
        fixture
            .store
            .ask_artwork(&fixture.claim, ArtworkIssue::Missing)
            .is_err()
    );
    let decision = fixture
        .store
        .check_artwork(fixture.job, fixture.version)?
        .ok_or("question")?;
    let raw = Connection::open(&fixture.database)?;
    assert!(
        raw.execute("UPDATE artwork_questions SET question_id=question_id+1", [])
            .is_err()
    );
    assert!(raw.execute("DELETE FROM artwork_questions", []).is_err());
    raw.execute(
        "UPDATE jobs SET plan_version=plan_version+1 WHERE id=?1",
        [fixture.job],
    )?;
    assert!(
        fixture
            .store
            .answer(decision.question_id, fixture.version, USE_COVER)
            .is_err()
    );
    assert!(
        fixture
            .store
            .resume_artwork(decision.question_id, fixture.version)
            .is_err()
    );
    assert!(fixture.store.questions(Page::new(0, 20)?)?.is_empty());
    let mut uncertain = Fixture::build_with_receipts(AudioFormat::Flac, false, false, false)?;
    assert!(
        uncertain
            .store
            .ask_artwork(&uncertain.claim, ArtworkIssue::Missing)
            .is_err()
    );
    release_owner(&uncertain)?;
    assert!(
        uncertain
            .store
            .check_artwork(uncertain.job, uncertain.version)
            .is_err()
    );
    assert_eq!(
        uncertain.store.job_state(uncertain.job)?.as_deref(),
        Some("downloading")
    );
    assert!(uncertain.store.questions(Page::new(0, 20)?)?.is_empty());
    Ok(())
}

#[test]
fn artwork_question_writes_and_answer_bindings_are_atomic() -> TestResult {
    let mut fixture = Fixture::build_inner(AudioFormat::Flac, false, false)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("CREATE TRIGGER reject_artwork_question BEFORE INSERT ON artwork_questions BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
    assert!(
        fixture
            .store
            .ask_artwork(&fixture.claim, ArtworkIssue::Missing)
            .is_err()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("validating")
    );
    fixture.store.renew_claim(&fixture.claim, 300)?;
    assert!(fixture.store.questions(Page::new(0, 20)?)?.is_empty());
    raw.execute_batch("DROP TRIGGER reject_artwork_question;")?;
    let decision = fixture
        .store
        .ask_artwork(&fixture.claim, ArtworkIssue::Missing)?;
    let candidate = fixture.store.prepare_artwork(
        fixture.job,
        fixture.version,
        "123",
        &cover_source()?,
        None,
    )?;
    fixture.store.approve_artwork(
        fixture.job,
        fixture.version,
        candidate.id,
        &candidate.sha256,
    )?;
    raw.execute_batch("CREATE TRIGGER reject_artwork_answer BEFORE UPDATE OF answer ON questions BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
    assert!(
        fixture
            .store
            .answer(decision.question_id, fixture.version, USE_COVER)
            .is_err()
    );
    assert!(
        fixture
            .store
            .artwork_decision(decision.question_id)?
            .candidate_id
            .is_none()
    );
    raw.execute_batch("DROP TRIGGER reject_artwork_answer;")?;
    fixture
        .store
        .answer(decision.question_id, fixture.version, USE_COVER)?;
    raw.execute_batch("CREATE TRIGGER reject_artwork_resume BEFORE INSERT ON events WHEN NEW.event_type='artwork_resumed' BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
    assert!(
        fixture
            .store
            .resume_artwork(decision.question_id, fixture.version)
            .is_err()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("awaiting_input")
    );
    assert!(
        fixture
            .store
            .artwork_decision(decision.question_id)?
            .resumed_at
            .is_none()
    );
    Ok(())
}
