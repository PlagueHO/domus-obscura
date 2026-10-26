use super::*;
use domus::Error;
use domus::infrastructure::catalog::{CatalogQuery, MusicBrainzClient};
use std::time::Instant;

const MUSIC_HOST: &str = "musicbrainz.org";
const ARTIST: &str = "10000000-0000-4000-8000-000000000001";
const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
type Recorded = Vec<(Instant, String, axum::http::HeaderMap)>;

#[tokio::test]
async fn shared_cache_musicbrainz_reuses_exact_release_and_freezes_plans_without_sqlite_cache()
-> TestResult {
    use domus::application::services::cache::{Cache, CacheKey};
    use domus::infrastructure::cache::{CacheLimits, FileCache};
    use domus::infrastructure::store::PlanRequest;
    let identity_material = certificates(AUTHORITY)?;
    let provider = Provider::new()?;
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
    let material = certificates(MUSIC_HOST)?;
    let server = FixtureServer::start(fixture.router(), &material, MUSIC_HOST)?;
    let state = private::tempdir()?;
    let root = private::tempdir()?;
    let media = tempfile::tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let location = domus::application::add_location(&store, media.path(), "main")?;
    domus::application::scan_location(&mut store, location)?;
    let settings = store.save_settings(None, &OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let cache = Arc::new(FileCache::open(
        &mut store,
        root.path(),
        CacheLimits::default(),
    )?);
    let api = Api::new_with_catalog(
        ORIGIN,
        store.with_cache(cache.clone()),
        broker,
        client(&material, &server)?,
    )?;
    let router = api.router();
    assert_eq!(
        api_request(
            &router,
            "GET",
            "/api/system/configuration",
            None,
            None,
            None
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = browser_login(&router, &provider, None).await?;
    assert_eq!(
        api_request(
            &router,
            "GET",
            "/api/system/configuration",
            Some(&cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let path = format!("/api/discovery/releases/{RELEASE}");
    let first = api_request(&router, "GET", &path, Some(&cookie), None, None).await?;
    assert_eq!(first.status(), StatusCode::OK);
    let result = json_body(first).await?;
    assert_eq!(result["data"]["summary"]["id"], RELEASE);
    assert_eq!(fixture.count()?, 1);
    let raw = rusqlite::Connection::open(&database)?;
    assert_eq!(
        raw.query_row("SELECT count(*) FROM catalog_cache", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    let mut reopened = Store::open_current_existing(&database)?;
    let reopened_cache = Arc::new(FileCache::open(
        &mut reopened,
        root.path(),
        CacheLimits::default(),
    )?);
    let mut reopened = reopened.with_cache(reopened_cache);
    let query = CatalogQuery::release(RELEASE)?;
    let exact = domus::application::catalog::cached_catalog(
        &mut reopened,
        &query,
        CatalogQuery::parse_release,
    )?
    .ok_or("exact release missing")?;
    let plan = reopened.prepare_catalog_plan(
        PlanRequest {
            job_id: job,
            expected_version: 1,
            settings_version: settings,
            format: None,
            edition: exact.data.selection(None)?,
            locations: vec![location],
            accepts_incomplete_coverage: false,
        },
        exact,
    )?;
    assert_eq!(plan.version, 2);
    assert_eq!(plan.plan.edition.tracks.len(), 2);
    raw.execute("UPDATE catalog_gate SET next_request_ms=9223372036854775807,delay_seconds=4294967295 WHERE id=1", [])?;
    let second = api_request(&router, "GET", &path, Some(&cookie), None, None).await?;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(json_body(second).await?["cached"], true);
    assert_eq!(fixture.count()?, 1);
    let key = CacheKey::new("musicbrainz", "responses-v1", query.source())?;
    assert!(cache.invalidate(&key)?);
    assert!(
        domus::application::catalog::cached_catalog(
            &mut reopened,
            &query,
            CatalogQuery::parse_release
        )?
        .is_none()
    );
    assert_eq!(
        reopened
            .plan(job, 2)?
            .plan
            .catalog
            .ok_or("frozen evidence missing")?
            .data
            .summary
            .id,
        RELEASE
    );
    let missed = api_request(&router, "GET", &path, Some(&cookie), None, None).await?;
    assert_eq!(missed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(fixture.count()?, 1);
    server.stop().await?;
    identity_server.stop().await?;
    Ok(())
}

#[path = "artwork.rs"]
mod artwork_tests;
#[path = "catalog_batches.rs"]
mod batch_tests;
#[path = "catalog_filters.rs"]
mod filter_tests;

#[derive(Clone, Default)]
struct CatalogFixture {
    mode: Arc<AtomicUsize>,
    requests: Arc<Mutex<Recorded>>,
    redirects: Arc<AtomicUsize>,
}

impl CatalogFixture {
    fn router(&self) -> Router {
        Router::new()
            .fallback(catalog_response)
            .with_state(self.clone())
    }

    fn count(&self) -> TestResult<usize> {
        Ok(self
            .requests
            .lock()
            .map_err(|_| "fixture lock failed")?
            .len())
    }
}

fn artist_page() -> Value {
    json!({"offset":0,"count":1,"artists":[{"id":ARTIST,"name":"The Fixture","disambiguation":"generated test artist"}]})
}

fn release_data() -> Value {
    json!({
        "id":RELEASE,"title":"Exact: Fixture Edition","artist-credit":[{"name":"The Fixture","artist":{"id":ARTIST,"name":"Canonical Artist"}}],
        "date":"2024","country":null,"status":"Official","disambiguation":"generated two-disc edition",
        "release-group":{"id":"30000000-0000-4000-8000-000000000001","primary-type":"Album","secondary-types":[]},
        "media":[
            {"position":1,"format":"CD","track-count":1,"tracks":[
                {"id":"40000000-0000-4000-8000-000000000001","position":1,"number":"1","title":"First / Track","length":120000,
                 "artist-credit":[{"name":"Track Artist","artist":{"id":ARTIST,"name":"Canonical Artist"}}],
                 "recording":{"id":"50000000-0000-4000-8000-000000000001","video":false}}
            ]},
            {"position":2,"format":"CD","track-count":1,"tracks":[
                {"id":"40000000-0000-4000-8000-000000000002","position":1,"number":"1","title":"Second Track","length":null,
                 "artist-credit":[{"name":"Guest","artist":{"id":TENANT,"name":"Guest"}}],
                 "recording":{"id":"50000000-0000-4000-8000-000000000002","video":false}}
            ]}
        ]
    })
}

async fn catalog_response(
    State(fixture): State<CatalogFixture>,
    request: Request<Body>,
) -> axum::response::Response {
    if request.uri().path() == "/destination" {
        fixture.redirects.fetch_add(1, Ordering::SeqCst);
        return StatusCode::OK.into_response();
    }
    match fixture.requests.lock() {
        Ok(mut requests) => requests.push((
            Instant::now(),
            request.uri().to_string(),
            request.headers().clone(),
        )),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    match fixture.mode.load(Ordering::SeqCst) {
        1 => return Redirect::temporary("https://musicbrainz.org/destination").into_response(),
        2 => {
            return (
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (header::RETRY_AFTER, "120"),
                ],
                vec![b'x'; 2 * 1024 * 1024 + 1],
            )
                .into_response();
        }
        3 => {
            let chunks = futures_util::stream::iter([
                Ok::<_, Infallible>(Bytes::from(vec![b'x'; 2 * 1024 * 1024])),
                Ok(Bytes::from_static(b"x")),
            ]);
            return (
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (header::RETRY_AFTER, "120"),
                ],
                Body::from_stream(chunks),
            )
                .into_response();
        }
        4 => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "120")],
                "private upstream diagnostic must not reach the browser",
            )
                .into_response();
        }
        5 => {
            return (
                [
                    (header::CONTENT_TYPE, "text/html"),
                    (header::RETRY_AFTER, "120"),
                ],
                Json(artist_page()),
            )
                .into_response();
        }
        6 => {
            return (
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"private upstream diagnostic":"malformed"#,
            )
                .into_response();
        }
        7 => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "999999999999999999999")],
                "private upstream diagnostic",
            )
                .into_response();
        }
        9 => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(
                    header::RETRY_AFTER,
                    httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120)),
                )],
                "private upstream diagnostic",
            )
                .into_response();
        }
        11..=14 => {
            let mut response = Json(artist_page()).into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("120"),
            );
            match fixture.mode.load(Ordering::SeqCst) {
                11 => {
                    headers.append(
                        header::CONTENT_ENCODING,
                        axum::http::HeaderValue::from_static("identity"),
                    );
                    headers.append(
                        header::CONTENT_ENCODING,
                        axum::http::HeaderValue::from_static("gzip"),
                    );
                }
                12 => {
                    for index in 0..65 {
                        let Ok(name) = axum::http::HeaderName::from_bytes(
                            format!("x-fixture-{index}").as_bytes(),
                        ) else {
                            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                        };
                        headers.insert(name, axum::http::HeaderValue::from_static("x"));
                    }
                }
                13 => {
                    let Ok(value) = axum::http::HeaderValue::from_bytes(&vec![b'x'; 17000]) else {
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    };
                    headers.insert("x-fixture-large", value);
                }
                _ => {
                    headers.append(
                        header::RETRY_AFTER,
                        axum::http::HeaderValue::from_static("600"),
                    );
                }
            }
            return response;
        }
        15 => {
            let chunks = futures_util::stream::unfold(0, |state| async move {
                match state {
                    0 => Some((Ok(Bytes::from_static(b"{")), 1)),
                    1 => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Some((Err(std::io::Error::other("fixture stream failure")), 2))
                    }
                    _ => None,
                }
            });
            return (
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (header::RETRY_AFTER, "120"),
                ],
                Body::from_stream(chunks),
            )
                .into_response();
        }
        _ => {}
    }
    let response = match request.uri().path() {
        "/ws/2/artist" => artist_page(),
        "/ws/2/release" => {
            let mixed = fixture.mode.load(Ordering::SeqCst) == 10;
            let mut rows = vec![release_data()];
            if mixed {
                for (index, primary, secondary, status) in [
                    (2, "Single", json!([]), "Official"),
                    (3, "Album", json!(["Live"]), "Official"),
                    (4, "Album", json!(["Compilation"]), "Official"),
                    (5, "Album", json!([]), "Bootleg"),
                ] {
                    let mut release = release_data();
                    release["id"] = json!(format!("20000000-0000-4000-8000-{index:012}"));
                    release["release-group"]["id"] =
                        json!(format!("30000000-0000-4000-8000-{index:012}"));
                    release["release-group"]["primary-type"] = json!(primary);
                    release["release-group"]["secondary-types"] = secondary;
                    release["status"] = json!(status);
                    rows.push(release);
                }
            }
            let count = if mixed { 6 } else { 1 };
            let search = request
                .uri()
                .query()
                .is_some_and(|query| query.split('&').any(|pair| pair.starts_with("query=")));
            if search {
                json!({"offset":0,"count":count,"releases":rows})
            } else {
                json!({"release-offset":0,"release-count":count,"releases":rows})
            }
        }
        path if path == format!("/ws/2/release/{RELEASE}") => release_data(),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut response = Json(response).into_response();
    if fixture.mode.load(Ordering::SeqCst) == 8 {
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("120"),
        );
    }
    response
}

fn client(material: &Certificates, server: &FixtureServer) -> TestResult<MusicBrainzClient> {
    Ok(MusicBrainzClient::from_builder(
        reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
            .resolve(MUSIC_HOST, server.address)
            .timeout(Duration::from_millis(1))
            .redirect(reqwest::redirect::Policy::limited(5)),
    )?)
}

fn allow_next(database: &std::path::Path) -> TestResult {
    let connection = rusqlite::Connection::open(database)?;
    connection.execute(
        "UPDATE catalog_gate SET lease_until_ms=0, next_request_ms=0, delay_seconds=0",
        [],
    )?;
    Ok(())
}

#[tokio::test]
async fn catalog_transport_enforces_tls_bounds_single_use_and_real_cadence() -> TestResult {
    let fixture = CatalogFixture::default();
    let material = certificates(MUSIC_HOST)?;
    let server = FixtureServer::start(fixture.router(), &material, MUSIC_HOST)?;
    let client = client(&material, &server)?;
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    let mut store = Store::open(&database)?;
    let query = CatalogQuery::artists("Fixture", 0, 25)?;
    let mut permit = store.claim_catalog_request()?;
    let response = client.execute(&query, &mut permit).await?;
    assert_eq!(response.status, 200);
    assert_eq!(query.parse_artists(&response.body)?.items[0].id, ARTIST);
    assert!(client.execute(&query, &mut permit).await.is_err());
    assert_eq!(fixture.count()?, 1);
    store.complete_catalog_request(permit, 0, Some((&query, &response.body)))?;
    assert!(matches!(
        store.claim_catalog_request(),
        Err(Error::Catalog { .. })
    ));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let mut permit = store.claim_catalog_request()?;
    let response = client.execute(&query, &mut permit).await?;
    store.complete_catalog_request(permit, 0, Some((&query, &response.body)))?;
    {
        let requests = fixture.requests.lock().map_err(|_| "fixture lock failed")?;
        assert!(requests[1].0.duration_since(requests[0].0) >= Duration::from_secs(1));
        for (_, _, headers) in requests.iter() {
            assert_eq!(
                headers[header::USER_AGENT],
                concat!(
                    "DomusObscura/",
                    env!("CARGO_PKG_VERSION"),
                    " (https://github.com/PlagueHO)"
                )
            );
            assert!(headers.get(header::AUTHORIZATION).is_none());
            assert!(headers.get(header::COOKIE).is_none());
        }
    }
    for mode in [1, 2, 3, 5, 11, 12, 13, 14, 15] {
        allow_next(&database)?;
        fixture.mode.store(mode, Ordering::SeqCst);
        let mut permit = store.claim_catalog_request()?;
        let result = client.execute(&query, &mut permit).await;
        let cooldown = if mode == 14 {
            u32::MAX
        } else if mode == 1 {
            5
        } else {
            120
        };
        if mode == 1 {
            assert_eq!(result?.status, 307);
        } else {
            assert!(
                matches!(result, Err(Error::Catalog { retry_after_seconds, .. }) if retry_after_seconds == cooldown)
            );
        }
        store.complete_catalog_request(permit, cooldown, None)?;
    }
    assert_eq!(fixture.redirects.load(Ordering::SeqCst), 0);
    allow_next(&database)?;
    let untrusted = MusicBrainzClient::from_builder(
        reqwest::Client::builder()
            .resolve(MUSIC_HOST, server.address)
            .tls_danger_accept_invalid_certs(true)
            .tls_danger_accept_invalid_hostnames(true),
    )?;
    let mut permit = store.claim_catalog_request()?;
    assert!(untrusted.execute(&query, &mut permit).await.is_err());
    store.complete_catalog_request(permit, 5, None)?;

    let wrong_material = certificates("wrong.fixture.invalid")?;
    let wrong_server =
        FixtureServer::start(fixture.router(), &wrong_material, "wrong.fixture.invalid")?;
    let wrong_host = MusicBrainzClient::from_builder(
        reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(
                wrong_material.ca.as_bytes(),
            )?])
            .resolve(MUSIC_HOST, wrong_server.address)
            .tls_danger_accept_invalid_hostnames(true),
    )?;
    allow_next(&database)?;
    let mut permit = store.claim_catalog_request()?;
    assert!(wrong_host.execute(&query, &mut permit).await.is_err());
    store.complete_catalog_request(permit, 5, None)?;
    wrong_server.stop().await?;
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn catalog_authenticated_discovery_freezes_evidence_and_preserves_backoff() -> TestResult {
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
    let catalog_fixture = CatalogFixture::default();
    let material = certificates(MUSIC_HOST)?;
    let metadata_server = FixtureServer::start(catalog_fixture.router(), &material, MUSIC_HOST)?;
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    let media = directory.path().join("media");
    std::fs::create_dir(&media)?;
    let mut store = Store::open(&database)?;
    let location = domus::application::add_location(&store, &media, "main")?;
    domus::application::scan_location(&mut store, location)?;
    let settings = store.save_settings(None, &OutputSettings::default())?;
    let job = store.enqueue(1)?;
    let artist_query = CatalogQuery::artist(ARTIST)?;
    let artist_body = serde_json::to_vec(&json!({
        "id":ARTIST,"name":"Canonical Artist","type":"Group","country":"NZ",
        "disambiguation":"generated exact identity"
    }))?;
    {
        use sha2::{Digest, Sha256};
        let raw = rusqlite::Connection::open(&database)?;
        raw.execute(
            "INSERT INTO catalog_cache VALUES (?1,?2,?3,unixepoch(),unixepoch()+3600,unixepoch()*1000)",
            rusqlite::params![artist_query.source(), artist_body, &Sha256::digest(&artist_body)[..]],
        )?;
        let group = "30000000-0000-4000-8000-000000000001";
        let query = CatalogQuery::group_releases(group, 0, 25, Default::default())?;
        let body = serde_json::to_vec(&json!({
            "release-offset":0,"release-count":1,"releases":[release_data()]
        }))?;
        raw.execute(
            "INSERT INTO catalog_cache VALUES (?1,?2,?3,unixepoch(),unixepoch()+3600,unixepoch()*1000)",
            rusqlite::params![query.source(),body,&Sha256::digest(&body)[..]],
        )?;
    }
    let api = Api::new_with_catalog(ORIGIN, store, broker, client(&material, &metadata_server)?)?;
    let router = api.router();
    for path in [
        "/api/discovery/artists?query=Fixture",
        &format!("/api/discovery/artists/{ARTIST}"),
        "/api/discovery/release-groups/30000000-0000-4000-8000-000000000001/releases",
        "/api/discovery/releases/invalid",
    ] {
        assert_eq!(
            api_request(&router, "GET", path, None, None, None)
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(catalog_fixture.count()?, 0);
    let cookie = browser_login(&router, &provider, None).await?;
    let editions_path =
        "/api/discovery/release-groups/30000000-0000-4000-8000-000000000001/releases";
    let editions =
        json_body(api_request(&router, "GET", editions_path, Some(&cookie), None, None).await?)
            .await?;
    assert_eq!(editions["cached"], true);
    assert_eq!(editions["data"]["items"][0]["id"], RELEASE);
    assert_eq!(catalog_fixture.count()?, 0);
    assert_eq!(
        api_request(
            &router,
            "GET",
            &format!("{editions_path}?query=not-a-group-filter"),
            Some(&cookie),
            None,
            None,
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    let identity = json_body(
        api_request(
            &router,
            "GET",
            &format!("/api/discovery/artists/{ARTIST}"),
            Some(&cookie),
            None,
            None,
        )
        .await?,
    )
    .await?;
    assert_eq!(identity["cached"], true);
    assert_eq!(identity["data"]["id"], ARTIST);
    assert_eq!(identity["data"]["name"], "Canonical Artist");
    assert_eq!(catalog_fixture.count()?, 0);
    let application_tls = certificates("domus.fixture.invalid")?;
    let application_server =
        FixtureServer::start(router.clone(), &application_tls, "domus.fixture.invalid")?;
    let browser = reqwest::Client::builder()
        .tls_backend_rustls()
        .no_proxy()
        .tls_certs_only([reqwest::Certificate::from_pem(
            application_tls.ca.as_bytes(),
        )?])
        .resolve("domus.fixture.invalid", application_server.address)
        .build()?;
    let response = browser
        .get(format!("{ORIGIN}/api/discovery/artists?query=Fixture"))
        .header(header::COOKIE, &cookie)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let first: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert_eq!(first["cached"], false);
    assert_eq!(first["data"]["items"][0]["id"], ARTIST);
    let response = api_request(
        &router,
        "GET",
        "/api/discovery/artists?query=Fixture",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(json_body(response).await?["cached"], true);
    assert_eq!(catalog_fixture.count()?, 1);
    let invalid = api_request(
        &router,
        "GET",
        "/api/discovery/artists?query=Fixture&limit=26",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(catalog_fixture.count()?, 1);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let response = api_request(
        &router,
        "GET",
        &format!("/api/discovery/artists/{ARTIST}/releases"),
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await?["data"]["items"][0]["id"],
        RELEASE
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let response = api_request(
        &router,
        "GET",
        &format!("/api/discovery/releases/{RELEASE}"),
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let details = json_body(response).await?;
    assert_eq!(
        details["data"]["media"][1]["tracks"][0]["duration_ms"],
        Value::Null
    );
    let request = json!({
        "job_id":job, "expected_version":1, "settings_version":settings, "format":"flac",
        "locations":[location], "accepts_incomplete_coverage":false, "bucket":null
    });
    let path = format!("/api/discovery/releases/{RELEASE}/plans");
    assert_eq!(
        api_request(
            &router,
            "POST",
            &path,
            Some(&cookie),
            None,
            Some(request.clone())
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    let response = api_request(
        &router,
        "POST",
        &path,
        Some(&cookie),
        Some(ORIGIN),
        Some(request),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let plan = json_body(response).await?;
    assert_eq!(plan["plan"]["format"], "flac");
    assert_eq!(
        plan["plan"]["catalog"]["payload_sha256"],
        details["payload_sha256"]
    );
    assert_eq!(plan["plan"]["catalog"]["data"]["summary"]["date"], "2024");
    assert_eq!(
        plan["plan"]["edition"]["tracks"][0]["title"],
        "First / Track"
    );
    assert!(
        plan["plan"]["outputs"][1]["filename"]
            .as_str()
            .ok_or("filename missing")?
            .contains("02")
    );
    assert_eq!(catalog_fixture.count()?, 3);
    {
        let requests = catalog_fixture
            .requests
            .lock()
            .map_err(|_| "fixture lock failed")?;
        for (_, uri, headers) in requests.iter() {
            assert_eq!(headers[header::ACCEPT], "application/json");
            let url = reqwest::Url::parse(&format!("https://{MUSIC_HOST}{uri}"))?;
            let inc = url
                .query_pairs()
                .find(|(name, _)| name == "inc")
                .map(|(_, value)| value.into_owned());
            if url.path().ends_with(RELEASE) {
                assert_eq!(
                    inc.as_deref(),
                    Some("recordings artist-credits release-groups")
                );
            } else if url.path() == "/ws/2/release" {
                assert_eq!(inc.as_deref(), Some("artist-credits release-groups media"));
            }
        }
    }
    {
        let mut verification = Store::open_existing(&database)?;
        let original = verification
            .plan(job, 2)?
            .plan
            .catalog
            .ok_or("catalog evidence missing")?;
        for change in 0..4 {
            let mut forged = original.clone();
            match change {
                0 => forged.payload_sha256[0] ^= 1,
                1 => forged.source = "https://untrusted.fixture.invalid/".into(),
                2 => forged.data.summary.title = "Forged title".into(),
                _ => forged.fetched_at -= 100000,
            }
            let request = domus::infrastructure::store::PlanRequest {
                job_id: job,
                expected_version: 2,
                settings_version: settings,
                format: None,
                edition: forged.data.selection(None)?,
                locations: vec![location],
                accepts_incomplete_coverage: false,
            };
            assert!(matches!(
                verification.prepare_catalog_plan(request, forged),
                Err(Error::Conflict(_))
            ));
            assert!(verification.plan(job, 3).is_err());
        }
    }
    assert_eq!(
        api_request(
            &router,
            "POST",
            &format!("/api/jobs/{job}/plans/2/approve"),
            Some(&cookie),
            Some(ORIGIN),
            None
        )
        .await?
        .status(),
        StatusCode::NO_CONTENT
    );

    artwork_tests::check_artwork(&router, &browser, &database, job, &cookie).await?;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    catalog_fixture.mode.store(4, Ordering::SeqCst);
    let response = api_request(
        &router,
        "GET",
        "/api/discovery/artists?query=Outage",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "120");
    let error = json_body(response).await?.to_string();
    assert!(!error.contains("private upstream"));
    assert_eq!(catalog_fixture.count()?, 4);
    let response = api_request(
        &router,
        "GET",
        "/api/discovery/artists?query=Another",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(catalog_fixture.count()?, 4);
    let response = api_request(
        &router,
        "GET",
        &format!("/api/discovery/releases/{RELEASE}"),
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await?["cached"], true);
    let mut reopened = Store::open_existing(&database)?;
    assert!(matches!(
        reopened.claim_catalog_request(),
        Err(Error::Catalog { .. })
    ));
    let saved = reopened.plan(job, 2)?;
    assert!(saved.approved_at.is_some());
    assert_eq!(
        saved
            .plan
            .catalog
            .ok_or("catalog snapshot missing")?
            .data
            .summary
            .id,
        RELEASE
    );
    let response = api_request(
        &router,
        "POST",
        "/api/discovery/resume",
        Some(&cookie),
        Some(ORIGIN),
        Some(json!({"acknowledge_pause":true})),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    for (mode, query) in [
        (6, "InvalidJSON"),
        (9, "DateBackoff"),
        (7, "UnsupportedInterval"),
    ] {
        allow_next(&database)?;
        catalog_fixture.mode.store(mode, Ordering::SeqCst);
        let response = api_request(
            &router,
            "GET",
            &format!("/api/discovery/artists?query={query}"),
            Some(&cookie),
            None,
            None,
        )
        .await?;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        if mode == 9 {
            assert!(
                response.headers()[header::RETRY_AFTER]
                    .to_str()?
                    .parse::<u32>()?
                    >= 119
            );
        }
        assert!(
            !json_body(response)
                .await?
                .to_string()
                .contains("private upstream")
        );
        assert!(
            reopened
                .catalog_cache(&CatalogQuery::artists(query, 0, 25)?)?
                .is_none()
        );
    }
    let response = api_request(
        &router,
        "GET",
        "/api/discovery/status",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(json_body(response).await?["state"], "paused");
    assert!(matches!(
        reopened.claim_catalog_request(),
        Err(Error::Catalog {
            retry_after_seconds: u32::MAX,
            ..
        })
    ));
    let response = api_request(
        &router,
        "POST",
        "/api/discovery/resume",
        Some(&cookie),
        None,
        Some(json!({"acknowledge_pause":true})),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = api_request(
        &router,
        "POST",
        "/api/discovery/resume",
        Some(&cookie),
        Some(ORIGIN),
        Some(json!({"acknowledge_pause":false})),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = api_request(
        &router,
        "POST",
        "/api/discovery/resume",
        Some(&cookie),
        Some(ORIGIN),
        Some(json!({"acknowledge_pause":true})),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = api_request(
        &router,
        "GET",
        "/api/discovery/status",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    let status = json_body(response).await?;
    assert_eq!(status["state"], "backoff");
    assert_eq!(status["failures"], 0);
    assert_eq!(catalog_fixture.count()?, 7);
    allow_next(&database)?;
    catalog_fixture.mode.store(8, Ordering::SeqCst);
    let response = api_request(
        &router,
        "GET",
        "/api/discovery/artists?query=SuccessDelay",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await?["cached"], false);
    assert!(matches!(
        reopened.claim_catalog_request(),
        Err(Error::Catalog {
            retry_after_seconds: 120..=121,
            ..
        })
    ));
    let response = api_request(
        &router,
        "GET",
        "/api/discovery/artists?query=SuccessDelay",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(json_body(response).await?["cached"], true);
    assert_eq!(catalog_fixture.count()?, 8);
    assert_eq!(std::fs::read_dir(&media)?.count(), 0);
    application_server.stop().await?;
    metadata_server.stop().await?;
    identity_server.stop().await?;
    Ok(())
}
