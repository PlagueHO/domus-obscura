use super::*;
use crate::domain::Page;
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn runtime(directory: &Path) -> Result<PreparedProductionWorker> {
    let database = directory.join("state.sqlite");
    drop(Store::open(&database)?);
    Ok(PreparedProductionWorker {
        _lock: ProcessLock::acquire(&database, "worker")?,
        database,
        configuration: WorkerConfiguration {
            queue_capacity: 64,
            acquisition: None,
            sharing: None,
        },
        services: Default::default(),
        revision: "a".repeat(64),
    })
}

#[test]
fn worker_runtime_runs_a_durable_scan_and_releases_native_ownership_on_stop() -> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let worker = runtime(temp.path())?;
    let mut store = Store::open_current_existing(&worker.database)?;
    let root = temp.path().join("media");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("generated.txt"), b"generated scan fixture")?;
    let location = crate::application::add_location(&store, &root, "main")?;
    store.enqueue_worker(
        WorkerOperation::Scan {
            location_id: location,
        },
        64,
        &"a".repeat(64),
    )?;
    assert!(matches!(
        ProcessLock::acquire(&worker.database, "worker"),
        Err(Error::Busy(_))
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let handle = std::thread::spawn(move || worker.supervise(stopping));
    let mut complete = false;
    for _ in 0..100 {
        if store.worker_requests(Page::new(0, 10)?)?[0].state == "complete" {
            complete = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Release);
    handle
        .join()
        .map_err(|_| Error::Internal("fixture supervisor panicked"))??;
    assert!(complete, "worker did not finish its accepted scan");
    assert_eq!(store.location(location)?.coverage, "complete");
    assert_eq!(
        store
            .browse(
                location,
                &crate::domain::LibraryPath::parse("")?,
                Page::new(0, 10)?
            )?
            .len(),
        1
    );
    assert!(!store.worker_status(64)?.online);
    assert!(!store.worker_status(64)?.stopping);
    let _next = ProcessLock::acquire(store.database_path(), "worker")?;
    Ok(())
}

#[test]
fn worker_folder_refresh_recovers_its_scope_without_scanning_children() -> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let worker = runtime(temp.path())?;
    let stop = Arc::new(AtomicBool::new(true));
    let mut store = Store::open_current_existing(&worker.database)?.with_worker_stop(stop.clone());
    let root = temp.path().join("media");
    std::fs::create_dir_all(root.join("selected").join("child"))?;
    std::fs::write(root.join("selected").join("direct.txt"), b"generated")?;
    std::fs::write(
        root.join("selected").join("child").join("nested.txt"),
        b"generated",
    )?;
    let location = crate::application::add_location(&store, &root, "main")?;
    let folder = crate::domain::LibraryPath::parse("selected")?;
    assert!(matches!(
        crate::application::refresh_library_folder(&mut store, location, &folder),
        Err(Error::WorkerStopping)
    ));
    stop.store(false, Ordering::Release);
    let raw = rusqlite::Connection::open(&worker.database)?;
    raw.execute("UPDATE scans SET lease_until=0 WHERE status='running'", [])?;
    let request = store.enqueue_worker(
        WorkerOperation::RefreshFolder {
            location_id: location,
            folder: "selected".into(),
        },
        64,
        &"a".repeat(64),
    )?;
    let stopping = stop.clone();
    let handle = std::thread::spawn(move || worker.supervise(stopping));
    let mut complete = false;
    for _ in 0..100 {
        if store.worker_request(request.id)?.state == "complete" {
            complete = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Release);
    handle
        .join()
        .map_err(|_| Error::Internal("fixture supervisor panicked"))??;
    assert!(complete, "worker did not resume the scoped refresh");
    assert_eq!(store.location(location)?.coverage, "unknown");
    assert_eq!(store.browse(location, &folder, Page::new(0, 10)?)?.len(), 2);
    assert!(
        store
            .browse(
                location,
                &crate::domain::LibraryPath::parse("selected/child")?,
                Page::new(0, 10)?
            )?
            .is_empty()
    );
    assert!(store.jobs(Page::new(0, 10)?)?.is_empty());
    Ok(())
}

#[test]
fn worker_shutdown_does_not_start_queued_work() -> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let worker = runtime(temp.path())?;
    let mut store = Store::open_current_existing(&worker.database)?;
    let root = temp.path().join("media");
    std::fs::create_dir(&root)?;
    let location = crate::application::add_location(&store, &root, "main")?;
    store.enqueue_worker(
        WorkerOperation::Scan {
            location_id: location,
        },
        64,
        &"a".repeat(64),
    )?;
    worker.supervise(Arc::new(AtomicBool::new(true)))?;
    assert_eq!(store.worker_requests(Page::new(0, 10)?)?[0].state, "queued");
    assert_eq!(store.location(location)?.coverage, "unknown");
    Ok(())
}

#[test]
fn worker_execute_rejects_configuration_changes_and_leaves_saved_decisions_idle() -> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let worker = runtime(temp.path())?;
    let mut store = Store::open_current_existing(&worker.database)?;
    let job = store.enqueue(1)?;
    let operation = WorkerOperation::Acquire {
        job_id: job,
        plan_version: 1,
        source_candidate: None,
        acknowledge_new_only_publication: true,
    };
    let run = |binding: &str| {
        execute(
            &worker.database,
            &worker.configuration,
            worker.services.clone(),
            operation.clone(),
            Arc::new(AtomicBool::new(false)),
            binding,
            None,
        )
    };
    assert!(matches!(run(&"b".repeat(64)), Err(Error::Conflict(_))));
    let raw = rusqlite::Connection::open(&worker.database)?;
    raw.execute("UPDATE jobs SET state='awaiting_input' WHERE id=?1", [job])?;
    assert!(matches!(run(&"b".repeat(64)), Err(Error::WorkerWaiting)));
    for state in ["complete", "cancelled"] {
        raw.execute(
            "UPDATE jobs SET state=?2 WHERE id=?1",
            rusqlite::params![job, state],
        )?;
        run(&"b".repeat(64))?;
        assert_eq!(store.job_state(job)?.as_deref(), Some(state));
    }
    assert!(store.transfers(job, 1, Page::new(0, 10)?)?.is_empty());
    Ok(())
}

#[test]
fn worker_supervisor_unwind_drains_operations_before_releasing_native_ownership() -> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let database = temp.path().join("state.sqlite");
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let drained = Arc::new(AtomicBool::new(false));
    let completed = drained.clone();
    let path = database.clone();
    let thread = std::thread::spawn(move || {
        while !stopping.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            ProcessLock::acquire(&path, "worker"),
            Err(Error::Busy(_))
        ));
        completed.store(true, Ordering::Release);
        Ok(())
    });
    let lock = ProcessLock::acquire(&database, "worker")?;
    let mut store = Store::open(&database)?;
    let root = temp.path().join("media");
    std::fs::create_dir(&root)?;
    let location = crate::application::add_location(&store, &root, "main")?;
    store.enqueue_worker(
        WorkerOperation::Scan {
            location_id: location,
        },
        64,
        &"a".repeat(64),
    )?;
    let host = store.start_worker_host("fixture", &"a".repeat(64))?;
    let claim = Arc::new(
        store
            .claim_worker_request(&host, "scan")?
            .ok_or("missing claim")?,
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _lock = lock;
        let _active = ActiveLanes {
            slots: [Some(Running { claim, thread }), None],
            stop,
        };
        panic!("injected supervisor unwind");
    }));
    assert!(result.is_err());
    assert!(drained.load(Ordering::Acquire));
    let _next = ProcessLock::acquire(&database, "worker")?;
    Ok(())
}

#[test]
fn worker_interrupted_scan_keeps_unknown_coverage_and_can_resume() -> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let worker = runtime(temp.path())?;
    let stop = Arc::new(AtomicBool::new(true));
    let mut store = Store::open_current_existing(&worker.database)?.with_worker_stop(stop.clone());
    let root = temp.path().join("media");
    std::fs::create_dir(&root)?;
    let location = crate::application::add_location(&store, &root, "main")?;
    assert!(matches!(
        crate::application::scan_location(&mut store, location),
        Err(Error::WorkerStopping)
    ));
    assert_ne!(store.location(location)?.coverage, "complete");
    stop.store(false, Ordering::Release);
    crate::application::scan_location(&mut store, location)?;
    assert_eq!(store.location(location)?.coverage, "complete");
    Ok(())
}

#[test]
fn worker_native_lock_survives_competing_process_and_recovers_after_exit() -> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let database = temp.path().join("state.sqlite");
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "infrastructure::production_worker::tests::worker_lock_child",
            "--ignored",
            "--nocapture",
        ])
        .env("DOMUS_GENERATED_LOCK_TEST", &database)
        .stdout(Stdio::piped())
        .spawn()?;
    let output = std::io::BufReader::new(
        child
            .stdout
            .take()
            .ok_or(Error::Internal("missing child stdout"))?,
    );
    let mut ready = false;
    for line in output.lines() {
        if line? == "DOMUS_LOCK_READY" {
            ready = true;
            break;
        }
    }
    let contended = ProcessLock::acquire(&database, "worker");
    let killed = child.kill();
    let exited = child.wait();
    killed?;
    exited?;
    assert!(ready, "child did not acquire the native lock");
    assert!(matches!(contended, Err(Error::Busy(_))));
    let _recovered = ProcessLock::acquire(&database, "worker")?;
    Ok(())
}

#[test]
#[ignore = "subprocess fixture; called only with its generated temporary path"]
fn worker_lock_child() -> Result<()> {
    let database = std::env::var_os("DOMUS_GENERATED_LOCK_TEST")
        .ok_or(Error::Internal("missing generated subprocess path"))?;
    let _lock = ProcessLock::acquire(Path::new(&database), "worker")?;
    println!("DOMUS_LOCK_READY");
    std::io::stdout().flush()?;
    std::thread::sleep(Duration::from_secs(30));
    Ok(())
}
