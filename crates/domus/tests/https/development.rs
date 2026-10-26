use super::*;
use axum::extract::ConnectInfo;
use domus::infrastructure::server::PreparedServer;

pub(super) fn self_signed() -> TestResult<Certificates> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec!["localhost".into()])?;
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let certificate = params.self_signed(&key)?.pem();
    Ok(Certificates {
        ca: certificate.clone(),
        chain: certificate,
        key: key.serialize_pem(),
    })
}

#[tokio::test]
async fn disabled_provider_roles_stop_api_queries_and_plan_batches_before_effects() -> TestResult {
    use domus::application::services::ServiceConfiguration;
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.db");
    let configuration = ServiceConfiguration {
        discovery: Vec::new(),
        metadata: Vec::new(),
        artwork: Vec::new(),
        ..Default::default()
    };
    let origin = "https://localhost:8443";
    let api = Api::development_with_services(origin, Store::open(&database)?, configuration)?;
    let router = api.router();
    for (method, path, body) in [
        ("GET", "/api/discovery/artists?query=Fixture", None),
        (
            "GET",
            "/api/discovery/releases/20000000-0000-4000-8000-000000000001",
            None,
        ),
        (
            "POST",
            "/api/discovery/plans",
            Some(json!({"albums":[{
                "release_id":"20000000-0000-4000-8000-000000000001",
                "request":{"job_id":1,"expected_version":1,"settings_version":1,"format":"mp3",
                "locations":[1],"accepts_incomplete_coverage":false,"bucket":null}
            }]})),
        ),
        ("POST", "/api/jobs/1/plans/2/artwork/caa/search", None),
        (
            "POST",
            "/api/jobs/1/plans/2/artwork/caa/download",
            Some(json!({
                "image_id":"123","background":null,"acknowledge_permitted_use":true
            })),
        ),
        (
            "POST",
            "/api/jobs/1/plans/2/artwork/caa",
            Some(json!({
                "image_id":"123","background":null,"acknowledge_permitted_use":true
            })),
        ),
    ] {
        let body = body.map(|value| serde_json::to_vec(&value)).transpose()?;
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "localhost:8443")
            .header(header::ORIGIN, origin)
            .header("x-domus-csrf", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map(Body::from).unwrap_or_else(Body::empty))?;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        let response = router.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            json_body(response).await?["error"]
                .as_str()
                .ok_or("error text")?
                .contains("disabled")
        );
    }
    let raw = rusqlite::Connection::open(&database)?;
    let effects: i64 = raw.query_row(
        "SELECT (SELECT COUNT(*) FROM jobs) + (SELECT COUNT(*) FROM catalog_cache)
        + (SELECT COUNT(*) FROM events) + (SELECT generation FROM catalog_gate WHERE id=1)",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(effects, 0);
    Ok(())
}

#[tokio::test]
async fn development_https_uses_anonymous_without_sessions_or_entra() -> TestResult {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    let material = self_signed()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let origin = format!("https://localhost:{}", address.port());
    let api =
        Api::development(&origin, Store::open(&database)?)?.with_system_settings_read_only(true)?;
    let config = tls::configuration(
        "localhost",
        material.chain.as_bytes(),
        material.key.as_bytes(),
    )?;
    let handle = axum_server::Handle::new();
    let task = tokio::spawn(domus::infrastructure::server::serve_listener(
        listener,
        config.clone(),
        api.router(),
        handle.clone(),
    ));
    let server = FixtureServer {
        address,
        tls: config,
        handle,
        task: Some(task),
    };
    let client = reqwest::Client::builder()
        .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
        .resolve("localhost", address)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    // Only this generated certificate is trusted. No certificate checks are disabled.
    let response = client.get(format!("{origin}/api/session")).send().await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let identity: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert_eq!(identity["object_id"], "_anonymous");
    assert_eq!(identity["tenant_id"], "_development");
    assert_eq!(identity["client_id"], "_development");
    let local: Value = serde_json::from_slice(
        &client
            .get(format!("{origin}/api/local/status"))
            .send()
            .await?
            .bytes()
            .await?,
    )?;
    assert_eq!(local["enabled"], false);
    assert_eq!(local["busy"], false);
    assert_eq!(
        client
            .post(format!("{origin}/api/local/source-search"))
            .header(header::ORIGIN, &origin)
            .header("x-domus-csrf", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(r#"{"release_id":"20000000-0000-4000-8000-000000000001"}"#)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let again = client
        .get(format!("{origin}/api/session"))
        .header(header::COOKIE, "__Host-domus-session=not-an-identity")
        .header(header::AUTHORIZATION, "Bearer not-an-identity")
        .send()
        .await?;
    let again: Value = serde_json::from_slice(&again.bytes().await?)?;
    assert_eq!(identity, again);
    assert_eq!(
        client
            .get(format!("{origin}/api/settings"))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    let policy: Value = serde_json::from_slice(
        &client
            .get(format!("{origin}/api/system/settings-policy"))
            .send()
            .await?
            .bytes()
            .await?,
    )?;
    assert_eq!(policy["read_only"], true);
    let blocked_save = client
        .put(format!("{origin}/api/settings"))
        .header(header::ORIGIN, &origin)
        .header("x-domus-csrf", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(
            &json!({"expected_version": null, "settings": domus::domain::OutputSettings::default()}),
        )?)
        .send()
        .await?;
    assert_eq!(blocked_save.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        client
            .post(format!("{origin}/api/jobs"))
            .header(header::CONTENT_TYPE, "application/json")
            .body("{}")
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = client
        .post(format!("{origin}/api/jobs"))
        .header(header::ORIGIN, &origin)
        .header("x-domus-csrf", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body("{}")
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        client
            .get(format!("{origin}/api/session"))
            .header(header::ORIGIN, "https://untrusted.invalid")
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .get(format!("{origin}/auth/login"))
            .send()
            .await?
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        client
            .get(format!("{origin}/auth/callback?state=unused&code=unused"))
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .post(format!("{origin}/auth/logout"))
            .header(header::ORIGIN, &origin)
            .header("x-domus-csrf", "1")
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let connection = rusqlite::Connection::open(&database)?;
    assert_eq!(
        connection.query_row("SELECT count(*) FROM sessions", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    let untrusted = reqwest::Client::builder()
        .tls_certs_only([])
        .resolve("localhost", address)
        .no_proxy()
        .build()?;
    assert!(
        untrusted
            .get(format!("{origin}/api/session"))
            .send()
            .await
            .is_err()
    );
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn development_rejects_remote_peers_forwarded_identity_and_remote_origins() -> TestResult {
    for origin in [
        "http://localhost:8443",
        "https://example.invalid:8443",
        "https://localhost.example:8443",
        "https://192.168.1.2:8443",
        "https://localhost:8443/path",
        "https://user@localhost:8443",
    ] {
        let directory = tempfile::tempdir()?;
        assert!(
            Api::development(origin, Store::open(&directory.path().join("state.db"))?).is_err()
        );
    }
    let directory = tempfile::tempdir()?;
    let api = Api::development(
        "https://localhost:8443",
        Store::open(&directory.path().join("state.db"))?,
    )?;
    for peer in [None, Some("192.0.2.1:9000".parse::<SocketAddr>()?)] {
        let mut request = Request::builder()
            .uri("/api/session")
            .header(header::HOST, "localhost:8443")
            .header("x-forwarded-for", "127.0.0.1")
            .header("x-user", "_anonymous")
            .body(Body::empty())?;
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        assert_eq!(
            api.router().oneshot(request).await?.status(),
            StatusCode::FORBIDDEN
        );
    }
    Ok(())
}

#[tokio::test]
async fn explicit_development_bootstrap_accepts_developer_certificate_not_missing_production_auth()
-> TestResult {
    tls::initialize_crypto()?;
    let directory = private::tempdir()?;
    let state = directory.path();
    let database = state.join("state.db");
    drop(Store::open(&database)?);
    let material = self_signed()?;
    let certificate = state.join("certificate.pem");
    let key = state.join("private-key.pem");
    std::fs::write(&certificate, &material.chain)?;
    std::fs::write(&key, &material.key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))?;
    }
    let path = state.join("development.json");
    let valid = json!({
        "environment":"development", "database":database, "listen":"127.0.0.1:8443",
        "origin":"https://localhost:8443", "certificate_file":certificate, "private_key_file":key
    });
    std::fs::write(&path, serde_json::to_vec(&valid)?)?;
    drop(PreparedServer::load(&path)?);
    for (field, value) in [
        ("environment", json!("production")),
        ("environment", Value::Null),
        ("environment", json!("develpment")),
        ("listen", json!("0.0.0.0:8443")),
        ("origin", json!("http://localhost:8443")),
        ("origin", json!("https://remote.invalid:8443")),
        ("tenant_id", json!(TENANT)),
        ("object_id", json!(OBJECT)),
        ("client_id", json!(CLIENT)),
        ("client_secret_file", json!(key)),
    ] {
        let mut changed = valid.clone();
        changed[field] = value;
        std::fs::write(&path, serde_json::to_vec(&changed)?)?;
        assert!(PreparedServer::load(&path).is_err(), "accepted {field}");
    }
    let mut production = valid.clone();
    production
        .as_object_mut()
        .ok_or("configuration is not an object")?
        .remove("environment");
    std::fs::write(&path, serde_json::to_vec(&production)?)?;
    assert!(PreparedServer::load(&path).is_err());
    std::fs::write(&path, serde_json::to_vec(&valid)?)?;
    let wrong_key = KeyPair::generate()?.serialize_pem();
    std::fs::write(&key, wrong_key)?;
    assert!(PreparedServer::load(&path).is_err());
    std::fs::write(&key, &material.key)?;
    let cache_root = private::tempdir()?;
    let mut configured = valid.clone();
    configured["cache"] = json!({"root":cache_root.path()});
    std::fs::write(&path, serde_json::to_vec(&configured)?)?;
    drop(PreparedServer::load(&path)?);
    assert!(cache_root.path().join("owner.lock").exists());
    for root in [state.to_path_buf(), state.join("missing-cache")] {
        let mut rejected = valid.clone();
        rejected["cache"] = json!({"root":root});
        std::fs::write(&path, serde_json::to_vec(&rejected)?)?;
        assert!(PreparedServer::load(&path).is_err());
    }
    assert!(!state.join("missing-cache").exists());
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    drop(listener);
    let origin = format!("https://localhost:{}", address.port());
    let mut runtime = configured;
    runtime["listen"] = json!(address.to_string());
    runtime["origin"] = json!(origin);
    std::fs::write(&path, serde_json::to_vec(&runtime)?)?;
    let prepared = PreparedServer::load(&path)?;
    let task = tokio::spawn(prepared.run());
    let abort = task.abort_handle();
    struct StopOnDrop(tokio::task::AbortHandle);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _stop = StopOnDrop(abort);
    let client = reqwest::Client::builder()
        .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
        .resolve("localhost", address)
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()?;
    let mut response = None;
    for _ in 0..40 {
        if let Ok(reply) = client.get(format!("{origin}/api/session")).send().await {
            response = Some(reply);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let response = response.ok_or("development service did not start without Entra")?;
    assert_eq!(response.status(), StatusCode::OK);
    let identity: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert_eq!(identity["object_id"], "_anonymous");
    let configuration_url = format!("{origin}/api/system/configuration");
    let inspected = client.get(&configuration_url).send().await?;
    assert_eq!(inspected.status(), StatusCode::OK);
    let inspected_bytes = inspected.bytes().await?;
    let inspected: Value = serde_json::from_slice(&inspected_bytes)?;
    assert_eq!(inspected["active"]["environment"], "development");
    assert_eq!(inspected["active"]["origin"], origin);
    assert_eq!(inspected["active"]["cache"]["entries"], 64);
    assert_eq!(inspected["active"]["private_key_file_configured"], true);
    assert_eq!(inspected["active"]["tenant_id_configured"], false);
    assert_eq!(inspected["restart_required"], false);
    assert!(!String::from_utf8_lossy(&inspected_bytes).contains("private-key.pem"));
    let active_revision = inspected["active_revision"].clone();
    let mut changed = runtime.clone();
    changed["system_settings_read_only"] = json!(false);
    changed["origin"] = json!("https://localhost:9443");
    let saved_bytes = serde_json::to_vec(&changed)?;
    std::fs::write(&path, &saved_bytes)?;
    let inspected: Value =
        serde_json::from_slice(&client.get(&configuration_url).send().await?.bytes().await?)?;
    assert_eq!(inspected["active_revision"], active_revision);
    assert_eq!(inspected["active"]["origin"], origin);
    assert_eq!(inspected["saved"]["origin"], "https://localhost:9443");
    assert_eq!(inspected["restart_required"], true);
    assert_eq!(inspected["saved_requires_validation"], true);
    let policy: Value = serde_json::from_slice(
        &client
            .get(format!("{origin}/api/system/settings-policy"))
            .send()
            .await?
            .bytes()
            .await?,
    )?;
    assert_eq!(policy["read_only"], true);
    assert_eq!(std::fs::read(&path)?, saved_bytes);
    for method in [reqwest::Method::PUT, reqwest::Method::POST] {
        assert_eq!(
            client
                .request(method, &configuration_url)
                .header(header::ORIGIN, &origin)
                .header("x-domus-csrf", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(saved_bytes.clone())
                .send()
                .await?
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    assert_eq!(
        client
            .get(&configuration_url)
            .header(header::ORIGIN, "https://untrusted.invalid")
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    std::fs::write(&path, b"{broken")?;
    let invalid = client.get(&configuration_url).send().await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        client
            .get(format!("{origin}/api/session"))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    task.abort();
    assert!(task.await.is_err_and(|error| error.is_cancelled()));
    Ok(())
}
