use super::super::production_worker::{WorkerConfiguration, WorkerOperation};
use super::*;
use axum::extract::ConnectInfo;
use axum::http::Request;
use tower::ServiceExt;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn command(path: &str, csrf: bool) -> Result<Request<axum::body::Body>> {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "localhost:8443")
        .header(header::ORIGIN, "https://localhost:8443")
        .header(header::CONTENT_TYPE, "application/json");
    if csrf {
        request = request.header("x-domus-csrf", "1");
    }
    let mut request = request
        .body(axum::body::Body::from("{}"))
        .map_err(|_| Error::Internal("invalid fixture request"))?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    Ok(request)
}

#[tokio::test]
async fn worker_api_queues_offline_scans_without_running_them_and_preserves_command_guards()
-> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let database = temp.path().join("state.sqlite");
    let store = Store::open(&database)?;
    let root = temp.path().join("media");
    std::fs::create_dir(&root)?;
    let location = crate::application::add_location(&store, &root, "main")?;
    let api = Api::development("https://localhost:8443", store)?.with_production_worker(
        WorkerConfiguration {
            queue_capacity: 1,
            acquisition: None,
            sharing: None,
        },
    )?;
    let path = format!("/api/locations/{location}/scan");
    let router = api.router();
    assert_eq!(
        router
            .clone()
            .oneshot(command(&path, false)?)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        assert_eq!(
            router
                .clone()
                .oneshot(command(&path, true)?)
                .await?
                .status(),
            StatusCode::ACCEPTED
        );
    }
    let store = Store::open_read_current(&database)?;
    assert_eq!(store.location(location)?.coverage, "unknown");
    assert_eq!(store.worker_requests(Page::new(0, 10)?)?.len(), 1);
    assert!(!store.worker_status(1)?.online);
    let raw = rusqlite::Connection::open(&database)?;
    let scans: i64 = raw.query_row("SELECT count(*) FROM scans", [], |row| row.get(0))?;
    assert_eq!(scans, 0);
    let mut discovery = command("/api/local/source-search", true)?;
    *discovery.body_mut() =
        axum::body::Body::from(r#"{"release_id":"20000000-0000-4000-8000-000000000001"}"#);
    assert_eq!(
        router.oneshot(discovery).await?.status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

#[tokio::test]
async fn worker_api_scan_retry_rechecks_system_settings_read_only() -> TestResult {
    let temp = super::super::permissions::private_test_directory()?;
    let database = temp.path().join("state.sqlite");
    let mut store = Store::open(&database)?;
    let root = temp.path().join("media");
    std::fs::create_dir(&root)?;
    let location = crate::application::add_location(&store, &root, "main")?;
    let request = store.enqueue_worker(
        WorkerOperation::Scan {
            location_id: location,
        },
        1,
        &"a".repeat(64),
    )?;
    let host = store.start_worker_host("fixture", &"a".repeat(64))?;
    let claim = store
        .claim_worker_request(&host, "scan")?
        .ok_or(Error::Internal("missing fixture request"))?;
    store.finish_worker_request(&claim, "failed", Some("generated failure"))?;
    let api = Api::development("https://localhost:8443", store)?
        .with_system_settings_read_only(true)?
        .with_production_worker(WorkerConfiguration {
            queue_capacity: 1,
            acquisition: None,
            sharing: None,
        })?;
    let router = api.router();
    for path in [
        format!("/api/locations/{location}/scan"),
        format!("/api/worker/requests/{}/retry", request.id),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(command(&path, true)?)
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        Store::open_read_current(&database)?
            .worker_request(request.id)?
            .state,
        "failed"
    );
    Ok(())
}
