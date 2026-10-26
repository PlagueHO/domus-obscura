use super::*;
use domus::application::{add_location, scan_location};
use domus::domain::{JobState, Page};
use domus::infrastructure::catalog::CatalogQuery;
use domus::infrastructure::local_runtime::LocalRuntime;
use domus::infrastructure::staging::StagingRoot;
use domus::infrastructure::worker::LocalAcquisition;
use id3::{TagLike, Version};
use sha2::{Digest, Sha256};
use std::io::Cursor;
#[path = "../support/icc.rs"]
mod icc;
#[path = "../support/media.rs"]
#[allow(dead_code)]
mod media;

const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
const ARTIST: &str = "Fixture Artist";
const ALBUM: &str = "Fixture Album";
const TITLES: [&str; 3] = ["First Light", "Second Signal", "Third Return"];

fn seed_release(database: &std::path::Path) -> TestResult {
    let credit = json!([{"name":ARTIST,"joinphrase":"","artist":{
        "id":"10000000-0000-4000-8000-000000000001","name":ARTIST
    }}]);
    let track = |number: usize, position: usize| {
        json!({
            "id":format!("40000000-0000-4000-8000-{number:012}"),
            "position":position,"number":position.to_string(),"title":TITLES[number-1],
            "artist-credit":credit,"recording":{
                "id":format!("50000000-0000-4000-8000-{number:012}"),"video":false
            }
        })
    };
    let body = serde_json::to_vec(&json!({
        "id":RELEASE,"title":ALBUM,"date":"2026-10-25","artist-credit":credit,
        "release-group":{"id":"30000000-0000-4000-8000-000000000001"},
        "media":[
            {"position":1,"track-count":2,"tracks":[track(1,1),track(2,2)]},
            {"position":2,"track-count":1,"tracks":[track(3,1)]}
        ]
    }))?;
    rusqlite::Connection::open(database)?.execute(
        "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
         VALUES (?1,?2,?3,unixepoch(),unixepoch()+86400,unixepoch()*1000)",
        rusqlite::params![
            CatalogQuery::release(RELEASE)?.source(),
            body,
            Sha256::digest(&body).to_vec()
        ],
    )?;
    Ok(())
}

async fn command(
    client: &reqwest::Client,
    origin: &str,
    path: &str,
    body: Value,
) -> TestResult<Value> {
    let request = client
        .post(format!("{origin}{path}"))
        .header(header::ORIGIN, origin)
        .header("x-domus-csrf", "1");
    let request = if body.is_null() {
        request
    } else {
        request
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body)?)
    };
    let response = request.send().await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    assert!(
        status.is_success(),
        "{path}: {status} {}",
        String::from_utf8_lossy(&bytes)
    );
    if bytes.is_empty() {
        Ok(Value::Null)
    } else {
        Ok(serde_json::from_slice(&bytes)?)
    }
}

async fn idle(client: &reqwest::Client, origin: &str) -> TestResult<Value> {
    for _ in 0..200 {
        let status: Value = serde_json::from_slice(
            &client
                .get(format!("{origin}/api/local/status"))
                .send()
                .await?
                .bytes()
                .await?,
        )?;
        if status["busy"] == false {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err("local worker did not finish".into())
}

#[tokio::test]
async fn browser_caa_search_and_download_keep_exact_release_evidence_and_separate_approval()
-> TestResult {
    use axum::extract::ConnectInfo;
    use domus::application::catalog::{CatalogPlanRequest, prepare_cached_catalog_plan};
    let directory = private::tempdir()?;
    let database = directory.path().join("state.db");
    let library = directory.path().join("library");
    std::fs::create_dir(&library)?;
    let mut store = Store::open(&database)?;
    let location = add_location(&store, &library, "main")?;
    scan_location(&mut store, location)?;
    let settings = store.save_settings(None, &OutputSettings::default())?;
    seed_release(&database)?;
    let job = store.enqueue(1)?;
    let plan = prepare_cached_catalog_plan(
        &mut store,
        RELEASE,
        CatalogPlanRequest {
            job_id: job,
            expected_version: 1,
            settings_version: settings,
            format: Some(domus::domain::AudioFormat::Mp3),
            locations: vec![location],
            accepts_incomplete_coverage: false,
            bucket: None,
        },
    )?;
    let base = format!("/api/jobs/{job}/plans/{}", plan.version);
    let image_id = "9007199254740995";
    let listing_source = format!("https://coverartarchive.org/release/{RELEASE}/");
    let image_source = format!("https://coverartarchive.org/release/{RELEASE}/{image_id}");
    let listing = serde_json::to_vec(&json!({
        "release":format!("https://musicbrainz.org/release/{RELEASE}"),
        "images":[{
            "id":image_id,"edit":321,"types":["Front"],"front":true,"back":false,
            "approved":true,"comment":"Generated cover",
            "image":format!("{image_source}.png")
        }]
    }))?;
    let raw = rusqlite::Connection::open(&database)?;
    let mut profiled_cover = Vec::new();
    {
        use image::ImageEncoder;
        let mut encoder = image::codecs::png::PngEncoder::new(&mut profiled_cover);
        encoder.set_icc_profile(icc::profile(moxcms::ColorProfile::new_adobe_rgb())?)?;
        encoder.write_image(
            &[80, 140, 180, 80, 140, 180],
            2,
            1,
            image::ExtendedColorType::Rgb8,
        )?;
    }
    for (key, bytes, sources) in [
        (listing_source.clone(), listing, vec![listing_source]),
        (
            format!("{image_source}#mbid-{RELEASE}-{image_id}.png"),
            profiled_cover,
            vec![
                image_source,
                format!(
                    "https://archive.org/download/mbid-{RELEASE}/mbid-{RELEASE}-{image_id}.png"
                ),
            ],
        ),
    ] {
        raw.execute(
            "INSERT INTO caa_cache(cache_key,body,sha256,sources,fetched_at,expires_at,accessed_at_ms)
             VALUES (?1,?2,?3,?4,unixepoch(),unixepoch()+86400,unixepoch()*1000)",
            rusqlite::params![key, bytes, Sha256::digest(&bytes).to_vec(), serde_json::to_string(&sources)?],
        )?;
    }
    let origin = "https://localhost:8443";
    let router = Api::development(origin, Store::open_existing(&database)?)?.router();
    let call = |path: String, body: Option<Value>, guarded: bool| {
        let router = router.clone();
        async move {
            let mut builder = Request::builder()
                .method("POST")
                .uri(path)
                .header(header::HOST, "localhost:8443")
                .header(header::CONTENT_TYPE, "application/json");
            if guarded {
                builder = builder
                    .header(header::ORIGIN, origin)
                    .header("x-domus-csrf", "1");
            }
            let mut request = builder.body(match body {
                Some(body) => Body::from(serde_json::to_vec(&body)?),
                None => Body::empty(),
            })?;
            request
                .extensions_mut()
                .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
            Ok::<_, Box<dyn std::error::Error>>(router.oneshot(request).await?)
        }
    };
    let search = format!("{base}/artwork/caa/search");
    let download = format!("{base}/artwork/caa/download");
    let body = json!({"image_id":image_id,"background":null,"acknowledge_permitted_use":true});
    for (path, body) in [
        (search.clone(), None),
        (download.clone(), Some(body.clone())),
    ] {
        assert_eq!(
            call(path, body, false).await?.status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(search.clone(), None, true).await?.status(),
        StatusCode::CONFLICT
    );
    store.approve_plan(job, plan.version)?;
    let response = call(search.clone(), None, true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let found = json_body(response).await?;
    assert_eq!(found["release_id"], RELEASE);
    assert_eq!(found["images"][0]["image_id"], image_id);
    assert_eq!(found["images"][0]["front"], true);
    assert_eq!(found["images"][0]["provider_approved"], true);
    assert!(found["images"][0].get("url").is_none());
    let response = call(
        download.clone(),
        Some(json!({
            "image_id":"123", "background":null,"acknowledge_permitted_use":true
        })),
        true,
    )
    .await?;
    assert!(!response.status().is_success());
    let response = call(download.clone(), Some(body.clone()), true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let candidate = json_body(response).await?;
    assert!(candidate["approved_at"].is_null());
    let candidate_id = candidate["id"].as_i64().ok_or("candidate ID")?;
    let jpeg = store.artwork_jpeg(job, plan.version, candidate_id)?;
    assert_eq!(
        candidate["sha256"],
        format!("{:x}", Sha256::digest(jpeg.bytes()))
    );
    let evidence = store
        .artwork_caa_evidence(job, plan.version, candidate_id)?
        .ok_or("CAA evidence")?;
    assert_eq!(evidence.release_id, RELEASE);
    assert_eq!(evidence.image.id, image_id);
    assert!(evidence.permitted_use_acknowledged);
    assert!(store.approved_artwork(job, plan.version).is_err());
    let repeat = call(download.clone(), Some(body), true).await?;
    assert_eq!(repeat.status(), StatusCode::OK);
    assert_eq!(json_body(repeat).await?["id"], candidate["id"]);
    assert_eq!(
        raw.query_row(
            "SELECT generation FROM catalog_gate WHERE id=1",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    assert_eq!(std::fs::read_dir(&library)?.count(), 0);
    // The old cache-only preparation route stays separate from network fetching.
    let missing_permission = call(
        download,
        Some(json!({
            "image_id":image_id,"background":null,"acknowledge_permitted_use":false
        })),
        true,
    )
    .await?;
    assert_eq!(missing_permission.status(), StatusCode::CONFLICT);
    let error = json_body(missing_permission).await?;
    let message = error["error"].as_str().ok_or("error")?;
    assert!(message.contains("question"));
    assert!(message.contains("confirm permitted use"));
    raw.execute(
        "UPDATE jobs SET plan_version=plan_version+1 WHERE id=?1",
        [job],
    )?;
    assert_eq!(
        call(search, None, true).await?.status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

/// The API remains the real guarded HTTPS router. Only catalog input and already
/// received peer bytes are generated. No live provider or account is contacted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_https_approves_exact_album_and_publishes_verified_mp3_folder() -> TestResult {
    approved_album(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_https_resumes_retained_publication_after_offline_backup() -> TestResult {
    approved_album(true).await
}

async fn approved_album(recover_backup: bool) -> TestResult {
    let directory = private::tempdir()?;
    let database = directory.path().join("state.db");
    let library = directory.path().join("library");
    let scratch = directory.path().join("staging");
    let backup = directory.path().join("backup");
    let offline = directory.path().join("backup-offline");
    std::fs::create_dir(&library)?;
    std::fs::create_dir(&scratch)?;
    private::directory(&scratch)?;
    let credentials = directory.path().join("credentials.json");
    std::fs::write(
        &credentials,
        br#"{"username":"fixture-user","password":"unused-fixture"}"#,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut store = Store::open(&database)?;
    let location = add_location(&store, &library, "main")?;
    scan_location(&mut store, location)?;
    let mut locations = vec![location];
    if recover_backup {
        std::fs::create_dir(&backup)?;
        let backup_id = add_location(&store, &backup, "backup")?;
        scan_location(&mut store, backup_id)?;
        locations.push(backup_id);
    }
    store.save_settings(None, &OutputSettings::default())?;
    drop(StagingRoot::open(&mut store, &scratch)?);
    seed_release(&database)?;
    let local = LocalRuntime::new(
        database.clone(),
        LocalAcquisition {
            staging_root: scratch.clone(),
            credentials_file: credentials,
            server: "8.8.8.8:2242".parse()?,
            acknowledge_plaintext_login: true,
        },
        Default::default(),
    )?;
    let material = development_tests::self_signed()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let origin = format!("https://localhost:{}", address.port());
    let assets = directory.path().join("browser");
    std::fs::create_dir_all(assets.join("assets"))?;
    std::fs::write(
        assets.join("index.html"),
        "<!doctype html><title>Local Music</title>",
    )?;
    std::fs::write(
        assets.join("assets").join("app.js"),
        "console.log('generated asset');",
    )?;
    let api = Api::development(&origin, store)?
        .with_browser(domus::infrastructure::browser::BrowserAssets::load(
            &assets,
        )?)?
        .with_local_runtime(local)?;
    let config = tls::configuration(
        "localhost",
        material.chain.as_bytes(),
        material.key.as_bytes(),
    )?;
    let handle = axum_server::Handle::new();
    let task = tokio::spawn(domus::infrastructure::server::serve_listener(
        listener,
        config.clone(),
        api.router(),
        handle.clone(),
    ));
    let server = FixtureServer {
        address,
        tls: config,
        handle,
        task: Some(task),
    };
    let client = reqwest::Client::builder()
        .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
        .resolve("localhost", address)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let identity: Value = serde_json::from_slice(
        &client
            .get(format!("{origin}/api/session"))
            .send()
            .await?
            .bytes()
            .await?,
    )?;
    assert_eq!(identity["object_id"], "_anonymous");
    let page = client.get(&origin).send().await?;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        page.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert!(page.headers().contains_key("content-security-policy"));
    assert_eq!(
        client
            .get(format!("{origin}/assets/app.js"))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    for path in [
        "/assets/private-key.pem",
        "/assets/..%2fstate.db",
        "/assets/missing.js",
    ] {
        assert_eq!(
            client.get(format!("{origin}{path}")).send().await?.status(),
            StatusCode::NOT_FOUND
        );
    }
    let created = command(&client, &origin, "/api/jobs", json!({})).await?;
    let job = created["job_id"].as_i64().ok_or("job ID")?;
    let planned = command(
        &client,
        &origin,
        &format!("/api/discovery/releases/{RELEASE}/plans"),
        json!({
            "job_id":job,"expected_version":1,"settings_version":1,"format":"mp3",
            "locations":locations,"accepts_incomplete_coverage":false,"bucket":null
        }),
    )
    .await?;
    let version = planned["version"].as_i64().ok_or("plan version")?;
    let base = format!("/api/jobs/{job}/plans/{version}");
    command(&client, &origin, &format!("{base}/approve"), Value::Null).await?;
    // A raw upload is still subject to both command guards.
    let cover = media::cover_source()?;
    assert_eq!(
        client
            .post(format!(
                "{origin}{base}/artwork/local?acknowledge_permitted_use=true"
            ))
            .body(cover.clone())
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = client
        .post(format!(
            "{origin}{base}/artwork/local?acknowledge_permitted_use=true"
        ))
        .header(header::ORIGIN, &origin)
        .header("x-domus-csrf", "1")
        .body(cover)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let artwork: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert!(
        artwork["source"]
            .as_str()
            .ok_or("local provenance")?
            .starts_with("urn:domus:local-artwork:sha256:")
    );
    let image = client
        .get(format!("{origin}{base}/artwork/{}/image", artwork["id"]))
        .send()
        .await?
        .bytes()
        .await?;
    assert_eq!(format!("{:x}", Sha256::digest(&image)), artwork["sha256"]);
    command(
        &client,
        &origin,
        &format!("{base}/artwork/approve"),
        json!({
            "candidate_id":artwork["id"],"sha256":artwork["sha256"]
        }),
    )
    .await?;
    let source = media::mp3()?;
    let files = TITLES
        .iter()
        .enumerate()
        .map(|(index, title)| {
            json!({
                "remote_path":format!("Peer\\Album\\{:02} - {title}.mp3",index+1),
                "expected_bytes":source.len(),"extension":"mp3","bitrate_kbps":null,
                "duration_seconds":null,"vbr":null,"sample_rate":null,"bit_depth":null
            })
        })
        .collect::<Vec<_>>();
    let candidate = command(
        &client,
        &origin,
        &format!("{base}/source-candidates"),
        json!({
            "peer":"fixture-peer","folder":"Peer\\Album","directories":1,"files":files
        }),
    )
    .await?;
    assert_eq!(candidate["assessment"]["complete_mapping"], true);
    assert_eq!(candidate["assessment"]["can_approve"], true);
    assert_eq!(candidate["assessment"]["advertised_quality_matches"], false);
    assert_eq!(candidate["assessment"]["audio_verified"], false);
    command(
        &client,
        &origin,
        &format!("{base}/run"),
        json!({
            "source_candidate":candidate["id"],"acknowledge_new_only_publication":true
        }),
    )
    .await?;
    let rejected = idle(&client, &origin).await?;
    assert!(rejected["last_error"].is_string());
    let raw = rusqlite::Connection::open(&database)?;
    let effects: i64 = raw.query_row(
        "SELECT (SELECT COUNT(*) FROM acquisition_sources) +
         (SELECT COUNT(*) FROM transfer_files) +
         (SELECT COUNT(*) FROM jobs WHERE owner IS NOT NULL)",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        effects, 0,
        "unapproved source must fail before any acquisition effect"
    );
    drop(raw);
    command(
        &client,
        &origin,
        &format!("/api/source-candidates/{}/approve", candidate["id"]),
        json!({
            "plan_version":version,"review_sha256":candidate["review_sha256"],
            "acknowledge_exact_edition_and_permitted_download":true
        }),
    )
    .await?;
    // Seal all generated input streams with the real reviewed acquisition
    // journal. The bound Soulseek channel tests cover the socket receive lane.
    let db = database.clone();
    let staging_path = scratch.clone();
    let source_copy = source.clone();
    let candidate_id = candidate["id"].as_i64().ok_or("candidate ID")?;
    tokio::task::spawn_blocking(move || -> domus::Result<()> {
        let mut store = Store::open_current_existing(&db)?;
        let claim = store
            .claim_approved_plan(job, version, "fixture-receiver", 300)?
            .ok_or(domus::Error::Internal("fixture claim missing"))?
            .claim;
        store.advance(&claim, JobState::Queued, JobState::Searching)?;
        store.begin_reviewed_acquisition(&claim, candidate_id)?;
        let staging = StagingRoot::open(&mut store, &staging_path)?;
        for file in store.transfers(job, version, Page::new(0, 200)?)? {
            store.start_transfer(&claim, file.id)?;
            store.accept_transfer(&claim, file.id, u32::from(file.track_number))?;
            let active = store.active_transfer(&claim, file.id)?;
            let received = staging.receive(&active, &mut Cursor::new(&source_copy))?;
            store.receive_transfer(
                &claim,
                file.id,
                i64::try_from(received.byte_length())
                    .map_err(|_| domus::Error::Internal("fixture length overflow"))?,
                received.sha256(),
            )?;
        }
        // Simulate a stopped receiver with all exact sealed receipts, not an
        // uncertain submission. The runtime must finish without any socket.
        rusqlite::Connection::open(&db)?
            .execute("UPDATE jobs SET lease_until=0 WHERE id=?1", [job])?;
        Ok(())
    })
    .await??;
    assert_eq!(
        client
            .post(format!("{origin}{base}/run"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&json!({
                "source_candidate":null,"acknowledge_new_only_publication":true
            }))?)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    if recover_backup {
        std::fs::rename(&backup, &offline)?;
    }
    command(
        &client,
        &origin,
        &format!("{base}/run"),
        json!({
            "source_candidate":null,"acknowledge_new_only_publication":true
        }),
    )
    .await?;
    let status = idle(&client, &origin).await?;
    assert!(status["last_error"].is_null(), "{status}");
    if recover_backup {
        let store = Store::open_read_snapshot(&database)?;
        assert_eq!(store.job_state(job)?.as_deref(), Some("backup_pending"));
        let targets = store.publication_destinations(job, version)?;
        let run = targets.first().ok_or("publication journal")?.run_id;
        assert_eq!(targets.len(), 2);
        let saved_transfers =
            serde_json::to_value(store.transfers(job, version, Page::new(0, 200)?)?)?;
        let raw = rusqlite::Connection::open(&database)?;
        let runs: i64 = raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| {
            row.get(0)
        })?;
        let path = format!("{origin}{base}/publication/resume");
        let body = json!({"normalization_run":run,"acknowledge_new_only_publication":true});
        assert_eq!(
            client
                .post(&path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(&body)?)
                .send()
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
        for invalid in [
            json!({"normalization_run":run,"acknowledge_new_only_publication":false}),
            json!({"normalization_run":0,"acknowledge_new_only_publication":true}),
            json!({"normalization_run":run,"acknowledge_new_only_publication":true,"staging_root":"injected"}),
        ] {
            assert!(
                client
                    .post(&path)
                    .header(header::ORIGIN, &origin)
                    .header("x-domus-csrf", "1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(serde_json::to_vec(&invalid)?)
                    .send()
                    .await?
                    .status()
                    .is_client_error()
            );
        }
        command(
            &client,
            &origin,
            &format!("{base}/publication/resume"),
            json!({"normalization_run":run+1,"acknowledge_new_only_publication":true}),
        )
        .await?;
        assert!(idle(&client, &origin).await?["last_error"].is_string());
        std::fs::rename(&offline, &backup)?;
        // Recovery uses no account credentials, catalog request or audio connection.
        std::fs::remove_file(directory.path().join("credentials.json"))?;
        command(
            &client,
            &origin,
            &format!("{base}/publication/resume"),
            body,
        )
        .await?;
        let resumed = idle(&client, &origin).await?;
        assert!(resumed["last_error"].is_null(), "{resumed}");
        let recovered = Store::open_read_snapshot(&database)?;
        assert_eq!(recovered.job_state(job)?.as_deref(), Some("complete"));
        assert_eq!(
            serde_json::to_value(recovered.transfers(job, version, Page::new(0, 200)?)?)?,
            saved_transfers
        );
        assert_eq!(
            raw.query_row("SELECT count(*) FROM normalization_runs", [], |row| row
                .get::<_, i64>(0))?,
            runs
        );
        assert_eq!(
            client.get(&path).send().await?.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    let store = Store::open_read_snapshot(&database)?;
    assert_eq!(store.job_state(job)?.as_deref(), Some("complete"));
    let destinations = store.publication_destinations(job, version)?;
    assert_eq!(destinations.len(), if recover_backup { 2 } else { 1 });
    assert!(
        destinations
            .iter()
            .all(|target| target.verified_at.is_some() && target.last_error.is_none())
    );
    let folder = library
        .join("F")
        .join("FIXTURE ARTIST")
        .join("FIXTURE ALBUM");
    let mut names = std::fs::read_dir(&folder)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    let mut expected = TITLES
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{ARTIST} - {ALBUM} - {:02} - {t}.mp3", i + 1))
        .collect::<Vec<_>>();
    expected.push("folder.jpg".into());
    expected.sort();
    assert_eq!(
        names,
        expected
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(std::fs::read(folder.join("folder.jpg"))?, image.as_ref());
    if recover_backup {
        let backup_folder = backup
            .join("F")
            .join("FIXTURE ARTIST")
            .join("FIXTURE ALBUM");
        for name in &expected {
            assert_eq!(
                std::fs::read(backup_folder.join(name))?,
                std::fs::read(folder.join(name))?
            );
        }
    }
    let source_proof = domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&source))?;
    for (index, title) in TITLES.iter().enumerate() {
        let path = folder.join(format!(
            "{ARTIST} - {ALBUM} - {:02} - {title}.mp3",
            index + 1
        ));
        let bytes = std::fs::read(&path)?;
        let proof = domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&bytes))?;
        assert_eq!(proof.mpeg_sha256, source_proof.mpeg_sha256);
        assert_eq!(proof.audio_frames, source_proof.audio_frames);
        let tag = id3::Tag::read_from_path(&path)?;
        assert_eq!(tag.version(), Version::Id3v23);
        assert_eq!(tag.title(), Some(*title));
        assert_eq!(tag.artist(), Some(ARTIST));
        assert_eq!(tag.album_artist(), Some(ARTIST));
        assert_eq!(tag.album(), Some(ALBUM));
        assert_eq!(tag.track(), Some((index + 1) as u32));
        assert_eq!(tag.total_tracks(), Some(3));
        assert_eq!(tag.disc(), Some(if index == 2 { 2 } else { 1 }));
        assert_eq!(tag.total_discs(), Some(2));
        assert_eq!(tag.year(), Some(2026));
        assert_eq!(
            tag.get("TDAT").and_then(|f| f.content().text()),
            Some("2510")
        );
        let extra = tag
            .extended_texts()
            .map(|t| (t.description.as_str(), t.value.as_str()))
            .collect::<HashMap<_, _>>();
        assert_eq!(extra.len(), 7);
        assert_eq!(
            extra.get("ORIGINALTRACKNUMBER"),
            Some(&if index == 1 { "2" } else { "1" })
        );
        assert_eq!(extra.get("DATE"), Some(&"2026-10-25"));
        assert_eq!(extra.get("MusicBrainz Album Id"), Some(&RELEASE));
        assert_eq!(
            extra.get("MusicBrainz Release Group Id"),
            Some(&"30000000-0000-4000-8000-000000000001")
        );
        assert_eq!(
            extra.get("MusicBrainz Release Track Id").copied(),
            Some(format!("40000000-0000-4000-8000-{:012}", index + 1).as_str())
        );
        for key in ["MusicBrainz Artist Id", "MusicBrainz Album Artist Id"] {
            assert_eq!(
                extra.get(key),
                Some(&"10000000-0000-4000-8000-000000000001")
            );
        }
        let identifiers = tag.unique_file_identifiers().collect::<Vec<_>>();
        assert_eq!(identifiers.len(), 1);
        assert_eq!(identifiers[0].owner_identifier, "http://musicbrainz.org");
        assert_eq!(
            identifiers[0].identifier,
            format!("50000000-0000-4000-8000-{:012}", index + 1).as_bytes()
        );
        let pictures = tag.pictures().collect::<Vec<_>>();
        assert_eq!(pictures.len(), 1);
        assert_eq!(pictures[0].mime_type, "image/jpeg");
        assert_eq!(
            pictures[0].picture_type,
            id3::frame::PictureType::CoverFront
        );
        assert_eq!(pictures[0].data, image.as_ref());
        assert!(tag.genre().is_none());
        assert!(!extra.contains_key("REPLAYGAIN_TRACK_GAIN"));
    }
    server.stop().await?;
    Ok(())
}
