use super::*;
use domus::domain::sources::{SourceFileMetadata, SourceFolder};
use domus::infrastructure::store::SourceCandidate;
use rusqlite::Connection;

fn listing() -> SourceFolder {
    SourceFolder {
        peer: "generated-peer".into(),
        folder: "Peer Music\\Generated Album".into(),
        directories: 1,
        files: files()
            .into_iter()
            .map(|file| SourceFileMetadata {
                remote_path: file.remote_path,
                expected_bytes: file.expected_bytes,
                extension: "mp3".into(),
                bitrate_kbps: Some(320),
                duration_seconds: Some(30),
                vbr: Some(false),
                sample_rate: Some(44100),
                bit_depth: None,
            })
            .collect(),
    }
}

fn idle() -> TestResult<Fixture> {
    let fixture = Fixture::new()?;
    Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
    Ok(fixture)
}

fn prepare(fixture: &mut Fixture) -> TestResult<SourceCandidate> {
    Ok(fixture
        .store
        .prepare_source_candidate(fixture.job, fixture.version, listing())?)
}

fn approve(fixture: &mut Fixture, candidate: &SourceCandidate) -> TestResult<SourceCandidate> {
    Ok(fixture.store.approve_source_candidate(
        candidate.id,
        fixture.version,
        &candidate.review_sha256,
        true,
    )?)
}

fn claim(fixture: &mut Fixture) -> TestResult<JobClaim> {
    Ok(fixture
        .store
        .claim_approved_plan(fixture.job, fixture.version, "reviewed-worker", 300)?
        .ok_or("claim")?
        .claim)
}

#[test]
fn source_review_is_separate_exact_digest_approval_and_preserves_unknown_proofs() -> TestResult {
    let mut fixture = idle()?;
    let candidate = prepare(&mut fixture)?;
    assert_eq!(candidate.job_id, fixture.job);
    assert!(candidate.approved_at.is_none());
    assert!(
        candidate.assessment.complete_mapping && candidate.assessment.advertised_quality_matches
    );
    assert!(!candidate.assessment.edition_verified && !candidate.assessment.audio_verified);
    assert!(candidate.assessment.requires_user_review);
    assert_eq!(candidate.plan_sha256.len(), 64);
    assert_eq!(candidate.review_sha256.len(), 64);
    let duplicate = prepare(&mut fixture)?;
    assert_eq!(duplicate.id, candidate.id);
    assert!(
        fixture
            .store
            .approve_source_candidate(
                candidate.id,
                fixture.version,
                &candidate.review_sha256,
                false
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .approve_source_candidate(candidate.id, fixture.version, &"0".repeat(64), true)
            .is_err()
    );
    assert!(
        fixture
            .store
            .approve_source_candidate(
                candidate.id,
                fixture.version + 1,
                &candidate.review_sha256,
                true
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    let approved = approve(&mut fixture, &candidate)?;
    assert!(approved.approved_at.is_some());
    assert!(!approved.assessment.edition_verified && !approved.assessment.audio_verified);
    assert!(approved.assessment.requires_user_review);
    assert!(approve(&mut fixture, &candidate).is_err());
    assert!(
        fixture
            .store
            .acquisition_source_candidate(fixture.job, fixture.version)?
            .is_none()
    );
    let reopened = Store::open_read_snapshot(&fixture.database)?;
    assert_eq!(
        serde_json::to_value(reopened.source_candidate(candidate.id)?)?,
        serde_json::to_value(approved)?
    );
    Ok(())
}

#[test]
fn source_review_commits_exact_source_and_receipt_in_one_fenced_transaction() -> TestResult {
    let mut fixture = idle()?;
    let candidate = prepare(&mut fixture)?;
    approve(&mut fixture, &candidate)?;
    let current = claim(&mut fixture)?;
    assert!(
        fixture
            .store
            .begin_reviewed_acquisition(&fixture.claim, candidate.id)
            .is_err()
    );
    fixture
        .store
        .begin_reviewed_acquisition(&current, candidate.id)?;
    assert_eq!(
        fixture
            .store
            .acquisition_source_candidate(fixture.job, fixture.version)?,
        Some(candidate.id)
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("downloading")
    );
    let transfers = fixture
        .store
        .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
    assert_eq!(transfers.len(), 2);
    for (transfer, expected) in transfers.iter().zip(files()) {
        assert_eq!(transfer.peer, "generated-peer");
        assert_eq!(transfer.remote_path, expected.remote_path);
        assert_eq!(transfer.expected_bytes, expected.expected_bytes);
        assert_eq!(transfer.state, "planned");
    }
    assert!(
        fixture
            .store
            .begin_reviewed_acquisition(&current, candidate.id)
            .is_err()
    );
    assert!(prepare(&mut fixture).is_err());
    let raw = Connection::open(&fixture.database)?;
    for query in [
        "UPDATE source_candidates SET listing='{}'",
        "UPDATE source_candidates SET review_sha256=printf('%064d',0)",
        "UPDATE source_candidates SET plan_version=99",
        "UPDATE source_candidates SET approved_at=NULL",
        "DELETE FROM source_candidates",
        "UPDATE acquisition_source_reviews SET candidate_id=99",
        "DELETE FROM acquisition_source_reviews",
    ] {
        assert!(raw.execute(query, []).is_err(), "{query}");
    }
    Ok(())
}

#[test]
fn source_review_does_not_invent_receipts_for_manual_selection_or_legacy_upgrade() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture
        .store
        .begin_acquisition(&fixture.claim, "generated-peer", &files())?;
    assert!(
        fixture
            .store
            .acquisition_source_candidate(fixture.job, fixture.version)?
            .is_none()
    );
    let before = serde_json::to_value(fixture.store.transfers(
        fixture.job,
        fixture.version,
        Page::new(0, 200)?,
    )?)?;
    drop(fixture.store);
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations; DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; PRAGMA user_version=17;")?;
    assert!(Store::open_current_existing(&fixture.database).is_err());
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        17
    );
    fixture.store = Store::open_existing(&fixture.database)?;
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        23
    );
    assert!(
        fixture
            .store
            .acquisition_source_candidate(fixture.job, fixture.version)?
            .is_none()
    );
    assert_eq!(
        serde_json::to_value(fixture.store.transfers(
            fixture.job,
            fixture.version,
            Page::new(0, 200)?
        )?)?,
        before
    );
    Ok(())
}

#[test]
fn source_review_never_approves_incomplete_ambiguous_or_advertised_bad_inputs() -> TestResult {
    let mut fixture = idle()?;
    for fault in [
        "missing",
        "extra",
        "ambiguous",
        "bad-quality",
        "vbr",
        "bad-quality-without-vbr",
        "vbr-without-bitrate",
        "mixed-format",
    ] {
        let mut folder = listing();
        match fault {
            "missing" => {
                folder.files.pop();
            }
            "extra" => {
                let mut extra = folder.files[0].clone();
                extra.remote_path = "Peer Music\\Generated Album\\Bonus.mp3".into();
                folder.files.push(extra);
            }
            "ambiguous" => {
                let mut duplicate = folder.files[0].clone();
                duplicate.remote_path = "Peer Music\\Generated Album\\01 - First.mp3".into();
                folder.files.push(duplicate);
            }
            "bad-quality" => folder.files[0].bitrate_kbps = Some(256),
            "vbr" => folder.files[0].vbr = Some(true),
            "bad-quality-without-vbr" => {
                folder.files[0].bitrate_kbps = Some(256);
                folder.files[0].vbr = None;
            }
            "vbr-without-bitrate" => {
                folder.files[0].bitrate_kbps = None;
                folder.files[0].vbr = Some(true);
            }
            "mixed-format" => {
                folder.files[0].remote_path = "Peer Music\\Generated Album\\First.flac".into();
                folder.files[0].extension = "flac".into();
            }
            _ => return Err("fixture fault".into()),
        }
        let candidate =
            fixture
                .store
                .prepare_source_candidate(fixture.job, fixture.version, folder)?;
        assert!(!candidate.assessment.advertised_quality_matches, "{fault}");
        assert!(approve(&mut fixture, &candidate).is_err(), "{fault}");
        let current = claim(&mut fixture)?;
        assert!(
            fixture
                .store
                .begin_reviewed_acquisition(&current, candidate.id)
                .is_err()
        );
        Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
    }
    assert!(
        fixture
            .store
            .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?
            .is_empty()
    );
    let mut overflow = listing();
    overflow.files[0].expected_bytes = i64::MAX;
    let candidate =
        fixture
            .store
            .prepare_source_candidate(fixture.job, fixture.version, overflow)?;
    assert!(approve(&mut fixture, &candidate).is_err());
    Ok(())
}

#[test]
fn source_review_accepts_unknown_attributes_without_inventing_audio_proof() -> TestResult {
    for (bitrate, vbr) in [(Some(320), None), (None, Some(false)), (None, None)] {
        let mut fixture = idle()?;
        let mut folder = listing();
        for file in &mut folder.files {
            file.bitrate_kbps = bitrate;
            file.vbr = vbr;
        }
        let candidate =
            fixture
                .store
                .prepare_source_candidate(fixture.job, fixture.version, folder)?;
        assert!(candidate.assessment.can_approve);
        assert!(!candidate.assessment.advertised_quality_matches);
        assert!(!candidate.assessment.audio_verified);
        assert!(
            fixture
                .store
                .approve_source_candidate(
                    candidate.id,
                    fixture.version,
                    &candidate.review_sha256,
                    false
                )
                .is_err()
        );
        let approved = approve(&mut fixture, &candidate)?;
        assert!(!approved.assessment.audio_verified);
        assert_eq!(approved.listing.files[0].bitrate_kbps, bitrate);
        assert_eq!(approved.listing.files[0].vbr, vbr);
        let current = claim(&mut fixture)?;
        fixture
            .store
            .begin_reviewed_acquisition(&current, candidate.id)?;
        let transfers =
            fixture
                .store
                .transfers(fixture.job, fixture.version, Page::new(0, 200)?)?;
        assert_eq!(transfers.len(), 2);
        assert!(transfers.iter().all(|transfer| transfer.state == "planned"));
    }
    Ok(())
}

#[test]
fn source_review_rechecks_idle_state_observations_pending_control_and_job_binding() -> TestResult {
    let mut fixture = Fixture::new()?;
    assert!(prepare(&mut fixture).is_err());
    Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
    let candidate = prepare(&mut fixture)?;
    let current = claim(&mut fixture)?;
    assert!(approve(&mut fixture, &candidate).is_err());
    Connection::open(&fixture.database)?.execute("UPDATE jobs SET lease_until=0", [])?;
    scan_location(&mut fixture.store, fixture.main)?;
    assert!(approve(&mut fixture, &candidate).is_err());
    assert!(prepare(&mut fixture).is_err());
    assert!(
        fixture
            .store
            .begin_reviewed_acquisition(&current, candidate.id)
            .is_err()
    );
    let mut other = idle()?;
    let candidate = prepare(&mut other)?;
    approve(&mut other, &candidate)?;
    let current = claim(&mut other)?;
    other.store.request_job_control(
        other.job,
        other.version,
        domus::infrastructure::store::JobControlAction::Pause,
    )?;
    assert!(
        other
            .store
            .begin_reviewed_acquisition(&current, candidate.id)
            .is_err()
    );
    assert!(
        other
            .store
            .transfers(other.job, other.version, Page::new(0, 200)?)?
            .is_empty()
    );
    Ok(())
}

#[test]
fn source_review_intent_and_approval_roll_back_with_events() -> TestResult {
    let mut fixture = idle()?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch("CREATE TRIGGER fail_candidate_event BEFORE INSERT ON events WHEN NEW.event_type='source_candidate_prepared' BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;
    assert!(prepare(&mut fixture).is_err());
    assert_eq!(
        raw.query_row("SELECT count(*) FROM source_candidates", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    raw.execute_batch("DROP TRIGGER fail_candidate_event;")?;
    let candidate = prepare(&mut fixture)?;
    raw.execute_batch("CREATE TRIGGER fail_approval_event BEFORE INSERT ON events WHEN NEW.event_type='source_candidate_approved' BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;
    assert!(approve(&mut fixture, &candidate).is_err());
    assert!(
        fixture
            .store
            .source_candidate(candidate.id)?
            .approved_at
            .is_none()
    );
    raw.execute_batch("DROP TRIGGER fail_approval_event;")?;
    approve(&mut fixture, &candidate)?;
    let current = claim(&mut fixture)?;
    raw.execute_batch("CREATE TRIGGER fail_reviewed_file BEFORE INSERT ON transfer_files WHEN NEW.track_number=2 BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;
    assert!(
        fixture
            .store
            .begin_reviewed_acquisition(&current, candidate.id)
            .is_err()
    );
    assert_eq!(
        raw.query_row("SELECT count(*) FROM acquisition_sources", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM acquisition_source_reviews",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
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
    raw.execute_batch("DROP TRIGGER fail_reviewed_file;")?;
    fixture
        .store
        .begin_reviewed_acquisition(&current, candidate.id)?;
    Ok(())
}

#[test]
fn source_review_has_bounded_retained_history_and_idempotent_preparation() -> TestResult {
    let mut fixture = idle()?;
    let mut ids = vec![];
    for number in 0..25 {
        let mut folder = listing();
        folder.files[0].expected_bytes = number + 1;
        let candidate =
            fixture
                .store
                .prepare_source_candidate(fixture.job, fixture.version, folder)?;
        ids.push(candidate.id);
    }
    let mut extra = listing();
    extra.files[0].expected_bytes = 99;
    assert!(
        fixture
            .store
            .prepare_source_candidate(fixture.job, fixture.version, extra)
            .is_err()
    );
    let duplicate = prepare(&mut fixture)?;
    assert_eq!(duplicate.id, ids[9]);
    let mut seen = vec![];
    let mut after = 0;
    loop {
        let page = fixture.store.source_candidates(
            fixture.job,
            fixture.version,
            Page::new(after, 200)?,
        )?;
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 5);
        after = page.last().ok_or("history")?.id;
        seen.extend(page.into_iter().map(|candidate| candidate.id));
    }
    assert_eq!(seen, ids);
    Ok(())
}

#[test]
fn source_review_rejects_unapproved_plan_and_remains_readable_after_replan() -> TestResult {
    let mut fixture = idle()?;
    let candidate = prepare(&mut fixture)?;
    Connection::open(&fixture.database)?.execute("UPDATE jobs SET state='queued'", [])?;
    let mut replacement = edition();
    replacement.release_id = "12345678-1234-4321-9876-123456789abd".into();
    let catalog = support::catalog_snapshot(&fixture.database, &replacement)?;
    let next = fixture.store.prepare_catalog_plan(
        PlanRequest {
            job_id: fixture.job,
            expected_version: fixture.version,
            settings_version: 1,
            format: None,
            edition: catalog.data.selection(None)?,
            locations: vec![fixture.main],
            accepts_incomplete_coverage: false,
        },
        catalog,
    )?;
    assert!(
        fixture
            .store
            .approve_source_candidate(
                candidate.id,
                fixture.version,
                &candidate.review_sha256,
                true
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .prepare_source_candidate(fixture.job, next.version, listing())
            .is_err()
    );
    assert_eq!(
        fixture.store.source_candidate(candidate.id)?.review_sha256,
        candidate.review_sha256
    );
    fixture.store.approve_plan(fixture.job, next.version)?;
    let current = fixture
        .store
        .claim_approved_plan(fixture.job, next.version, "new-version", 300)?
        .ok_or("new claim")?
        .claim;
    fixture
        .store
        .advance(&current, JobState::Queued, JobState::Searching)?;
    assert!(
        fixture
            .store
            .begin_reviewed_acquisition(&current, candidate.id)
            .is_err()
    );
    Ok(())
}

#[test]
fn source_review_rejects_another_jobs_candidate_and_oversized_listing() -> TestResult {
    let mut fixture = idle()?;
    let candidate = prepare(&mut fixture)?;
    approve(&mut fixture, &candidate)?;
    let job = fixture.store.enqueue(1)?;
    let mut edition = edition();
    edition.release_id = "12345678-1234-4321-9876-123456789abd".into();
    let catalog = support::catalog_snapshot(&fixture.database, &edition)?;
    let plan = fixture.store.prepare_catalog_plan(
        PlanRequest {
            job_id: job,
            expected_version: 1,
            settings_version: 1,
            format: None,
            edition: catalog.data.selection(None)?,
            locations: vec![fixture.main],
            accepts_incomplete_coverage: false,
        },
        catalog,
    )?;
    fixture.store.approve_plan(job, plan.version)?;
    let current = fixture
        .store
        .claim_approved_plan(job, plan.version, "another-job", 300)?
        .ok_or("claim")?
        .claim;
    fixture
        .store
        .advance(&current, JobState::Queued, JobState::Searching)?;
    assert!(
        fixture
            .store
            .begin_reviewed_acquisition(&current, candidate.id)
            .is_err()
    );
    assert!(
        fixture
            .store
            .transfers(job, plan.version, Page::new(0, 200)?)?
            .is_empty()
    );
    let mut oversized = listing();
    oversized.files = (0..600)
        .map(|index| {
            let mut file = listing().files.remove(0);
            file.remote_path = format!(
                "Peer Music\\Generated Album\\{}-{index}.mp3",
                "x".repeat(1800)
            );
            file
        })
        .collect();
    assert!(
        fixture
            .store
            .prepare_source_candidate(fixture.job, fixture.version, oversized)
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .source_candidates(fixture.job, fixture.version, Page::new(0, 200)?)?
            .len(),
        1
    );
    Ok(())
}
