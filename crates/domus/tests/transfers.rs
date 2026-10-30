use domus::application::{add_location, scan_location};
use domus::domain::{EditionSelection, JobState, OutputSettings, Page, PlannedTrack, Question};
use domus::infrastructure::store::{JobClaim, PlanRequest, SourceFile, Store};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tempfile::{TempDir, tempdir};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[path = "transfers/catalog.rs"]
mod catalog_tests;
#[path = "transfers/controls.rs"]
mod control_tests;
#[path = "transfers/progress.rs"]
mod progress_tests;
#[path = "transfers/sources.rs"]
mod source_tests;
mod support;

struct Fixture {
    store: Store,
    claim: JobClaim,
    job: i64,
    version: i64,
    main: i64,
    database: PathBuf,
    _state: TempDir,
    _media: TempDir,
}

fn edition() -> EditionSelection {
    EditionSelection {
        release_id: "12345678-1234-4321-9876-123456789abc".into(),
        album_artist: "Generated Artist".into(),
        album: "Generated Album".into(),
        bucket: None,
        tracks: ["First", "Second"]
            .into_iter()
            .map(|title| PlannedTrack {
                artist: "Generated Artist".into(),
                title: title.into(),
                disc: Some(1),
                disc_track: None,
            })
            .collect(),
    }
}

fn files() -> Vec<SourceFile> {
    ["First.mp3", "Second.mp3"]
        .into_iter()
        .map(|name| SourceFile {
            remote_path: format!("Peer Music\\Generated Album\\{name}"),
            expected_bytes: 10,
        })
        .collect()
}

impl Fixture {
    fn new() -> TestResult<Self> {
        Self::with_edition(edition())
    }

    fn with_edition(edition: EditionSelection) -> TestResult<Self> {
        let state = tempdir()?;
        let media = tempdir()?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let main = add_location(&store, media.path(), "main")?;
        scan_location(&mut store, main)?;
        store.save_settings(None, &OutputSettings::default())?;
        let job = store.enqueue(1)?;
        let catalog = support::catalog_snapshot(&database, &edition)?;
        let plan = store.prepare_catalog_plan(
            PlanRequest {
                audio_profile: None,
                job_id: job,
                expected_version: 1,
                settings_version: 1,
                format: None,
                edition: catalog.data.selection(None)?,
                locations: vec![main],
                accepts_incomplete_coverage: false,
            },
            catalog,
        )?;
        store.approve_plan(job, plan.version)?;
        let claimed = store
            .claim_approved_plan(job, plan.version, "worker", 60)?
            .ok_or("claim missing")?;
        store.advance(&claimed.claim, JobState::Queued, JobState::Searching)?;
        Ok(Self {
            store,
            claim: claimed.claim,
            job,
            version: plan.version,
            main,
            database,
            _state: state,
            _media: media,
        })
    }
}

#[test]
fn transfer_intent_survives_reclaim_without_duplicate_submission() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture
        .store
        .begin_acquisition(&fixture.claim, "generated-peer", &files())?;
    let rows = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 1)?)?;
    assert_eq!(rows.len(), 1);
    let first = rows[0].id;
    assert_eq!(rows[0].state, "planned");
    assert_eq!(rows[0].peer, "generated-peer");
    assert!(
        fixture
            .store
            .advance(&fixture.claim, JobState::Downloading, JobState::Validating)
            .is_err()
    );
    fixture.store.start_transfer(&fixture.claim, first)?;
    assert!(fixture.store.start_transfer(&fixture.claim, first).is_err());
    drop(fixture.store);
    let raw = rusqlite::Connection::open(&fixture.database)?;
    raw.execute(
        "UPDATE jobs SET lease_until = unixepoch() - 1 WHERE id = ?1",
        [fixture.job],
    )?;
    fixture.store = Store::open_existing(&fixture.database)?;
    let new_claim = fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "worker", 60)?
        .ok_or("reclaim missing")?
        .claim;
    let digest: [u8; 32] = Sha256::digest(b"0123456789").into();
    assert!(fixture.store.start_transfer(&fixture.claim, first).is_err());
    assert!(
        fixture
            .store
            .accept_transfer(&fixture.claim, first, 7)
            .is_err()
    );
    assert!(
        fixture
            .store
            .transfer_progress(&fixture.claim, first, 3)
            .is_err()
    );
    assert!(
        fixture
            .store
            .receive_transfer(&fixture.claim, first, 10, &digest)
            .is_err()
    );
    assert!(fixture.store.start_transfer(&new_claim, first).is_err());
    assert!(
        fixture
            .store
            .begin_acquisition(&new_claim, "another-peer", &files())
            .is_err()
    );
    let rows = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].state, "submitting");
    assert!(rows[0].remote_token.is_none());
    fixture.store.accept_transfer(&new_claim, first, 7)?;
    assert!(
        fixture
            .store
            .start_transfer(&fixture.claim, rows[1].id)
            .is_err()
    );
    assert!(
        fixture
            .store
            .transfer_progress(&fixture.claim, first, 3)
            .is_err()
    );
    assert!(
        fixture
            .store
            .receive_transfer(&fixture.claim, first, 10, &digest)
            .is_err()
    );
    assert!(fixture.store.accept_transfer(&new_claim, first, 8).is_err());
    fixture.store.transfer_progress(&new_claim, first, 5)?;
    assert!(
        fixture
            .store
            .transfer_progress(&new_claim, first, 4)
            .is_err()
    );
    assert!(
        fixture
            .store
            .transfer_progress(&new_claim, first, 11)
            .is_err()
    );
    assert!(
        fixture
            .store
            .receive_transfer(&new_claim, first, 9, &digest)
            .is_err()
    );
    fixture
        .store
        .receive_transfer(&new_claim, first, 10, &digest)?;
    assert!(
        fixture
            .store
            .advance(&new_claim, JobState::Downloading, JobState::Validating)
            .is_err()
    );
    let next = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(first, 1)?)?;
    assert_eq!(next.len(), 1);
    fixture.store.start_transfer(&new_claim, next[0].id)?;
    fixture.store.accept_transfer(&new_claim, next[0].id, 0)?;
    fixture
        .store
        .receive_transfer(&new_claim, next[0].id, 10, &digest)?;
    fixture
        .store
        .advance(&new_claim, JobState::Downloading, JobState::Validating)?;
    let rows = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    assert!(
        rows.iter().all(|row| row.state == "received"
            && row.payload_sha256.as_deref() == Some(digest.as_slice()))
    );
    assert!(std::fs::read_dir(fixture._media.path())?.next().is_none());
    assert!(
        raw.execute("UPDATE transfer_files SET expected_bytes = 99", [])
            .is_err()
    );
    assert!(raw.execute("DELETE FROM transfer_files", []).is_err());
    assert!(
        raw.execute("UPDATE acquisition_sources SET peer = 'changed'", [])
            .is_err()
    );
    Ok(())
}

#[test]
fn incomplete_or_invalid_sources_leave_no_intent() -> TestResult {
    let mut fixture = Fixture::new()?;
    assert!(
        fixture
            .store
            .advance(&fixture.claim, JobState::Searching, JobState::Downloading)
            .is_err()
    );
    for invalid in [
        vec![
            SourceFile {
                remote_path: "Album\\First.mp3".into(),
                expected_bytes: 10,
            },
            SourceFile {
                remote_path: "album/FIRST.MP3".into(),
                expected_bytes: 10,
            },
        ],
        vec![
            SourceFile {
                remote_path: "Album\\..\\First.mp3".into(),
                expected_bytes: 10,
            },
            SourceFile {
                remote_path: "Album\\Second.mp3".into(),
                expected_bytes: 10,
            },
        ],
        vec![SourceFile {
            remote_path: "Only.mp3".into(),
            expected_bytes: 10,
        }],
        vec![
            SourceFile {
                remote_path: "First.mp3".into(),
                expected_bytes: 10,
            },
            SourceFile {
                remote_path: "FIRST.MP3".into(),
                expected_bytes: 10,
            },
        ],
        vec![
            SourceFile {
                remote_path: "First.flac".into(),
                expected_bytes: 10,
            },
            SourceFile {
                remote_path: "Second.mp3".into(),
                expected_bytes: 10,
            },
        ],
        vec![
            SourceFile {
                remote_path: "First.mp3".into(),
                expected_bytes: 0,
            },
            SourceFile {
                remote_path: "Second.mp3".into(),
                expected_bytes: 10,
            },
        ],
        vec![
            SourceFile {
                remote_path: "First.mp3".into(),
                expected_bytes: i64::MAX,
            },
            SourceFile {
                remote_path: "Second.mp3".into(),
                expected_bytes: 10,
            },
        ],
    ] {
        assert!(
            fixture
                .store
                .begin_acquisition(&fixture.claim, "generated-peer", &invalid)
                .is_err()
        );
    }
    assert!(
        fixture
            .store
            .begin_acquisition(&fixture.claim, "bad\npeer", &files())
            .is_err()
    );
    assert!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("searching")
    );
    Ok(())
}

#[test]
fn failed_intent_transaction_rolls_back_source_files_state_and_event() -> TestResult {
    let mut fixture = Fixture::new()?;
    let raw = rusqlite::Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER fixture_failure BEFORE INSERT ON transfer_files WHEN NEW.track_number = 2
         BEGIN SELECT RAISE(ABORT, 'isolated injected failure'); END;",
    )?;
    assert!(
        fixture
            .store
            .begin_acquisition(&fixture.claim, "generated-peer", &files())
            .is_err()
    );
    assert_eq!(
        raw.query_row("SELECT COUNT(*) FROM acquisition_sources", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    assert_eq!(
        raw.query_row("SELECT COUNT(*) FROM transfer_files", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    assert_eq!(
        raw.query_row(
            "SELECT COUNT(*) FROM events WHERE event_type = 'acquisition_prepared'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("searching")
    );
    raw.execute_batch("DROP TRIGGER fixture_failure")?;
    fixture
        .store
        .begin_acquisition(&fixture.claim, "generated-peer", &files())?;
    Ok(())
}

#[test]
fn unresolved_submission_blocks_plan_revision_even_while_awaiting_input() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture
        .store
        .begin_acquisition(&fixture.claim, "generated-peer", &files())?;
    let rows = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    fixture.store.start_transfer(&fixture.claim, rows[0].id)?;
    assert!(
        fixture
            .store
            .advance(&fixture.claim, JobState::Downloading, JobState::Cancelled)
            .is_err()
    );
    assert!(
        fixture
            .store
            .advance(&fixture.claim, JobState::Downloading, JobState::Failed)
            .is_err()
    );
    fixture.store.ask(
        &fixture.claim,
        JobState::Downloading,
        &Question::new(
            "Generated decision".into(),
            vec!["Wait".into(), "Review".into()],
        )?,
    )?;
    assert!(
        fixture
            .store
            .prepare_plan(PlanRequest {
                audio_profile: None,
                job_id: fixture.job,
                expected_version: fixture.version,
                settings_version: 1,
                format: None,
                edition: edition(),
                locations: vec![fixture.main],
                accepts_incomplete_coverage: false,
            })
            .is_err()
    );
    assert_eq!(
        fixture.store.plan(fixture.job, fixture.version)?.version,
        fixture.version
    );
    assert_eq!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?[0]
            .state,
        "submitting"
    );
    Ok(())
}

#[test]
fn changed_index_stops_new_submissions_but_does_not_lose_existing_receipts() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture
        .store
        .begin_acquisition(&fixture.claim, "generated-peer", &files())?;
    let rows = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    fixture.store.start_transfer(&fixture.claim, rows[0].id)?;
    scan_location(&mut fixture.store, fixture.main)?;
    fixture
        .store
        .accept_transfer(&fixture.claim, rows[0].id, 7)?;
    fixture
        .store
        .transfer_progress(&fixture.claim, rows[0].id, 5)?;
    let digest: [u8; 32] = Sha256::digest(b"0123456789").into();
    fixture
        .store
        .receive_transfer(&fixture.claim, rows[0].id, 10, &digest)?;
    assert!(
        fixture
            .store
            .start_transfer(&fixture.claim, rows[1].id)
            .is_err()
    );
    let current = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    assert_eq!(current[0].state, "received");
    assert_eq!(current[1].state, "planned");
    Ok(())
}
