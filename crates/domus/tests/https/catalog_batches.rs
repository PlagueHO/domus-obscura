use super::*;
use domus::application::services::cache::{Cache, CacheKey};
use domus::domain::{EditionSelection, PlannedTrack};
use domus::infrastructure::cache::{CacheLimits, FileCache};

#[tokio::test]
async fn catalog_batch_api_is_authenticated_atomic_cache_only_and_replay_safe() -> TestResult {
    catalog_batch(false).await
}

#[tokio::test]
async fn shared_cache_catalog_batch_preserves_atomicity_expiry_and_separate_approvals() -> TestResult
{
    catalog_batch(true).await
}

async fn catalog_batch(shared: bool) -> TestResult {
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
    let mut albums = Vec::new();
    for number in 1..=2 {
        let edition = EditionSelection {
            release_id: format!("20000000-0000-4000-8000-{number:012x}"),
            album_artist: "Generated Artist".into(),
            album: format!("Generated Album {number}"),
            bucket: None,
            tracks: vec![PlannedTrack {
                artist: "Generated Artist".into(),
                title: "Generated Track".into(),
                disc: Some(1),
                disc_track: Some(1),
            }],
        };
        support::catalog_snapshot(&database, &edition)?;
        albums.push(json!({
            "release_id":edition.release_id,
            "request":{
                "job_id":store.enqueue(1)?, "expected_version":1, "settings_version":settings,
                "format":if number == 1 { "mp3" } else { "flac" },
                "locations":[location], "accepts_incomplete_coverage":false, "bucket":null
            }
        }));
    }
    let cache_root = private::tempdir()?;
    let mut entries = Vec::new();
    let cache = if shared {
        let connection = rusqlite::Connection::open(&database)?;
        let rows = connection
            .prepare("SELECT source,body,fetched_at FROM catalog_cache")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let cache = Arc::new(FileCache::open(
            &mut store,
            cache_root.path(),
            CacheLimits::default(),
        )?);
        for (source, body, fetched) in rows {
            let key = CacheKey::new("musicbrainz", "responses-v1", &source)?;
            let fetched = u64::try_from(fetched)?;
            cache.put(&key, &body, fetched, fetched + 86400)?;
            entries.push((key, body, fetched));
        }
        assert_eq!(entries.len(), 2);
        connection.execute("DELETE FROM catalog_cache", [])?;
        store = store.with_cache(cache.clone());
        Some(cache)
    } else {
        None
    };
    let api = Api::new_with_catalog(ORIGIN, store, broker, client(&material, &metadata_server)?)?;
    let router = api.router();
    let path = "/api/discovery/plans";
    let request = json!({"albums":albums});
    assert_eq!(
        api_request(
            &router,
            "POST",
            path,
            None,
            Some(ORIGIN),
            Some(request.clone())
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = browser_login(&router, &provider, None).await?;
    assert_eq!(
        api_request(
            &router,
            "POST",
            path,
            Some(&cookie),
            None,
            Some(request.clone())
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut invalid_id = request.clone();
    invalid_id["albums"][1]["release_id"] = json!("invalid");
    let mut missing = request.clone();
    missing["albums"][1]["release_id"] = json!("20000000-0000-4000-8000-000000000099");
    let mut stale = request.clone();
    stale["albums"][1]["request"]["expected_version"] = json!(7);
    let mut duplicate_job = request.clone();
    duplicate_job["albums"][1]["request"]["job_id"] =
        request["albums"][0]["request"]["job_id"].clone();
    let mut duplicate_release = request.clone();
    duplicate_release["albums"][1]["release_id"] = request["albums"][0]["release_id"].clone();
    for (body, status) in [
        (json!({"albums":[]}), StatusCode::BAD_REQUEST),
        (
            json!({"albums":vec![request["albums"][0].clone();26]}),
            StatusCode::BAD_REQUEST,
        ),
        (invalid_id, StatusCode::BAD_REQUEST),
        (missing, StatusCode::CONFLICT),
        (stale, StatusCode::CONFLICT),
        (duplicate_job, StatusCode::BAD_REQUEST),
        (duplicate_release, StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            api_request(
                &router,
                "POST",
                path,
                Some(&cookie),
                Some(ORIGIN),
                Some(body)
            )
            .await?
            .status(),
            status
        );
    }
    let connection = rusqlite::Connection::open(&database)?;
    if let Some(cache) = &cache {
        for (key, body, fetched) in &entries {
            cache.put(key, body, fetched - 86402, fetched - 2)?;
        }
    } else {
        connection.execute(
            "UPDATE catalog_cache SET fetched_at=unixepoch()-86402, expires_at=unixepoch()-1",
            [],
        )?;
    }
    assert_eq!(
        api_request(
            &router,
            "POST",
            path,
            Some(&cookie),
            Some(ORIGIN),
            Some(request.clone())
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    if let Some(cache) = &cache {
        for (key, body, fetched) in &entries {
            cache.put(key, body, *fetched, fetched + 86400)?;
        }
    } else {
        connection.execute(
            "UPDATE catalog_cache SET fetched_at=unixepoch(), expires_at=unixepoch()+86400",
            [],
        )?;
    }
    assert_eq!(
        connection.query_row("SELECT count(*) FROM job_plans", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    assert_eq!(
        connection.query_row(
            "SELECT count(*) FROM jobs WHERE plan_version!=1",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    let response = api_request(
        &router,
        "POST",
        path,
        Some(&cookie),
        Some(ORIGIN),
        Some(request.clone()),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let result = json_body(response).await?;
    assert_eq!(result.as_array().ok_or("batch response missing")?.len(), 2);
    for index in 0..2 {
        assert_eq!(
            result[index]["job_id"],
            request["albums"][index]["request"]["job_id"]
        );
        assert_eq!(
            result[index]["plan"]["edition"]["release_id"],
            request["albums"][index]["release_id"]
        );
        assert_eq!(
            result[index]["plan"]["format"],
            request["albums"][index]["request"]["format"]
        );
        assert_eq!(result[index]["version"], 2);
        assert_eq!(result[index]["approved_at"], Value::Null);
        assert_eq!(result[index]["plan"]["catalog"]["cached"], true);
    }
    let events: i64 = connection.query_row("SELECT count(*) FROM events", [], |row| row.get(0))?;
    assert_eq!(
        api_request(
            &router,
            "POST",
            path,
            Some(&cookie),
            Some(ORIGIN),
            Some(request)
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        connection.query_row("SELECT count(*) FROM events", [], |row| row
            .get::<_, i64>(0))?,
        events
    );
    assert_eq!(
        connection.query_row("SELECT count(*) FROM job_plans", [], |row| row
            .get::<_, i64>(0))?,
        2
    );
    assert_eq!(catalog_fixture.count()?, 0);
    if shared {
        assert_eq!(
            connection.query_row("SELECT count(*) FROM catalog_cache", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
    }
    assert_eq!(std::fs::read_dir(&media)?.count(), 0);
    metadata_server.stop().await?;
    identity_server.stop().await?;
    Ok(())
}
