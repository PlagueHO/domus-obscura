//! Explicit fixture export for an approved isolated Linux service check.
//! This test binary is not part of the application package.
use super::*;
use domus::application::services::ServiceConfiguration;
use domus::infrastructure::production_worker::{WorkerConfiguration, WorkerOperation};
use domus::infrastructure::worker::LocalAcquisition;
use serde_json::json;
use std::path::Path;

fn write_new(path: &Path, bytes: &[u8]) -> TestResult {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn prepare(root: &Path, large: bool, browser_assets: Option<&Path>) -> TestResult {
    if root.symlink_metadata()?.file_type().is_symlink()
        || !root.is_absolute()
        || std::fs::read_dir(root)?.next().is_some()
    {
        return Err("acceptance fixture needs a new empty absolute directory".into());
    }
    let make = |name| -> Result<TempDir, Box<dyn std::error::Error>> {
        let directory = tempfile::Builder::new().prefix(name).tempdir_in(root)?;
        private::directory(directory.path())?;
        Ok(directory)
    };
    // Large input leaves an observable copy window; no sleep changes application behavior.
    let source = if large {
        let mut frame = vec![0; 1044];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0xe0, 0]);
        Some(frame.repeat(32_768))
    } else {
        None
    };
    let mut fixture = Fixture::build_with_inputs(
        AudioFormat::Mp3,
        true,
        true,
        true,
        None,
        None,
        FixtureInputs {
            directories: Some([make("state-")?, make("library-")?, make("staging-")?]),
            source,
            ..Default::default()
        },
    )?;
    let staging = fixture.staging.as_ref().ok_or("staging")?;
    let run = domus::application::normalize_album(
        &mut fixture.store,
        &fixture.claim,
        staging,
        &fixture.cover,
    )?;
    // Fail receipt persistence, not approval or intent creation. Recovery must verify
    // the already-written file rather than replace it.
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER acceptance_interrupt BEFORE UPDATE OF verified_at ON publication_files
         BEGIN SELECT RAISE(ABORT,'generated acceptance interruption'); END;",
    )?;
    let interrupted =
        domus::application::publish_new_album(&mut fixture.store, &fixture.claim, staging, run);
    raw.execute_batch("DROP TRIGGER acceptance_interrupt")?;
    assert!(interrupted.is_err());
    assert!(
        !fixture
            .store
            .publication_destinations(fixture.job, fixture.version)?
            .is_empty()
    );
    fixture.store.renew_claim(&fixture.claim, 1)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !raw.query_row(
        "SELECT lease_until<=unixepoch() FROM jobs WHERE id=?1",
        [fixture.job],
        |row| row.get::<_, bool>(0),
    )? {
        if std::time::Instant::now() >= deadline {
            return Err("generated owner lease did not expire".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let credentials = fixture._state.path().join("generated-credentials.json");
    write_new(
        &credentials,
        br#"{"username":"generated-service-fixture","password":"not-a-live-credential"}"#,
    )?;
    let acquisition = LocalAcquisition {
        staging_root: fixture.scratch.path().to_owned(),
        credentials_file: credentials,
        server: "1.1.1.1:2242".parse()?,
        acknowledge_plaintext_login: true,
    };
    let worker = WorkerConfiguration {
        queue_capacity: 64,
        acquisition: Some(acquisition),
        sharing: None,
    };
    let services = ServiceConfiguration {
        discovery: Vec::new(),
        metadata: Vec::new(),
        artwork: Vec::new(),
        acquisition: vec!["soulseek".into()],
        artist_images: Vec::new(),
    };
    let binding = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&services, &worker.acquisition))?)
    );
    let scan_directory = make("scan-")?;
    let scan_root = scan_directory.path().to_owned();
    for index in 0..if large { 8192 } else { 4 } {
        let directory = scan_root.join(format!("directory-{index:05}"));
        std::fs::create_dir(&directory)?;
        std::fs::write(directory.join("generated.txt"), b"generated service scan")?;
    }
    let scan = add_location(&fixture.store, &scan_root, "backup")?;
    let scan_request =
        fixture
            .store
            .enqueue_worker(WorkerOperation::Scan { location_id: scan }, 64, &binding)?;
    let publication_request = fixture.store.enqueue_worker(
        WorkerOperation::Publish {
            job_id: fixture.job,
            plan_version: fixture.version,
            normalization_run: run,
            acknowledge_new_only_publication: true,
        },
        64,
        &binding,
    )?;
    let configuration = json!({
        "environment":"single_user", "system_settings_read_only":true,
        "services":services, "database":fixture.database,
        "listen":"127.0.0.1:8080", "origin":"http://127.0.0.1:8080",
        "browser_assets":browser_assets,
        "production_worker":worker
    });
    write_new(
        &root.join("service.json"),
        &serde_json::to_vec_pretty(&configuration)?,
    )?;
    let descriptor = json!({
        "format":1, "database":fixture.database,
        "scan_root":scan_root, "library":fixture._library.path(),
        "staging":fixture.scratch.path(), "job":fixture.job,
        "version":fixture.version, "scan_location":scan,
        "scan_request":scan_request.id, "publication_request":publication_request.id,
        "scan_files":if large {8192} else {4},
        "normalization_run":run, "configuration_binding":binding,
        "offline_only":true
    });
    write_new(
        &root.join("fixture.json"),
        &serde_json::to_vec_pretty(&descriptor)?,
    )?;
    drop(fixture.staging.take());
    let _ = fixture._state.keep();
    let _ = fixture._library.keep();
    let _ = fixture.scratch.keep();
    let _ = scan_directory.keep();
    Ok(())
}

#[test]
fn service_acceptance_fixture_uses_real_approved_receipts_and_durable_requests() -> TestResult {
    let root = tempdir()?;
    let browser = tempdir()?;
    std::fs::create_dir(browser.path().join("assets"))?;
    std::fs::write(browser.path().join("index.html"), b"generated browser")?;
    prepare(root.path(), false, Some(browser.path()))?;
    let configuration: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("service.json"))?)?;
    assert_eq!(
        configuration["browser_assets"],
        browser.path().to_str().ok_or("browser path")?
    );
    domus::infrastructure::server::PreparedServer::load(&root.path().join("service.json"))?;
    let descriptor: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("fixture.json"))?)?;
    let database = descriptor["database"].as_str().ok_or("database")?;
    let store = Store::open_current_existing(Path::new(database))?;
    let job = descriptor["job"].as_i64().ok_or("job")?;
    let version = descriptor["version"].as_i64().ok_or("version")?;
    assert!(store.plan(job, version)?.approved_at.is_some());
    store.approved_artwork(job, version)?;
    assert_eq!(store.worker_requests(Page::new(0, 10)?)?.len(), 2);
    assert_eq!(store.job_state(job)?.as_deref(), Some("publishing"));
    let raw = Connection::open(database)?;
    let receipts: i64 =
        raw.query_row("SELECT count(*) FROM normalization_receipts", [], |row| {
            row.get(0)
        })?;
    assert_eq!(receipts, 3);
    assert!(prepare(root.path(), false, None).is_err());
    Ok(())
}

#[tokio::test]
async fn service_acceptance_offline_worker_completes_generated_scan_and_publication() -> TestResult
{
    use domus::infrastructure::lifecycle::Lifecycle;
    use domus::infrastructure::production_worker::PreparedProductionWorker;
    use std::time::Duration;

    let root = tempdir()?;
    prepare(root.path(), false, None)?;
    let descriptor: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("fixture.json"))?)?;
    let database = descriptor["database"].as_str().ok_or("database")?;
    let worker = PreparedProductionWorker::load(&root.path().join("service.json"))?;
    let lifecycle = Lifecycle::controlled(|| Ok(()));
    let running = tokio::spawn(worker.run_with_lifecycle(lifecycle.clone()));
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let store = Store::open_current_existing(Path::new(database))?;
            let requests = store.worker_requests(Page::new(0, 10)?)?;
            if let Some(request) = requests
                .iter()
                .find(|request| matches!(request.state.as_str(), "failed" | "waiting"))
            {
                return Err(format!(
                    "generated request {} {}: {:?}",
                    request.id, request.state, request.last_error
                )
                .into());
            }
            if requests.len() == 2 && requests.iter().all(|request| request.state == "complete") {
                return Ok::<(), Box<dyn std::error::Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    lifecycle.request_stop();
    tokio::time::timeout(Duration::from_secs(10), running).await???;
    outcome??;
    let raw = Connection::open(database)?;
    let files = raw
        .prepare(
            "SELECT d.root,d.relative_folder,f.name,f.sha256 FROM publication_destinations d
         JOIN publication_files f ON f.destination_id=d.id WHERE f.verified_at IS NOT NULL",
        )?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(files.len(), 4);
    for (root, folder, name, expected) in files {
        let output = Path::new(&root)
            .join(folder.replace('/', std::path::MAIN_SEPARATOR_STR))
            .join(name);
        assert_eq!(
            format!("{:x}", Sha256::digest(std::fs::read(output)?)),
            expected
        );
    }
    Ok(())
}

#[test]
#[ignore = "writes generated persistent state only under an explicitly supplied empty path"]
fn service_acceptance_export() -> TestResult {
    let root = std::env::var_os("DOMUS_SERVICE_ACCEPTANCE_ROOT").ok_or("missing fixture root")?;
    let root = Path::new(&root);
    let package =
        std::env::var_os("DOMUS_SERVICE_ACCEPTANCE_PACKAGE").ok_or("missing verified package")?;
    let package = Path::new(&package);
    if !package.is_absolute() {
        return Err("acceptance package needs an absolute path".into());
    }
    let browser = package.join("browser");
    domus::infrastructure::browser::BrowserAssets::load(&browser)?;
    prepare(root, true, Some(&browser))
}
