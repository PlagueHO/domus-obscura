use domus::domain::{JobState, Question};
use domus::infrastructure::store::Store;
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
    raw.execute_batch("DROP TABLE acquisition_source_reviews; DROP TABLE source_candidates; DROP TABLE job_controls; DROP TABLE publication_files; DROP TABLE publication_destinations; DROP TABLE artwork_question_resolutions; DROP TABLE artwork_questions; PRAGMA user_version=14;")?;
    let mut store = Store::open_existing(&database)?;
    assert_eq!(store.question(id)?.prompt, "Choose format");
    assert!(store.artwork_decision(id).is_err());
    store.answer(id, 1, "FLAC")?;
    assert!(store.claim(job, "other", 60)?.is_some());
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        18
    );
    Ok(())
}
