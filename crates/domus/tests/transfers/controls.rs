use super::*;
use domus::Error;
use domus::infrastructure::store::JobControlAction::{Cancel, Pause};

fn expire(fixture: &Fixture) -> TestResult {
    rusqlite::Connection::open(&fixture.database)?.execute(
        "UPDATE jobs SET lease_until=unixepoch()-1 WHERE id=?1",
        [fixture.job],
    )?;
    Ok(())
}

#[test]
fn idle_pause_survives_restart_and_needs_answer_then_explicit_continue() -> TestResult {
    let state = tempdir()?;
    let path = state.path().join("state.db");
    let mut store = Store::open(&path)?;
    let job = store.enqueue(1)?;
    let control = store.request_job_control(job, 1, Pause)?;
    let question = control.question_id.ok_or("missing pause question")?;
    assert!(control.applied_at.is_some());
    assert_eq!(control.paused_state.as_deref(), Some("queued"));
    assert_eq!(store.job_state(job)?.as_deref(), Some("awaiting_input"));
    assert!(store.continue_job_control(control.id, 1).is_err());
    assert_eq!(store.questions(Page::new(0, 20)?)?.len(), 1);
    drop(store);
    let mut store = Store::open_current_existing(&path)?;
    assert_eq!(store.job_control(control.id)?.question_id, Some(question));
    store.answer(question, 1, "Resume")?;
    assert!(store.claim(job, "generic", 60)?.is_none());
    assert!(store.continue_job_control(control.id, 2).is_err());
    assert_eq!(store.job_state(job)?.as_deref(), Some("awaiting_input"));
    assert_eq!(store.continue_job_control(control.id, 1)?, "queued");
    assert!(store.job_control(control.id)?.continued_at.is_some());
    assert!(store.continue_job_control(control.id, 1).is_err());
    assert!(store.claim(job, "worker", 60)?.is_some());
    Ok(())
}

#[test]
fn live_pause_waits_for_boundary_and_fences_original_claim() -> TestResult {
    let mut f = Fixture::new()?;
    assert!(
        f.store
            .request_job_control(f.job, f.version + 1, Pause)
            .is_err()
    );
    let control = f.store.request_job_control(f.job, f.version, Pause)?;
    assert!(control.applied_at.is_none());
    assert_eq!(f.store.job_state(f.job)?.as_deref(), Some("searching"));
    assert_eq!(
        f.store.request_job_control(f.job, f.version, Pause)?.id,
        control.id
    );
    assert!(
        f.store
            .request_job_control(f.job, f.version, Cancel)
            .is_err()
    );
    assert!(
        f.store
            .begin_acquisition(&f.claim, "generated-peer", &files())
            .is_err()
    );
    let question = Question::new("Different decision".into(), vec!["Continue".into()])?;
    assert!(
        f.store
            .ask(&f.claim, JobState::Searching, &question)
            .is_err()
    );
    assert!(
        matches!(f.store.job_work_boundary(&f.claim),Err(Error::JobStopped(id)) if id==control.id)
    );
    assert!(f.store.renew_claim(&f.claim, 60).is_err());
    let saved = f.store.job_control(control.id)?;
    f.store
        .answer(saved.question_id.ok_or("question")?, f.version, "Resume")?;
    assert!(f.store.claim(f.job, "generic", 60)?.is_none());
    assert!(
        f.store
            .claim_approved_plan(f.job, f.version, "worker", 60)?
            .is_none()
    );
    assert_eq!(
        f.store.continue_job_control(control.id, f.version)?,
        "searching"
    );
    let claim = f
        .store
        .claim_approved_plan(f.job, f.version, "worker", 60)?
        .ok_or("new claim")?
        .claim;
    assert!(f.store.job_work_boundary(&f.claim).is_err());
    f.store
        .begin_acquisition(&claim, "generated-peer", &files())?;
    Ok(())
}

#[test]
fn queued_unapproved_preview_can_resume_without_granting_approval() -> TestResult {
    let mut f = Fixture::new()?;
    let job = f.store.enqueue(1)?;
    let plan = f.store.prepare_plan(PlanRequest {
        job_id: job,
        expected_version: 1,
        settings_version: 1,
        format: None,
        edition: edition(),
        locations: vec![f.main],
        accepts_incomplete_coverage: false,
    })?;
    let control = f.store.request_job_control(job, plan.version, Pause)?;
    f.store.answer(
        control.question_id.ok_or("question")?,
        plan.version,
        "Resume",
    )?;
    assert_eq!(
        f.store.continue_job_control(control.id, plan.version)?,
        "queued"
    );
    assert!(f.store.plan(job, plan.version)?.approved_at.is_none());
    assert!(
        f.store
            .claim_approved_plan(job, plan.version, "worker", 60)
            .is_err()
    );
    f.store.approve_plan(job, plan.version)?;
    assert!(f.store.plan(job, plan.version)?.approved_at.is_some());
    Ok(())
}

#[test]
fn cancellation_preserves_uncertain_submission_through_reclaim_and_received_receipt() -> TestResult
{
    let mut f = Fixture::new()?;
    f.store
        .begin_acquisition(&f.claim, "generated-peer", &files())?;
    let rows = f.store.transfers(f.job, f.version, Page::new(0, 20)?)?;
    f.store.start_transfer(&f.claim, rows[0].id)?;
    let control = f.store.request_job_control(f.job, f.version, Cancel)?;
    assert!(control.applied_at.is_none());
    assert!(f.store.job_work_boundary(&f.claim).is_err());
    assert!(f.store.start_transfer(&f.claim, rows[1].id).is_err());
    expire(&f)?;
    assert!(
        f.store
            .request_job_control(f.job, f.version, Cancel)?
            .applied_at
            .is_none()
    );
    assert_eq!(
        f.store.transfers(f.job, f.version, Page::new(0, 20)?)?[0].state,
        "submitting"
    );
    f.store = Store::open_current_existing(&f.database)?;
    let claim = f
        .store
        .claim_approved_plan(f.job, f.version, "reconciler", 60)?
        .ok_or("claim")?
        .claim;
    assert!(f.store.accept_transfer(&f.claim, rows[0].id, 77).is_err());
    f.store.accept_transfer(&claim, rows[0].id, 77)?;
    assert!(f.store.job_work_boundary(&claim).is_err());
    f.store.receive_transfer(&claim, rows[0].id, 10, &[9; 32])?;
    assert!(
        matches!(f.store.job_work_boundary(&claim),Err(Error::JobStopped(id)) if id==control.id)
    );
    assert_eq!(f.store.job_state(f.job)?.as_deref(), Some("cancelled"));
    let retained = f.store.transfers(f.job, f.version, Page::new(0, 20)?)?;
    assert_eq!(retained[0].state, "received");
    assert_eq!(retained[0].payload_sha256, Some(vec![9; 32]));
    assert_eq!(retained[1].state, "planned");
    assert!(f.store.claim(f.job, "worker", 60)?.is_none());
    Ok(())
}

#[test]
fn pause_after_received_file_retains_source_and_resumes_only_remaining_transfer() -> TestResult {
    let mut f = Fixture::new()?;
    f.store
        .begin_acquisition(&f.claim, "generated-peer", &files())?;
    let rows = f.store.transfers(f.job, f.version, Page::new(0, 20)?)?;
    f.store.start_transfer(&f.claim, rows[0].id)?;
    f.store.accept_transfer(&f.claim, rows[0].id, 14)?;
    let control = f.store.request_job_control(f.job, f.version, Pause)?;
    f.store
        .receive_transfer(&f.claim, rows[0].id, 10, &[7; 32])?;
    assert!(matches!(
        f.store.job_work_boundary(&f.claim),
        Err(Error::JobStopped(_))
    ));
    let question = f
        .store
        .job_control(control.id)?
        .question_id
        .ok_or("question")?;
    f.store.answer(question, f.version, "Resume")?;
    assert_eq!(
        f.store.continue_job_control(control.id, f.version)?,
        "downloading"
    );
    let claim = f
        .store
        .claim_approved_plan(f.job, f.version, "worker", 60)?
        .ok_or("claim")?
        .claim;
    let retained = f.store.transfers(f.job, f.version, Page::new(0, 20)?)?;
    assert_eq!(retained[0].state, "received");
    assert_eq!(retained[0].payload_sha256, Some(vec![7; 32]));
    assert_eq!(retained[1].state, "planned");
    assert!(f.store.start_transfer(&claim, retained[0].id).is_err());
    assert!(
        f.store
            .begin_acquisition(&claim, "another-peer", &files())
            .is_err()
    );
    f.store.start_transfer(&claim, retained[1].id)?;
    Ok(())
}

#[test]
fn idle_cancel_and_pause_never_modify_publication_or_terminal_states() -> TestResult {
    let mut f = Fixture::new()?;
    expire(&f)?;
    let raw = rusqlite::Connection::open(&f.database)?;
    for state in [
        "publishing",
        "backup_pending",
        "complete",
        "failed",
        "cancelled",
    ] {
        raw.execute(
            "UPDATE jobs SET state=?2 WHERE id=?1",
            rusqlite::params![f.job, state],
        )?;
        for action in [Pause, Cancel] {
            assert!(
                f.store
                    .request_job_control(f.job, f.version, action)
                    .is_err()
            );
            assert_eq!(f.store.job_state(f.job)?.as_deref(), Some(state));
        }
    }
    assert_eq!(
        raw.query_row("SELECT count(*) FROM job_controls", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    raw.execute("UPDATE jobs SET state='searching' WHERE id=?1", [f.job])?;
    let control = f.store.request_job_control(f.job, f.version, Cancel)?;
    assert!(control.applied_at.is_some());
    assert!(control.question_id.is_none());
    assert_eq!(f.store.job_state(f.job)?.as_deref(), Some("cancelled"));
    Ok(())
}

#[test]
fn resume_rechecks_observations_but_explicit_cancel_needs_no_new_media_authority() -> TestResult {
    for choice in ["Resume", "Cancel"] {
        let mut f = Fixture::new()?;
        expire(&f)?;
        let control = f.store.request_job_control(f.job, f.version, Pause)?;
        let question = control.question_id.ok_or("question")?;
        f.store.answer(question, f.version, choice)?;
        scan_location(&mut f.store, f.main)?;
        if choice == "Resume" {
            assert!(f.store.continue_job_control(control.id, f.version).is_err());
            assert_eq!(f.store.job_state(f.job)?.as_deref(), Some("awaiting_input"));
            assert!(f.store.job_control(control.id)?.continued_at.is_none());
        } else {
            assert_eq!(
                f.store.continue_job_control(control.id, f.version)?,
                "cancelled"
            );
        }
    }
    Ok(())
}

#[test]
fn paused_jobs_cannot_bypass_control_with_a_new_plan() -> TestResult {
    let mut f = Fixture::new()?;
    expire(&f)?;
    let control = f.store.request_job_control(f.job, f.version, Pause)?;
    assert!(
        f.store
            .prepare_plan(PlanRequest {
                job_id: f.job,
                expected_version: f.version,
                settings_version: 1,
                format: None,
                edition: edition(),
                locations: vec![f.main],
                accepts_incomplete_coverage: false,
            })
            .is_err()
    );
    assert_eq!(f.store.job_state(f.job)?.as_deref(), Some("awaiting_input"));
    assert!(f.store.job_control(control.id)?.continued_at.is_none());
    Ok(())
}

#[test]
fn applied_control_intents_and_receipts_are_immutable_and_retained() -> TestResult {
    let state = tempdir()?;
    let path = state.path().join("state.db");
    let mut store = Store::open(&path)?;
    let job = store.enqueue(1)?;
    let control = store.request_job_control(job, 1, Pause)?;
    let raw = rusqlite::Connection::open(&path)?;
    for sql in [
        "UPDATE job_controls SET action='cancel' WHERE id=?1",
        "UPDATE job_controls SET plan_version=2 WHERE id=?1",
        "UPDATE job_controls SET applied_at=NULL,paused_state=NULL,question_id=NULL WHERE id=?1",
        "DELETE FROM job_controls WHERE id=?1",
    ] {
        assert!(raw.execute(sql, [control.id]).is_err());
    }
    store.answer(control.question_id.ok_or("question")?, 1, "Resume")?;
    store.continue_job_control(control.id, 1)?;
    assert!(
        raw.execute(
            "UPDATE job_controls SET continued_at=NULL WHERE id=?1",
            [control.id]
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn control_history_pages_by_job_version_and_durable_cursor() -> TestResult {
    let state = tempdir()?;
    let path = state.path().join("state.db");
    let mut store = Store::open(&path)?;
    let job = store.enqueue(1)?;
    let mut ids = Vec::new();
    for _ in 0..5 {
        let control = store.request_job_control(job, 1, Pause)?;
        ids.push(control.id);
        store.answer(control.question_id.ok_or("question")?, 1, "Resume")?;
        store.continue_job_control(control.id, 1)?;
    }
    let first = store.job_controls(job, 1, Page::new(0, 2)?)?;
    assert_eq!(
        first.iter().map(|control| control.id).collect::<Vec<_>>(),
        ids[..2]
    );
    let second = store.job_controls(job, 1, Page::new(first[1].id, 2)?)?;
    assert_eq!(
        second.iter().map(|control| control.id).collect::<Vec<_>>(),
        ids[2..4]
    );
    let last = store.job_controls(job, 1, Page::new(second[1].id, 2)?)?;
    assert_eq!(last[0].id, ids[4]);
    assert_eq!(last.len(), 1);
    assert!(
        store
            .job_controls(job, 1, Page::new(last[0].id, 2)?)?
            .is_empty()
    );
    assert!(store.job_controls(job, 2, Page::new(0, 2)?)?.is_empty());
    let other = store.enqueue(1)?;
    assert!(store.job_controls(other, 1, Page::new(0, 2)?)?.is_empty());
    Ok(())
}

#[test]
fn failed_pause_rolls_back_request_question_job_and_events() -> TestResult {
    let state = tempdir()?;
    let path = state.path().join("state.db");
    let mut store = Store::open(&path)?;
    let job = store.enqueue(1)?;
    let raw = rusqlite::Connection::open(&path)?;
    let events: i64 = raw.query_row("SELECT count(*) FROM events", [], |row| row.get(0))?;
    raw.execute_batch("CREATE TRIGGER reject_control_question BEFORE INSERT ON questions BEGIN SELECT RAISE(ABORT,'generated failure'); END;")?;
    assert!(store.request_job_control(job, 1, Pause).is_err());
    assert_eq!(store.job_state(job)?.as_deref(), Some("queued"));
    for table in ["job_controls", "questions"] {
        assert_eq!(
            raw.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))?,
            0
        );
    }
    assert_eq!(
        raw.query_row("SELECT count(*) FROM events", [], |row| row
            .get::<_, i64>(0))?,
        events
    );
    Ok(())
}
