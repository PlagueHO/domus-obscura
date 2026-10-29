use domus::domain::Page;
use domus::domain::{JobState, Question};
use domus::infrastructure::store::{JobControlAction, QuestionRecovery, Store};
use tempfile::tempdir;

#[test]
fn questions_and_answers_survive_restart_without_auto_resuming()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("state.db");
    let mut store = Store::open(&database)?;
    let job = store.enqueue(1)?;
    let claim = store.claim(job, "worker", 60)?.ok_or("missing claim")?;
    let question = Question::new(
        "Choose an available format".into(),
        vec!["FLAC".into(), "Cancel".into()],
    )?;
    let id = store.ask(&claim, JobState::Queued, &question)?;
    assert!(store.claim(job, "other-worker", 60)?.is_none());
    drop(store);
    let mut store = Store::open(&database)?;
    assert_eq!(store.question(id)?.choices, vec!["FLAC", "Cancel"]);
    assert!(store.answer(id, 1, "Not offered").is_err());
    store.answer(id, 1, "FLAC")?;
    assert!(store.answer(id, 1, "Cancel").is_err());
    drop(store);
    let store = Store::open(&database)?;
    assert_eq!(store.question(id)?.answer.as_deref(), Some("FLAC"));
    assert_eq!(store.job_state(job)?.as_deref(), Some("awaiting_input"));
    assert!(store.claim(job, "new-worker", 60)?.is_some());
    Ok(())
}

#[test]
fn a_changed_plan_invalidates_an_old_question() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("state.db");
    let mut store = Store::open(&database)?;
    let job = store.enqueue(1)?;
    let claim = store.claim(job, "worker", 60)?.ok_or("missing claim")?;
    let question = Question::new(
        "Approve this plan?".into(),
        vec!["Approve".into(), "Cancel".into()],
    )?;
    let id = store.ask(&claim, JobState::Queued, &question)?;
    rusqlite::Connection::open(&database)?
        .execute("UPDATE jobs SET plan_version = 2 WHERE id = ?1", [job])?;
    assert!(store.answer(id, 1, "Approve").is_err());
    let old = store.question_details(id)?;
    assert!(!old.current);
    assert_eq!(old.current_plan_version, 2);
    assert!(store.inbox(Page::new(0, 50)?)?.is_empty());
    assert!(store.question(id)?.answer.is_none());
    rusqlite::Connection::open(&database)?
        .execute("UPDATE jobs SET state = 'queued' WHERE id = ?1", [job])?;
    let current_claim = store
        .claim(job, "new-worker", 60)?
        .ok_or("missing new claim")?;
    let current = store.ask(&current_claim, JobState::Queued, &question)?;
    store.answer(current, 2, "Cancel")?;
    assert!(store.question(id)?.answer.is_none());
    assert_eq!(store.question(current)?.answer.as_deref(), Some("Cancel"));
    Ok(())
}

#[test]
fn inbox_keeps_saved_answers_until_explicit_recovery_and_never_infers_a_workflow()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("state.db");
    let mut store = Store::open(&database)?;
    let job = store.enqueue(1)?;
    let claim = store.claim(job, "worker", 60)?.ok_or("claim")?;
    let id = store.ask(
        &claim,
        JobState::Queued,
        &Question::new("Use approved cover".into(), vec!["Resume".into()])?,
    )?;
    let event = store.last_event_id()?;
    let snapshot = Store::open_read_snapshot(&database)?;
    let details = snapshot.question_details(id)?;
    assert!(details.current);
    assert!(matches!(details.recovery, QuestionRecovery::Manual));
    assert_eq!(snapshot.inbox(Page::new(0, 10)?)?.len(), 1);
    assert_eq!(snapshot.last_event_id()?, event);
    store.answer(id, 1, "Resume")?;
    assert!(snapshot.question_details(id)?.question.answer.is_none());
    assert!(
        snapshot
            .job_decision(job, 1)?
            .ok_or("decision")?
            .question
            .answer
            .is_none()
    );
    drop(snapshot);
    let reopened = Store::open_read_snapshot(&database)?;
    assert_eq!(
        reopened.inbox(Page::new(0, 10)?)?[0].answer.as_deref(),
        Some("Resume")
    );
    assert!(reopened.questions(Page::new(0, 10)?)?.is_empty());
    assert_eq!(
        reopened
            .job_decision(job, 1)?
            .ok_or("decision")?
            .question
            .answer
            .as_deref(),
        Some("Resume")
    );
    assert!(matches!(
        reopened.question_details(id)?.recovery,
        QuestionRecovery::Manual
    ));
    assert_eq!(reopened.job_state(job)?.as_deref(), Some("awaiting_input"));
    assert!(reopened.question_details(0).is_err());
    assert!(reopened.question_details(9999).is_err());
    assert!(reopened.job_decision(job, 2)?.is_none());
    Ok(())
}

#[test]
fn inbox_pagination_is_bounded_and_retains_typed_pause_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("state.db");
    let mut store = Store::open(&database)?;
    let mut ids = Vec::new();
    for _ in 0..53 {
        let job = store.enqueue(1)?;
        let control = store.request_job_control(job, 1, JobControlAction::Pause)?;
        ids.push(control.question_id.ok_or("question")?);
    }
    let first = store.inbox(Page::new(0, 50)?)?;
    let second = store.inbox(Page::new(first[49].id, 50)?)?;
    assert_eq!(first.len(), 50);
    assert_eq!(second.len(), 3);
    assert_eq!(
        first
            .iter()
            .chain(second.iter())
            .map(|q| q.id)
            .collect::<Vec<_>>(),
        ids
    );
    let details = store.question_details(ids[0])?;
    let QuestionRecovery::JobControl { control } = details.recovery else {
        return Err("missing typed pause binding".into());
    };
    store.answer(ids[0], 1, "Cancel")?;
    assert_eq!(store.inbox(Page::new(0, 1)?)?[0].id, ids[0]);
    store.continue_job_control(control.id, 1)?;
    assert!(!store.question_details(ids[0])?.current);
    assert_eq!(store.inbox(Page::new(0, 1)?)?[0].id, ids[1]);
    assert_eq!(
        store.job_state(control.job_id)?.as_deref(),
        Some("cancelled")
    );
    Ok(())
}
#[test]
fn schema_fourteen_upgrade_keeps_generic_questions_without_inventing_artwork_context()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("state.db");
    let mut store = Store::open(&database)?;
    let job = store.enqueue(1)?;
    let claim = store.claim(job, "worker", 60)?.ok_or("claim")?;
    let id = store.ask(
        &claim,
        JobState::Queued,
        &Question::new("Choose format".into(), vec!["FLAC".into()])?,
    )?;
    drop(store);
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute_batch(include_str!("support/drop_sharing.sql"))?;
    raw.execute_batch("DROP TABLE worker_requests; DROP TABLE worker_host; DROP TABLE transfer_evidence; DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update; DROP TABLE queue_presentations;")?;
    raw.execute_batch("DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; PRAGMA user_version=14;")?;
    let mut store = Store::open_existing(&database)?;
    assert_eq!(store.question(id)?.prompt, "Choose format");
    assert!(store.artwork_decision(id).is_err());
    store.answer(id, 1, "FLAC")?;
    assert!(store.claim(job, "other", 60)?.is_some());
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        26
    );
    Ok(())
}
