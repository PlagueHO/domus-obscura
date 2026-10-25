use super::*;
use axum::http::HeaderValue;
use domus::Error;
use domus::infrastructure::caa::{CaaHttpClient, CaaPayload, CaaRequest, MAX_CAA_LISTING};
use sha2::{Digest, Sha256};

const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
const IMAGE: &str = "9007199254740995";
const CAA: &str = "coverartarchive.org";
const ARCHIVE: &str = "archive.org";
const SHARD: &str = "dn721806.ca.archive.org";

fn listing() -> Value {
    json!({
        "release": format!("https://musicbrainz.org/release/{RELEASE}"),
        "images": [{
            "id": IMAGE, "edit": 123, "types": ["Front"],
            "image": format!("http://{CAA}/release/{RELEASE}/{IMAGE}.png"),
            "front": true, "back": false, "approved": false, "comment": "Fixture"
        }]
    })
}

#[tokio::test]
async fn caa_https_fetches_exact_listing_and_original_image_through_budgeted_redirects()
-> TestResult {
    let caa_material = certificates(CAA)?;
    let archive_material = certificates(ARCHIVE)?;
    let shard_material = certificates(SHARD)?;
    let list_path = format!("/release/{RELEASE}/");
    let image_path = format!("/release/{RELEASE}/{IMAGE}");
    let archive_list = format!("/download/mbid-{RELEASE}/index.json");
    let archive_image = format!("/download/mbid-{RELEASE}/mbid-{RELEASE}-{IMAGE}.png");
    let shard_list = format!("/9/items/mbid-{RELEASE}/index.json");
    let shard_image = format!("/9/items/mbid-{RELEASE}/mbid-{RELEASE}-{IMAGE}.png");
    let hits = Arc::new(AtomicUsize::new(0));
    let fixture_hits = hits.clone();
    let caa_router = Router::new()
        .route(
            &list_path,
            get({
                let target = format!("https://{ARCHIVE}{archive_list}");
                move || {
                    let target = target.clone();
                    let hits = fixture_hits.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, target)])
                    }
                }
            }),
        )
        .route(
            &image_path,
            get({
                let target = format!("https://{ARCHIVE}{archive_image}");
                move || {
                    let target = target.clone();
                    async move { (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, target)]) }
                }
            }),
        );
    let archive_router = Router::new()
        .route(
            &archive_list,
            get({
                let target = format!("https://{SHARD}{shard_list}");
                move || {
                    let target = target.clone();
                    async move { (StatusCode::FOUND, [(header::LOCATION, target)]) }
                }
            }),
        )
        .route(
            &archive_image,
            get({
                let target = format!("https://{SHARD}{shard_image}");
                move || {
                    let target = target.clone();
                    async move { (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, target)]) }
                }
            }),
        );
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 1).write_to(&mut png, image::ImageFormat::Png)?;
    let png = png.into_inner();
    let shard_router = Router::new()
        .route(&shard_list, get(|| async { Json(listing()) }))
        .route(
            &shard_image,
            get({
                let png = png.clone();
                move || {
                    let png = png.clone();
                    async move { ([(header::CONTENT_TYPE, "image/png")], png) }
                }
            }),
        );
    let caa = FixtureServer::start(caa_router, &caa_material, CAA)?;
    let archive = FixtureServer::start(archive_router, &archive_material, ARCHIVE)?;
    let shard = FixtureServer::start(shard_router, &shard_material, SHARD)?;
    let client = CaaHttpClient::from_builder(
        reqwest::Client::builder()
            .tls_certs_only([
                reqwest::Certificate::from_pem(caa_material.ca.as_bytes())?,
                reqwest::Certificate::from_pem(archive_material.ca.as_bytes())?,
                reqwest::Certificate::from_pem(shard_material.ca.as_bytes())?,
            ])
            .resolve(CAA, caa.address)
            .resolve(ARCHIVE, archive.address)
            .resolve(SHARD, shard.address)
            .redirect(reqwest::redirect::Policy::limited(20)),
    )?;
    let state = tempfile::tempdir()?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let fetched = client
        .fetch(&mut store, CaaRequest::listing(RELEASE)?)
        .await?;
    assert_eq!(fetched.sources.len(), 3);
    assert_eq!(fetched.sources[0], format!("https://{CAA}{list_path}"));
    assert_eq!(fetched.sources[2], format!("https://{SHARD}{shard_list}"));
    let CaaPayload::Listing {
        listing: parsed,
        raw,
    } = fetched.payload
    else {
        return Err("expected exact-release listing".into());
    };
    assert_eq!(
        parsed.payload_sha256(),
        format!("{:x}", Sha256::digest(&raw))
    );
    assert!(!parsed.image(IMAGE)?.provider_approved);
    assert!(matches!(
        store.claim_catalog_request(),
        Err(Error::Catalog { .. })
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    let image = client
        .fetch(&mut store, CaaRequest::image(&parsed, IMAGE)?)
        .await?;
    assert_eq!(image.sources.len(), 3);
    assert_eq!(image.sources[2], format!("https://{SHARD}{shard_image}"));
    let CaaPayload::Image { bytes, sha256 } = image.payload else {
        return Err("expected original image".into());
    };
    assert_eq!(bytes, png);
    assert_eq!(sha256, format!("{:x}", Sha256::digest(&png)));
    assert_eq!(
        domus::infrastructure::artwork::normalize_cover(&bytes, None)?.dimensions(),
        (2, 1)
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    {
        use domus::application::services::{
            ArtworkContent, ArtworkProvider, ArtworkRequest, EntityRef,
        };
        use domus::infrastructure::services::CaaArtworkService;
        let service_state = tempfile::tempdir()?;
        let service_database = service_state.path().join("state.db");
        drop(Store::open(&service_database)?);
        let service = CaaArtworkService::new(&service_database, client.clone());
        let request = ArtworkRequest {
            provider: "cover-art-archive".into(),
            release: EntityRef {
                provider: "musicbrainz".into(),
                id: RELEASE.into(),
            },
            image_id: Some(IMAGE.into()),
        };
        let result = service.fetch(request.clone()).await?;
        assert_eq!(result.release, request.release);
        assert_eq!(result.sources.len(), 6);
        let ArtworkContent::Image { bytes, sha256 } = result.content else {
            return Err("service must fetch the listing-bound original image".into());
        };
        assert_eq!(bytes, png);
        assert_eq!(sha256, format!("{:x}", Sha256::digest(&png)));
        let saved = rusqlite::Connection::open(&service_database)?;
        let before: i64 = saved.query_row(
            "SELECT generation FROM catalog_gate WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            saved.query_row("SELECT count(*) FROM caa_cache", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        drop(service);
        let service = CaaArtworkService::new(&service_database, client.clone());
        assert!(matches!(
            service.fetch(request.clone()).await?.content,
            ArtworkContent::Image { .. }
        ));
        assert_eq!(
            saved.query_row(
                "SELECT generation FROM catalog_gate WHERE id=1",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            before
        );
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        saved.execute("UPDATE caa_cache SET sha256=zeroblob(32)", [])?;
        assert!(service.fetch(request.clone()).await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        let mut invalid = request;
        invalid.release.provider = "spotify".into();
        assert!(service.fetch(invalid).await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }
    let query = domus::infrastructure::catalog::CatalogQuery::artists("not requested", 0, 1)?;
    assert!(store.catalog_cache(&query)?.is_none());
    let unsafe_client = CaaHttpClient::from_builder(
        reqwest::Client::builder()
            .resolve(CAA, caa.address)
            .tls_danger_accept_invalid_certs(true)
            .tls_danger_accept_invalid_hostnames(true),
    )?;
    let state = tempfile::tempdir()?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    assert!(
        unsafe_client
            .fetch(&mut store, CaaRequest::listing(RELEASE)?)
            .await
            .is_err()
    );
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    caa.stop().await?;
    archive.stop().await?;
    shard.stop().await?;
    Ok(())
}

#[tokio::test]
async fn caa_https_rejects_unsafe_redirects_bad_metadata_and_preserves_backoff() -> TestResult {
    let material = certificates(CAA)?;
    let mode = Arc::new(AtomicUsize::new(0));
    let fixture_mode = mode.clone();
    let archive_location =
        format!("https://archive.org/download/mbid-{RELEASE}/index.json").parse::<HeaderValue>()?;
    let oversized_header = "x".repeat(16384).parse::<HeaderValue>()?;
    let router = Router::new().route(
        &format!("/release/{RELEASE}/"),
        get(move || {
            let mode = fixture_mode.clone();
            let archive_location = archive_location.clone();
            let oversized_header = oversized_header.clone();
            async move {
                match mode.load(Ordering::SeqCst) {
                    0 => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        [(header::RETRY_AFTER, "120")],
                        "private error",
                    )
                        .into_response(),
                    1 => (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(header::LOCATION, "https://untrusted.example/never-request")],
                    )
                        .into_response(),
                    2 => (
                        [(header::CONTENT_TYPE, "application/json")],
                        vec![b' '; MAX_CAA_LISTING + 1],
                    )
                        .into_response(),
                    3 => (
                        [
                            (header::CONTENT_TYPE, "application/json"),
                            (header::RETRY_AFTER, "180"),
                        ],
                        "{",
                    )
                        .into_response(),
                    4 => (
                        [
                            (header::CONTENT_TYPE, "application/json"),
                            (header::CONTENT_ENCODING, "gzip"),
                        ],
                        Json(listing()),
                    )
                        .into_response(),
                    5 => (
                        [(header::CONTENT_TYPE, "text/html")],
                        "<html>not JSON</html>",
                    )
                        .into_response(),
                    6 => (StatusCode::NOT_FOUND, "not an artwork license").into_response(),
                    7 => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        [(header::RETRY_AFTER, "unsupported")],
                    )
                        .into_response(),
                    8 => (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(header::LOCATION, "http://archive.org/never-request")],
                    )
                        .into_response(),
                    9 => {
                        let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
                        response
                            .headers_mut()
                            .append(header::LOCATION, archive_location);
                        response.headers_mut().append(
                            header::LOCATION,
                            HeaderValue::from_static("https://untrusted.example/never-request"),
                        );
                        response
                    }
                    10 => {
                        let mut response = Json(listing()).into_response();
                        response
                            .headers_mut()
                            .append(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
                        response
                    }
                    11 => (
                        [
                            (header::CONTENT_TYPE, "application/json"),
                            (header::RETRY_AFTER, "240"),
                        ],
                        Body::from_stream(futures_util::stream::iter([
                            Ok::<_, Infallible>(Bytes::from(vec![b' '; MAX_CAA_LISTING])),
                            Ok(Bytes::from_static(b"x")),
                        ])),
                    )
                        .into_response(),
                    12 => {
                        let mut response = Json(listing()).into_response();
                        response
                            .headers_mut()
                            .insert(header::RETRY_AFTER, HeaderValue::from_static("600"));
                        response
                            .headers_mut()
                            .insert("x-too-large", oversized_header);
                        response
                    }
                    13 => (
                        StatusCode::TEMPORARY_REDIRECT,
                        [
                            (
                                header::LOCATION,
                                format!("https://archive.org/download/mbid-{RELEASE}/index.json"),
                            ),
                            (header::RETRY_AFTER, "900".into()),
                        ],
                    )
                        .into_response(),
                    _ => {
                        let mut response = Json(listing()).into_response();
                        response
                            .headers_mut()
                            .append(header::RETRY_AFTER, HeaderValue::from_static("120"));
                        response
                            .headers_mut()
                            .append(header::RETRY_AFTER, HeaderValue::from_static("600"));
                        response
                    }
                }
            }
        }),
    );
    let server = FixtureServer::start(router, &material, CAA)?;
    let client = CaaHttpClient::from_builder(
        reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
            .resolve(CAA, server.address)
            .redirect(reqwest::redirect::Policy::limited(20)),
    )?;
    for selected in 0..=14 {
        mode.store(selected, Ordering::SeqCst);
        let state = tempfile::tempdir()?;
        let mut store = Store::open(&state.path().join("state.db"))?;
        let fetched = client
            .fetch(&mut store, CaaRequest::listing(RELEASE)?)
            .await;
        if selected == 6 {
            assert!(matches!(fetched?.payload, CaaPayload::Missing));
        } else {
            assert!(fetched.is_err(), "mode {selected}");
            if matches!(selected, 0 | 3 | 11 | 12 | 13) {
                let minimum = match selected {
                    0 => 120,
                    3 => 180,
                    11 => 240,
                    12 => 600,
                    _ => 900,
                };
                let status = store.catalog_status()?;
                assert!(status.retry_after_seconds >= minimum - 1);
            }
            if selected == 7 || selected == 14 {
                assert_eq!(store.catalog_status()?.retry_after_seconds, u32::MAX);
            }
        }
        assert!(matches!(
            store.claim_catalog_request(),
            Err(Error::Catalog { .. })
        ));
    }
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn caa_image_transport_enforces_exact_eight_mib_and_chunked_body_limits() -> TestResult {
    let material = certificates(CAA)?;
    let mode = Arc::new(AtomicUsize::new(0));
    let fixture_mode = mode.clone();
    let router = Router::new().route(
        &format!("/release/{RELEASE}/{IMAGE}"),
        get(move || {
            let mode = fixture_mode.clone();
            async move {
                let limit = 8 * 1024 * 1024;
                match mode.load(Ordering::SeqCst) {
                    0 => ([(header::CONTENT_TYPE, "image/png")], vec![b'x'; limit]).into_response(),
                    1 => ([(header::CONTENT_TYPE, "image/png")], vec![b'x'; limit + 1])
                        .into_response(),
                    2 => (
                        [(header::CONTENT_TYPE, "image/png")],
                        Body::from_stream(futures_util::stream::iter([
                            Ok::<_, Infallible>(Bytes::from(vec![b'x'; limit])),
                            Ok(Bytes::from_static(b"x")),
                        ])),
                    )
                        .into_response(),
                    3 => ([(header::CONTENT_TYPE, "image/png")], Vec::<u8>::new()).into_response(),
                    _ => (
                        [(header::CONTENT_TYPE, "image/tiff")],
                        b"unsupported".as_slice(),
                    )
                        .into_response(),
                }
            }
        }),
    );
    let server = FixtureServer::start(router, &material, CAA)?;
    let client = CaaHttpClient::from_builder(
        reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
            .resolve(CAA, server.address),
    )?;
    let body = serde_json::to_vec(&listing())?;
    let listing = domus::infrastructure::caa::CaaListing::parse(RELEASE, &body)?;
    for selected in 0..5 {
        mode.store(selected, Ordering::SeqCst);
        let state = tempfile::tempdir()?;
        let mut store = Store::open(&state.path().join("state.db"))?;
        let fetched = client
            .fetch(&mut store, CaaRequest::image(&listing, IMAGE)?)
            .await;
        if selected == 0 {
            let CaaPayload::Image { bytes, sha256 } = fetched?.payload else {
                return Err("expected bounded original bytes".into());
            };
            assert_eq!(bytes.len(), 8 * 1024 * 1024);
            assert_eq!(sha256, format!("{:x}", Sha256::digest(&bytes)));
            assert!(domus::infrastructure::artwork::normalize_cover(&bytes, None).is_err());
        } else {
            assert!(fetched.is_err(), "mode {selected}");
        }
    }
    server.stop().await?;
    Ok(())
}
