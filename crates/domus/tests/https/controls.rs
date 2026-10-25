use super::*;
use axum::extract::ConnectInfo;

#[tokio::test]
async fn source_reviews_need_guarded_separate_digest_approval_without_starting_media() -> TestResult
{
    use domus::application::{add_location, scan_location};
    use domus::domain::{EditionSelection, PlannedTrack};
    use domus::infrastructure::store::PlanRequest;
    let state = tempfile::tempdir()?;
    let media = tempfile::tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let main = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, main)?;
    store.save_settings(None, &OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let catalog = support::catalog_snapshot(
        &database,
        &EditionSelection {
            release_id: "12345678-1234-4321-9876-123456789abc".into(),
            album_artist: "Fixture Artist".into(),
            album: "Fixture Album".into(),
            bucket: None,
            tracks: vec![PlannedTrack {
                artist: "Fixture Artist".into(),
                title: "First".into(),
                disc: None,
                disc_track: None,
            }],
        },
    )?;
    let plan = store.prepare_catalog_plan(
        PlanRequest {
            job_id: job,
            expected_version: 1,
            settings_version: 1,
            format: None,
            edition: catalog.data.selection(None)?,
            locations: vec![main],
            accepts_incomplete_coverage: false,
        },
        catalog,
    )?;
    store.approve_plan(job, plan.version)?;
    let router = Api::development("https://localhost:8443", store)?.router();
    let path = format!("/api/jobs/{job}/plans/{}/source-candidates", plan.version);
    let folder = json!({"peer":"fixture-peer","folder":"Peer\\Album","directories":1,"files":[{
        "remote_path":"Peer\\Album\\First.mp3","expected_bytes":123,"extension":"mp3",
        "bitrate_kbps":320,"duration_seconds":30,"vbr":false,"sample_rate":44100,"bit_depth":null,
    }]});
    assert_eq!(
        request(&router, "POST", &path, Some(folder.clone()), false)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut unexpected = folder.clone();
    unexpected["approved"] = json!(true);
    assert_eq!(
        request(&router, "POST", &path, Some(unexpected), true)
            .await?
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut oversized = folder.clone();
    oversized["folder"] = json!("x".repeat(1_048_576));
    assert_eq!(
        request(&router, "POST", &path, Some(oversized), true)
            .await?
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let response = request(&router, "POST", &path, Some(folder.clone()), true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let candidate = json_body(response).await?;
    assert!(candidate["approved_at"].is_null());
    assert_eq!(candidate["assessment"]["edition_verified"], false);
    assert_eq!(candidate["assessment"]["audio_verified"], false);
    let id = candidate["id"].as_i64().ok_or("candidate")?;
    let detail = format!("/api/source-candidates/{id}");
    assert_eq!(
        json_body(request(&router, "GET", &detail, None, false).await?).await?,
        candidate
    );
    assert_eq!(
        json_body(request(&router, "GET", &path, None, false).await?).await?,
        json!([candidate])
    );
    assert_eq!(
        request(&router, "GET", &format!("{path}?limit=201"), None, false)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let approval = format!("{detail}/approve");
    let body = json!({"plan_version":plan.version,"review_sha256":candidate["review_sha256"],
        "acknowledge_exact_edition_and_permitted_download":true});
    assert_eq!(
        request(&router, "POST", &approval, Some(body.clone()), false)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut invalid = body.clone();
    invalid["review_sha256"] = json!("x".repeat(513));
    assert_eq!(
        request(&router, "POST", &approval, Some(invalid), true)
            .await?
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut invalid = body.clone();
    invalid["force"] = json!(true);
    assert_eq!(
        request(&router, "POST", &approval, Some(invalid), true)
            .await?
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut invalid = body.clone();
    invalid["acknowledge_exact_edition_and_permitted_download"] = json!(false);
    assert_eq!(
        request(&router, "POST", &approval, Some(invalid), true)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = request(&router, "POST", &approval, Some(body.clone()), true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let approved = json_body(response).await?;
    assert!(approved["approved_at"].is_number());
    assert_eq!(approved["assessment"], candidate["assessment"]);
    assert_eq!(
        request(&router, "POST", &approval, Some(body), true)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let raw = rusqlite::Connection::open(&database)?;
    let effects: i64 = raw.query_row(
        "SELECT (SELECT count(*) FROM acquisition_sources)
        +(SELECT count(*) FROM transfer_files)+(SELECT count(*) FROM acquisition_source_reviews)
        +(SELECT count(*) FROM jobs WHERE owner IS NOT NULL)",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(effects, 0);
    assert_eq!(std::fs::read_dir(media.path())?.count(), 0);
    assert_eq!(
        Store::open_read_snapshot(&database)?
            .job_state(job)?
            .as_deref(),
        Some("queued")
    );
    Ok(())
}

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    guard: bool,
) -> TestResult<axum::response::Response> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost:8443")
        .header(header::CONTENT_TYPE, "application/json");
    if guard {
        builder = builder
            .header(header::ORIGIN, "https://localhost:8443")
            .header("x-domus-csrf", "1");
    }
    let mut request = builder.body(
        body.map(|value| Body::from(value.to_string()))
            .unwrap_or_else(Body::empty),
    )?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    Ok(router.clone().oneshot(request).await?)
}

#[tokio::test]
async fn operator_controls_need_guards_and_explicit_answer_then_continue() -> TestResult {
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let job = store.enqueue(1)?;
    let router = Api::development("https://localhost:8443", store)?.router();
    let path = format!("/api/jobs/{job}/plans/1/controls");
    let body = json!({"action":"pause"});
    assert_eq!(
        request(&router, "POST", &path, Some(body.clone()), false)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &path,
            Some(json!({"action":"restart"})),
            true
        )
        .await?
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &path,
            Some(json!({"action":"pause","force":true})),
            true
        )
        .await?
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let response = request(&router, "POST", &path, Some(body), true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let control = json_body(response).await?;
    let id = control["id"].as_i64().ok_or("control ID")?;
    let question = control["question_id"].as_i64().ok_or("question ID")?;
    assert!(control["applied_at"].is_number());
    let response = request(
        &router,
        "GET",
        &format!("/api/job-controls/{id}"),
        None,
        false,
    )
    .await?;
    assert_eq!(json_body(response).await?, control);
    let response = request(&router, "GET", &path, None, false).await?;
    assert_eq!(json_body(response).await?, json!([control]));
    assert_eq!(
        request(&router, "GET", &format!("{path}?limit=201"), None, false)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&router, "GET", &format!("{path}?after=-1"), None, false)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let continuation = format!("/api/job-controls/{id}/continue");
    assert_eq!(
        request(
            &router,
            "POST",
            &continuation,
            Some(json!({"plan_version":1})),
            false
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &continuation,
            Some(json!({"plan_version":1})),
            true
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &format!("/api/questions/{question}/answer"),
            Some(json!({"plan_version":1,"choice":"Resume"})),
            true
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    let reopened = Store::open_read_snapshot(&database)?;
    assert_eq!(reopened.job_state(job)?.as_deref(), Some("awaiting_input"));
    drop(reopened);
    assert_eq!(
        request(
            &router,
            "POST",
            &continuation,
            Some(json!({"plan_version":2})),
            true
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let response = request(
        &router,
        "POST",
        &continuation,
        Some(json!({"plan_version":1})),
        true,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await?, json!("queued"));
    assert_eq!(
        Store::open_read_snapshot(&database)?
            .job_state(job)?
            .as_deref(),
        Some("queued")
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &continuation,
            Some(json!({"plan_version":1})),
            true
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}
