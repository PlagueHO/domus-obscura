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
    assert_eq!(version, 22);
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
fn cache_root_upgrade_preserves_existing_bindings_and_reservations()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let database = data.path().join("state.db");
    let raw = rusqlite::Connection::open(&database)?;
    let mut migrations =
        std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
    migrations.sort();
    for migration in migrations.iter().take(20) {
        raw.execute_batch(&std::fs::read_to_string(migration)?)?;
    }
    raw.pragma_update(None, "user_version", 20)?;
    raw.execute("INSERT INTO private_roots(id,root,kind,created_at) VALUES (17,'generated-staging','staging',123)", [])?;
    raw.execute("INSERT INTO filesystem_roots(root,kind,identity,body,bound_at) VALUES ('generated-staging','staging','generated-id','{}',456)", [])?;
    assert!(
        raw.execute(
            "INSERT INTO private_roots(root,kind) VALUES ('no-cache-yet','cache')",
            []
        )
        .is_err()
    );
    drop(raw);
    drop(Store::open(&database)?);
    let current = rusqlite::Connection::open(&database)?;
    let saved: (i64, String, i64) = current.query_row(
        "SELECT id,kind,created_at FROM private_roots WHERE root='generated-staging'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(saved, (17, "staging".into(), 123));
    let native: (String, String, i64) = current.query_row(
        "SELECT identity,body,bound_at FROM filesystem_roots WHERE root='generated-staging'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(native, ("generated-id".into(), "{}".into(), 456));
    assert!(current.execute("DELETE FROM private_roots", []).is_err());
    assert!(
        current
            .execute("UPDATE private_roots SET root='other'", [])
            .is_err()
    );
    assert!(current.execute("DELETE FROM filesystem_roots", []).is_err());
    assert!(
        current
            .execute("UPDATE filesystem_roots SET identity='other'", [])
            .is_err()
    );
    current.execute(
        "INSERT INTO private_roots(root,kind) VALUES ('generated-cache','cache')",
        [],
    )?;
    let id = current.last_insert_rowid();
    assert!(
        id > 17,
        "upgrade must preserve the private root ID sequence"
    );
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
