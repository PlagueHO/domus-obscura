use super::*;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::io::Cursor;
use std::path::Path;

pub(super) async fn check_artwork(
    router: &Router,
    browser: &reqwest::Client,
    database: &Path,
    job: i64,
    cookie: &str,
) -> TestResult {
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 20, Rgb([10, 100, 240])))
        .write_to(&mut png, ImageFormat::Png)?;
    let mut store = Store::open_existing(database)?;
    let selected = store.prepare_artwork(job, 2, "123456789", png.get_ref(), None)?;
    let jpeg = store.artwork_jpeg(job, 2, selected.id)?;
    let base = format!("/api/jobs/{job}/plans/2/artwork");
    let image = format!("{base}/{}/image", selected.id);
    let approve = format!("{base}/approve");
    for (method, path) in [("GET", &base), ("GET", &image), ("POST", &approve)] {
        assert_eq!(
            api_request(router, method, path, None, Some(ORIGIN), None)
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let listing = api_request(router, "GET", &base, Some(cookie), None, None).await?;
    assert_eq!(listing.status(), StatusCode::OK);
    let candidates = json_body(listing).await?;
    assert_eq!(candidates[0]["id"], selected.id);
    assert_eq!(candidates[0]["sha256"], selected.sha256);
    assert_eq!(candidates[0]["approved_at"], Value::Null);
    let evidence_path = format!("{base}/{}/evidence", selected.id);
    assert_eq!(
        api_request(router, "GET", &evidence_path, None, None, None)
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        json_body(api_request(router, "GET", &evidence_path, Some(cookie), None, None).await?)
            .await?,
        Value::Null
    );
    let caa_path = format!("{base}/caa");
    let request =
        json!({"image_id":"9007199254740995","background":null,"acknowledge_permitted_use":true});
    assert_eq!(
        api_request(
            router,
            "POST",
            &caa_path,
            None,
            Some(ORIGIN),
            Some(request.clone())
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        api_request(
            router,
            "POST",
            &caa_path,
            Some(cookie),
            None,
            Some(request.clone())
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    // Generated retained responses exercise the API without a live provider.
    let raw = rusqlite::Connection::open(database)?;
    let listing_source = format!("https://coverartarchive.org/release/{RELEASE}/");
    let image_source = format!("https://coverartarchive.org/release/{RELEASE}/9007199254740995");
    let listing = serde_json::to_vec(&json!({
        "release":format!("https://musicbrainz.org/release/{RELEASE}"),
        "images":[{"id":"9007199254740995","edit":123,"types":["Front"],"front":true,"back":false,
            "approved":true,"comment":"Generated fixture",
            "image":format!("{image_source}.png")}]
    }))?;
    use sha2::{Digest, Sha256};
    for (key, body, source) in [
        (listing_source.clone(), listing, listing_source),
        (
            format!("{image_source}#mbid-{RELEASE}-9007199254740995.png"),
            png.get_ref().clone(),
            image_source,
        ),
    ] {
        raw.execute(
            "INSERT INTO caa_cache(cache_key,body,sha256,sources,fetched_at,expires_at,accessed_at_ms)
             VALUES (?1,?2,?3,?4,unixepoch(),unixepoch()+86400,unixepoch()*1000)",
            rusqlite::params![key,body,&Sha256::digest(&body)[..],serde_json::to_string(&vec![source])?],
        )?;
    }
    let mut unpermitted = request.clone();
    unpermitted["acknowledge_permitted_use"] = json!(false);
    assert_eq!(
        api_request(
            router,
            "POST",
            &caa_path,
            Some(cookie),
            Some(ORIGIN),
            Some(unpermitted)
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let questions = store.questions(domus::domain::Page::new(0, 200)?)?;
    let question = questions
        .iter()
        .find(|q| q.job_id == job)
        .ok_or("artwork question missing")?;
    let question_id = question.id;
    let question_path = format!("/api/questions/{question_id}/artwork");
    let resume_path = format!("{question_path}/resume");
    let check_path = format!("{base}/check");
    for (method, path) in [
        ("GET", &question_path),
        ("POST", &check_path),
        ("POST", &resume_path),
    ] {
        assert_eq!(
            api_request(router, method, path, None, Some(ORIGIN), None)
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for path in [&check_path, &resume_path] {
        assert_eq!(
            api_request(router, "POST", path, Some(cookie), None, None)
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let check = json_body(
        api_request(
            router,
            "POST",
            &check_path,
            Some(cookie),
            Some(ORIGIN),
            None,
        )
        .await?,
    )
    .await?;
    assert_eq!(check["question_id"], question_id);
    assert_eq!(check["issue"], "permission");
    let saved =
        json_body(api_request(router, "GET", &question_path, Some(cookie), None, None).await?)
            .await?;
    assert_eq!(saved["paused_state"], "queued");
    assert!(saved["candidate_id"].is_null());
    let details = json_body(
        api_request(
            router,
            "GET",
            &format!("/api/questions/{question_id}"),
            Some(cookie),
            None,
            None,
        )
        .await?,
    )
    .await?;
    assert_eq!(details["current"], true);
    assert_eq!(details["recovery"]["kind"], "artwork");
    assert_eq!(details["recovery"]["decision"], saved);
    let answer_path = format!("/api/questions/{question_id}/answer");
    let use_cover = json!({"plan_version":2,"choice":"Use approved cover"});
    assert_eq!(
        api_request(
            router,
            "POST",
            &answer_path,
            Some(cookie),
            Some(ORIGIN),
            Some(use_cover.clone())
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        api_request(
            router,
            "POST",
            &resume_path,
            Some(cookie),
            Some(ORIGIN),
            Some(json!({"plan_version":2}))
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let prepared = api_request(
        router,
        "POST",
        &caa_path,
        Some(cookie),
        Some(ORIGIN),
        Some(request.clone()),
    )
    .await?;
    assert_eq!(prepared.status(), StatusCode::OK);
    let prepared = json_body(prepared).await?;
    assert_eq!(prepared["approved_at"], Value::Null);
    let caa_id = prepared["id"].as_i64().ok_or("CAA candidate ID")?;
    let evidence_path = format!("{base}/{caa_id}/evidence");
    let evidence =
        json_body(api_request(router, "GET", &evidence_path, Some(cookie), None, None).await?)
            .await?;
    assert_eq!(evidence["image"]["provider_approved"], true);
    assert_eq!(evidence["permitted_use_acknowledged"], true);
    assert_eq!(evidence["image_sha256"], prepared["input_sha256"]);
    assert!(store.approved_artwork(job, 2).is_err());
    let saved_gate: (i64, u32) = raw.query_row(
        "SELECT next_request_ms,delay_seconds FROM catalog_gate WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    raw.execute(
        "UPDATE catalog_gate SET next_request_ms=9223372036854775807,delay_seconds=4294967295 WHERE id=1",
        [],
    )?;
    let retrieved = api_request(
        router,
        "POST",
        &format!("{base}/caa/download"),
        Some(cookie),
        Some(ORIGIN),
        Some(request),
    )
    .await?;
    assert_eq!(retrieved.status(), StatusCode::OK);
    let retrieved = json_body(retrieved).await?;
    assert_eq!(retrieved["id"], prepared["id"]);
    assert_eq!(retrieved["sha256"], prepared["sha256"]);
    assert_eq!(retrieved["width"], 40);
    assert_eq!(retrieved["height"], 20);
    assert_eq!(retrieved["approved_at"], Value::Null);
    assert!(store.approved_artwork(job, 2).is_err());
    raw.execute(
        "UPDATE catalog_gate SET next_request_ms=?1,delay_seconds=?2 WHERE id=1",
        rusqlite::params![saved_gate.0, saved_gate.1],
    )?;
    assert_eq!(
        api_request(
            router,
            "GET",
            &format!("/api/jobs/{}/plans/2/artwork/{caa_id}/evidence", job + 1),
            Some(cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );
    let wire = browser
        .get(format!("{ORIGIN}{image}"))
        .header(header::COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(wire.status(), StatusCode::OK);
    assert_eq!(wire.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(wire.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(wire.headers()["x-content-type-options"], "nosniff");
    assert_eq!(&wire.bytes().await?[..], jpeg.bytes());
    let decision = json!({"candidate_id":selected.id,"sha256":selected.sha256});
    assert_eq!(
        api_request(
            router,
            "POST",
            &approve,
            Some(cookie),
            None,
            Some(decision.clone())
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api_request(
            router,
            "POST",
            &approve,
            Some(cookie),
            Some(ORIGIN),
            Some(json!({"candidate_id":selected.id,"sha256":"0".repeat(64)}))
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        api_request(
            router,
            "POST",
            &approve,
            Some(cookie),
            Some(ORIGIN),
            Some(json!({"candidate_id":selected.id,"sha256":selected.sha256,"extra":true}))
        )
        .await?
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        api_request(
            router,
            "POST",
            &approve,
            Some(cookie),
            Some(ORIGIN),
            Some(json!({"candidate_id":selected.id,"sha256":"0".repeat(600)}))
        )
        .await?
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(store.approved_artwork(job, 2).is_err());
    assert_eq!(
        api_request(
            router,
            "GET",
            &format!(
                "/api/jobs/{}/plans/2/artwork/{}/image",
                job + 1,
                selected.id
            ),
            Some(cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        assert_eq!(
            api_request(
                router,
                "POST",
                &approve,
                Some(cookie),
                Some(ORIGIN),
                Some(decision.clone())
            )
            .await?
            .status(),
            StatusCode::NO_CONTENT
        );
    }
    let approved = api_request(router, "GET", &base, Some(cookie), None, None).await?;
    assert!(json_body(approved).await?[0]["approved_at"].is_number());
    assert_eq!(store.approved_artwork(job, 2)?.bytes(), jpeg.bytes());
    assert_eq!(
        api_request(
            router,
            "POST",
            &answer_path,
            Some(cookie),
            Some(ORIGIN),
            Some(use_cover)
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(store.job_state(job)?.as_deref(), Some("awaiting_input"));
    assert_eq!(
        api_request(
            router,
            "POST",
            &resume_path,
            Some(cookie),
            Some(ORIGIN),
            Some(json!({"plan_version":3}))
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        api_request(
            router,
            "POST",
            &resume_path,
            Some(cookie),
            Some(ORIGIN),
            Some(json!({"plan_version":2,"extra":true}))
        )
        .await?
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        api_request(
            router,
            "POST",
            &resume_path,
            Some(cookie),
            Some(ORIGIN),
            Some(json!({"plan_version":2}))
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(store.job_state(job)?.as_deref(), Some("queued"));
    let resolved =
        json_body(api_request(router, "GET", &question_path, Some(cookie), None, None).await?)
            .await?;
    assert_eq!(resolved["candidate_id"], selected.id);
    assert_eq!(resolved["sha256"], selected.sha256);
    assert!(resolved["resumed_at"].is_number());
    Ok(())
}
