use domus::domain::Page;
use domus::infrastructure::store::{QueueSelection, Store};
use rusqlite::{Connection, params};
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn selection(job: i64, version: i64) -> QueueSelection {
    QueueSelection {
        job_id: job,
        plan_version: version,
    }
}

#[test]
fn queue_dismissal_is_version_bound_atomic_durable_and_never_deletes_jobs() -> TestResult {
    let state = tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let raw = Connection::open(&database)?;
    let finished = store.enqueue(1)?;
    let active = store.enqueue(1)?;
    let backup = store.enqueue(1)?;
    raw.execute("UPDATE jobs SET state='complete' WHERE id=?1", [finished])?;
    raw.execute(
        "UPDATE jobs SET state='backup_pending' WHERE id=?1",
        [backup],
    )?;
    for invalid in [
        vec![],
        vec![selection(finished, 2)],
        vec![selection(finished, 1), selection(active, 1)],
        vec![selection(finished, 1), selection(backup, 1)],
        vec![selection(finished, 1), selection(finished, 1)],
        vec![selection(-1, 1)],
        (1..=51).map(|id| selection(id, 1)).collect(),
    ] {
        assert!(store.dismiss_finished(&invalid).is_err());
        assert_eq!(store.queue(Page::new(0, 10)?)?.len(), 3);
        let count: i64 = raw.query_row(
            "SELECT count(*) FROM events WHERE event_type='queue_dismissed'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0);
    }
    raw.execute_batch(
        "CREATE TRIGGER reject_dismiss BEFORE INSERT ON events
         WHEN NEW.event_type='queue_dismissed'
         BEGIN SELECT RAISE(ABORT,'generated failure'); END;",
    )?;
    assert!(store.dismiss_finished(&[selection(finished, 1)]).is_err());
    assert_eq!(store.queue(Page::new(0, 10)?)?.len(), 3);
    raw.execute_batch("DROP TRIGGER reject_dismiss;")?;
    store.dismiss_finished(&[selection(finished, 1)])?;
    store.dismiss_finished(&[selection(finished, 1)])?;
    let count: i64 = raw.query_row(
        "SELECT count(*) FROM events WHERE event_type='queue_dismissed'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(count, 1);
    drop(store);
    let store = Store::open_current_existing(&database)?;
    assert_eq!(store.jobs(Page::new(0, 10)?)?.len(), 3);
    assert_eq!(store.job_state(finished)?.as_deref(), Some("complete"));
    assert_eq!(
        store
            .queue(Page::new(0, 10)?)?
            .iter()
            .map(|job| job.id)
            .collect::<Vec<_>>(),
        vec![active, backup]
    );
    Ok(())
}

#[test]
fn queue_uses_completion_age_and_filters_before_bounded_pagination() -> TestResult {
    let state = tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let raw = Connection::open(&database)?;
    let expired = store.enqueue(1)?;
    let cancelled = store.enqueue(1)?;
    let recent = store.enqueue(1)?;
    let backup = store.enqueue(1)?;
    raw.execute(
        "UPDATE jobs SET state='complete', created_at=0 WHERE id IN (?1,?2)",
        params![expired, recent],
    )?;
    raw.execute(
        "UPDATE queue_presentations SET completed_at=unixepoch()-604800 WHERE job_id=?1",
        [expired],
    )?;
    raw.execute("UPDATE jobs SET state='cancelled' WHERE id=?1", [cancelled])?;
    raw.execute(
        "UPDATE jobs SET state='backup_pending',created_at=0 WHERE id=?1",
        [backup],
    )?;
    let first = store.queue(Page::new(0, 1)?)?;
    assert_eq!(first[0].id, recent);
    assert_eq!(store.queue(Page::new(recent, 1)?)?[0].id, backup);
    assert!(store.queue(Page::new(backup, 1)?)?.is_empty());
    assert_eq!(store.jobs(Page::new(0, 10)?)?.len(), 4);
    let completed_at: i64 = raw.query_row(
        "SELECT completed_at FROM queue_presentations WHERE job_id=?1",
        [recent],
        |row| row.get(0),
    )?;
    raw.execute("UPDATE jobs SET state='complete' WHERE id=?1", [recent])?;
    assert_eq!(
        raw.query_row(
            "SELECT completed_at FROM queue_presentations WHERE job_id=?1",
            [recent],
            |row| row.get::<_, i64>(0),
        )?,
        completed_at
    );
    Ok(())
}

#[test]
fn queue_upgrade_retains_legacy_completion_times_and_unknown_age() -> TestResult {
    let state = tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let historical = store.enqueue(1)?;
    let unknown = store.enqueue(1)?;
    drop(store);
    let raw = Connection::open(&database)?;
    raw.execute_batch(
        "DROP INDEX plan_events; DROP TRIGGER queue_completed_insert; DROP TRIGGER queue_completed_update;
         DROP TABLE queue_presentations; PRAGMA user_version=18;
         UPDATE jobs SET state='complete';
         INSERT INTO events(job_id,plan_version,event_type,created_at)
         VALUES (1,1,'complete',1);",
    )?;
    assert!(Store::open_current_existing(&database).is_err());
    let store = Store::open_existing(&database)?;
    assert_eq!(store.queue(Page::new(0, 10)?)?[0].id, unknown);
    assert_eq!(store.job_state(historical)?.as_deref(), Some("complete"));
    assert_eq!(store.jobs(Page::new(0, 10)?)?.len(), 2);
    let rows: i64 = raw.query_row("SELECT count(*) FROM queue_presentations", [], |row| {
        row.get(0)
    })?;
    assert_eq!(rows, 2);
    Ok(())
}
