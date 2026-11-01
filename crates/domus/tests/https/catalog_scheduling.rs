use super::*;
use std::process::{Command, Stdio};

async fn configured_api(
    database: &std::path::Path,
    fixture: &CatalogFixture,
) -> TestResult<(Api, String, FixtureServer, FixtureServer)> {
    let identity_material = certificates(AUTHORITY)?;
    let provider = Provider::new()?;
    let identity = FixtureServer::start(provider.router(), &identity_material, AUTHORITY)?;
    let broker = EntraBroker::discover(
        TENANT,
        OBJECT,
        CLIENT,
        ClientSecret::new("generated-fixture-secret".into()),
        RedirectUrl::new(format!("{ORIGIN}/auth/callback"))?,
        trusted_http(&identity_material, &identity)?,
    )
    .await?;
    let material = certificates(MUSIC_HOST)?;
    let metadata = FixtureServer::start(fixture.router(), &material, MUSIC_HOST)?;
    let api = Api::new_with_catalog(
        ORIGIN,
        Store::open(database)?,
        broker,
        client(&material, &metadata)?,
    )?;
    let cookie = browser_login(&api.router(), &provider, None).await?;
    Ok((api, cookie, metadata, identity))
}

async fn saved_headers(database: &std::path::Path, seconds: u32) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let connection = rusqlite::Connection::open(database)?;
        let saved: u32 =
            connection.query_row("SELECT delay_seconds FROM catalog_gate", [], |row| {
                row.get(0)
            })?;
        if saved == seconds {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("provider did not persist response headers".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn catalog_concurrent_reads_reuse_cache_and_distinct_requests_keep_cadence() -> TestResult {
    let root = tempfile::tempdir()?;
    let database = root.path().join("state.db");
    let fixture = CatalogFixture::default();
    let (api, cookie, metadata, identity) = configured_api(&database, &fixture).await?;
    let router = api.router();
    let responses = futures_util::future::join_all((0..4).map(|_| {
        api_request(
            &router,
            "GET",
            "/api/discovery/artists?query=shared",
            Some(&cookie),
            None,
            None,
        )
    }))
    .await;
    for response in responses {
        assert_eq!(response?.status(), StatusCode::OK);
    }
    assert_eq!(fixture.count()?, 1);
    let paths = (0..4)
        .map(|index| format!("/api/discovery/artists?query=distinct-{index}"))
        .collect::<Vec<_>>();
    let responses = futures_util::future::join_all(
        paths
            .iter()
            .map(|path| api_request(&router, "GET", path, Some(&cookie), None, None)),
    )
    .await;
    for response in responses {
        assert_eq!(response?.status(), StatusCode::OK);
    }
    assert_eq!(fixture.count()?, 5);
    {
        let requests = fixture.requests.lock().map_err(|_| "fixture lock failed")?;
        for pair in requests.windows(2) {
            assert!(pair[1].0.duration_since(pair[0].0) >= Duration::from_secs(1));
        }
    }
    metadata.stop().await?;
    identity.stop().await?;
    Ok(())
}

#[tokio::test]
async fn catalog_capacity_and_cancelled_waits_never_submit_network_requests() -> TestResult {
    let root = tempfile::tempdir()?;
    let database = root.path().join("state.db");
    let fixture = CatalogFixture::default();
    let (api, cookie, metadata, identity) = configured_api(&database, &fixture).await?;
    let mut store = Store::open_existing(&database)?;
    let owner = store.claim_catalog_request()?;
    let router = api.router();
    let paths = (0..8)
        .map(|index| format!("/api/discovery/artists?query=pending-{index}"))
        .collect::<Vec<_>>();
    let mut work = Vec::new();
    for path in paths {
        let router = router.clone();
        let cookie = cookie.clone();
        work.push(tokio::spawn(async move {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let response = api_request(&router, "GET", &path, Some(&cookie), None, None)
                    .await
                    .map_err(|error| error.to_string())?;
                let status = response.status();
                let body = json_body(response)
                    .await
                    .map_err(|error| error.to_string())?;
                // Cache lookup has its own capacity; reach the provider lane before checking it.
                if status == StatusCode::SERVICE_UNAVAILABLE
                    && body["error"] == "database is busy; retry later"
                    && Instant::now() < deadline
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
                return Ok::<_, String>((status, body));
            }
        }));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    // The held owner blocks admission; requests beyond the four waiting slots must finish.
    while work.iter().filter(|task| task.is_finished()).count() < work.len() - 4 {
        if Instant::now() >= deadline {
            return Err("catalog requests did not reach capacity".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut saturated = 0;
    let mut waiting = Vec::new();
    for task in work {
        if task.is_finished() {
            let (status, body) = task.await?.map_err(std::io::Error::other)?;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            if body["provider_state"] == "saturated" {
                saturated += 1;
            }
        } else {
            waiting.push(task);
        }
    }
    assert!(saturated > 0);
    assert!(waiting.len() <= 4);
    assert!(store.catalog_status()?.known_retry_after_seconds.is_none());
    for task in waiting {
        task.abort();
        assert!(task.await.is_err());
    }
    drop(owner);
    allow_next(&database)?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fixture.count()?, 0);
    metadata.stop().await?;
    identity.stop().await?;
    Ok(())
}

#[tokio::test]
async fn catalog_disconnected_browser_drains_response_and_keeps_header_backoff() -> TestResult {
    let root = tempfile::tempdir()?;
    let database = root.path().join("state.db");
    let fixture = CatalogFixture::default();
    fixture.mode.store(16, Ordering::SeqCst);
    let (api, cookie, metadata, identity) = configured_api(&database, &fixture).await?;
    let router = api.router();
    let task = tokio::spawn(async move {
        api_request(
            &router,
            "GET",
            "/api/discovery/artists?query=interrupted",
            Some(&cookie),
            None,
            None,
        )
        .await
        .map_err(|error| error.to_string())
    });
    saved_headers(&database, 120).await?;
    task.abort();
    assert!(task.await.is_err());
    let mut store = Store::open_existing(&database)?;
    assert!(store.catalog_status()?.known_retry_after_seconds.is_none());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if matches!(
            store.catalog_status()?.state,
            domus::infrastructure::store::CatalogProviderState::Backoff
        ) {
            break;
        }
        if Instant::now() >= deadline {
            return Err("disconnected provider operation did not drain".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(store.catalog_status()?.retry_after_seconds >= 119);
    assert!(
        store
            .catalog_cache(&CatalogQuery::artists("interrupted", 0, 25)?)?
            .is_none()
    );
    assert_eq!(fixture.count()?, 1);
    metadata.stop().await?;
    identity.stop().await?;
    Ok(())
}

#[test]
fn catalog_header_owner_child() -> TestResult {
    let Some(database) = std::env::var_os("DOMUS_TEST_HEADER_DATABASE") else {
        return Ok(());
    };
    let authority = std::env::var("DOMUS_TEST_HEADER_CA")?;
    let address = std::env::var("DOMUS_TEST_HEADER_ADDRESS")?.parse()?;
    let mut store = Store::open_current_existing(std::path::Path::new(&database))?;
    let mut permit = store.claim_catalog_request()?;
    let client = MusicBrainzClient::from_builder(
        reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(authority.as_bytes())?])
            .resolve(MUSIC_HOST, address),
    )?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let _response = client
                .execute(&CatalogQuery::artists("child", 0, 25)?, &mut permit)
                .await;
            Ok::<(), domus::Error>(())
        })?;
    Ok(())
}

#[tokio::test]
async fn catalog_process_exit_after_headers_preserves_delay_and_manual_pause() -> TestResult {
    for (mode, delay) in [(16, 120), (17, u32::MAX)] {
        let root = tempfile::tempdir()?;
        let database = root.path().join("state.db");
        let mut store = Store::open(&database)?;
        let fixture = CatalogFixture::default();
        fixture.mode.store(mode, Ordering::SeqCst);
        let material = certificates(MUSIC_HOST)?;
        let metadata = FixtureServer::start(fixture.router(), &material, MUSIC_HOST)?;
        let mut child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "catalog_tests::scheduling_tests::catalog_header_owner_child",
                "--nocapture",
            ])
            .env("DOMUS_TEST_HEADER_DATABASE", &database)
            .env("DOMUS_TEST_HEADER_CA", &material.ca)
            .env("DOMUS_TEST_HEADER_ADDRESS", metadata.address.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;
        let observed = saved_headers(&database, delay).await;
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()?;
        observed?;
        assert!(
            matches!(store.claim_catalog_request(), Err(Error::Catalog { retry_after_seconds, .. }) if retry_after_seconds >= delay.saturating_sub(1))
        );
        if delay == u32::MAX {
            assert!(store.resume_catalog_requests(false).is_err());
            store.resume_catalog_requests(true)?;
        }
        assert_eq!(fixture.count()?, 1);
        metadata.stop().await?;
    }
    Ok(())
}
