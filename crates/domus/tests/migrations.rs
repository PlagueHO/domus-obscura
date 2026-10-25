use domus::infrastructure::store::Store;
use tempfile::tempdir;

#[test]
fn schema_one_upgrades_without_losing_existing_jobs() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("state.db");
    let old = rusqlite::Connection::open(&database)?;
    old.execute_batch(include_str!("../migrations/001_initial.sql"))?;
    old.pragma_update(None, "user_version", 1)?;
    old.execute(
        "INSERT INTO jobs(plan_version, state) VALUES (1, 'queued')",
        [],
    )?;
    drop(old);
    let store = Store::open(&database)?;
    assert_eq!(store.job_state(1)?.as_deref(), Some("queued"));
    assert!(store.question(1).is_err());
    drop(store);
    let current = rusqlite::Connection::open(&database)?;
    let version: i64 = current.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 18);
    Ok(())
}

#[test]
fn a_newer_database_is_not_downgraded() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("state.db");
    let connection = rusqlite::Connection::open(&database)?;
    connection.pragma_update(None, "user_version", 99)?;
    drop(connection);
    assert!(Store::open(&database).is_err());
    let connection = rusqlite::Connection::open(&database)?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 99);
    Ok(())
}

#[test]
fn legacy_catalog_timing_requires_acknowledgment_without_losing_cache()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    for pending in [false, true] {
        let database = data.path().join(format!("{pending}.db"));
        let old = rusqlite::Connection::open(&database)?;
        for migration in [
            include_str!("../migrations/001_initial.sql"),
            include_str!("../migrations/002_questions.sql"),
            include_str!("../migrations/003_question_versions.sql"),
            include_str!("../migrations/004_plans.sql"),
            include_str!("../migrations/005_claim_generations.sql"),
            include_str!("../migrations/006_sessions.sql"),
            include_str!("../migrations/007_transfers.sql"),
            include_str!("../migrations/008_catalog.sql"),
        ] {
            old.execute_batch(migration)?;
        }
        old.pragma_update(None, "user_version", 8)?;
        if pending {
            old.execute(
                "UPDATE catalog_gate SET generation=2, lease_until_ms=1, next_request_ms=1",
                [],
            )?;
        }
        old.execute(
            "INSERT INTO catalog_cache VALUES ('fixture', x'01', zeroblob(32), 1, 2, 1)",
            [],
        )?;
        drop(old);
        let mut store = Store::open(&database)?;
        assert_eq!(
            matches!(
                store.catalog_status()?.state,
                domus::infrastructure::store::CatalogProviderState::Paused
            ),
            pending
        );
        if pending {
            assert!(store.claim_catalog_request().is_err());
            assert!(store.resume_catalog_requests(false).is_err());
            store.resume_catalog_requests(true)?;
            assert!(matches!(
                store.catalog_status()?.state,
                domus::infrastructure::store::CatalogProviderState::Backoff
            ));
        }
        let current = rusqlite::Connection::open(&database)?;
        let count: i64 =
            current.query_row("SELECT count(*) FROM catalog_cache", [], |row| row.get(0))?;
        assert_eq!(count, 1);
    }
    Ok(())
}
