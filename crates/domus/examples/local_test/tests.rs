use super::*;
use axum::body::Body;
use serde_json::{Value, json};
use tower::ServiceExt;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn app() -> Result<Router> {
    Ok(router(
        Fixture::new()?,
        "test harness".into(),
        vec![],
        vec![],
    ))
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    body: Value,
) -> std::result::Result<Response, Box<dyn std::error::Error>> {
    let request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, AUTHORITY)
        .header(header::ORIGIN, ORIGIN)
        .header("x-domus-csrf", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body)?))?;
    Ok(app.clone().oneshot(request).await?)
}

async fn json_response(
    response: Response,
) -> std::result::Result<Value, Box<dyn std::error::Error>> {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn edition() -> Value {
    json!({
        "release_id": "20000000-0000-4000-8000-000000000001",
        "album_artist": "The Example", "album": "Night: Signals", "bucket": null,
        "tracks": [
            {"artist":"The Example","title":"First / Light","disc":1,"disc_track":1},
            {"artist":"The Example","title":"Last Light","disc":2,"disc_track":1}
        ]
    })
}

#[tokio::test]
async fn harness_uses_real_rust_preview_planning_approval_and_stale_observations() -> TestResult {
    let app = app()?;
    let snapshot =
        json_response(call(&app, Method::GET, "/local/snapshot", Value::Null).await?).await?;
    assert_eq!(snapshot["location"]["coverage"], "complete");
    let preview = json_response(
        call(
            &app,
            Method::POST,
            "/local/preview",
            json!({
                "edition": edition(), "settings": OutputSettings::default()
            }),
        )
        .await?,
    )
    .await?;
    assert_eq!(
        preview[0]["relative_folder"],
        "E/THE EXAMPLE/NIGHT_ SIGNALS"
    );
    assert!(
        preview[1]["filename"]
            .as_str()
            .ok_or("missing filename")?
            .contains("02")
    );
    let request = json!({
        "job_id": snapshot["job_id"], "expected_version": 1,
        "settings_version": 1, "format": "flac", "edition": edition(),
        "locations": [snapshot["location"]["id"]], "accepts_incomplete_coverage": false
    });
    let saved =
        json_response(call(&app, Method::POST, "/local/plan", request.clone()).await?).await?;
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["plan"]["format"], "flac");
    let approved =
        json_response(call(&app, Method::POST, "/local/approve", json!({"version":2})).await?)
            .await?;
    assert!(approved["plan"]["approved_at"].is_i64());
    assert_eq!(approved["jobs"][0]["state"], "queued");
    assert_eq!(
        call(&app, Method::POST, "/local/plan", request)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    json_response(call(&app, Method::POST, "/local/scan", json!({})).await?).await?;
    assert_eq!(
        call(&app, Method::POST, "/local/approve", json!({"version":2}))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

#[tokio::test]
async fn harness_library_routes_only_use_generated_index_evidence() -> TestResult {
    let app = app()?;
    let artists = json_response(
        call(
            &app,
            Method::GET,
            "/api/locations/1/library/artists",
            Value::Null,
        )
        .await?,
    )
    .await?;
    assert_eq!(artists.as_array().ok_or("artist page")?.len(), 2);
    let matches = json_response(
        call(
            &app,
            Method::POST,
            "/api/locations/1/library/matches",
            json!([{"artist":"Sample Artist","album":"Sample Album"}]),
        )
        .await?,
    )
    .await?;
    assert_eq!(
        matches["matches"][0]["albums"]
            .as_array()
            .ok_or("album matches")?
            .len(),
        2
    );
    let refreshed = json_response(
        call(
            &app,
            Method::POST,
            "/api/locations/1/library/refresh",
            json!({"folder":"Other folder"}),
        )
        .await?,
    )
    .await?;
    assert_eq!(refreshed["coverage"]["folder_state"], "complete");
    for path in [
        "/api/locations/2/library/artists",
        "/api/locations/1/library/anything",
        "/api/locations/1/library/coverage?folder=..%2Foutside",
    ] {
        assert_eq!(
            call(&app, Method::GET, path, Value::Null).await?.status(),
            StatusCode::BAD_REQUEST
        );
    }
    Ok(())
}

#[tokio::test]
async fn harness_settings_are_versioned_and_do_not_change_frozen_plans() -> TestResult {
    let app = app()?;
    let snapshot =
        json_response(call(&app, Method::GET, "/local/snapshot", Value::Null).await?).await?;
    let request = json!({
        "job_id": snapshot["job_id"], "expected_version": 1, "settings_version": 1,
        "format": "mp3", "edition": edition(), "locations": [snapshot["location"]["id"]],
        "accepts_incomplete_coverage": false
    });
    json_response(call(&app, Method::POST, "/local/plan", request).await?).await?;
    let mut settings = OutputSettings::default();
    settings.format = domus::domain::AudioFormat::Flac;
    let change = json!({"expected_version":1,"settings":settings});
    let saved =
        json_response(call(&app, Method::PUT, "/local/settings", change.clone()).await?).await?;
    assert_eq!(saved["settings"]["version"], 2);
    assert_eq!(saved["settings"]["settings"]["format"], "flac");
    assert_eq!(saved["plan"]["plan"]["format"], "mp3");
    assert_eq!(
        call(&app, Method::PUT, "/local/settings", change)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

#[tokio::test]
async fn harness_only_browses_generated_roots_and_never_serves_raw_files() -> TestResult {
    let fixture = Fixture::new()?;
    let sentinel = fixture
        ._directory
        .path()
        .join("sample-library/S/SAMPLE ARTIST/SAMPLE ALBUM/README.txt");
    let before = std::fs::read(&sentinel)?;
    let app = router(fixture, "test".into(), vec![], vec![]);
    let root = json_response(call(&app, Method::GET, "/local/entries", Value::Null).await?).await?;
    assert!(
        root.as_array()
            .ok_or("root entries")?
            .iter()
            .any(|entry| entry["relative_path"] == "S")
    );
    let rows = json_response(
        call(
            &app,
            Method::GET,
            "/local/entries?folder=S%2FSAMPLE%20ARTIST%2FSAMPLE%20ALBUM",
            Value::Null,
        )
        .await?,
    )
    .await?;
    assert_eq!(rows[0]["kind"], "file");
    for path in [
        "/local/entries?folder=../outside",
        "/local/entries?folder=C%3A%5Cprivate",
        "/local/entries?after=-1",
    ] {
        assert_eq!(
            call(&app, Method::GET, path, Value::Null).await?.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        call(&app, Method::GET, "/README.txt", Value::Null)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, Method::GET, "/api/session", Value::Null)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read(sentinel)?, before);
    Ok(())
}

#[tokio::test]
async fn harness_guards_host_origin_csrf_and_body_size_without_cors() -> TestResult {
    let app = app()?;
    for (host, origin, csrf) in [
        ("untrusted.example", ORIGIN, "1"),
        (AUTHORITY, "http://untrusted.example", "1"),
        (AUTHORITY, ORIGIN, "wrong"),
        (AUTHORITY, "", "1"),
    ] {
        let request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/local/scan")
            .header(header::HOST, host)
            .header(header::ORIGIN, origin)
            .header("x-domus-csrf", csrf)
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            !response
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
    }
    let request = axum::http::Request::builder()
        .uri("/")
        .header(header::HOST, AUTHORITY)
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())?;
    assert_eq!(
        app.clone().oneshot(request).await?.status(),
        StatusCode::FORBIDDEN
    );
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/local/preview")
        .header(header::HOST, AUTHORITY)
        .header(header::ORIGIN, ORIGIN)
        .header("x-domus-csrf", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(vec![b' '; 64 * 1024 + 1]))?;
    assert_eq!(
        app.clone().oneshot(request).await?.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let response = call(&app, Method::GET, "/", Value::Null).await?;
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    assert!(
        response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()?
            .contains("frame-ancestors 'none'")
    );
    Ok(())
}

#[tokio::test]
async fn harness_invalid_preview_does_not_create_a_plan() -> TestResult {
    let app = app()?;
    let mut settings = OutputSettings::default();
    settings.template.filename = "../escape.mp3".into();
    assert_eq!(
        call(
            &app,
            Method::POST,
            "/local/preview",
            json!({
                "edition": edition(), "settings": settings
            })
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    let snapshot =
        json_response(call(&app, Method::GET, "/local/snapshot", Value::Null).await?).await?;
    assert!(snapshot["plan"].is_null());
    assert_eq!(snapshot["plan_version"], 1);
    Ok(())
}

#[tokio::test]
async fn harness_album_route_returns_verified_files_and_only_generated_audio() -> TestResult {
    let app = app()?;
    assert!(
        json_response(call(&app, Method::GET, "/local/album-test", Value::Null).await?)
            .await?
            .is_null()
    );
    assert_eq!(
        call(&app, Method::GET, "/local/album-test/audio/1", Value::Null)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let report =
        json_response(call(&app, Method::POST, "/local/album-test", json!({})).await?).await?;
    assert_eq!(
        report["tracks"].as_array().ok_or("missing tracks")?.len(),
        3
    );
    let directory = Path::new(report["directory"].as_str().ok_or("missing output path")?);
    for number in 1..=3 {
        let response = call(
            &app,
            Method::GET,
            &format!("/local/album-test/audio/{number}"),
            Value::Null,
        )
        .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "audio/flac");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await?;
        let track = &report["tracks"][number - 1];
        assert_eq!(
            bytes.as_ref(),
            std::fs::read(directory.join(track["filename"].as_str().ok_or("missing filename")?))?
        );
        assert_eq!(track["evidence"]["md5_verified"], true);
    }
    assert_eq!(
        call(&app, Method::GET, "/local/album-test/audio/0", Value::Null)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&app, Method::GET, "/local/album-test/audio/4", Value::Null)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let current =
        json_response(call(&app, Method::GET, "/local/album-test", Value::Null).await?).await?;
    assert_eq!(current, report);
    let snapshot =
        json_response(call(&app, Method::GET, "/local/snapshot", Value::Null).await?).await?;
    assert!(snapshot["plan"].is_null());
    drop(app);
    assert!(!directory.exists());
    Ok(())
}
