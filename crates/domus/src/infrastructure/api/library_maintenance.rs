use super::*;
use crate::domain::library::{
    LibraryComparison, LibraryScanPolicy, LibrarySetup, SaveLibraryScanPolicy,
};

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct RegisterSelectedLibraryFolder {
    pub root_id: String,
    pub paired: bool,
    pub kind: String,
}

pub(super) async fn library_setup(State(api): State<Api>) -> ApiResult<Json<LibrarySetup>> {
    Ok(Json(api.database(|store| store.library_setup()).await?))
}

pub(super) async fn register_selected_library_folder(
    State(api): State<Api>,
    Json(request): Json<RegisterSelectedLibraryFolder>,
) -> ApiResult<Json<Location>> {
    api.require_configuration_write().await?;
    Ok(Json(
        api.database(move |store| {
            store.register_selected_library_folder(&request.root_id, request.paired, &request.kind)
        })
        .await?,
    ))
}

pub(super) async fn initialize_library_output(
    State(api): State<Api>,
    Json(_request): Json<ScanLocation>,
) -> ApiResult<Json<i64>> {
    api.require_configuration_write().await?;
    Ok(Json(
        api.database(|store| {
            super::super::server::configuration::ConfigurationService::save_output_settings(
                store,
                None,
                &OutputSettings::default(),
            )
        })
        .await?,
    ))
}

pub(super) async fn library_scan_policy(
    State(api): State<Api>,
) -> ApiResult<Json<LibraryScanPolicy>> {
    Ok(Json(
        api.database(|store| store.library_scan_policy()).await?,
    ))
}

pub(super) async fn save_library_scan_policy(
    State(api): State<Api>,
    Json(policy): Json<SaveLibraryScanPolicy>,
) -> ApiResult<Json<LibraryScanPolicy>> {
    api.require_configuration_write().await?;
    if api.0.production_worker.is_none() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Scheduled scans require a configured background worker.",
        ));
    }
    Ok(Json(
        api.database(move |store| store.save_library_scan_policy(&policy))
            .await?,
    ))
}

pub(super) async fn scan_library(
    State(api): State<Api>,
    Json(_request): Json<ScanLocation>,
) -> ApiResult<(StatusCode, Json<Location>)> {
    api.require_configuration_write().await?;
    if let Some(worker) = &api.0.production_worker {
        let capacity = worker.queue_capacity;
        let binding = worker.binding(&api.0.service_configuration)?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(
                api.database(move |store| {
                    store.enqueue_library_scan(capacity, &binding)?;
                    store.location(store.primary_library_id()?)
                })
                .await?,
            ),
        ));
    }
    Ok((
        StatusCode::OK,
        Json(
            api.database(|store| {
                let primary = store.primary_library_id()?;
                crate::application::scan_library(store, primary)?;
                store.location(primary)
            })
            .await?,
        ),
    ))
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ComparisonQuery {
    after: String,
    limit: u16,
}

impl Default for ComparisonQuery {
    fn default() -> Self {
        Self {
            after: String::new(),
            limit: 50,
        }
    }
}

pub(super) async fn library_backup_comparison(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Query(query): Query<ComparisonQuery>,
) -> ApiResult<Json<LibraryComparison>> {
    Ok(Json(
        api.database(move |store| store.compare_library_backup(id, &query.after, query.limit))
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn command(
        path: &str,
        body: &str,
        csrf: bool,
    ) -> Result<axum::http::Request<axum::body::Body>> {
        let mut builder = axum::http::Request::builder()
            .uri(path)
            .method("POST")
            .header(header::HOST, "localhost:8443")
            .header(header::ORIGIN, "https://localhost:8443")
            .header(header::CONTENT_TYPE, "application/json");
        if csrf {
            builder = builder.header("x-domus-csrf", "1");
        }
        let mut request = builder
            .body(axum::body::Body::from(body.to_owned()))
            .map_err(|_| Error::Internal("invalid fixture request"))?;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        Ok(request)
    }

    #[tokio::test]
    async fn library_maintenance_commands_keep_policy_csrf_and_primary_guards() -> TestResult {
        for read_only in [false, true] {
            let directory = super::super::super::permissions::private_test_directory()?;
            let mut store = Store::open(&directory.path().join("state.db"))?;
            let mut ids = Vec::new();
            for kind in ["main", "backup"] {
                let path = directory.path().join(kind);
                std::fs::create_dir(&path)?;
                ids.push(crate::application::add_location(&store, &path, kind)?);
            }
            store.save_settings(None, &OutputSettings::default())?;
            let source = store.source();
            let router = Api::development("https://localhost:8443", store)?
                .with_system_settings_read_only(read_only)?
                .with_production_worker(
                    super::super::super::production_worker::WorkerConfiguration {
                        queue_capacity: 64,
                        acquisition: None,
                        sharing: None,
                    },
                )?
                .router();
            for path in ["/api/library/scan", "/api/library/setup/output"] {
                assert_eq!(
                    router
                        .clone()
                        .oneshot(command(path, "{}", false)?)
                        .await?
                        .status(),
                    StatusCode::FORBIDDEN
                );
            }
            assert_eq!(
                router
                    .clone()
                    .oneshot(command(
                        &format!("/api/locations/{}/scan", ids[1]),
                        "{}",
                        true,
                    )?)
                    .await?
                    .status(),
                if read_only {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::CONFLICT
                }
            );
            for (path, body) in [
                (
                    "/api/library/primary/inspect",
                    r#"{"path":"D:\\generated-media"}"#,
                ),
                (
                    "/api/library/primary/changes",
                    r#"{"path":"D:\\generated-media","expected_revision":"fixture","previous_location_id":null,"review_token":"fixture","confirm_separation":true}"#,
                ),
                ("/api/library/primary/changes/1/activate", "{}"),
                ("/api/library/primary/changes/1/cancel", "{}"),
            ] {
                assert_eq!(
                    router
                        .clone()
                        .oneshot(command(path, body, false)?)
                        .await?
                        .status(),
                    StatusCode::FORBIDDEN,
                );
                if read_only {
                    assert_eq!(
                        router
                            .clone()
                            .oneshot(command(path, body, true)?)
                            .await?
                            .status(),
                        StatusCode::FORBIDDEN,
                    );
                }
            }
            assert_eq!(
                router
                    .clone()
                    .oneshot(command("/api/library/scan", "{}", true)?)
                    .await?
                    .status(),
                if read_only {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::ACCEPTED
                }
            );
            assert_eq!(
                router
                    .clone()
                    .oneshot(command(
                        "/api/library/folders",
                        r#"{"root_id":"unselected","paired":false,"kind":"backup"}"#,
                        true,
                    )?)
                    .await?
                    .status(),
                if read_only {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::CONFLICT
                }
            );
            let inspect = source.open(super::super::super::store::StoreOpenMode::ReadCurrent)?;
            assert_eq!(
                inspect.worker_requests(Page::new(0, 100)?)?.len(),
                usize::from(!read_only)
            );
            assert_eq!(inspect.location(ids[0])?.coverage, "unknown");
            assert_eq!(inspect.location(ids[1])?.coverage, "unknown");
            let mut schedule = command(
                "/api/library/scan-policy",
                r#"{"expected_version":1,"interval_minutes":1440}"#,
                true,
            )?;
            *schedule.method_mut() = axum::http::Method::PUT;
            assert_eq!(
                router.clone().oneshot(schedule).await?.status(),
                if read_only {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::OK
                },
            );
            assert_eq!(
                inspect.library_scan_policy()?.interval_minutes,
                if read_only { None } else { Some(1440) },
            );
        }
        Ok(())
    }
}
