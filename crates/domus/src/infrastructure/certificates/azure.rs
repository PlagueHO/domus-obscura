//! Azure DNS has one explicit identity and one exact TXT resource.
//! Every update is conditional; no adapter retries an uncertain submission.
use super::http::{RetryDelay, bounded_body, endpoint, retry_after};
use crate::domain::certificates::{CertificateNames, TxtRecord};
use crate::domain::identifiers::is_canonical_uuid;
use crate::infrastructure::http::catalog_client;
use crate::{Error, Result};
use axum::http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureDnsConfiguration {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret_file: PathBuf,
    pub subscription_id: String,
    pub resource_group: String,
    pub zone: String,
}

impl AzureDnsConfiguration {
    pub fn names(&self, hostname: &str) -> Result<CertificateNames> {
        if ![&self.tenant_id, &self.client_id, &self.subscription_id]
            .into_iter()
            .all(|id| is_canonical_uuid(id))
            || self.resource_group.is_empty()
            || self.resource_group.len() > 90
            || self.resource_group.ends_with('.')
            || !self
                .resource_group
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-()".contains(&byte))
        {
            return Err(Error::InvalidInput(
                "Azure DNS requires canonical IDs and a valid resource group".into(),
            ));
        }
        crate::infrastructure::local_file::validate_reference(&self.client_secret_file)?;
        CertificateNames::new(hostname, &self.zone)
    }

    pub fn resource_url(&self, hostname: &str) -> Result<reqwest::Url> {
        let names = self.names(hostname)?;
        let mut url = endpoint("https://management.azure.com/", "management.azure.com")?;
        url.path_segments_mut()
            .map_err(|_| Error::Internal("Azure resource URL has no path"))?
            .extend([
                "subscriptions",
                &self.subscription_id,
                "resourceGroups",
                &self.resource_group,
                "providers",
                "Microsoft.Network",
                "dnsZones",
                &names.zone,
                "TXT",
                &names.record,
            ]);
        url.set_query(Some("api-version=2018-05-01"));
        Ok(url)
    }
}

struct Token {
    value: String,
    expires: Instant,
}

pub struct AzureDns {
    configuration: AzureDnsConfiguration,
    resource: reqwest::Url,
    client: reqwest::Client,
    token: Option<Token>,
    credential_digest: Option<[u8; 32]>,
    retry_delay: RetryDelay,
}

impl AzureDns {
    pub fn new(configuration: AzureDnsConfiguration, hostname: &str) -> Result<Self> {
        Self::from_builder(configuration, hostname, reqwest::Client::builder())
    }

    pub fn from_builder(
        configuration: AzureDnsConfiguration,
        hostname: &str,
        builder: reqwest::ClientBuilder,
    ) -> Result<Self> {
        let resource = configuration.resource_url(hostname)?;
        Ok(Self {
            configuration,
            resource,
            client: catalog_client(builder)?,
            token: None,
            credential_digest: None,
            retry_delay: RetryDelay::default(),
        })
    }

    pub(crate) fn retry_delay(&self) -> Result<Option<u32>> {
        self.retry_delay.take()
    }

    pub(crate) fn pin_credential(&mut self) -> Result<()> {
        let bytes = crate::infrastructure::local_file::read_local(
            &self.configuration.client_secret_file,
            4096,
            true,
        )?;
        self.credential_digest = Some(Sha256::digest(bytes).into());
        Ok(())
    }

    async fn bearer(&mut self) -> Result<&str> {
        if let Some(expected) = self.credential_digest {
            let path = self.configuration.client_secret_file.clone();
            let current: [u8; 32] = tokio::task::spawn_blocking(move || {
                Ok::<_, Error>(
                    Sha256::digest(crate::infrastructure::local_file::read_local(
                        &path, 4096, true,
                    )?)
                    .into(),
                )
            })
            .await
            .map_err(|_| Error::Internal("Azure DNS credential binding task failed"))??;
            if current != expected {
                return Err(Error::Conflict(
                    "Azure DNS credential binding changed; explicit recovery is required".into(),
                ));
            }
        }
        if self
            .token
            .as_ref()
            .is_none_or(|token| token.expires <= Instant::now())
        {
            let path = self.configuration.client_secret_file.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                crate::infrastructure::local_file::read_local(&path, 4096, true)
            })
            .await
            .map_err(|_| Error::Internal("Azure DNS secret-file task failed"))??;
            if self
                .credential_digest
                .is_some_and(|expected| <[u8; 32]>::from(Sha256::digest(&bytes)) != expected)
            {
                return Err(Error::Conflict(
                    "Azure DNS credential changed before token submission".into(),
                ));
            }
            let secret = String::from_utf8(bytes)
                .map_err(|_| Error::InvalidInput("Azure DNS secret file must be UTF-8".into()))?;
            let secret = secret.trim_end_matches(['\r', '\n']);
            if secret.is_empty() || secret.chars().any(char::is_control) {
                return Err(Error::InvalidInput(
                    "Azure DNS secret file must contain one nonempty line".into(),
                ));
            }
            let url = endpoint(
                &format!(
                    "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
                    self.configuration.tenant_id
                ),
                "login.microsoftonline.com",
            )?;
            let response = self
                .client
                .post(url)
                .form(&[
                    ("grant_type", "client_credentials"),
                    ("client_id", self.configuration.client_id.as_str()),
                    ("client_secret", secret),
                    ("scope", "https://management.azure.com/.default"),
                ])
                .send()
                .await
                .map_err(|_| Error::Certificate {
                    reason: "Azure DNS token request failed",
                    retry_after_seconds: Some(60),
                })?;
            let status = response.status();
            let headers = response.headers().clone();
            let bytes = bounded_body(response).await?;
            self.retry_delay.observe(&headers)?;
            check_status(status, &headers)?;
            #[derive(Deserialize)]
            struct TokenResponse {
                access_token: String,
                expires_in: u64,
                token_type: String,
            }
            let token: TokenResponse = serde_json::from_slice(&bytes)
                .map_err(|_| Error::Remote("Azure DNS token response is invalid"))?;
            if !token.token_type.eq_ignore_ascii_case("bearer")
                || !(60..=86400).contains(&token.expires_in)
                || token.access_token.is_empty()
                || token.access_token.len() > 16384
                || token.access_token.chars().any(char::is_control)
            {
                return Err(Error::Remote("Azure DNS bearer response is invalid"));
            }
            self.token = Some(Token {
                value: token.access_token,
                expires: Instant::now() + Duration::from_secs(token.expires_in - 30),
            });
        }
        Ok(&self
            .token
            .as_ref()
            .ok_or(Error::Internal("Azure DNS token is missing"))?
            .value)
    }

    pub async fn read(&mut self) -> Result<Option<TxtRecord>> {
        let (status, headers, bytes) = self.request(Method::GET, None, None).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        check_status(status, &headers)?;
        Ok(Some(record(&bytes)?))
    }

    /// A missing resource uses If-None-Match. Existing resources use their exact
    /// observed ETag. Persist this body and condition before invoking the method.
    pub async fn write(&mut self, etag: Option<&str>, body: &Value) -> Result<TxtRecord> {
        self.write_owned(etag, body)
            .await
            .map_err(|error| match error {
                Error::CertificateNoDnsEffect(error) => match *error {
                    Error::CertificateDnsConflict => Error::Conflict(
                        "Azure DNS conditional update conflicted; reread before recovery".into(),
                    ),
                    error => error,
                },
                error => error,
            })
    }

    /// Retains definitive no-mutation evidence for the saved lifecycle intent.
    pub(crate) async fn write_owned(
        &mut self,
        etag: Option<&str>,
        body: &Value,
    ) -> Result<TxtRecord> {
        TxtRecord {
            etag: etag.unwrap_or("new-record").into(),
            properties: body["properties"].clone(),
        }
        .validate()
        .map_err(|_| {
            Error::CertificateNoDnsEffect(Box::new(Error::InvalidInput(
                "Azure DNS update policy is invalid".into(),
            )))
        })?;
        let (status, headers, bytes) = self.request(Method::PUT, etag, Some(body)).await?;
        if status == StatusCode::PRECONDITION_FAILED || status == StatusCode::CONFLICT {
            return Err(Error::CertificateNoDnsEffect(Box::new(
                Error::CertificateDnsConflict,
            )));
        }
        if matches!(status.as_u16(), 400 | 401 | 403 | 404 | 422) {
            return Err(Error::CertificateNoDnsEffect(Box::new(
                Error::Certificate {
                    reason: "Azure DNS rejected the update; check credentials and exact-record permissions",
                    retry_after_seconds: None,
                },
            )));
        }
        check_status(status, &headers).map_err(|_| Error::Certificate {
            reason: "Azure DNS update failed; reconcile its durable intent before recovery",
            retry_after_seconds: None,
        })?;
        let record = record(&bytes).map_err(|_| Error::Certificate {
            reason: "Azure DNS update response is incomplete; ownership is uncertain",
            retry_after_seconds: None,
        })?;
        let mut properties = record.properties.clone();
        properties
            .as_object_mut()
            .ok_or(Error::Remote("Azure DNS TXT properties are invalid"))?
            .remove("fqdn");
        if properties != body["properties"] {
            return Err(Error::Certificate {
                reason: "Azure DNS update response differs from its intent; ownership is uncertain",
                retry_after_seconds: None,
            });
        }
        Ok(record)
    }

    async fn request(
        &mut self,
        method: Method,
        etag: Option<&str>,
        body: Option<&Value>,
    ) -> Result<(StatusCode, axum::http::HeaderMap, Vec<u8>)> {
        let condition = if method == Method::PUT {
            match etag {
                Some(etag)
                    if !etag.is_empty()
                        && etag.len() <= 512
                        && etag.bytes().all(|byte| byte.is_ascii_graphic()) =>
                {
                    Some(("If-Match", etag))
                }
                Some(_) => {
                    return Err(Error::CertificateNoDnsEffect(Box::new(
                        Error::InvalidInput("Azure DNS ETag is invalid".into()),
                    )));
                }
                None => Some(("If-None-Match", "*")),
            }
        } else {
            None
        };
        let bytes = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|error| Error::CertificateNoDnsEffect(Box::new(Error::from(error))))?;
        if bytes.as_ref().is_some_and(|bytes| bytes.len() > 65536) {
            return Err(Error::CertificateNoDnsEffect(Box::new(
                Error::InvalidInput("Azure DNS update exceeds 64 KiB".into()),
            )));
        }
        let client = self.client.clone();
        let resource = self.resource.clone();
        let token = self.bearer().await.map_err(|error| {
            if method == Method::PUT {
                Error::CertificateNoDnsEffect(Box::new(error))
            } else {
                error
            }
        })?;
        let mut request = client.request(method.clone(), resource).bearer_auth(token);
        if let Some((header, value)) = condition {
            request = request.header(header, value);
        }
        if let Some(bytes) = bytes {
            request = request
                .header("Content-Type", "application/json")
                .body(bytes);
        }
        let response = request.send().await.map_err(|_| Error::Certificate {
            reason: "Azure DNS request failed; a submitted update may be uncertain",
            retry_after_seconds: if method == Method::GET {
                Some(60)
            } else {
                None
            },
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = match bounded_body(response).await {
            Ok(bytes) => bytes,
            Err(error) if method == Method::GET => return Err(error),
            Err(_) => {
                return Err(Error::Certificate {
                    reason: "Azure DNS update response is incomplete; ownership is uncertain",
                    retry_after_seconds: None,
                });
            }
        };
        self.retry_delay.observe(&headers).map_err(|error| {
            if method == Method::PUT {
                Error::Certificate {
                    reason: "Azure DNS update backoff is unsupported; ownership is uncertain",
                    retry_after_seconds: None,
                }
            } else {
                error
            }
        })?;
        Ok((status, headers, bytes))
    }
}

fn record(bytes: &[u8]) -> Result<TxtRecord> {
    #[derive(Deserialize)]
    struct RecordResponse {
        etag: String,
        properties: Value,
    }
    let record: RecordResponse = serde_json::from_slice(bytes)
        .map_err(|_| Error::Remote("Azure DNS TXT response is invalid"))?;
    let record = TxtRecord {
        etag: record.etag,
        properties: record.properties,
    };
    record.validate()?;
    Ok(record)
}

fn check_status(status: StatusCode, headers: &axum::http::HeaderMap) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(Error::Certificate {
            reason: "Azure DNS is temporarily unavailable",
            retry_after_seconds: Some(retry_after(headers, SystemTime::now())?.unwrap_or(60)),
        });
    }
    Err(Error::Certificate {
        reason: "Azure DNS rejected the request; check credentials and exact-record permissions",
        retry_after_seconds: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Request, State};
    use axum::response::{IntoResponse, Response};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

    struct Observation {
        method: Method,
        path: String,
        condition: Option<String>,
        body: Value,
    }

    struct FixtureState {
        token_requests: usize,
        token_status: StatusCode,
        token_body: Value,
        responses: VecDeque<(StatusCode, Value)>,
        observations: Vec<Observation>,
    }

    async fn fixture_request(
        State(state): State<Arc<Mutex<FixtureState>>>,
        request: Request,
    ) -> Response {
        let (parts, body) = request.into_parts();
        let bytes = match axum::body::to_bytes(body, 65536).await {
            Ok(bytes) => bytes,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        let Ok(mut state) = state.lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if parts.uri.path().ends_with("/oauth2/v2.0/token") {
            let Ok(form) = std::str::from_utf8(&bytes)
                .map_err(|_| ())
                .and_then(|body| {
                    reqwest::Url::parse(&format!("https://fixture.invalid/?{body}")).map_err(|_| ())
                })
            else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            let form = form
                .query_pairs()
                .collect::<std::collections::BTreeMap<_, _>>();
            if parts.method != Method::POST
                || form.get("grant_type").map(|v| v.as_ref()) != Some("client_credentials")
                || form.get("client_id").map(|v| v.as_ref())
                    != Some("22222222-2222-2222-2222-222222222222")
                || form.get("client_secret").map(|v| v.as_ref()) != Some("fixture-secret")
                || form.get("scope").map(|v| v.as_ref())
                    != Some("https://management.azure.com/.default")
            {
                return StatusCode::BAD_REQUEST.into_response();
            }
            state.token_requests += 1;
            let mut response =
                (state.token_status, axum::Json(state.token_body.clone())).into_response();
            if state.token_status.is_redirection() {
                response.headers_mut().insert("location", axum::http::HeaderValue::from_static(
                    "https://login.microsoftonline.com/11111111-1111-1111-1111-111111111111/oauth2/v2.0/token"
                ));
            }
            if state.token_status == StatusCode::TOO_MANY_REQUESTS {
                response
                    .headers_mut()
                    .insert("retry-after", axum::http::HeaderValue::from_static("120"));
            }
            return response;
        }
        if parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            != Some("Bearer fixture-token")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            match serde_json::from_slice(&bytes) {
                Ok(body) => body,
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            }
        };
        let condition = ["if-match", "if-none-match"]
            .into_iter()
            .filter_map(|name| {
                parts
                    .headers
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .map(|value| format!("{name}:{value}"))
            })
            .collect::<Vec<_>>();
        if condition.len() > 1 {
            return StatusCode::BAD_REQUEST.into_response();
        }
        state.observations.push(Observation {
            method: parts.method,
            path: parts.uri.to_string(),
            condition: condition.into_iter().next(),
            body,
        });
        match state.responses.pop_front() {
            Some((status, body)) => (status, axum::Json(body)).into_response(),
            None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }

    struct Fixture {
        client: AzureDns,
        state: Arc<Mutex<FixtureState>>,
        handle: axum_server::Handle<std::net::SocketAddr>,
        serving: tokio::task::JoinHandle<std::io::Result<()>>,
        _directory: tempfile::TempDir,
    }

    impl Fixture {
        async fn start(responses: Vec<(StatusCode, Value)>) -> TestResult<Self> {
            let directory = crate::infrastructure::permissions::private_test_directory()?;
            let secret = directory.path().join("dns.secret");
            std::fs::write(&secret, "fixture-secret\r\n")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600))?;
            }
            let generated = rcgen::generate_simple_self_signed(vec![
                "management.azure.com".into(),
                "login.microsoftonline.com".into(),
            ])?;
            let certificate = generated.cert.pem();
            let tls = crate::infrastructure::tls::configuration(
                "management.azure.com",
                certificate.as_bytes(),
                generated.signing_key.serialize_pem().as_bytes(),
            )?;
            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            let address = listener.local_addr()?;
            let state = Arc::new(Mutex::new(FixtureState {
                token_requests: 0,
                token_status: StatusCode::OK,
                token_body: json!({
                    "access_token": "fixture-token", "token_type": "Bearer", "expires_in": 3600
                }),
                responses: responses.into(),
                observations: Vec::new(),
            }));
            let router = axum::Router::new()
                .fallback(fixture_request)
                .with_state(state.clone());
            let handle = axum_server::Handle::new();
            let serving = tokio::spawn(crate::infrastructure::server::serve_listener(
                listener,
                tls,
                router,
                handle.clone(),
            ));
            assert!(handle.listening().await.is_some());
            let client = AzureDns::from_builder(
                configuration(secret),
                "api.home.example",
                reqwest::Client::builder()
                    .tls_certs_only([reqwest::Certificate::from_pem(certificate.as_bytes())?])
                    .resolve("management.azure.com", address)
                    .resolve("login.microsoftonline.com", address),
            )?;
            Ok(Self {
                client,
                state,
                handle,
                serving,
                _directory: directory,
            })
        }

        async fn finish(self) -> TestResult {
            self.handle.shutdown();
            self.serving.await??;
            Ok(())
        }
    }

    fn configuration(secret: PathBuf) -> AzureDnsConfiguration {
        AzureDnsConfiguration {
            tenant_id: "11111111-1111-1111-1111-111111111111".into(),
            client_id: "22222222-2222-2222-2222-222222222222".into(),
            subscription_id: "33333333-3333-3333-3333-333333333333".into(),
            resource_group: "dns-group".into(),
            zone: "home.example".into(),
            client_secret_file: secret,
        }
    }

    fn properties() -> Value {
        json!({
            "TTL": 300,
            "metadata": { "domus-setup-owned": "true", "other": "preserved" },
            "TXTRecords": [{ "value": ["unrelated", "-segmented"] }]
        })
    }

    #[tokio::test]
    async fn certificates_azure_credential_rotation_blocks_even_a_cached_token() -> TestResult {
        let mut fixture = Fixture::start(vec![(
            StatusCode::OK,
            json!({"etag":"first","properties":properties()}),
        )])
        .await?;
        fixture.client.pin_credential()?;
        assert!(fixture.client.read().await?.is_some());
        std::fs::write(
            &fixture.client.configuration.client_secret_file,
            "different-fixture-secret",
        )?;
        assert!(matches!(
            fixture.client.read().await,
            Err(Error::Conflict(_))
        ));
        let body = json!({"properties":properties()});
        assert!(matches!(
            fixture.client.write(Some("first"), &body).await,
            Err(Error::Conflict(_))
        ));
        {
            let state = fixture.state.lock().map_err(|_| "fixture lock failed")?;
            assert_eq!(state.token_requests, 1);
            assert_eq!(state.observations.len(), 1);
        }
        fixture.finish().await
    }

    #[tokio::test]
    async fn certificates_azure_round_trip_preserves_values_and_conditional_ownership() -> TestResult
    {
        use crate::domain::certificates::{TxtAddition, TxtReceipt};
        let value = "a".repeat(43);
        let original = TxtRecord {
            etag: "first".into(),
            properties: properties(),
        };
        let TxtAddition::Update(addition) = original.add(&value)? else {
            return Err("fixture challenge must be new".into());
        };
        let mut observed = addition["properties"].clone();
        observed["fqdn"] = json!("_acme-challenge.api.home.example");
        let mut fixture = Fixture::start(vec![
            (
                StatusCode::OK,
                json!({ "etag": "first", "properties": properties() }),
            ),
            (
                StatusCode::OK,
                json!({ "etag": "second", "properties": observed }),
            ),
            (
                StatusCode::OK,
                json!({ "etag": "third", "properties": properties() }),
            ),
        ])
        .await?;
        let record = fixture.client.read().await?.ok_or("record is missing")?;
        assert_eq!(record.properties, properties());
        let written = fixture.client.write(Some(&record.etag), &addition).await?;
        let removal = written.remove(&TxtReceipt {
            value,
            etag: written.etag.clone(),
        })?;
        let cleaned = fixture.client.write(Some(&written.etag), &removal).await?;
        assert_eq!(cleaned.properties, properties());
        {
            let state = fixture.state.lock().map_err(|_| "fixture lock failed")?;
            assert_eq!(state.token_requests, 1);
            assert_eq!(state.observations.len(), 3);
            assert_eq!(state.observations[0].method, Method::GET);
            assert!(state.observations[0].condition.is_none());
            assert_eq!(
                state.observations[1].condition.as_deref(),
                Some("if-match:first")
            );
            assert_eq!(
                state.observations[2].condition.as_deref(),
                Some("if-match:second")
            );
            assert_eq!(state.observations[1].body, addition);
            assert_eq!(state.observations[2].body, removal);
            assert!(
                state
                    .observations
                    .iter()
                    .all(|request| request.path.ends_with(
                        "/dnsZones/home.example/TXT/_acme-challenge.api?api-version=2018-05-01"
                    ))
            );
        }
        fixture.finish().await
    }

    #[tokio::test]
    async fn certificates_azure_creation_conflicts_and_incomplete_responses_never_retry()
    -> TestResult {
        let body = json!({ "properties": properties() });
        let mut fixture = Fixture::start(vec![
            (
                StatusCode::CREATED,
                json!({ "etag": "created", "properties": properties() }),
            ),
            (
                StatusCode::PRECONDITION_FAILED,
                json!({ "error": "fixture-conflict" }),
            ),
            (StatusCode::OK, json!({ "etag": "missing-properties" })),
        ])
        .await?;
        assert_eq!(fixture.client.write(None, &body).await?.etag, "created");
        assert!(matches!(
            fixture.client.write(Some("created"), &body).await,
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            fixture.client.write(Some("created"), &body).await,
            Err(Error::Certificate {
                retry_after_seconds: None,
                ..
            })
        ));
        {
            let state = fixture.state.lock().map_err(|_| "fixture lock failed")?;
            assert_eq!(state.token_requests, 1);
            assert_eq!(state.observations.len(), 3);
            assert!(
                state
                    .observations
                    .iter()
                    .all(|request| request.method == Method::PUT)
            );
            assert_eq!(
                state.observations[0].condition.as_deref(),
                Some("if-none-match:*")
            );
            assert_eq!(
                state.observations[1].condition.as_deref(),
                Some("if-match:created")
            );
        }
        fixture.finish().await
    }

    #[tokio::test]
    async fn certificates_azure_rejects_invalid_inputs_before_credentials_or_network() -> Result<()>
    {
        let mut client = AzureDns::new(
            configuration(std::env::current_dir()?.join("not-read.secret")),
            "api.home.example",
        )?;
        let body = json!({ "properties": properties() });
        for etag in ["", "unsafe\r\nheader", "space etag"] {
            assert!(matches!(
                client.write(Some(etag), &body).await,
                Err(Error::InvalidInput(_))
            ));
        }
        assert!(matches!(
            client.write(None, &Value::Null).await,
            Err(Error::InvalidInput(_))
        ));
        let mut oversized = body;
        oversized["properties"]["metadata"]["large"] = json!("a".repeat(65536));
        assert!(matches!(
            client.write(None, &oversized).await,
            Err(Error::InvalidInput(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn certificates_azure_invalid_tokens_never_submit_dns_updates() -> TestResult {
        let mut fixture = Fixture::start(Vec::new()).await?;
        fixture
            .state
            .lock()
            .map_err(|_| "fixture lock failed")?
            .token_body =
            json!({ "access_token": "fixture-token", "token_type": "Bearer", "expires_in": 1 });
        assert!(matches!(
            fixture
                .client
                .write(None, &json!({ "properties": properties() }))
                .await,
            Err(Error::Remote(_))
        ));
        {
            let state = fixture.state.lock().map_err(|_| "fixture lock failed")?;
            assert_eq!(state.token_requests, 1);
            assert!(state.observations.is_empty());
        }
        fixture.finish().await
    }

    #[tokio::test]
    async fn certificates_azure_owned_writes_distinguish_rejection_from_a_lost_result() -> TestResult
    {
        let mut fixture = Fixture::start(vec![
            (
                StatusCode::FORBIDDEN,
                json!({"error":"fixture-permission-rejected"}),
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error":"fixture-uncertain"}),
            ),
        ])
        .await?;
        let body = json!({"properties":properties()});
        assert!(matches!(
            fixture.client.write_owned(None, &body).await,
            Err(Error::CertificateNoDnsEffect(_))
        ));
        assert!(matches!(
            fixture.client.write_owned(None, &body).await,
            Err(Error::Certificate {
                retry_after_seconds: None,
                ..
            })
        ));
        assert_eq!(
            fixture
                .state
                .lock()
                .map_err(|_| "fixture lock failed")?
                .observations
                .len(),
            2
        );
        fixture.finish().await
    }

    #[tokio::test]
    async fn certificates_azure_token_redirects_and_backoff_never_submit_dns_updates() -> TestResult
    {
        for (status, delay) in [
            (StatusCode::TEMPORARY_REDIRECT, None),
            (StatusCode::TOO_MANY_REQUESTS, Some(120)),
        ] {
            let mut fixture = Fixture::start(Vec::new()).await?;
            fixture
                .state
                .lock()
                .map_err(|_| "fixture lock failed")?
                .token_status = status;
            let error = fixture
                .client
                .write(None, &json!({ "properties": properties() }))
                .await;
            if status.is_redirection() {
                assert!(matches!(
                    error,
                    Err(Error::Remote("certificate endpoints must not redirect"))
                ));
            } else {
                assert!(matches!(
                    error,
                    Err(Error::Certificate { retry_after_seconds, .. }) if retry_after_seconds == delay
                ));
            }
            {
                let state = fixture.state.lock().map_err(|_| "fixture lock failed")?;
                assert_eq!(state.token_requests, 1);
                assert!(state.observations.is_empty());
            }
            fixture.finish().await?;
        }
        Ok(())
    }

    #[test]
    fn certificates_azure_resource_is_exact_and_cannot_inject_url_paths() -> Result<()> {
        let configuration = AzureDnsConfiguration {
            tenant_id: "11111111-1111-1111-1111-111111111111".into(),
            client_id: "22222222-2222-2222-2222-222222222222".into(),
            subscription_id: "33333333-3333-3333-3333-333333333333".into(),
            resource_group: "dns-group".into(),
            zone: "home.example".into(),
            client_secret_file: std::env::current_dir()?.join("not-read.secret"),
        };
        let url = configuration.resource_url("api.home.example")?;
        assert_eq!(url.host_str(), Some("management.azure.com"));
        assert!(
            url.path()
                .ends_with("/dnsZones/home.example/TXT/_acme-challenge.api")
        );
        assert_eq!(url.query(), Some("api-version=2018-05-01"));
        let mut invalid = configuration;
        invalid.resource_group = "group/other".into();
        assert!(invalid.resource_url("api.home.example").is_err());
        Ok(())
    }
}
