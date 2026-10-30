use super::*;
use crate::infrastructure::store::{
    RecoveryContinue, RecoveryDecision, RecoveryEntry, RecoveryOperation, RecoveryPreview,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecoveryQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "recovery_limit")]
    limit: u16,
    job: Option<i64>,
    category: Option<String>,
    location: Option<i64>,
}

fn recovery_limit() -> u16 {
    50
}

pub(super) async fn inventory(
    State(api): State<Api>,
    Query(query): Query<RecoveryQuery>,
) -> ApiResult<Json<Vec<RecoveryEntry>>> {
    let page = Page::new(query.after, query.limit)?;
    Ok(Json(
        api.database(move |store| {
            store.recovery_inventory_filtered(
                page,
                query.job,
                query.category.as_deref(),
                query.location,
            )
        })
        .await?,
    ))
}

pub(super) async fn preview(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<Json<RecoveryPreview>> {
    let permit = api.reload_permit()?;
    Ok(Json(
        api.database(move |store| {
            let _permit = permit;
            crate::application::recovery::preview_recovery(store, id)
        })
        .await?,
    ))
}

pub(super) async fn delete(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(decision): Json<RecoveryDecision>,
) -> ApiResult<Json<RecoveryOperation>> {
    let permit = api.reload_permit()?;
    Ok(Json(
        api.database(move |store| {
            let _permit = permit;
            crate::application::recovery::delete_recovery(store, id, decision)
        })
        .await?,
    ))
}

pub(super) async fn operation(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Option<RecoveryOperation>>> {
    Ok(Json(
        api.database(move |store| store.recovery_operation(id))
            .await?,
    ))
}

pub(super) async fn continue_delete(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(request): Json<RecoveryContinue>,
) -> ApiResult<Json<RecoveryOperation>> {
    proceed(api, id, request, false).await
}

pub(super) async fn cleanup(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(request): Json<RecoveryContinue>,
) -> ApiResult<Json<RecoveryOperation>> {
    proceed(api, id, request, true).await
}

async fn proceed(
    api: Api,
    id: i64,
    request: RecoveryContinue,
    cleanup: bool,
) -> ApiResult<Json<RecoveryOperation>> {
    let permit = api.reload_permit()?;
    Ok(Json(
        api.database(move |store| {
            let _permit = permit;
            crate::application::recovery::continue_recovery(
                store,
                id,
                request.expected_operation_id,
                cleanup,
            )
        })
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn request(path: &str, body: Option<&str>, csrf: bool, origin: &str) -> Result<Request<Body>> {
        let mut builder = Request::builder()
            .uri(path)
            .method(if body.is_some() { "POST" } else { "GET" })
            .header(header::HOST, "localhost:8443")
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/json");
        if csrf {
            builder = builder.header("x-domus-csrf", "1");
        }
        let mut request = builder
            .body(Body::from(body.unwrap_or("").to_owned()))
            .map_err(|_| Error::Internal("invalid recovery test request"))?;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        Ok(request)
    }

    #[tokio::test]
    async fn recovery_api_preserves_command_guards_limits_and_reload_exclusion()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let temp = super::super::super::permissions::private_test_directory()?;
        let api = Api::development(
            "https://localhost:8443",
            Store::open(&temp.path().join("state.db"))?,
        )?;
        let router = api.router();
        let body = r#"{"expected_preview_sha256":"0000000000000000000000000000000000000000000000000000000000000000","acknowledge_irreversible_deletion":false}"#;
        for path in [
            "/api/recovery/11/delete",
            "/api/recovery/11/continue",
            "/api/recovery/11/cleanup",
        ] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(path, Some(body), false, "https://localhost:8443")?)
                    .await?
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(path, Some(body), true, "https://wrong.invalid")?)
                    .await?
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            router
                .clone()
                .oneshot(request(
                    "/api/recovery/11/delete",
                    Some(body),
                    true,
                    "https://localhost:8443"
                )?)
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            router
                .clone()
                .oneshot(request(
                    "/api/recovery?limit=101",
                    None,
                    false,
                    "https://localhost:8443"
                )?)
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            router
                .clone()
                .oneshot(request(
                    "/api/recovery?limit=50",
                    None,
                    false,
                    "https://localhost:8443"
                )?)
                .await?
                .status(),
            StatusCode::OK
        );
        let _reload = api.reload_permit()?;
        for path in [
            "/api/recovery/11/preview",
            "/api/jobs/1/plans/1/publication/replacement/1/restore",
        ] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(path, None, false, "https://localhost:8443")?)
                    .await?
                    .status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
        Ok(())
    }
}
