use super::*;
use crate::application::services::artist_images::tests::{Provider, artist, selection, service};
use tower::ServiceExt;

fn request(
    method: &str,
    path: &str,
    body: serde_json::Value,
    csrf: bool,
) -> Result<axum::http::Request<axum::body::Body>> {
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost:8443")
        .header(header::ORIGIN, "https://localhost:8443")
        .header(header::CONTENT_TYPE, "application/json");
    if csrf {
        request = request.header("x-domus-csrf", "1");
    }
    let body = if method == "GET" {
        String::new()
    } else {
        body.to_string()
    };
    let mut request = request
        .body(axum::body::Body::from(body))
        .map_err(|_| Error::Internal("photo fixture request failed"))?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    Ok(request)
}

fn fixture_api(path: &std::path::Path, readonly: bool) -> Result<Api> {
    let mut api = Api::development(
        "https://localhost:8443",
        Store::open_current_existing(path)?,
    )?
    .with_system_settings_read_only(readonly)?;
    let shared = Arc::get_mut(&mut api.0).ok_or(Error::Internal("fixture API is shared"))?;
    shared.service_configuration.artist_images = vec!["wikimedia-commons".into()];
    shared.services.images = service(Arc::new(Provider::new("wikimedia-commons")?))?;
    Ok(api)
}

async fn json(response: Response) -> Result<serde_json::Value> {
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .map_err(|_| Error::Internal("fixture response exceeds its bound"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
async fn artist_images_api_check_save_read_remove_and_stale_preview_are_guarded()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let state = crate::infrastructure::store::artist_images::tests::state()?;
    let path = state.path().join("state.db");
    let mut store = Store::open(&path)?;
    let api = fixture_api(&path, false)?;
    let router = api.router();
    let body = serde_json::to_value(selection("wikimedia-commons"))?;
    let missing_csrf = router
        .clone()
        .oneshot(request(
            "POST",
            "/api/system/artist-photos/check",
            body.clone(),
            false,
        )?)
        .await?;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);
    let checked = router
        .clone()
        .oneshot(request(
            "POST",
            "/api/system/artist-photos/check",
            body.clone(),
            true,
        )?)
        .await?;
    assert_eq!(checked.status(), StatusCode::OK);
    let checked = json(checked).await?;
    let preview = checked["preview_url"]
        .as_str()
        .ok_or("missing preview URL")?;
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", preview, serde_json::Value::Null, false)?)
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "GET",
                &preview.replace("confirmation=", "confirmation=stale"),
                serde_json::Value::Null,
                false
            )?)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let mut command = serde_json::json!({"selection":body, "expected_version":null,
        "expected_confirmation":checked["confirmation"], "acknowledge_permitted_use":false});
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "POST",
                "/api/system/artist-photos/save",
                command.clone(),
                true
            )?)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    command["acknowledge_permitted_use"] = serde_json::json!(true);
    command["expected_confirmation"] = serde_json::json!("wrong");
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "POST",
                "/api/system/artist-photos/save",
                command.clone(),
                true
            )?)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    command["expected_confirmation"] = checked["confirmation"].clone();
    let saved = router
        .clone()
        .oneshot(request(
            "POST",
            "/api/system/artist-photos/save",
            command.clone(),
            true,
        )?)
        .await?;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "POST",
                "/api/system/artist-photos/save",
                command,
                true
            )?)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let path = format!(
        "/api/artist-photos?artist_provider=musicbrainz&artist_id={}",
        artist().id
    );
    let read = router
        .clone()
        .oneshot(request("GET", &path, serde_json::Value::Null, false)?)
        .await?;
    let read = json(read).await?;
    assert_eq!(read["status"], "ready");
    let image_url = read["image_url"]
        .as_str()
        .ok_or("missing fixture image URL")?;
    let mut changed = fixture_api(&state.path().join("state.db"), false)?;
    let mut provider = Provider::new("wikimedia-commons")?;
    provider.source.rights.creator = "Changed credit".into();
    Arc::get_mut(&mut changed.0)
        .ok_or("shared fixture")?
        .services
        .images = service(Arc::new(provider))?;
    assert_eq!(
        changed
            .router()
            .oneshot(request("GET", &path, serde_json::Value::Null, false)?)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let image = router
        .clone()
        .oneshot(request("GET", image_url, serde_json::Value::Null, false)?)
        .await?;
    assert_eq!(image.status(), StatusCode::OK);
    assert_eq!(image.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(image.headers()[header::CACHE_CONTROL], "no-store");
    let binding = store
        .artist_photo_binding(&artist(), "wikimedia-commons")?
        .ok_or("missing binding")?;
    store.remove_artist_photo(binding.id, binding.version)?;
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", image_url, serde_json::Value::Null, false)?)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let read = router
        .oneshot(request("GET", &path, serde_json::Value::Null, false)?)
        .await?;
    assert_eq!(json(read).await?["status"], "unbound");
    Ok(())
}

#[tokio::test]
async fn artist_images_api_readonly_allows_reads_but_rejects_binding_commands()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let state = crate::infrastructure::store::artist_images::tests::state()?;
    let path = state.path().join("state.db");
    let store = Store::open(&path)?;
    let readonly = fixture_api(&path, true)?.router();
    for (endpoint, body) in [
        (
            "check",
            serde_json::to_value(selection("wikimedia-commons"))?,
        ),
        (
            "save",
            serde_json::json!({"selection":selection("wikimedia-commons"),"expected_version":null,
            "expected_confirmation":"a","acknowledge_permitted_use":true}),
        ),
        ("remove", serde_json::json!({"id":1,"expected_version":1})),
    ] {
        assert_eq!(
            readonly
                .clone()
                .oneshot(request(
                    "POST",
                    &format!("/api/system/artist-photos/{endpoint}"),
                    body,
                    true
                )?)
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        readonly
            .clone()
            .oneshot(request(
                "GET",
                "/api/system/artist-photos?limit=50",
                serde_json::Value::Null,
                false
            )?)
            .await?
            .status(),
        StatusCode::OK
    );
    let disabled = Api::development("https://localhost:8443", store)?.router();
    let response = disabled
        .oneshot(request(
            "GET",
            &format!(
                "/api/artist-photos?artist_provider=musicbrainz&artist_id={}",
                artist().id
            ),
            serde_json::Value::Null,
            false,
        )?)
        .await?;
    assert_eq!(json(response).await?["status"], "disabled");
    Ok(())
}
