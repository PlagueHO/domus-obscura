use super::*;
use std::process::{Child, Command, Stdio};

struct FixtureChild(Option<Child>);

impl FixtureChild {
    fn stop(&mut self) -> std::io::Result<()> {
        if let Some(child) = self.0.as_mut() {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            child.wait()?;
        }
        self.0 = None;
        Ok(())
    }
}

impl Drop for FixtureChild {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("catalog fixture process cleanup failed: {error}");
        }
    }
}

#[test]
fn catalog_request_owner_child() -> TestResult {
    let Some(database) = std::env::var_os("DOMUS_TEST_CATALOG_OWNER_DATABASE") else {
        return Ok(());
    };
    let ready = std::env::var_os("DOMUS_TEST_CATALOG_OWNER_READY")
        .ok_or("catalog child readiness path is missing")?;
    let mut store = Store::open_existing(std::path::Path::new(&database))?;
    if std::env::var_os("DOMUS_TEST_CATALOG_BACKOFF").is_some() {
        assert_eq!(store.catalog_status()?.retry_after_seconds, 120);
        assert!(matches!(
            store.claim_catalog_request(),
            Err(Error::Catalog {
                retry_after_seconds: 119..=120,
                ..
            })
        ));
        std::fs::write(ready, b"backoff")?;
        return Ok(());
    }
    let _permit = store.claim_catalog_request()?;
    std::fs::write(ready, b"owned")?;
    let mut release = String::new();
    std::io::stdin().read_line(&mut release)?;
    Ok(())
}

#[test]
fn catalog_new_process_keeps_full_saved_backoff_when_wall_deadline_is_past() -> TestResult {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    let ready = directory.path().join("backoff-ready");
    let mut store = Store::open(&database)?;
    let permit = store.claim_catalog_request()?;
    store.complete_catalog_request(permit, 120, None)?;
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute("UPDATE catalog_gate SET next_request_ms=1", [])?;
    let mut child = FixtureChild(Some(
        Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "ownership::catalog_request_owner_child",
                "--nocapture",
            ])
            .env("DOMUS_TEST_CATALOG_OWNER_DATABASE", &database)
            .env("DOMUS_TEST_CATALOG_OWNER_READY", &ready)
            .env("DOMUS_TEST_CATALOG_BACKOFF", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?,
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child
            .0
            .as_mut()
            .ok_or("fixture child is missing")?
            .try_wait()?
        {
            assert!(
                status.success(),
                "new process did not preserve provider backoff"
            );
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err("catalog backoff process exceeded its deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(&ready)?, b"backoff");
    child.stop()?;
    Ok(())
}

#[test]
fn catalog_os_ownership_blocks_other_processes_until_owner_exit() -> TestResult {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    let ready = directory.path().join("owner-ready");
    let mut store = Store::open(&database)?;
    let mut child = FixtureChild(Some(
        Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "ownership::catalog_request_owner_child",
                "--nocapture",
            ])
            .env("DOMUS_TEST_CATALOG_OWNER_DATABASE", &database)
            .env("DOMUS_TEST_CATALOG_OWNER_READY", &ready)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?,
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ready.try_exists()? {
        if child
            .0
            .as_mut()
            .ok_or("fixture child is missing")?
            .try_wait()?
            .is_some()
        {
            return Err("catalog fixture process exited before taking ownership".into());
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "catalog fixture process did not take ownership before its deadline".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute(
        "UPDATE catalog_gate SET lease_until_ms=1, next_request_ms=0",
        [],
    )?;
    assert!(matches!(
        store.catalog_status()?.state,
        domus::infrastructure::store::CatalogProviderState::Active
    ));
    assert!(matches!(
        store.claim_catalog_request(),
        Err(Error::Catalog { .. })
    ));
    child.stop()?;
    assert!(matches!(
        store.claim_catalog_request(),
        Err(Error::Catalog {
            retry_after_seconds: 59..=60,
            ..
        })
    ));
    connection.execute(
        "UPDATE catalog_gate SET lease_until_ms=0, next_request_ms=0, delay_seconds=0",
        [],
    )?;
    let permit = store.claim_catalog_request()?;
    drop(permit);
    Ok(())
}
