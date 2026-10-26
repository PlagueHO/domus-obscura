use super::*;
use axum::extract::ConnectInfo;

#[tokio::test]
async fn queue_dismissal_requires_command_guards_and_keeps_diagnostic_jobs() -> TestResult {
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let finished = store.enqueue(1)?;
    let active = store.enqueue(1)?;
    let raw = rusqlite::Connection::open(&database)?;
    raw.execute("UPDATE jobs SET state='complete' WHERE id=?1", [finished])?;
    let router = Api::development("https://localhost:8443", store)?.router();
    let path = "/api/queue/dismiss";
    let body = json!([{"job_id":finished,"plan_version":1}]);
    assert_eq!(
        request(&router, "POST", path, Some(body.clone()), false)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &router,
            "POST",
            path,
            Some(json!([
                {"job_id":finished,"plan_version":1},
                {"job_id":active,"plan_version":1}
            ])),
            true
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let queue =
        json_body(request(&router, "GET", "/api/queue?limit=50", None, false).await?).await?;
    assert_eq!(queue.as_array().ok_or("queue")?.len(), 2);
    assert_eq!(
        request(&router, "POST", path, Some(body), true)
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let queue =
        json_body(request(&router, "GET", "/api/queue?limit=50", None, false).await?).await?;
    assert_eq!(queue.as_array().ok_or("queue")?.len(), 1);
    assert_eq!(queue[0]["id"], active);
    let jobs = json_body(request(&router, "GET", "/api/jobs?limit=50", None, false).await?).await?;
    assert_eq!(jobs.as_array().ok_or("jobs")?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn library_root_configuration_is_guarded_bound_and_never_changes_media() -> TestResult {
    let state = tempfile::tempdir()?;
    let media = tempfile::tempdir()?;
    let backup = tempfile::tempdir()?;
    std::fs::write(media.path().join("untouched.txt"), b"keep these bytes")?;
    let database = state.path().join("state.db");
    let store = Store::open(&database)?;
    let router = Api::development("https://localhost:8443", store)?
        .with_system_settings_read_only(false)?
        .router();
    let reader = Api::development("https://localhost:8443", Store::open_existing(&database)?)?
        .with_system_settings_read_only(true)?
        .router();
    let body = json!({"root": media.path(), "kind":"main"});
    for (api, guarded) in [(&reader, true), (&router, false)] {
        assert_eq!(
            request(api, "POST", "/api/locations", Some(body.clone()), guarded)
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(
            Store::open_existing(&database)?
                .locations(domus::domain::Page::new(0, 100)?)?
                .is_empty()
        );
    }
    for invalid in [
        json!({"root":"relative", "kind":"main"}),
        json!({"root":media.path(), "kind":"other"}),
        json!({"root":media.path(), "kind":"main", "replace":true}),
    ] {
        assert!(
            request(&router, "POST", "/api/locations", Some(invalid), true)
                .await?
                .status()
                .is_client_error()
        );
    }
    let response = request(&router, "POST", "/api/locations", Some(body.clone()), true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let registered = json_body(response).await?;
    let id = registered["id"].as_i64().ok_or("location ID")?;
    assert!(registered["native_identity"].is_string());
    assert_ne!(registered["coverage"], "complete");
    assert_eq!(
        request(&router, "POST", "/api/locations", Some(body), true)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &router,
            "POST",
            "/api/locations",
            Some(json!({"root":backup.path(), "kind":"main"})),
            true
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let path = format!("/api/locations/{id}/scan");
    for (api, guarded) in [(&reader, true), (&router, false)] {
        assert_eq!(
            request(api, "POST", &path, Some(json!({})), guarded)
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = request(&router, "POST", &path, Some(json!({})), true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await?["coverage"], "complete");
    let response = request(
        &router,
        "POST",
        "/api/locations",
        Some(json!({"root":backup.path(), "kind":"backup"})),
        true,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let roots = json_body(request(&reader, "GET", "/api/locations", None, false).await?).await?;
    assert_eq!(roots.as_array().ok_or("locations")?.len(), 2);
    assert_eq!(
        std::fs::read(media.path().join("untouched.txt"))?,
        b"keep these bytes"
    );
    assert!(std::fs::read_dir(backup.path())?.next().is_none());
    let nested = media.path().join("nested");
    std::fs::create_dir(&nested)?;
    assert_eq!(
        request(
            &router,
            "POST",
            "/api/locations",
            Some(json!({"root":nested, "kind":"backup"})),
            true
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    Ok(())
}

#[tokio::test]
async fn settings_policy_guards_writes_and_preserves_versions_after_failed_saves() -> TestResult {
    for read_only in [true, false] {
        let state = tempfile::tempdir()?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let original = OutputSettings::default();
        store.save_settings(None, &original)?;
        let router = Api::development("https://localhost:8443", store)?
            .with_system_settings_read_only(read_only)?
            .router();
        let reader = Api::development("https://localhost:8443", Store::open_existing(&database)?)?
            .with_system_settings_read_only(true)?
            .router();
        assert_eq!(
            json_body(request(&router, "GET", "/api/system/settings-policy", None, false).await?)
                .await?,
            json!({"read_only": read_only})
        );
        let mut replacement = original.clone();
        replacement.format = domus::domain::AudioFormat::Flac;
        let body = json!({"expected_version":1,"settings":replacement});
        assert_eq!(
            request(&router, "PUT", "/api/settings", Some(body.clone()), false)
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
        let response = request(&router, "PUT", "/api/settings", Some(body.clone()), true).await?;
        if read_only {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(
                Store::open_existing(&database)?
                    .settings()?
                    .ok_or("settings")?
                    .version,
                1
            );
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(json_body(response).await?, json!(2));
            assert_eq!(
                request(&router, "PUT", "/api/settings", Some(body), true)
                    .await?
                    .status(),
                StatusCode::CONFLICT
            );
            let mut invalid = replacement.clone();
            invalid.template.filename = "../<Title>.<ext>".into();
            assert_eq!(
                request(
                    &router,
                    "PUT",
                    "/api/settings",
                    Some(json!({"expected_version":2,"settings":invalid})),
                    true
                )
                .await?
                .status(),
                StatusCode::BAD_REQUEST
            );
            let saved = Store::open_existing(&database)?
                .settings()?
                .ok_or("settings")?;
            assert_eq!(saved.version, 2);
            assert_eq!(saved.settings, replacement);
            let shared =
                json_body(request(&reader, "GET", "/api/settings", None, false).await?).await?;
            assert_eq!(shared["version"], 2);
            assert_eq!(shared["settings"], serde_json::to_value(&replacement)?);
        }
        assert_eq!(
            request(
                &router,
                "PUT",
                "/api/system/settings-policy",
                Some(json!({"read_only":false})),
                true
            )
            .await?
            .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            request(
                &router,
                "PUT",
                "/api/settings",
                Some(json!({"expected_version":1,"settings":original,"read_only":false})),
                true
            )
            .await?
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    Ok(())
}

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
    let details_path = format!("/api/questions/{question}");
    let details = json_body(request(&router, "GET", &details_path, None, false).await?).await?;
    assert_eq!(details["current"], true);
    assert_eq!(details["recovery"]["kind"], "job_control");
    assert_eq!(details["recovery"]["control"], control);
    assert!(details["question"]["answer"].is_null());
    assert_eq!(
        json_body(
            request(
                &router,
                "GET",
                &format!("/api/jobs/{job}/plans/1/decision"),
                None,
                false
            )
            .await?
        )
        .await?,
        details
    );
    assert_eq!(
        request(&router, "GET", "/api/inbox?limit=201", None, false)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&router, "GET", "/api/questions/0", None, false)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&router, "GET", "/api/questions/999999", None, false)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
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
    let pending = json_body(request(&router, "GET", "/api/inbox", None, false).await?).await?;
    assert_eq!(pending[0]["id"], question);
    assert_eq!(pending[0]["answer"], "Resume");
    let answered = json_body(request(&router, "GET", &details_path, None, false).await?).await?;
    assert_eq!(answered["current"], true);
    assert_eq!(answered["question"]["answer"], "Resume");
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
        json_body(request(&router, "GET", "/api/inbox", None, false).await?).await?,
        json!([])
    );
    let applied = json_body(request(&router, "GET", &details_path, None, false).await?).await?;
    assert_eq!(applied["current"], false);
    assert!(applied["recovery"]["control"]["continued_at"].is_number());
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
