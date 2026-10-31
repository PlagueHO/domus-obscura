use super::*;
use domus::application::services::artist_images::ArtistImageProvider;
use domus::infrastructure::cache::{CacheLimits, FileCache};
use domus::infrastructure::commons::CommonsClient;
use sha1::{Digest, Sha1};

const COMMONS: &str = "commons.wikimedia.org";
const UPLOAD: &str = "upload.wikimedia.org";
const FILE: &str = "File:Generated.jpg";

#[tokio::test]
async fn artist_images_commons_cancellation_before_submission_does_not_send_a_request() -> TestResult
{
    let material = certificates(COMMONS)?;
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/w/api.php",
        get({
            let hits = hits.clone();
            move || {
                hits.fetch_add(1, Ordering::SeqCst);
                async { Json(json!({})) }
            }
        }),
    );
    let server = FixtureServer::start(app, &material, COMMONS)?;
    let state = private::tempdir()?;
    let root = private::tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let cache = Arc::new(FileCache::open(
        &mut store,
        root.path(),
        CacheLimits::default(),
    )?);
    let client = client(&database, cache, &material, &server, COMMONS)?;
    let task = tokio::spawn(async move { client.inspect(FILE.into()).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    task.abort();
    assert!(task.await.is_err());
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    server.stop().await?;
    Ok(())
}

fn metadata(original: &[u8]) -> Value {
    json!({"query":{"pages":[{
        "pageid":1,"ns":6,"title":FILE,"lastrevid":2,"imagerepository":"local",
        "imageinfo":[{
            "timestamp":"2026-01-01T00:00:00Z","sha1":format!("{:x}",Sha1::digest(original)),
            "size":original.len(),"width":80,"height":40,"mime":"image/jpeg",
            "url":"https://upload.wikimedia.org/wikipedia/commons/a/ab/Generated.jpg",
            "descriptionurl":"https://commons.wikimedia.org/wiki/File:Generated.jpg",
            "extmetadata":{
                "ObjectName":{"value":"Generated fixture"},"Artist":{"value":"Test fixture"},
                "Credit":{"value":"Generated locally"},"LicenseShortName":{"value":"CC BY-SA 4.0"},
                "LicenseUrl":{"value":"https://creativecommons.org/licenses/by-sa/4.0/"},
                "AttributionRequired":{"value":"true"},"UsageTerms":{"value":"CC BY-SA 4.0"}
            }
        }]
    }]}})
}

fn client(
    database: &std::path::Path,
    cache: Arc<FileCache>,
    material: &Certificates,
    server: &FixtureServer,
    host: &str,
) -> TestResult<CommonsClient> {
    Ok(CommonsClient::from_builder(
        database.into(),
        cache,
        reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
            .resolve(host, server.address),
    )?)
}

#[tokio::test]
async fn artist_images_commons_tls_checks_exact_metadata_and_reuses_file_cache_across_restart()
-> TestResult {
    let material = certificates(COMMONS)?;
    let upload_material = certificates(UPLOAD)?;
    let mut original = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut original).encode_image(
        &image::RgbImage::from_pixel(80, 40, image::Rgb([25, 50, 75])),
    )?;
    let hits = Arc::new(AtomicUsize::new(0));
    let metadata_value = metadata(&original);
    let app = Router::new().route(
        "/w/api.php",
        get({
            let hits = hits.clone();
            move |request: Request<Body>| {
                let hits = hits.clone();
                let value = metadata_value.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    assert!(
                        request.headers()[header::USER_AGENT]
                            .to_str()
                            .unwrap_or("")
                            .contains("domus-obscura")
                    );
                    assert!(request.uri().query().unwrap_or("").contains("maxlag=5"));
                    (
                        [(header::CACHE_CONTROL, "public, max-age=3600")],
                        Json(value),
                    )
                }
            }
        }),
    );
    let upload = Router::new().route(
        "/wikipedia/commons/a/ab/Generated.jpg",
        get({
            let hits = hits.clone();
            let original = original.clone();
            move || {
                hits.fetch_add(1, Ordering::SeqCst);
                let original = original.clone();
                async move { ([(header::CONTENT_TYPE, "image/jpeg")], original) }
            }
        }),
    );
    let server = FixtureServer::start(app, &material, COMMONS)?;
    let upload_server = FixtureServer::start(upload, &upload_material, UPLOAD)?;
    let state = private::tempdir()?;
    let root = private::tempdir()?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let cache = Arc::new(FileCache::open(
        &mut store,
        root.path(),
        CacheLimits::default(),
    )?);
    let http = reqwest::Client::builder()
        .tls_certs_only([
            reqwest::Certificate::from_pem(material.ca.as_bytes())?,
            reqwest::Certificate::from_pem(upload_material.ca.as_bytes())?,
        ])
        .resolve(COMMONS, server.address)
        .resolve(UPLOAD, upload_server.address);
    let first = CommonsClient::from_builder(database.clone(), cache.clone(), http)?;
    let evidence = first
        .inspect(FILE.into())
        .await?
        .ok_or("missing generated file")?;
    assert_eq!(first.original(evidence.source.clone()).await?, original);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    drop(first);
    let reopened = client(&database, cache, &material, &server, COMMONS)?;
    let next = reopened
        .inspect(FILE.into())
        .await?
        .ok_or("missing cached file")?;
    assert_eq!(next.source, evidence.source);
    assert_eq!(reopened.original(next.source).await?, original);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    server.stop().await?;
    upload_server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn artist_images_commons_tls_persists_retry_after_without_automatic_retry() -> TestResult {
    for (status, body, retry) in [
        (StatusCode::TOO_MANY_REQUESTS, json!({}), "120"),
        (StatusCode::SERVICE_UNAVAILABLE, json!({}), "120"),
        (StatusCode::OK, json!({"error":{"code":"maxlag"}}), "120"),
        (StatusCode::TOO_MANY_REQUESTS, json!({}), "unsupported"),
    ] {
        let material = certificates(COMMONS)?;
        let hits = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/w/api.php",
            get({
                let hits = hits.clone();
                move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let body = body.clone();
                    async move { (status, [(header::RETRY_AFTER, retry)], Json(body)) }
                }
            }),
        );
        let server = FixtureServer::start(app, &material, COMMONS)?;
        let state = private::tempdir()?;
        let root = private::tempdir()?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let cache = Arc::new(FileCache::open(
            &mut store,
            root.path(),
            CacheLimits::default(),
        )?);
        let first = client(&database, cache.clone(), &material, &server, COMMONS)?;
        assert!(first.inspect(FILE.into()).await.is_err());
        drop(first);
        let next = client(&database, cache, &material, &server, COMMONS)?;
        assert!(
            matches!(next.inspect(FILE.into()).await, Err(domus::Error::Catalog { retry_after_seconds, .. })
            if retry_after_seconds >= 120)
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        server.stop().await?;
    }
    Ok(())
}

#[tokio::test]
async fn artist_images_commons_tls_rejects_redirects_types_bounds_and_forbidden_cache_policy()
-> TestResult {
    for case in ["redirect", "type", "size", "cache", "expired", "vary"] {
        let material = certificates(COMMONS)?;
        let hits = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route("/w/api.php", get({
            let hits = hits.clone();
            move || {
                hits.fetch_add(1, Ordering::SeqCst);
                async move {
                    match case {
                        "redirect" => Redirect::temporary("/redirect-target").into_response(),
                        "type" => ([(header::CONTENT_TYPE, "text/html")], "{}").into_response(),
                        "size" => ([(header::CONTENT_TYPE, "application/json")], vec![b' '; 262145]).into_response(),
                        "expired" => (
                            [(header::EXPIRES, "Thu, 01 Jan 1970 00:00:00 GMT")],
                            Json(json!({"query":{"pages":[{"title":FILE,"ns":6,"missing":true}]}})),
                        ).into_response(),
                        "vary" => (
                            [(header::VARY, "*")],
                            Json(json!({"query":{"pages":[{"title":FILE,"ns":6,"missing":true}]}})),
                        ).into_response(),
                        _ => ([(header::CACHE_CONTROL, "Public, No-Store")],
                            Json(json!({"query":{"pages":[{"title":FILE,"ns":6,"missing":true}]}}))).into_response(),
                    }
                }
            }
        })).route("/redirect-target", get(|| async { panic!("Commons followed a redirect"); #[allow(unreachable_code)] "unexpected" }));
        let server = FixtureServer::start(app, &material, COMMONS)?;
        let state = private::tempdir()?;
        let root = private::tempdir()?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let cache = Arc::new(FileCache::open(
            &mut store,
            root.path(),
            CacheLimits::default(),
        )?);
        assert!(
            client(&database, cache, &material, &server, COMMONS)?
                .inspect(FILE.into())
                .await
                .is_err(),
            "{case}"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        server.stop().await?;
    }
    Ok(())
}
