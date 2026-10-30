use domus::application::{add_location, scan_location};
use domus::domain::{
    ArtworkSize, AudioFormat, EditionSelection, JobState, LibraryPath, OutputSettings,
    PlannedTrack, Question,
};
use domus::infrastructure::store::{Observation, PlanRequest, Store};
use std::path::PathBuf;
use tempfile::{TempDir, tempdir};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[path = "planning/audio_profiles.rs"]
mod audio_profiles;
#[path = "planning/catalog_batches.rs"]
mod catalog_batches;
mod support;

struct Fixture {
    store: Store,
    main: i64,
    database: PathBuf,
    _state: TempDir,
    media: TempDir,
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let state = tempdir()?;
        let media = tempdir()?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let main = add_location(&store, media.path(), "main")?;
        store.save_settings(None, &OutputSettings::default())?;
        Ok(Self {
            store,
            main,
            database,
            _state: state,
            media,
        })
    }

    fn request(&mut self) -> TestResult<PlanRequest> {
        Ok(PlanRequest {
            audio_profile: None,
            job_id: self.store.enqueue(1)?,
            expected_version: 1,
            settings_version: 1,
            format: None,
            edition: edition(),
            locations: vec![self.main],
            accepts_incomplete_coverage: false,
        })
    }
}

fn edition() -> EditionSelection {
    EditionSelection {
        release_id: "12345678-1234-4321-9876-123456789abc".into(),
        album_artist: "The Example".into(),
        album: "Two Discs".into(),
        bucket: None,
        tracks: vec![
            PlannedTrack {
                artist: "First Artist".into(),
                title: "First".into(),
                disc: Some(1),
                disc_track: Some(1),
            },
            PlannedTrack {
                artist: "Second Artist".into(),
                title: "Second".into(),
                disc: Some(2),
                disc_track: Some(1),
            },
        ],
    }
}

#[test]
fn plan_events_are_bounded_durable_and_do_not_mix_jobs_or_versions() -> TestResult {
    use domus::domain::Page;
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let job = request.job_id;
    let saved = fixture.store.prepare_plan(request)?;
    let version = saved.version;
    let other = fixture.store.enqueue(1)?;
    let raw = rusqlite::Connection::open(&fixture.database)?;
    for (event_job, event_version, name) in [
        (job, Some(version), "generated_first"),
        (other, Some(version), "different_job"),
        (job, Some(1), "different_plan"),
        (job, None, "unversioned_legacy"),
        (job, Some(version), "generated_last"),
    ] {
        raw.execute(
            "INSERT INTO events(job_id,plan_version,event_type) VALUES (?1,?2,?3)",
            rusqlite::params![event_job, event_version, name],
        )?;
    }
    let before: i64 = raw.query_row("SELECT count(*) FROM events", [], |row| row.get(0))?;
    let first = fixture.store.plan_events(job, version, Page::new(0, 1)?)?;
    assert_eq!(first.len(), 1);
    let rest = fixture
        .store
        .plan_events(job, version, Page::new(first[0].id, 50)?)?;
    assert!(
        rest.iter()
            .all(|event| event.job_id == job && event.plan_version == Some(version))
    );
    assert_eq!(
        rest.last().ok_or("last event")?.event_type,
        "generated_last"
    );
    assert!(
        rest.iter()
            .any(|event| event.event_type == "generated_first")
    );
    assert!(rest.iter().all(|event| {
        !["different_job", "different_plan", "unversioned_legacy"]
            .contains(&event.event_type.as_str())
    }));
    assert!(
        fixture
            .store
            .plan_events(job, 99, Page::new(0, 10)?)
            .is_err()
    );
    assert!(
        fixture
            .store
            .plan_events(-1, version, Page::new(0, 10)?)
            .is_err()
    );
    let reopened = Store::open_read_snapshot(&fixture.database)?;
    assert_eq!(
        reopened.plan_events(job, version, Page::new(0, 50)?)?.len(),
        rest.len() + 1
    );
    let after: i64 = raw.query_row("SELECT count(*) FROM events", [], |row| row.get(0))?;
    assert_eq!(before, after);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_domus"))
        .args([
            "job-history",
            fixture.database.to_str().ok_or("database path")?,
            &job.to_string(),
            &version.to_string(),
            &first[0].id.to_string(),
            "50",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout)?,
        serde_json::to_value(&rest)?
    );
    let explanation: String = raw.query_row(
        "EXPLAIN QUERY PLAN SELECT id FROM events
         WHERE job_id=?1 AND plan_version=?2 AND id>?3 ORDER BY id LIMIT ?4",
        rusqlite::params![job, version, 0, 50],
        |row| row.get(3),
    )?;
    assert!(explanation.contains("plan_events"));
    Ok(())
}

#[test]
fn queue_summaries_use_only_the_current_frozen_plan() -> TestResult {
    use domus::domain::Page;
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let job = request.job_id;
    let initial = fixture.store.jobs(Page::new(0, 10)?)?;
    assert_eq!(initial.len(), 1);
    assert!(initial[0].album.is_none());
    assert!(initial[0].album_artist.is_none());
    assert!(initial[0].approved_at.is_none());
    assert!(initial[0].artwork_candidate_id.is_none());

    let saved = fixture.store.prepare_plan(request)?;
    let summary = fixture.store.jobs(Page::new(0, 1)?)?.remove(0);
    assert_eq!(summary.id, job);
    assert_eq!(summary.plan_version, saved.version);
    assert_eq!(summary.album.as_deref(), Some("Two Discs"));
    assert_eq!(summary.album_artist.as_deref(), Some("The Example"));
    assert!(summary.approved_at.is_none());
    assert!(summary.artwork_candidate_id.is_none());
    fixture.store.approve_plan(job, saved.version)?;
    assert!(
        fixture.store.jobs(Page::new(0, 1)?)?[0]
            .approved_at
            .is_some()
    );

    let mut revision = fixture.request()?;
    revision.job_id = job;
    revision.expected_version = saved.version;
    revision.edition.album = "New frozen edition".into();
    let updated = fixture.store.prepare_plan(revision)?;
    let summary = fixture.store.jobs(Page::new(0, 1)?)?.remove(0);
    assert_eq!(summary.plan_version, updated.version);
    assert_eq!(summary.album.as_deref(), Some("New frozen edition"));
    assert!(summary.approved_at.is_none());
    assert!(summary.artwork_candidate_id.is_none());
    assert_eq!(fixture.store.jobs(Page::new(job, 10)?)?.len(), 1);
    Ok(())
}

#[test]
fn accepted_audio_policy_requires_a_preferred_member_and_preserves_frozen_plans() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let frozen = fixture.store.prepare_plan(request)?;
    for accepted in [
        vec![],
        vec![AudioFormat::Flac],
        vec![AudioFormat::Mp3, AudioFormat::Mp3],
    ] {
        let invalid = OutputSettings {
            accepted_formats: Some(accepted),
            ..OutputSettings::default()
        };
        assert!(fixture.store.save_settings(Some(1), &invalid).is_err());
        assert_eq!(fixture.store.settings()?.ok_or("settings")?.version, 1);
    }
    let policy = OutputSettings {
        format: AudioFormat::Flac,
        accepted_formats: Some(vec![AudioFormat::Flac]),
        ..OutputSettings::default()
    };
    fixture.store.save_settings(Some(1), &policy)?;
    let mut request = fixture.request()?;
    request.settings_version = 2;
    request.format = Some(AudioFormat::Mp3);
    assert!(fixture.store.prepare_plan(request).is_err());
    let mut request = fixture.request()?;
    request.settings_version = 2;
    let preferred = fixture.store.prepare_plan(request)?;
    assert_eq!(preferred.plan.format, AudioFormat::Flac);
    assert_eq!(
        preferred.plan.settings.accepted_formats,
        policy.accepted_formats
    );
    assert_eq!(
        fixture.store.plan(frozen.job_id, frozen.version)?.plan,
        frozen.plan
    );
    let mut legacy = policy.clone();
    legacy.accepted_formats = None;
    assert!(legacy.accepts(AudioFormat::Mp3));
    assert!(legacy.accepts(AudioFormat::Flac));
    Ok(())
}

#[test]
fn settings_are_versioned_without_changing_a_frozen_plan() -> TestResult {
    assert_eq!(
        OutputSettings::default().artwork_size,
        Some(ArtworkSize::Recommended)
    );
    let legacy: OutputSettings = serde_json::from_str(
        r#"{"format":"mp3","template":{"folder":"<LETTER>\\<ALBUM ARTIST>\\<ALBUM>","filename":"<Track Artist> - <Album> - <Track:00> - <Title>.<ext>"} }"#,
    )?;
    assert_eq!(legacy.artwork_size, None);
    assert_eq!(legacy.accepted_formats, None);

    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let plan = fixture.store.prepare_plan(request)?;
    fixture.store.approve_plan(plan.job_id, plan.version)?;
    let flac = OutputSettings {
        format: AudioFormat::Flac,
        artwork_size: Some(ArtworkSize::Large),
        ..OutputSettings::default()
    };
    assert!(fixture.store.save_settings(None, &flac).is_err());
    assert_eq!(fixture.store.save_settings(Some(1), &flac)?, 2);
    assert!(
        fixture
            .store
            .save_settings(Some(1), &OutputSettings::default())
            .is_err()
    );
    drop(fixture.store);
    fixture.store = Store::open_existing(&fixture.database)?;
    let current = fixture.store.settings()?.ok_or("missing settings")?;
    assert_eq!(current.version, 2);
    assert_eq!(current.settings.format, AudioFormat::Flac);
    assert_eq!(current.settings.artwork_size, Some(ArtworkSize::Large));
    let frozen = fixture.store.plan(plan.job_id, plan.version)?;
    assert_eq!(frozen.plan, plan.plan);
    assert_eq!(frozen.settings_version, 1);
    assert_eq!(frozen.plan.settings.format, AudioFormat::Mp3);
    assert_eq!(
        frozen.plan.settings.artwork_size,
        Some(ArtworkSize::Recommended)
    );
    assert!(frozen.approved_at.is_some());
    let database = rusqlite::Connection::open(&fixture.database)?;
    assert!(
        database
            .execute("UPDATE settings_versions SET body = '{}' WHERE id = 1", [])
            .is_err()
    );
    assert!(
        database
            .execute("DELETE FROM settings_versions WHERE id = 2", [])
            .is_err()
    );
    assert!(
        database
            .execute(
                "UPDATE job_plans SET body = '{}' WHERE job_id = ?1",
                [plan.job_id]
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .claim_approved_plan(plan.job_id, plan.version, "worker", 60)?
            .is_some()
    );
    Ok(())
}

#[test]
fn approval_ownership_and_state_are_required_before_work() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let plan = fixture.store.prepare_plan(request)?;
    assert!(
        fixture
            .store
            .claim_approved_plan(plan.job_id, plan.version, "worker", 60)
            .is_err()
    );
    let old_claim = fixture
        .store
        .claim(plan.job_id, "worker", 60)?
        .ok_or("missing claim")?;
    assert!(
        fixture
            .store
            .advance(&old_claim, JobState::Queued, JobState::Searching)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approve_plan(plan.job_id, plan.version)
            .is_err()
    );
    rusqlite::Connection::open(&fixture.database)?.execute(
        "UPDATE jobs SET lease_until = 0 WHERE id = ?1",
        [plan.job_id],
    )?;
    fixture.store.approve_plan(plan.job_id, plan.version)?;
    assert!(
        fixture
            .store
            .approve_plan(plan.job_id, plan.version)
            .is_err()
    );
    let approved = fixture
        .store
        .claim_approved_plan(plan.job_id, plan.version, "worker", 60)?
        .ok_or("missing approved claim")?;
    let mut competing = Store::open_existing(&fixture.database)?;
    assert!(
        competing
            .claim_approved_plan(plan.job_id, plan.version, "other", 60)?
            .is_none()
    );
    assert!(
        competing
            .advance(&old_claim, JobState::Queued, JobState::Searching)
            .is_err()
    );
    fixture
        .store
        .advance(&approved.claim, JobState::Queued, JobState::Searching)?;
    let revise = PlanRequest {
        audio_profile: None,
        job_id: plan.job_id,
        expected_version: plan.version,
        settings_version: 1,
        format: None,
        edition: edition(),
        locations: vec![fixture.main],
        accepts_incomplete_coverage: false,
    };
    assert!(fixture.store.prepare_plan(revise).is_err());
    drop(fixture.store);
    fixture.store = Store::open_existing(&fixture.database)?;
    assert_eq!(
        fixture.store.job_state(plan.job_id)?.as_deref(),
        Some("searching")
    );
    Ok(())
}

#[test]
fn incomplete_coverage_requires_a_decision_bound_to_observations() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut request = fixture.request()?;
    let job = request.job_id;
    assert!(fixture.store.prepare_plan(request).is_err());
    request = PlanRequest {
        audio_profile: None,
        job_id: job,
        expected_version: 1,
        settings_version: 1,
        format: None,
        edition: edition(),
        locations: vec![fixture.main],
        accepts_incomplete_coverage: true,
    };
    let plan = fixture.store.prepare_plan(request)?;
    assert_eq!(plan.plan.destinations[0].coverage, "unknown");
    scan_location(&mut fixture.store, fixture.main)?;
    assert!(fixture.store.approve_plan(job, plan.version).is_err());
    let revised = fixture.store.prepare_plan(PlanRequest {
        audio_profile: None,
        job_id: job,
        expected_version: plan.version,
        settings_version: 1,
        format: None,
        edition: edition(),
        locations: vec![fixture.main],
        accepts_incomplete_coverage: false,
    })?;
    fixture.store.approve_plan(job, revised.version)?;
    assert!(
        fixture
            .store
            .claim_approved_plan(job, plan.version, "worker", 60)
            .is_err()
    );
    scan_location(&mut fixture.store, fixture.main)?;
    assert!(
        fixture
            .store
            .claim_approved_plan(job, revised.version, "worker", 60)
            .is_err()
    );
    let claim = fixture
        .store
        .claim(job, "worker", 60)?
        .ok_or("missing claim")?;
    assert!(
        fixture
            .store
            .advance(&claim, JobState::Queued, JobState::Searching)
            .is_err()
    );
    assert_eq!(fixture.store.job_state(job)?.as_deref(), Some("queued"));
    Ok(())
}

#[test]
fn progress_within_the_same_scan_also_invalidates_approval() -> TestResult {
    let mut fixture = Fixture::new()?;
    let claim = fixture.store.begin_scan(fixture.main)?;
    let mut request = fixture.request()?;
    request.accepts_incomplete_coverage = true;
    let plan = fixture.store.prepare_plan(request)?;
    fixture.store.approve_plan(plan.job_id, plan.version)?;
    fixture.store.record_batch(
        &claim,
        &[Observation {
            path: &LibraryPath::parse("new-file.mp3")?,
            kind: "file",
            size: Some(123),
            modified_at: None,
        }],
    )?;
    assert!(
        fixture
            .store
            .claim_approved_plan(plan.job_id, plan.version, "worker", 60)
            .is_err()
    );
    Ok(())
}

#[test]
fn revised_plans_invalidate_approval_and_questions_without_losing_history() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let job = request.job_id;
    let claim = fixture
        .store
        .claim(job, "worker", 60)?
        .ok_or("missing claim")?;
    let question = fixture.store.ask(
        &claim,
        JobState::Queued,
        &Question::new(
            "Use this earlier selection?".into(),
            vec!["Approve".into(), "Cancel".into()],
        )?,
    )?;
    let initial = fixture.store.prepare_plan(request)?;
    assert!(fixture.store.answer(question, 1, "Approve").is_err());
    fixture.store.approve_plan(job, initial.version)?;
    let mut revised_edition = edition();
    revised_edition.album = "A Different Edition".into();
    let revised = fixture.store.prepare_plan(PlanRequest {
        audio_profile: None,
        job_id: job,
        expected_version: initial.version,
        settings_version: 1,
        format: None,
        edition: revised_edition,
        locations: vec![fixture.main],
        accepts_incomplete_coverage: false,
    })?;
    assert_eq!(revised.version, initial.version + 1);
    assert!(revised.approved_at.is_none());
    assert!(
        fixture
            .store
            .claim_approved_plan(job, initial.version, "worker", 60)?
            .is_none()
    );
    assert!(
        fixture
            .store
            .claim_approved_plan(job, revised.version, "worker", 60)
            .is_err()
    );
    assert_eq!(
        fixture.store.plan(job, initial.version)?.plan.edition.album,
        "Two Discs"
    );
    let database = rusqlite::Connection::open(&fixture.database)?;
    let versions: Vec<i64> = database.prepare(
        "SELECT plan_version FROM events WHERE job_id = ?1 AND event_type IN ('plan_prepared', 'plan_approved') ORDER BY id"
    )?.query_map([job], |row| row.get(0))?.collect::<Result<_, _>>()?;
    assert_eq!(
        versions,
        vec![initial.version, initial.version, revised.version]
    );
    assert!(fixture.store.question(question)?.answer.is_none());
    Ok(())
}

#[test]
fn output_plans_keep_one_folder_and_unique_ordered_filenames() -> TestResult {
    let mut selected = edition();
    let mut settings = OutputSettings::default();
    let outputs = selected.preview(&settings)?;
    assert_eq!(outputs[0].relative_folder, "E/THE EXAMPLE/TWO DISCS");
    assert_eq!(
        outputs[1].filename,
        "Second Artist - Two Discs - 02 - Second.mp3"
    );
    assert_eq!(selected.tracks[1].disc_track, Some(1));
    settings.template.filename = "<Title>.<ext>".into();
    selected.tracks[0].title = "CON".into();
    selected.tracks[1].title = "con".into();
    assert!(selected.preview(&settings).is_err());
    settings.template.folder = "<Track Artist>".into();
    assert!(selected.preview(&settings).is_err());
    settings = OutputSettings::default();
    selected.release_id = "not-a-release-id".into();
    assert!(selected.preview(&settings).is_err());
    selected = edition();
    selected.tracks.clear();
    assert!(selected.preview(&settings).is_err());
    selected.tracks = vec![edition().tracks.remove(0); 1001];
    assert!(selected.preview(&settings).is_err());
    Ok(())
}

#[test]
fn invalid_settings_and_duplicate_destinations_leave_no_plan() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut invalid = OutputSettings::default();
    invalid.template.filename = "../<Title>.<ext>".into();
    assert!(fixture.store.save_settings(Some(1), &invalid).is_err());
    assert_eq!(
        fixture.store.settings()?.ok_or("missing settings")?.version,
        1
    );
    let mut request = fixture.request()?;
    let job = request.job_id;
    request.locations.push(fixture.main);
    assert!(fixture.store.prepare_plan(request).is_err());
    let database = rusqlite::Connection::open(&fixture.database)?;
    assert_eq!(
        database.query_row(
            "SELECT COUNT(*) FROM job_plans WHERE job_id = ?1",
            [job],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    assert!(serde_json::from_str::<OutputSettings>(
        r#"{"format":"mp3","template":{"folder":"<ALBUM>","filename":"<Title>.<ext>"},"password":"fixture"}"#
    ).is_err());
    Ok(())
}

#[test]
fn preview_and_approval_do_not_create_or_replace_media() -> TestResult {
    let mut fixture = Fixture::new()?;
    let existing = fixture.media.path().join("existing.mp3");
    std::fs::write(&existing, b"unchanged fixture bytes")?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let plan = fixture.store.prepare_plan(request)?;
    fixture.store.approve_plan(plan.job_id, plan.version)?;
    assert_eq!(std::fs::read(&existing)?, b"unchanged fixture bytes");
    assert_eq!(std::fs::read_dir(fixture.media.path())?.count(), 1);
    assert!(!fixture.media.path().join("E").exists());
    Ok(())
}

#[test]
fn an_album_can_select_flac_without_changing_the_saved_mp3_default() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let mut request = fixture.request()?;
    request.format = Some(AudioFormat::Flac);
    let plan = fixture.store.prepare_plan(request)?;
    assert_eq!(plan.plan.format, AudioFormat::Flac);
    assert!(
        plan.plan
            .outputs
            .iter()
            .all(|output| output.filename.ends_with(".flac"))
    );
    assert_eq!(plan.plan.settings.format, AudioFormat::Mp3);
    assert_eq!(
        fixture
            .store
            .settings()?
            .ok_or("missing settings")?
            .settings
            .format,
        AudioFormat::Mp3
    );
    fixture.store.approve_plan(plan.job_id, plan.version)?;
    drop(fixture.store);
    fixture.store = Store::open_existing(&fixture.database)?;
    assert_eq!(
        fixture.store.plan(plan.job_id, plan.version)?.plan.format,
        AudioFormat::Flac
    );
    Ok(())
}

#[test]
fn reclaim_with_the_same_worker_name_rejects_every_old_claim_operation() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let plan = fixture.store.prepare_plan(request)?;
    fixture.store.approve_plan(plan.job_id, plan.version)?;
    let first = fixture
        .store
        .claim_approved_plan(plan.job_id, plan.version, "worker", 60)?
        .ok_or("missing first claim")?;
    rusqlite::Connection::open(&fixture.database)?.execute(
        "UPDATE jobs SET lease_until = 0 WHERE id = ?1",
        [plan.job_id],
    )?;
    assert!(fixture.store.renew_claim(&first.claim, 60).is_err());
    let current = fixture
        .store
        .claim_approved_plan(plan.job_id, plan.version, "worker", 60)?
        .ok_or("missing reclaimed lease")?;
    assert!(fixture.store.renew_claim(&first.claim, 60).is_err());
    assert!(
        fixture
            .store
            .advance(&first.claim, JobState::Queued, JobState::Searching)
            .is_err()
    );
    let question = Question::new("Pause?".into(), vec!["Continue".into(), "Cancel".into()])?;
    assert!(
        fixture
            .store
            .ask(&first.claim, JobState::Queued, &question)
            .is_err()
    );
    assert_eq!(
        fixture.store.job_state(plan.job_id)?.as_deref(),
        Some("queued")
    );
    fixture.store.renew_claim(&current.claim, 120)?;
    fixture
        .store
        .advance(&current.claim, JobState::Queued, JobState::Searching)?;
    Ok(())
}
