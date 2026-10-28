use super::*;
use axum::extract::ConnectInfo;
use axum::http::Request;
use tower::ServiceExt;

use crate::infrastructure::permissions::private_fixture;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn request(
    method: &str,
    path: &str,
    body: String,
    csrf: bool,
) -> Result<Request<axum::body::Body>> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost:8443")
        .header(header::ORIGIN, "https://localhost:8443")
        .header(header::CONTENT_TYPE, "application/json");
    if csrf {
        request = request.header("x-domus-csrf", "1");
    }
    let mut request = request
        .body(axum::body::Body::from(body))
        .map_err(|_| Error::Internal("invalid sharing fixture request"))?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    Ok(request)
}

#[tokio::test]
async fn sharing_api_binding_is_explicit_guarded_and_read_only_policy_is_enforced() -> TestResult {
    let directory = tempfile::Builder::new()
        .prefix("sharing-api-")
        .tempdir_in(std::env::current_dir()?)?;
    private_fixture::directory(directory.path())?;
    let state = directory.path().join("state");
    let media = directory.path().join("media");
    std::fs::create_dir(&state)?;
    std::fs::create_dir(&media)?;
    let database = state.join("domus.sqlite");
    let store = Store::open(&database)?;
    let location_id = crate::application::add_location(&store, &media, "main")?;
    let router = Api::development("https://localhost:8443", store)?.router();
    let inspection = router
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/sharing/folders?location_id={location_id}"),
            String::new(),
            false,
        )?)
        .await?;
    assert_eq!(inspection.status(), StatusCode::OK);
    let inspection: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(inspection.into_body(), 16_384).await?)?;
    assert_eq!(inspection["bound"], false);
    assert!(inspection["binding_id"].is_null());
    let command = serde_json::json!({
        "location_id": location_id,
        "relative_path": "",
        "expected_identity": inspection["native_identity"],
    })
    .to_string();
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "POST",
                "/api/sharing/folders/bind",
                command.clone(),
                false,
            )?)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let bound = router
        .clone()
        .oneshot(request(
            "POST",
            "/api/sharing/folders/bind",
            command.clone(),
            true,
        )?)
        .await?;
    assert_eq!(bound.status(), StatusCode::OK);
    let bound: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(bound.into_body(), 16_384).await?)?;
    assert_eq!(bound["bound"], true);
    assert!(bound["binding_id"].as_i64().is_some_and(|id| id > 0));
    let bindings = router
        .clone()
        .oneshot(request(
            "GET",
            "/api/sharing/bindings?after=0&limit=1",
            String::new(),
            false,
        )?)
        .await?;
    assert_eq!(bindings.status(), StatusCode::OK);
    let bindings: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(bindings.into_body(), 16_384).await?)?;
    assert_eq!(bindings.as_array().ok_or("binding list")?.len(), 1);
    let readonly = Api::development(
        "https://localhost:8443",
        Store::open_current_existing(&database)?,
    )?
    .with_system_settings_read_only(true)?
    .router();
    assert_eq!(
        readonly
            .oneshot(request("POST", "/api/sharing/folders/bind", command, true,)?)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let escaped = router
        .oneshot(request(
            "GET",
            &format!("/api/sharing/folders?location_id={location_id}&relative_path=..%2Foutside"),
            String::new(),
            false,
        )?)
        .await?;
    assert_eq!(escaped.status(), StatusCode::BAD_REQUEST);
    Ok(())
}
