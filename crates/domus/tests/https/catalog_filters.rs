use super::*;

#[tokio::test]
async fn catalog_album_filters_reuse_raw_cache_and_protect_artist_scope() -> TestResult {
    let provider = Provider::new()?;
    let identity_material = certificates(AUTHORITY)?;
    let identity_server = FixtureServer::start(provider.router(), &identity_material, AUTHORITY)?;
    let broker = EntraBroker::discover(
        TENANT,
        OBJECT,
        CLIENT,
        ClientSecret::new("generated-fixture-secret".into()),
        RedirectUrl::new(format!("{ORIGIN}/auth/callback"))?,
        trusted_http(&identity_material, &identity_server)?,
    )
    .await?;
    let fixture = CatalogFixture::default();
    fixture.mode.store(10, Ordering::SeqCst);
    let material = certificates(MUSIC_HOST)?;
    let server = FixtureServer::start(fixture.router(), &material, MUSIC_HOST)?;
    let directory = tempfile::tempdir()?;
    let api = Api::new_with_catalog(
        ORIGIN,
        Store::open(&directory.path().join("state.db"))?,
        broker,
        client(&material, &server)?,
    )?;
    let router = api.router();
    let response = api_request(
        &router,
        "GET",
        "/api/discovery/releases?query=Fixture",
        None,
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let cookie = browser_login(&router, &provider, None).await?;
    for path in [
        "/api/discovery/releases?query=%0A".to_owned(),
        "/api/discovery/releases?query=Fixture&artist_id=invalid".to_owned(),
        format!("/api/discovery/artists/{ARTIST}/releases?artist_id={TENANT}"),
        format!("/api/discovery/artists/{ARTIST}/releases?include_live=maybe"),
    ] {
        assert_eq!(
            api_request(&router, "GET", &path, Some(&cookie), None, None)
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(fixture.count()?, 0);
    let path = format!("/api/discovery/artists/{ARTIST}/releases");
    let response = api_request(&router, "GET", &path, Some(&cookie), None, None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let baseline = json_body(response).await?;
    assert_eq!(
        baseline["data"]["items"]
            .as_array()
            .ok_or("release list missing")?
            .len(),
        1
    );
    assert_eq!(baseline["data"]["scanned"], 5);
    assert_eq!(baseline["data"]["count"], 6);
    assert_eq!(baseline["data"]["next_offset"], 5);
    let response = api_request(
        &router,
        "GET",
        &format!("{path}?include_live=true"),
        Some(&cookie),
        None,
        None,
    )
    .await?;
    let live = json_body(response).await?;
    assert_eq!(live["cached"], true);
    assert_eq!(live["payload_sha256"], baseline["payload_sha256"]);
    assert_eq!(
        live["data"]["items"]
            .as_array()
            .ok_or("release list missing")?
            .len(),
        2
    );
    let response = api_request(
        &router,
        "GET",
        &format!("{path}?include_live=true&include_compilations=true"),
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(
        json_body(response).await?["data"]["items"]
            .as_array()
            .ok_or("release list missing")?
            .len(),
        3
    );
    assert_eq!(fixture.count()?, 1);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let response = api_request(
        &router,
        "GET",
        &format!("{path}?include_singles=true"),
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await?["data"]["items"]
            .as_array()
            .ok_or("release list missing")?
            .len(),
        2
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let response = api_request(&router, "GET", &format!("{path}?include_singles=true&include_live=true&include_compilations=true&include_unofficial=true"), Some(&cookie), None, None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await?["data"]["items"]
            .as_array()
            .ok_or("release list missing")?
            .len(),
        5
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let search = format!("/api/discovery/releases?query=Fixture&artist_id={ARTIST}");
    let response = api_request(&router, "GET", &search, Some(&cookie), None, None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await?["data"]["items"]
            .as_array()
            .ok_or("release list missing")?
            .len(),
        1
    );
    let response = api_request(
        &router,
        "GET",
        &format!("{path}?query=Fixture"),
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await?["cached"], true);
    assert_eq!(fixture.count()?, 4);
    server.stop().await?;
    identity_server.stop().await?;
    Ok(())
}
