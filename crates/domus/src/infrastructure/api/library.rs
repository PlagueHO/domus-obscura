use super::*;
use crate::domain::library::*;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct LibraryPageQuery {
    after: i64,
    limit: u16,
    artist: Option<i64>,
    folder: String,
}

impl Default for LibraryPageQuery {
    fn default() -> Self {
        Self {
            after: 0,
            limit: 50,
            artist: None,
            folder: String::new(),
        }
    }
}

pub(super) async fn library_artists(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Query(q): Query<LibraryPageQuery>,
) -> ApiResult<Json<Vec<LibraryArtist>>> {
    let page = Page::new(q.after, q.limit)?;
    Ok(Json(
        api.database(move |store| store.library_artists(id, page))
            .await?,
    ))
}

pub(super) async fn library_albums(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Query(q): Query<LibraryPageQuery>,
) -> ApiResult<Json<Vec<LibraryAlbum>>> {
    let page = Page::new(q.after, q.limit)?;
    Ok(Json(
        api.database(move |store| store.library_albums(id, q.artist, page))
            .await?,
    ))
}

pub(super) async fn library_album(
    State(api): State<Api>,
    Path((id, album)): Path<(i64, i64)>,
) -> ApiResult<Json<LibraryAlbum>> {
    Ok(Json(
        api.database(move |store| store.library_album(id, album))
            .await?,
    ))
}

pub(super) async fn library_album_files(
    State(api): State<Api>,
    Path((id, album)): Path<(i64, i64)>,
    Query(q): Query<LibraryPageQuery>,
) -> ApiResult<Json<Vec<LibraryFile>>> {
    let page = Page::new(q.after, q.limit)?;
    Ok(Json(
        api.database(move |store| store.library_album_files(id, album, page))
            .await?,
    ))
}

pub(super) async fn library_album_assets(
    State(api): State<Api>,
    Path((id, album)): Path<(i64, i64)>,
    Query(q): Query<LibraryPageQuery>,
) -> ApiResult<Json<Vec<LibraryFile>>> {
    let page = Page::new(q.after, q.limit)?;
    Ok(Json(
        api.database(move |store| store.library_album_assets(id, album, page))
            .await?,
    ))
}

pub(super) async fn library_file(
    State(api): State<Api>,
    Path((id, entry)): Path<(i64, i64)>,
) -> ApiResult<Json<LibraryFile>> {
    Ok(Json(
        api.database(move |store| store.library_file(id, entry))
            .await?,
    ))
}

pub(super) async fn library_coverage(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Query(q): Query<LibraryPageQuery>,
) -> ApiResult<Json<LibraryCoverage>> {
    let folder = LibraryPath::parse(&q.folder)?;
    Ok(Json(
        api.database(move |store| store.library_coverage(id, &folder))
            .await?,
    ))
}

pub(super) async fn library_matches(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(queries): Json<Vec<LibraryMatchQuery>>,
) -> ApiResult<Json<LibraryMatches>> {
    Ok(Json(
        api.database(move |store| store.library_matches(id, &queries))
            .await?,
    ))
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct RefreshLibraryFolder {
    pub folder: String,
}

#[derive(Serialize, ts_rs::TS)]
pub struct LibraryRefresh {
    pub coverage: LibraryCoverage,
    pub request: Option<super::super::production_worker::WorkerRequest>,
}

pub(super) async fn library_refresh(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(request): Json<RefreshLibraryFolder>,
) -> ApiResult<(StatusCode, Json<LibraryRefresh>)> {
    api.require_configuration_write().await?;
    let folder = LibraryPath::parse(&request.folder)?;
    if let Some(worker) = &api.0.production_worker {
        let capacity = worker.queue_capacity;
        let binding = worker.binding(&api.0.service_configuration)?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(
                api.database(move |store| {
                    let request = store.enqueue_worker(
                        super::super::production_worker::WorkerOperation::RefreshFolder {
                            location_id: id,
                            folder: folder.as_str().into(),
                        },
                        capacity,
                        &binding,
                    )?;
                    Ok(LibraryRefresh {
                        coverage: store.library_coverage(id, &folder)?,
                        request: Some(request),
                    })
                })
                .await?,
            ),
        ));
    }
    Ok((
        StatusCode::OK,
        Json(
            api.database(move |store| {
                crate::application::refresh_library_folder(store, id, &folder)?;
                Ok(LibraryRefresh {
                    coverage: store.library_coverage(id, &folder)?,
                    request: None,
                })
            })
            .await?,
        ),
    ))
}

pub(super) async fn library_preview(
    State(api): State<Api>,
    Path((id, entry)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    let bytes = api
        .database(move |store| crate::application::library_jpeg_input(store, id, entry))
        .await?;
    let preview = api
        .0
        .services
        .images
        .process_local(
            bytes,
            crate::application::services::images::ImageOptions {
                size: Some(crate::domain::ArtworkSize::Small),
                background: None,
            },
        )
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        preview.jpeg,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn request(
        path: &str,
        body: Option<&str>,
        csrf: bool,
    ) -> Result<axum::http::Request<axum::body::Body>> {
        let mut builder = axum::http::Request::builder()
            .uri(path)
            .header(header::HOST, "localhost:8443");
        if body.is_some() {
            builder = builder
                .method("POST")
                .header(header::ORIGIN, "https://localhost:8443")
                .header(header::CONTENT_TYPE, "application/json");
            if csrf {
                builder = builder.header("x-domus-csrf", "1");
            }
        }
        let mut request = builder
            .body(axum::body::Body::from(body.unwrap_or("").to_owned()))
            .map_err(|_| Error::Internal("invalid fixture request"))?;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        Ok(request)
    }

    #[tokio::test]
    async fn library_api_offline_refresh_is_guarded_read_only_and_job_free() -> TestResult {
        for read_only in [false, true] {
            let state = super::super::super::permissions::private_test_directory()?;
            let media = state.path().join("media");
            std::fs::create_dir(&media)?;
            let database = state.path().join("state.db");
            let store = Store::open(&database)?;
            let location = crate::application::add_location(&store, &media, "main")?;
            let api = Api::development("https://localhost:8443", store)?
                .with_system_settings_read_only(read_only)?
                .with_production_worker(
                    super::super::super::production_worker::WorkerConfiguration {
                        queue_capacity: 2,
                        acquisition: None,
                        sharing: None,
                    },
                )?;
            let router = api.router();
            let path = format!("/api/locations/{location}/library/refresh");
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(&path, Some(r#"{"folder":""}"#), false)?)
                    .await?
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(&path, Some(r#"{"folder":""}"#), true)?)
                    .await?
                    .status(),
                if read_only {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::ACCEPTED
                }
            );
            let match_path = format!("/api/locations/{location}/library/matches");
            let matches = router
                .clone()
                .oneshot(request(
                    &match_path,
                    Some(r#"[{"artist":"A","album":"B"}]"#),
                    true,
                )?)
                .await?;
            assert_eq!(matches.status(), StatusCode::OK);
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(matches.into_body(), 1024 * 1024).await?,
            )?;
            assert_eq!(body["coverage"]["coverage"], "unknown");
            assert_eq!(body["matches"][0]["albums"], serde_json::json!([]));
            for bad in ["../escape", "a:b", "a\\b"] {
                let body = serde_json::json!({"folder":bad}).to_string();
                assert_eq!(
                    router
                        .clone()
                        .oneshot(request(&path, Some(&body), true)?)
                        .await?
                        .status(),
                    if read_only {
                        StatusCode::FORBIDDEN
                    } else {
                        StatusCode::BAD_REQUEST
                    }
                );
            }
            let store = Store::open_read_current(&database)?;
            assert!(store.jobs(Page::new(0, 200)?)?.is_empty());
            assert_eq!(store.location(location)?.coverage, "unknown");
            assert_eq!(
                store.worker_requests(Page::new(0, 200)?)?.len(),
                if read_only { 0 } else { 1 }
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn library_api_previews_decode_and_reject_changed_assets() -> TestResult {
        use std::io::Cursor;
        let state = super::super::super::permissions::private_test_directory()?;
        let media = state.path().join("media");
        std::fs::create_dir(&media)?;
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            16,
            16,
            image::Rgb([30, 60, 90]),
        ))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)?;
        std::fs::write(media.join("folder.jpg"), jpeg.get_ref())?;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let location = crate::application::add_location(&store, &media, "main")?;
        crate::application::scan_location(&mut store, location)?;
        let entry = store
            .browse(location, &LibraryPath::parse("")?, Page::new(0, 10)?)?
            .remove(0)
            .id;
        let router = Api::development("https://localhost:8443", store)?.router();
        let path = format!("/api/locations/{location}/library/files/{entry}/preview");
        let response = router.clone().oneshot(request(&path, None, false)?).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024).await?;
        assert!(bytes.starts_with(&[0xff, 0xd8]));
        assert_eq!(std::fs::read(media.join("folder.jpg"))?, *jpeg.get_ref());
        std::fs::write(media.join("folder.jpg"), b"not a JPEG")?;
        assert_eq!(
            router.oneshot(request(&path, None, false)?).await?.status(),
            StatusCode::CONFLICT
        );
        Ok(())
    }
}
