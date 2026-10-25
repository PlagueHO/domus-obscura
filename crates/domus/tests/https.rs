use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Json, Router};
use domus::domain::OutputSettings;
use domus::infrastructure::api::Api;
use domus::infrastructure::entra::EntraBroker;
use domus::infrastructure::http::EntraHttpClient;
use domus::infrastructure::store::Store;
use domus::infrastructure::tls;
use openidconnect::core::{
    CoreGenderClaim, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
    CoreRsaPrivateSigningKey,
};
use openidconnect::{
    AdditionalClaims, Audience, ClientSecret, IdToken, IdTokenClaims, IssuerUrl, JsonWebKeyId,
    PrivateSigningKey, RedirectUrl, StandardClaims, SubjectIdentifier,
};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tower::ServiceExt;

#[path = "https/caa.rs"]
mod caa_tests;
#[path = "https/catalog.rs"]
mod catalog_tests;
#[path = "https/controls.rs"]
mod control_tests;
#[path = "https/development.rs"]
mod development_tests;
#[path = "https/local_album.rs"]
mod local_album_tests;
mod support;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const TENANT: &str = "12345678-1234-4321-9876-123456789abc";
const OBJECT: &str = "87654321-4321-4321-9876-123456789abc";
const CLIENT: &str = "abcdef12-1234-4321-9876-123456789abc";
const ORIGIN: &str = "https://domus.fixture.invalid";
const AUTHORITY: &str = "login.microsoftonline.com";

struct Certificates {
    ca: String,
    chain: String,
    key: String,
}

fn certificates(host: &str) -> TestResult<Certificates> {
    certificates_in_years(host, 2020, 4090)
}

fn certificates_in_years(host: &str, first: i32, last: i32) -> TestResult<Certificates> {
    let mut ca = CertificateParams::default();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "Domus isolated test CA");
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate()?)?;
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec![host.into()])?;
    params.not_before = rcgen::date_time_ymd(first, 1, 1);
    params.not_after = rcgen::date_time_ymd(last, 1, 1);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let leaf = params.signed_by(&key, &ca)?;
    Ok(Certificates {
        ca: ca.pem(),
        chain: format!("{}{}", leaf.pem(), ca.pem()),
        key: key.serialize_pem(),
    })
}

struct FixtureServer {
    address: SocketAddr,
    tls: axum_server::tls_rustls::RustlsConfig,
    handle: axum_server::Handle<SocketAddr>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl FixtureServer {
    fn start(app: Router, material: &Certificates, hostname: &str) -> TestResult<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let config =
            tls::configuration(hostname, material.chain.as_bytes(), material.key.as_bytes())?;
        let handle = axum_server::Handle::new();
        let task = tokio::spawn(domus::infrastructure::server::serve_listener(
            listener,
            config.clone(),
            app,
            handle.clone(),
        ));
        Ok(Self {
            address,
            tls: config,
            handle,
            task: Some(task),
        })
    }

    async fn stop(mut self) -> TestResult {
        self.handle.graceful_shutdown(Some(Duration::from_secs(1)));
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(3), task).await???;
        }
        Ok(())
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn trusted_http(material: &Certificates, server: &FixtureServer) -> TestResult<EntraHttpClient> {
    Ok(EntraHttpClient::from_builder(
        reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
            .resolve(AUTHORITY, server.address),
    )?)
}

fn oidc_request(path: &str) -> TestResult<openidconnect::HttpRequest> {
    Ok(Request::builder()
        .uri(format!("https://{AUTHORITY}{path}"))
        .body(Vec::new())?)
}

#[test]
fn tls_rejects_invalid_material_wrong_hosts_and_mismatched_keys() -> TestResult {
    let material = certificates("localhost")?;
    assert!(
        tls::configuration(
            "localhost",
            material.chain.as_bytes(),
            material.key.as_bytes()
        )
        .is_ok()
    );
    assert!(
        tls::configuration(
            "other.invalid",
            material.chain.as_bytes(),
            material.key.as_bytes()
        )
        .is_err()
    );
    let other = certificates("localhost")?;
    assert!(
        tls::configuration("localhost", material.chain.as_bytes(), other.key.as_bytes()).is_err()
    );
    assert!(tls::configuration("localhost", b"not PEM", material.key.as_bytes()).is_err());
    assert!(tls::configuration("localhost", material.chain.as_bytes(), b"not PEM").is_err());
    assert!(tls::configuration("localhost", &vec![b'x'; 262145], material.key.as_bytes()).is_err());
    for (first, last) in [(2020, 2021), (4080, 4090)] {
        let invalid_time = certificates_in_years("localhost", first, last)?;
        assert!(
            tls::configuration(
                "localhost",
                invalid_time.chain.as_bytes(),
                invalid_time.key.as_bytes()
            )
            .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn oidc_transport_verifies_private_ca_bounds_bodies_and_never_follows_redirects() -> TestResult
{
    let material = certificates(AUTHORITY)?;
    let followed = Arc::new(AtomicUsize::new(0));
    let count = followed.clone();
    let app = Router::new()
        .route("/ok", get(|| async { Json(json!({"fixture": true})) }))
        .route(
            "/redirect",
            get(|| async { Redirect::temporary(&format!("https://{AUTHORITY}/followed")) }),
        )
        .route(
            "/followed",
            get(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    "must not be requested"
                }
            }),
        )
        .route("/large", get(|| async { vec![b'x'; 524289] }))
        .route(
            "/chunked",
            get(|| async {
                let chunks = (0..17).map(|_| Ok::<_, Infallible>(Bytes::from(vec![b'x'; 32768])));
                Body::from_stream(futures_util::stream::iter(chunks))
            }),
        );
    let server = FixtureServer::start(app, &material, AUTHORITY)?;
    let http = trusted_http(&material, &server)?;
    let response = http.clone().execute(oidc_request("/ok")?).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(response.body())?["fixture"],
        true
    );
    let response = http.clone().execute(oidc_request("/redirect")?).await?;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(followed.load(Ordering::SeqCst), 0);
    assert!(http.clone().execute(oidc_request("/large")?).await.is_err());
    assert!(
        http.clone()
            .execute(oidc_request("/chunked")?)
            .await
            .is_err()
    );
    for uri in [
        "http://login.microsoftonline.com/ok",
        "https://evil.invalid/ok",
        "https://login.microsoftonline.com:444/ok",
        "https://user@login.microsoftonline.com/ok",
    ] {
        let request = Request::builder().uri(uri).body(Vec::new())?;
        assert!(http.clone().execute(request).await.is_err());
    }
    let untrusted = EntraHttpClient::from_builder(
        reqwest::Client::builder()
            .tls_danger_accept_invalid_certs(true)
            .tls_danger_accept_invalid_hostnames(true)
            .resolve(AUTHORITY, server.address),
    )?;
    assert!(untrusted.execute(oidc_request("/ok")?).await.is_err());
    let wrong_material = certificates("other.invalid")?;
    let wrong_server = FixtureServer::start(
        Router::new().route("/ok", get(|| async { "wrong hostname" })),
        &wrong_material,
        "other.invalid",
    )?;
    let wrong_hostname = EntraHttpClient::from_builder(
        reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(
                wrong_material.ca.as_bytes(),
            )?])
            .tls_danger_accept_invalid_hostnames(true)
            .resolve(AUTHORITY, wrong_server.address),
    )?;
    assert!(wrong_hostname.execute(oidc_request("/ok")?).await.is_err());
    wrong_server.stop().await?;
    server.stop().await
}

#[derive(Clone)]
struct Provider(Arc<ProviderState>);

#[tokio::test]
async fn tls_reload_changes_new_connections_and_rejects_invalid_replacements() -> TestResult {
    tls::initialize_crypto()?;
    let first = certificates("localhost")?;
    let second = certificates("localhost")?;
    let server = FixtureServer::start(
        Router::new().route("/ok", get(|| async { "TLS fixture" })),
        &first,
        "localhost",
    )?;
    let client = |material: &Certificates| -> TestResult<reqwest::Client> {
        Ok(reqwest::Client::builder()
            .tls_backend_rustls()
            .no_proxy()
            .tls_certs_only([reqwest::Certificate::from_pem(material.ca.as_bytes())?])
            .resolve("localhost", server.address)
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(5))
            .build()?)
    };
    let old_client = client(&first)?;
    let new_client = client(&second)?;
    let url = format!("https://localhost:{}/ok", server.address.port());
    assert_eq!(old_client.get(&url).send().await?.status(), StatusCode::OK);
    assert!(new_client.get(&url).send().await.is_err());
    tls::reload(
        &server.tls,
        "localhost",
        second.chain.as_bytes(),
        second.key.as_bytes(),
    )?;
    assert_eq!(new_client.get(&url).send().await?.status(), StatusCode::OK);
    assert!(old_client.get(&url).send().await.is_err());
    let active = server.tls.get_inner();
    for invalid in [
        certificates("other.invalid")?,
        certificates_in_years("localhost", 2020, 2021)?,
    ] {
        assert!(
            tls::reload(
                &server.tls,
                "localhost",
                invalid.chain.as_bytes(),
                invalid.key.as_bytes()
            )
            .is_err()
        );
        assert!(Arc::ptr_eq(&active, &server.tls.get_inner()));
        assert_eq!(new_client.get(&url).send().await?.status(), StatusCode::OK);
    }
    assert!(
        tls::reload(
            &server.tls,
            "localhost",
            first.chain.as_bytes(),
            second.key.as_bytes()
        )
        .is_err()
    );
    assert!(Arc::ptr_eq(&active, &server.tls.get_inner()));
    server.stop().await
}

struct ProviderState {
    key: CoreRsaPrivateSigningKey,
    pending: Mutex<Option<FixtureLogin>>,
    object: Mutex<String>,
    posts: AtomicUsize,
}

struct FixtureLogin {
    nonce: String,
    challenge: String,
    code: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Claims {
    tid: String,
    oid: String,
    ver: String,
}
impl AdditionalClaims for Claims {}
type Token =
    IdToken<Claims, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm>;

impl Provider {
    fn new() -> TestResult<Self> {
        let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048)?;
        let key = CoreRsaPrivateSigningKey::from_pem(
            &key.to_pkcs1_pem(LineEnding::LF)?,
            Some(JsonWebKeyId::new("isolated-oidc-fixture".into())),
        )?;
        Ok(Self(Arc::new(ProviderState {
            key,
            pending: Mutex::new(None),
            object: Mutex::new(OBJECT.into()),
            posts: AtomicUsize::new(0),
        })))
    }

    fn router(&self) -> Router {
        Router::new()
            .route(
                &format!("/{TENANT}/v2.0/.well-known/openid-configuration"),
                get(metadata),
            )
            .route(&format!("/{TENANT}/discovery/v2.0/keys"), get(keys))
            .route(&format!("/{TENANT}/oauth2/v2.0/token"), post(token))
            .with_state(self.clone())
    }

    fn token(&self, nonce: String) -> TestResult<String> {
        let now = SystemTime::now();
        let claims = IdTokenClaims::new(
            IssuerUrl::new(format!("https://{AUTHORITY}/{TENANT}/v2.0"))?,
            vec![Audience::new(CLIENT.into())],
            (now + Duration::from_secs(600)).into(),
            now.into(),
            StandardClaims::new(SubjectIdentifier::new("generated-browser-subject".into())),
            Claims {
                tid: TENANT.into(),
                oid: self
                    .0
                    .object
                    .lock()
                    .map_err(|_| "fixture lock failed")?
                    .clone(),
                ver: "2.0".into(),
            },
        )
        .set_nonce(Some(openidconnect::Nonce::new(nonce)));
        Ok(Token::new(
            claims,
            &self.0.key,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            None,
            None,
        )?
        .to_string())
    }
}

async fn metadata() -> Json<Value> {
    Json(json!({
        "issuer": format!("https://{AUTHORITY}/{TENANT}/v2.0"),
        "authorization_endpoint": format!("https://{AUTHORITY}/{TENANT}/oauth2/v2.0/authorize"),
        "token_endpoint": format!("https://{AUTHORITY}/{TENANT}/oauth2/v2.0/token"),
        "jwks_uri": format!("https://{AUTHORITY}/{TENANT}/discovery/v2.0/keys"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["pairwise"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "token_endpoint_auth_methods_supported": ["client_secret_post"]
    }))
}

async fn keys(State(provider): State<Provider>) -> Json<CoreJsonWebKeySet> {
    Json(CoreJsonWebKeySet::new(vec![
        provider.0.key.as_verification_key(),
    ]))
}

#[tokio::test]
async fn discovery_and_exchange_reject_changed_metadata_and_report_upstream_failures() -> TestResult
{
    use domus::infrastructure::identity::LoginChallenges;

    let provider = Provider::new()?;
    let original = metadata().await.0;
    let document = Arc::new(Mutex::new(original.clone()));
    let served_document = document.clone();
    let reply = Arc::new(Mutex::new((
        StatusCode::BAD_REQUEST,
        json!({"error": "invalid_grant", "error_description": "private fixture detail"}),
    )));
    let served_reply = reply.clone();
    let app = Router::new()
        .route(
            &format!("/{TENANT}/v2.0/.well-known/openid-configuration"),
            get(move || {
                let document = served_document.clone();
                async move {
                    let body = document
                        .lock()
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                        .clone();
                    Ok::<_, StatusCode>(Json(body))
                }
            }),
        )
        .route(
            &format!("/{TENANT}/discovery/v2.0/keys"),
            get(move || {
                let provider = provider.clone();
                async move { keys(State(provider)).await }
            }),
        )
        .route(
            &format!("/{TENANT}/oauth2/v2.0/token"),
            post(move || {
                let reply = served_reply.clone();
                async move {
                    let (status, body) = reply
                        .lock()
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                        .clone();
                    Ok::<_, StatusCode>((status, Json(body)))
                }
            }),
        );
    let material = certificates(AUTHORITY)?;
    let server = FixtureServer::start(app, &material, AUTHORITY)?;
    let http = trusted_http(&material, &server)?;
    for (field, replacement) in [
        ("issuer", "https://login.microsoftonline.com/wrong/v2.0"),
        ("authorization_endpoint", "https://evil.invalid/authorize"),
        (
            "token_endpoint",
            "https://login.microsoftonline.com/other/token",
        ),
        ("jwks_uri", "https://evil.invalid/keys"),
    ] {
        let mut changed = original.clone();
        changed[field] = replacement.into();
        *document.lock().map_err(|_| "fixture document lock")? = changed;
        assert!(matches!(
            EntraBroker::discover(
                TENANT,
                OBJECT,
                CLIENT,
                ClientSecret::new("generated-fixture-secret".into()),
                RedirectUrl::new(format!("{ORIGIN}/auth/callback"))?,
                http.clone()
            )
            .await,
            Err(domus::Error::Remote(_))
        ));
    }
    *document.lock().map_err(|_| "fixture document lock")? = original;
    let broker = EntraBroker::discover(
        TENANT,
        OBJECT,
        CLIENT,
        ClientSecret::new("generated-fixture-secret".into()),
        RedirectUrl::new(format!("{ORIGIN}/auth/callback"))?,
        http,
    )
    .await?;
    for (status, mut body, authentication) in [
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "invalid_grant"}),
            true,
        ),
        (
            StatusCode::UNAUTHORIZED,
            json!({"error": "invalid_client"}),
            false,
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "unauthorized_client"}),
            false,
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "invalid_scope"}),
            false,
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "unsupported_grant_type"}),
            false,
        ),
        (
            StatusCode::OK,
            json!({"malformed": "private fixture detail"}),
            false,
        ),
    ] {
        body["error_description"] = "private fixture detail".into();
        *reply.lock().map_err(|_| "fixture reply lock")? = (status, body);
        let mut challenges = LoginChallenges::default();
        let start = challenges.begin()?;
        let pending = challenges.consume(start.state.secret(), start.browser_cookie.secret())?;
        let error = broker
            .exchange("isolated-fixture-code".into(), pending)
            .await
            .err()
            .ok_or("exchange unexpectedly succeeded")?;
        assert_eq!(
            matches!(error, domus::Error::Authentication),
            authentication
        );
        if !authentication {
            assert!(matches!(error, domus::Error::Remote(_)));
        }
        assert!(!error.to_string().contains("private fixture detail"));
    }
    server.stop().await
}

async fn token(State(provider): State<Provider>, bytes: Bytes) -> axum::response::Response {
    provider.0.posts.fetch_add(1, Ordering::SeqCst);
    let form = String::from_utf8_lossy(&bytes);
    let mut url = match reqwest::Url::parse("https://fixture.invalid/") {
        Ok(url) => url,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    url.set_query(Some(&form));
    let form: HashMap<_, _> = url.query_pairs().into_owned().collect();
    if form.get("grant_type").map(String::as_str) != Some("authorization_code")
        || form.get("client_id").map(String::as_str) != Some(CLIENT)
        || form.get("client_secret").map(String::as_str) != Some("generated-fixture-secret")
        || form.get("redirect_uri").map(String::as_str)
            != Some("https://domus.fixture.invalid/auth/callback")
        || form
            .get("code_verifier")
            .is_none_or(|value| value.len() < 43)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let pending = match provider.0.pending.lock() {
        Ok(mut pending) => pending.take(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Some(pending) = pending else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(verifier) = form.get("code_verifier") else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let challenge = openidconnect::PkceCodeChallenge::from_code_verifier_sha256(
        &openidconnect::PkceCodeVerifier::new(verifier.clone()),
    );
    if challenge.as_str() != pending.challenge || form.get("code") != Some(&pending.code) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match provider.token(pending.nonce) {
        Ok(token) => Json(json!({"access_token": "unused-generated-fixture-token",
            "token_type": "Bearer", "expires_in": 600, "id_token": token}))
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn api_request(
    router: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    command_origin: Option<&str>,
    body: Option<Value>,
) -> TestResult<axum::response::Response> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "domus.fixture.invalid");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(origin) = command_origin {
        request = request
            .header(header::ORIGIN, origin)
            .header("x-domus-csrf", "1");
    }
    let body = if let Some(body) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body)?)
    } else {
        Body::empty()
    };
    let mut request = request.body(body)?;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    Ok(router.clone().oneshot(request).await?)
}

fn response_cookie(response: &axum::response::Response, name: &str) -> TestResult<String> {
    for value in response.headers().get_all(header::SET_COOKIE) {
        let cookie = cookie::Cookie::parse(value.to_str()?)?;
        if cookie.name() == name {
            assert_eq!(cookie.secure(), Some(true));
            assert_eq!(cookie.http_only(), Some(true));
            assert_eq!(cookie.path(), Some("/"));
            assert!(cookie.domain().is_none());
            assert_eq!(cookie.same_site(), Some(cookie::SameSite::Lax));
            return Ok(format!("{}={}", cookie.name(), cookie.value()));
        }
    }
    Err("expected cookie was not set".into())
}

async fn json_body(response: axum::response::Response) -> TestResult<Value> {
    Ok(serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 2097152).await?,
    )?)
}

async fn next_event(stream: &mut axum::body::BodyDataStream) -> TestResult<String> {
    use futures_util::StreamExt;
    let bytes = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await?
        .ok_or("event stream closed")??;
    Ok(String::from_utf8(bytes.to_vec())?)
}

fn event_id(event: &str) -> TestResult<i64> {
    Ok(event
        .lines()
        .find_map(|line| line.strip_prefix("id:"))
        .ok_or("event ID missing")?
        .trim()
        .parse()?)
}

async fn start_browser_login(router: &Router, provider: &Provider) -> TestResult<(String, String)> {
    let response = api_request(router, "GET", "/auth/login", None, None, None).await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let binding = response_cookie(&response, "__Host-domus-login")?;
    let url = reqwest::Url::parse(response.headers()[header::LOCATION].to_str()?)?;
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(
        params.get("response_type").map(String::as_str),
        Some("code")
    );
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert_eq!(params.get("scope").map(String::as_str), Some("openid"));
    let state = params.get("state").ok_or("state missing")?;
    let code = format!("fixture-code-{state}");
    *provider
        .0
        .pending
        .lock()
        .map_err(|_| "fixture lock failed")? = Some(FixtureLogin {
        nonce: params.get("nonce").ok_or("nonce missing")?.clone(),
        challenge: params
            .get("code_challenge")
            .ok_or("PKCE challenge missing")?
            .clone(),
        code: code.clone(),
    });
    let path = format!("/auth/callback?state={state}&code={code}");
    Ok((path, binding))
}

async fn browser_login(
    router: &Router,
    provider: &Provider,
    previous: Option<&str>,
) -> TestResult<String> {
    let (path, binding) = start_browser_login(router, provider).await?;
    let cookie = match previous {
        Some(previous) => format!("{binding}; {previous}"),
        None => binding,
    };
    let response = api_request(router, "GET", &path, Some(&cookie), None, None).await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let session = response_cookie(&response, "__Host-domus-session")?;
    assert_eq!(
        response_cookie(&response, "__Host-domus-login")?,
        "__Host-domus-login="
    );
    let count = provider.0.posts.load(Ordering::SeqCst);
    let replay = api_request(router, "GET", &path, Some(&cookie), None, None).await?;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(provider.0.posts.load(Ordering::SeqCst), count);
    Ok(session)
}

#[tokio::test]
async fn browser_sign_in_guards_commands_rotates_sessions_and_preserves_durable_state() -> TestResult
{
    let provider = Provider::new()?;
    let material = certificates(AUTHORITY)?;
    let server = FixtureServer::start(provider.router(), &material, AUTHORITY)?;
    let broker = EntraBroker::discover(
        TENANT,
        OBJECT,
        CLIENT,
        ClientSecret::new("generated-fixture-secret".into()),
        RedirectUrl::new(format!("{ORIGIN}/auth/callback"))?,
        trusted_http(&material, &server)?,
    )
    .await?;
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.db");
    let media = directory.path().join("media");
    std::fs::create_dir(&media)?;
    let store = Store::open(&database)?;
    let location = domus::application::add_location(&store, &media, "main")?;
    support::catalog_snapshot(
        &database,
        &domus::domain::EditionSelection {
            release_id: "b1234567-1234-4321-9876-123456789abc".into(),
            album_artist: "Fixture Artist".into(),
            album: "Fixture Album".into(),
            bucket: None,
            tracks: vec![domus::domain::PlannedTrack {
                artist: "Fixture Artist".into(),
                title: "Fixture Track".into(),
                disc: None,
                disc_track: None,
            }],
        },
    )?;
    let api = Api::new(ORIGIN, store, broker)?;
    let router = api.router();
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
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?;
    let response = browser.get(format!("{ORIGIN}/api/session")).send().await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    for path in [
        "/api/session",
        "/api/jobs",
        "/api/locations",
        "/api/settings",
        "/api/questions",
        "/api/events",
        "/api/events/cursor",
        "/api/events/stream",
        "/api/jobs/1/plans/2/transfers",
        "/api/jobs/1/plans/2/publication",
        "/api/jobs/1/plans/2/controls",
        "/api/job-controls/1",
        "/api/jobs/1/plans/2/source-candidates",
        "/api/source-candidates/1",
    ] {
        let response = api_request(&router, "GET", path, None, None, None).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    }
    let cookie = browser_login(&router, &provider, None).await?;
    let response = api_request(&router, "GET", "/api/session", Some(&cookie), None, None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await?["object_id"], OBJECT);
    assert_eq!(
        browser
            .get(format!("{ORIGIN}/api/session"))
            .header(header::COOKIE, &cookie)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    let bad_host = Request::builder()
        .uri("/api/session")
        .header(header::HOST, "evil.invalid")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())?;
    assert_eq!(
        router.clone().oneshot(bad_host).await?.status(),
        StatusCode::MISDIRECTED_REQUEST
    );
    let missing_csrf = Request::builder()
        .method("POST")
        .uri("/api/jobs")
        .header(header::HOST, "domus.fixture.invalid")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))?;
    assert_eq!(
        router.clone().oneshot(missing_csrf).await?.status(),
        StatusCode::FORBIDDEN
    );
    for origin in [None, Some("https://evil.invalid")] {
        let response = api_request(
            &router,
            "POST",
            "/api/jobs",
            Some(&cookie),
            origin,
            Some(json!({})),
        )
        .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let response = api_request(
        &router,
        "POST",
        "/api/jobs",
        Some(&cookie),
        Some(ORIGIN),
        Some(json!({})),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let job = json_body(response).await?["job_id"]
        .as_i64()
        .ok_or("job ID missing")?;
    let response = api_request(
        &router,
        "PUT",
        "/api/settings",
        Some(&cookie),
        Some(ORIGIN),
        Some(json!({"expected_version": null, "settings": OutputSettings::default()})),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let settings_version = json_body(response).await?;
    let request = json!({
        "job_id": job, "expected_version": 1, "settings_version": settings_version,
        "locations": [location], "accepts_incomplete_coverage": true
    });
    let response = api_request(
        &router,
        "POST",
        "/api/discovery/releases/b1234567-1234-4321-9876-123456789abc/plans",
        Some(&cookie),
        Some(ORIGIN),
        Some(request),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let version = json_body(response).await?["version"]
        .as_i64()
        .ok_or("plan version missing")?;
    let response = api_request(
        &router,
        "POST",
        &format!("/api/jobs/{job}/plans/{version}/approve"),
        Some(&cookie),
        Some(ORIGIN),
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let publication_path = format!("/api/jobs/{job}/plans/{version}/publication");
    let response =
        api_request(&router, "GET", &publication_path, Some(&cookie), None, None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await?, json!([]));
    assert_eq!(
        api_request(
            &router,
            "POST",
            &publication_path,
            Some(&cookie),
            Some(ORIGIN),
            Some(json!({}))
        )
        .await?
        .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    for path in [
        "/api/jobs?limit=201",
        "/api/events?after=-1",
        "/api/locations?limit=0",
    ] {
        assert_eq!(
            api_request(&router, "GET", path, Some(&cookie), None, None)
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        api_request(
            &router,
            "GET",
            &format!("/api/locations/{location}/entries?folder=.."),
            Some(&cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    let response = api_request(&router, "GET", "/api/events", Some(&cookie), None, None).await?;
    assert!(
        !json_body(response)
            .await?
            .as_array()
            .ok_or("events missing")?
            .is_empty()
    );
    let mut worker_store = Store::open_existing(&database)?;
    let claimed = worker_store
        .claim_approved_plan(job, version, "isolated-fixture-worker", 60)?
        .ok_or("approved job was not claimable")?;
    worker_store.advance(
        &claimed.claim,
        domus::domain::JobState::Queued,
        domus::domain::JobState::Searching,
    )?;
    worker_store.begin_acquisition(
        &claimed.claim,
        "generated-peer",
        &[domus::infrastructure::store::SourceFile {
            remote_path: "Generated Album\\Fixture Track.mp3".into(),
            expected_bytes: 10,
        }],
    )?;
    let transfer_path = format!("/api/jobs/{job}/plans/{version}/transfers");
    let response = api_request(&router, "GET", &transfer_path, Some(&cookie), None, None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let transfers = json_body(response).await?;
    assert_eq!(transfers.as_array().ok_or("transfers missing")?.len(), 1);
    assert_eq!(transfers[0]["state"], "planned");
    assert_eq!(transfers[0]["peer"], "generated-peer");
    assert_eq!(transfers[0]["expected_bytes"], 10);
    assert_eq!(
        api_request(
            &router,
            "GET",
            &format!("{transfer_path}?limit=201"),
            Some(&cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    let question = domus::domain::Question::new(
        "Fixture decision".into(),
        vec!["Retry".into(), "Skip".into()],
    )?;
    let question_id = worker_store.ask(
        &claimed.claim,
        domus::domain::JobState::Downloading,
        &question,
    )?;
    let response = api_request(
        &router,
        "GET",
        "/api/questions?limit=1",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(json_body(response).await?[0]["id"], question_id);
    let response = api_request(
        &router,
        "POST",
        &format!("/api/questions/{question_id}/answer"),
        Some(&cookie),
        Some(ORIGIN),
        Some(json!({"plan_version": version + 1, "choice": "Retry"})),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        json_body(response).await?["error"]
            .as_str()
            .ok_or("error message missing")?
            .contains("changed")
    );
    let response = api_request(
        &router,
        "POST",
        &format!("/api/questions/{question_id}/answer"),
        Some(&cookie),
        Some(ORIGIN),
        Some(json!({"plan_version": version, "choice": "Retry"})),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = api_request(&router, "GET", "/api/questions", Some(&cookie), None, None).await?;
    assert_eq!(json_body(response).await?, json!([]));
    assert_eq!(
        worker_store.job_state(job)?.as_deref(),
        Some("awaiting_input")
    );
    let cursor = json_body(
        api_request(
            &router,
            "GET",
            "/api/events/cursor",
            Some(&cookie),
            None,
            None,
        )
        .await?,
    )
    .await?["after"]
        .as_i64()
        .ok_or("event cursor missing")?;
    assert_eq!(
        api_request(
            &router,
            "GET",
            &format!("/api/events/stream?after={}", cursor + 1),
            Some(&cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let response = api_request(
        &router,
        "GET",
        "/api/events/stream?limit=1",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()?
            .starts_with("text/event-stream")
    );
    let mut live = response.into_body().into_data_stream();
    let first = next_event(&mut live).await?;
    let first_id = event_id(&first)?;
    assert!(first.lines().any(|line| {
        line.strip_prefix("event:")
            .is_some_and(|value| value.trim() == "job")
    }));
    let event: Value = serde_json::from_str(
        first
            .lines()
            .find_map(|line| line.strip_prefix("data:"))
            .ok_or("event data missing")?
            .trim(),
    )?;
    assert_eq!(event["id"], first_id);
    let replay = Request::builder()
        .uri("/api/events/stream?after=0&limit=1")
        .header(header::HOST, "domus.fixture.invalid")
        .header(header::COOKIE, &cookie)
        .header("last-event-id", first_id.to_string())
        .body(Body::empty())?;
    let response = router.clone().oneshot(replay).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut replay = response.into_body().into_data_stream();
    assert!(event_id(&next_event(&mut replay).await?)? > first_id);
    drop(replay);
    for invalid in ["-1", "bad", "9223372036854775808"] {
        let request = Request::builder()
            .uri("/api/events/stream")
            .header(header::HOST, "domus.fixture.invalid")
            .header(header::COOKIE, &cookie)
            .header("last-event-id", invalid)
            .body(Body::empty())?;
        assert_eq!(
            router.clone().oneshot(request).await?.status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut held = Vec::new();
    for _ in 0..3 {
        let response = api_request(
            &router,
            "GET",
            "/api/events/stream",
            Some(&cookie),
            None,
            None,
        )
        .await?;
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    assert_eq!(
        api_request(
            &router,
            "GET",
            "/api/events/stream",
            Some(&cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    held.pop();
    let response = api_request(
        &router,
        "GET",
        "/api/events/stream",
        Some(&cookie),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    drop(held);
    let mut wire = browser
        .get(format!("{ORIGIN}/api/events/stream?after=0&limit=1"))
        .header(header::COOKIE, &cookie)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    assert_eq!(wire.status(), StatusCode::OK);
    assert!(
        wire.headers()[header::CONTENT_TYPE]
            .to_str()?
            .starts_with("text/event-stream")
    );
    assert!(wire.chunk().await?.is_some());
    let new_cookie = browser_login(&router, &provider, Some(&cookie)).await?;
    assert_ne!(cookie, new_cookie);
    let expired = next_event(&mut live).await?;
    assert!(expired.contains("authentication required"));
    use futures_util::StreamExt;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), live.next())
            .await?
            .is_none()
    );
    drop(live);
    let rejected = tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        while let Some(chunk) = wire.chunk().await? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > 65536 {
                return Err("unexpected event volume".into());
            }
            if String::from_utf8_lossy(&bytes).contains("authentication required") {
                return Ok::<_, Box<dyn std::error::Error>>(());
            }
        }
        Err("stream closed without reporting revocation".into())
    })
    .await?;
    rejected?;
    drop(wire);
    assert_eq!(
        api_request(&router, "GET", "/api/session", Some(&cookie), None, None)
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let oversized = Request::builder()
        .method("POST")
        .uri("/api/plans")
        .header(header::HOST, "domus.fixture.invalid")
        .header(header::ORIGIN, ORIGIN)
        .header("x-domus-csrf", "1")
        .header(header::COOKIE, &new_cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(vec![b' '; 2097153]))?;
    assert_eq!(
        router.clone().oneshot(oversized).await?.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let (path, binding) = start_browser_login(&router, &provider).await?;
    let count = provider.0.posts.load(Ordering::SeqCst);
    let response = api_request(
        &router,
        "GET",
        &path,
        Some("__Host-domus-login=wrong-browser"),
        None,
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(provider.0.posts.load(Ordering::SeqCst), count);
    *provider
        .0
        .object
        .lock()
        .map_err(|_| "fixture lock failed")? = CLIENT.into();
    let response = api_request(&router, "GET", &path, Some(&binding), None, None).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(provider.0.posts.load(Ordering::SeqCst), count + 1);
    assert!(response_cookie(&response, "__Host-domus-session").is_err());
    assert_eq!(
        api_request(&router, "GET", &path, Some(&binding), None, None)
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(provider.0.posts.load(Ordering::SeqCst), count + 1);
    assert_eq!(
        api_request(
            &router,
            "GET",
            "/auth/logout",
            Some(&new_cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        api_request(
            &router,
            "POST",
            "/auth/logout",
            Some(&new_cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    let response = api_request(
        &router,
        "POST",
        "/auth/logout",
        Some(&new_cookie),
        Some(ORIGIN),
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response_cookie(&response, "__Host-domus-session")?,
        "__Host-domus-session="
    );
    assert_eq!(
        api_request(
            &router,
            "GET",
            "/api/session",
            Some(&new_cookie),
            None,
            None
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let reopened = Store::open_existing(&database)?;
    assert!(reopened.plan(job, version)?.approved_at.is_some());
    assert_eq!(reopened.jobs(domus::domain::Page::new(0, 200)?)?.len(), 1);
    assert_eq!(
        reopened.question(question_id)?.answer.as_deref(),
        Some("Retry")
    );
    for _ in 0..9 {
        assert_eq!(
            api_request(&router, "GET", "/auth/login", None, None, None)
                .await?
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        api_request(&router, "GET", "/auth/login", None, None, None)
            .await?
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let mut other_peer = Request::builder()
        .uri("/auth/login")
        .header(header::HOST, "domus.fixture.invalid")
        .header("x-forwarded-for", "127.0.0.1")
        .body(Body::empty())?;
    other_peer
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 2],
            12345,
        ))));
    assert_eq!(
        router.clone().oneshot(other_peer).await?.status(),
        StatusCode::SEE_OTHER
    );
    application_server.stop().await?;
    server.stop().await
}

#[test]
fn service_bootstrap_rejects_public_binding_bad_tls_and_media_secret_overlap() -> TestResult {
    use domus::infrastructure::server::PreparedServer;
    let directory = tempfile::tempdir()?;
    let state = directory.path();
    let database = state.join("state.db");
    let store = Store::open(&database)?;
    drop(Store::open_existing(store.database_path())?);
    assert!(matches!(
        Store::open(std::path::Path::new(
            "\\\\domus-fixture.invalid\\share\\state.db"
        )),
        Err(domus::Error::InvalidInput(_))
    ));
    let material = certificates("localhost")?;
    let certificate = state.join("certificate.pem");
    let private_key = state.join("private-key.pem");
    let secret = state.join("client-secret.txt");
    let config = state.join("service.json");
    std::fs::write(&certificate, material.chain)?;
    std::fs::write(&private_key, material.key)?;
    std::fs::write(&secret, "generated-fixture-secret\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private_key, std::fs::Permissions::from_mode(0o600))?;
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600))?;
    }
    let valid = json!({
        "database": database, "listen": "127.0.0.1:8443", "origin": "https://localhost:8443",
        "tenant_id": TENANT, "object_id": OBJECT, "client_id": CLIENT,
        "client_secret_file": secret, "certificate_file": certificate, "private_key_file": private_key,
    });
    std::fs::write(&config, serde_json::to_vec(&valid)?)?;
    drop(PreparedServer::load(&config)?);
    for (field, value) in [
        ("listen", "0.0.0.0:8443"),
        ("listen", "127.0.0.1:0"),
        ("origin", "http://localhost:8443"),
        ("origin", "https://localhost:443"),
        ("origin", "https://other.invalid:8443"),
        ("client_id", "not-a-uuid"),
        ("database", "relative.db"),
    ] {
        let mut changed = valid.clone();
        changed[field] = value.into();
        std::fs::write(&config, serde_json::to_vec(&changed)?)?;
        assert!(
            PreparedServer::load(&config).is_err(),
            "{field} was accepted unexpectedly"
        );
    }
    let media = state.join("media");
    std::fs::create_dir(&media)?;
    domus::application::add_location(&store, &media, "main")?;
    let media_secret = media.join("secret.txt");
    std::fs::write(&media_secret, "generated-fixture-secret")?;
    let mut changed = valid.clone();
    changed["client_secret_file"] = serde_json::to_value(&media_secret)?;
    std::fs::write(&config, serde_json::to_vec(&changed)?)?;
    assert!(PreparedServer::load(&config).is_err());
    std::fs::write(&config, vec![b' '; 16385])?;
    assert!(PreparedServer::load(&config).is_err());
    std::fs::write(&config, serde_json::to_vec(&valid)?)?;
    std::fs::write(&secret, vec![b'x'; 4097])?;
    assert!(PreparedServer::load(&config).is_err());
    std::fs::write(&secret, "generated-fixture-secret")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644))?;
        assert!(PreparedServer::load(&config).is_err());
    }
    Ok(())
}
