use super::{MAX_FRAME, NativeAdapter, Request, Response, Selection, Session};
use crate::infrastructure::{server, tls};
use crate::{Error, Result};
use axum::body::Bytes;
use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response as HttpResponse};
use axum::routing::post;
use axum::{Json, Router};
use axum_server::tls_rustls::RustlsConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, RootCertStore, SignatureScheme};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

/// Prepared in memory with explicit paired identities. No discovery, service
/// registration, configuration writes or listener activation occurs in `new`.
pub struct PreparedAdapter {
    native: Arc<NativeAdapter>,
    tls: RustlsConfig,
}

impl PreparedAdapter {
    /// The caller retains/adopts private input files through the existing
    /// no-follow configuration boundary. Pass their actual handles to exclude
    /// hard-link aliases as well as the selected private directory tree.
    pub fn new(
        selection: Selection,
        protected_inputs: Vec<File>,
        certificate: &[u8],
        private_key: &[u8],
        client_ca: &[u8],
        paired_client: &[u8],
    ) -> Result<Self> {
        let roots = anchors(client_ca)?;
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .map_err(|_| Error::InvalidInput("native media client trust is invalid".into()))?;
        let paired = certificates(paired_client)?;
        verifier
            .verify_client_cert(&paired[0], &paired[1..], UnixTime::now())
            .map_err(|_| {
                Error::InvalidInput(
                    "paired native media client is not currently valid and trusted".into(),
                )
            })?;
        let verifier = PinnedClient {
            inner: verifier,
            certificate: paired[0].clone(),
        };
        let checked = tls::authenticated_configuration(
            "localhost",
            certificate,
            private_key,
            Arc::new(verifier),
        )?;
        // Never resume an authenticated connection without checking its peer
        // again. Capability tables are also unique for every TCP connection.
        let mut checked = (*checked.get_inner()).clone();
        checked.send_tls13_tickets = 0;
        checked.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
        Ok(Self {
            native: Arc::new(NativeAdapter::new(selection, protected_inputs)?),
            tls: RustlsConfig::from_config(Arc::new(checked)),
        })
    }

    /// Serve only a caller-created loopback listener. Dropping this future
    /// shuts it down through the shared guarded server. Native I/O runs on
    /// bounded blocking tasks, never on an async executor thread.
    pub async fn serve(
        self,
        listener: TcpListener,
        handle: axum_server::Handle<SocketAddr>,
    ) -> std::io::Result<()> {
        let address = listener.local_addr()?;
        if !address.ip().is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "native media adapter requires loopback",
            ));
        }
        let state = Arc::new(AdapterState {
            native: self.native,
            host: format!("localhost:{}", address.port()),
            slots: Arc::new(Semaphore::new(8)),
        });
        let router = Router::new()
            .route("/v1/media", post(dispatch))
            .layer(DefaultBodyLimit::max(MAX_FRAME))
            .layer(axum::middleware::from_fn(deadline))
            .with_state(state);
        server::serve_listener_with_connect_info::<Connection>(listener, self.tls, router, handle)
            .await
    }
}

struct AdapterState {
    native: Arc<NativeAdapter>,
    host: String,
    slots: Arc<Semaphore>,
}

async fn deadline(request: axum::extract::Request, next: axum::middleware::Next) -> HttpResponse {
    match tokio::time::timeout(Duration::from_secs(10), next.run(request)).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            [(header::CONNECTION, "close")],
            Json(Fault {
                error: FaultCode::Unavailable,
            }),
        )
            .into_response(),
    }
}

#[derive(Clone)]
struct Connection {
    peer: SocketAddr,
    session: Arc<Mutex<Result<Session>>>,
}

impl Connected<SocketAddr> for Connection {
    fn connect_info(peer: SocketAddr) -> Self {
        Self {
            peer,
            session: Arc::new(Mutex::new(Session::new())),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fault {
    error: FaultCode,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FaultCode {
    InvalidRequest,
    Stale,
    Unavailable,
    Capacity,
}

struct OperationError(Error);

impl From<Error> for OperationError {
    fn from(error: Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for OperationError {
    fn into_response(self) -> HttpResponse {
        let (status, error) = match self.0 {
            Error::InvalidInput(_) | Error::Json(_) => {
                (StatusCode::BAD_REQUEST, FaultCode::InvalidRequest)
            }
            Error::Conflict(_) => (StatusCode::CONFLICT, FaultCode::Stale),
            Error::Busy(_) => (StatusCode::SERVICE_UNAVAILABLE, FaultCode::Capacity),
            error => {
                eprintln!("native media operation failed: {error}");
                (StatusCode::SERVICE_UNAVAILABLE, FaultCode::Unavailable)
            }
        };
        (status, Json(Fault { error })).into_response()
    }
}

async fn dispatch(
    State(state): State<Arc<AdapterState>>,
    ConnectInfo(connection): ConnectInfo<Connection>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<HttpResponse, OperationError> {
    let exact = |name, expected: &str| {
        let mut values = headers.get_all(name).iter();
        values.next().is_some_and(|value| value == expected) && values.next().is_none()
    };
    if !connection.peer.ip().is_loopback()
        || !exact(header::HOST, &state.host)
        || !exact(header::CONTENT_TYPE, "application/json")
        || !exact(
            header::HeaderName::from_static("x-domus-media-adapter"),
            "1",
        )
        || headers.contains_key(header::ORIGIN)
        || headers.contains_key(header::COOKIE)
        || headers.contains_key("sec-fetch-site")
    {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let request = serde_json::from_slice::<Request>(&body)
        .map_err(|_| Error::InvalidInput("invalid bounded native media request".into()))?;
    let permit = state
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy("native media operation capacity reached"))?;
    let response = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let _permit = permit;
        let mut locked = connection
            .session
            .lock()
            .map_err(|_| Error::Internal("native media connection lock failed"))?;
        let session = locked
            .as_mut()
            .map_err(|_| Error::Internal("native media connection generation failed"))?;
        encode(&session.dispatch(&state.native, request)?)
    })
    .await
    .map_err(|_| Error::Internal("native media operation task failed"))??;
    Ok(([(header::CONTENT_TYPE, "application/json")], response).into_response())
}

/// One sequential HTTP/1 connection owns this client's live references.
/// A lost connection requires explicit reopening, never transparent replay.
pub struct Client {
    http: reqwest::Client,
    url: String,
    unavailable: bool,
}

impl Client {
    pub fn new(
        address: SocketAddr,
        server_ca: &[u8],
        paired_server: &[u8],
        certificate: &[u8],
        private_key: &[u8],
    ) -> Result<Self> {
        if !address.ip().is_loopback() {
            return Err(Error::InvalidInput(
                "native media client requires loopback".into(),
            ));
        }
        tls::initialize_crypto()?;
        let roots = anchors(server_ca)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots.clone()),
            provider.clone(),
        )
        .build()
        .map_err(|_| Error::InvalidInput("native media server trust is invalid".into()))?;
        let paired = certificates(paired_server)?;
        verifier
            .verify_server_cert(
                &paired[0],
                &paired[1..],
                &ServerName::try_from("localhost")
                    .map_err(|_| Error::Internal("native media TLS hostname is invalid"))?,
                &[],
                UnixTime::now(),
            )
            .map_err(|_| {
                Error::InvalidInput(
                    "paired native media server is not currently valid and trusted".into(),
                )
            })?;
        let key = key(private_key)?;
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| Error::InvalidInput("native media TLS versions are invalid".into()))?
            .with_root_certificates(roots)
            .with_client_auth_cert(certificates(certificate)?, key)
            .map_err(|_| Error::InvalidInput("native media client key pair is invalid".into()))?;
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(PinnedServer {
                inner: verifier,
                certificate: paired[0].clone(),
            }));
        config.resumption = rustls::client::Resumption::disabled();
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(config)
            .https_only(true)
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(1)
            .resolve("localhost", address)
            .build()
            .map_err(|_| {
                Error::InvalidInput("native media TLS client could not be built".into())
            })?;
        Ok(Self {
            http,
            url: format!("https://localhost:{}/v1/media", address.port()),
            unavailable: false,
        })
    }

    /// Failure never returns an empty page, confirmed absence, or an alternate
    /// provider. Protocol/transport loss disables this client's live session.
    pub async fn execute(&mut self, request: &Request) -> Result<Response> {
        if self.unavailable {
            return Err(Error::Remote(
                "native media connection is unavailable; reopen explicitly",
            ));
        }
        let body = encode(request)?;
        let result = self.exchange(body).await.and_then(|response| {
            require_response(request, &response)?;
            Ok(response)
        });
        if matches!(result, Err(Error::Remote(_))) {
            self.unavailable = true;
        }
        result
    }

    async fn exchange(&self, body: Vec<u8>) -> Result<Response> {
        let mut response = self
            .http
            .post(&self.url)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-domus-media-adapter", "1")
            .body(body)
            .send()
            .await
            .map_err(|_| Error::Remote("native media TLS connection failed"))?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|size| size > MAX_FRAME as u64)
        {
            return Err(Error::Remote("native media response exceeds its limit"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Remote("native media response was interrupted"))?
        {
            if chunk.len() > MAX_FRAME - bytes.len() {
                return Err(Error::Remote("native media response exceeds its limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if status != StatusCode::OK {
            let fault = serde_json::from_slice::<Fault>(&bytes)
                .map_err(|_| Error::Remote("native media error response is invalid"))?;
            return Err(match (status, fault.error) {
                (StatusCode::BAD_REQUEST, FaultCode::InvalidRequest) => {
                    Error::InvalidInput("native media request was rejected".into())
                }
                (StatusCode::CONFLICT, FaultCode::Stale) => {
                    Error::Conflict("native media reference or selection is stale".into())
                }
                (StatusCode::SERVICE_UNAVAILABLE, FaultCode::Capacity) => {
                    Error::Busy("native media operation capacity reached")
                }
                _ => Error::Remote("native media operation is unavailable"),
            });
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::Remote("native media response is invalid"))
    }
}

fn require_response(request: &Request, response: &Response) -> Result<()> {
    let valid_reference = |reference: &super::Reference| {
        reference.id != 0
            && reference.connection.len() == 64
            && reference
                .connection
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    let valid = match (request, response) {
        (
            Request::OpenRoot { root, .. },
            Response::Opened {
                reference,
                observed,
            },
        ) => {
            valid_reference(reference)
                && observed.kind == super::Kind::Directory
                && root.expected.components.last() == Some(&observed.identity)
        }
        (
            Request::OpenChild {
                parent, expected, ..
            }
            | Request::PrepareMove {
                parent, expected, ..
            }
            | Request::VerifyFile {
                parent, expected, ..
            }
            | Request::PrepareRemoval {
                parent, expected, ..
            },
            Response::Opened {
                reference,
                observed,
            },
        ) => {
            valid_reference(reference)
                && reference.connection == parent.connection
                && observed == expected
        }
        (Request::Page { limit, .. }, Response::Page { entries, complete }) => {
            entries.len() <= *limit
                && (*complete || entries.len() == *limit)
                && entries.iter().all(|entry| {
                    !entry.name.is_empty()
                        && !entry.name.contains('/')
                        && entry.name.len() <= 1024
                        && crate::domain::LibraryPath::parse(&entry.name).is_ok()
                })
        }
        (Request::Read { count, .. }, Response::Read { bytes, complete }) => {
            bytes.len() <= *count && (*complete || !bytes.is_empty())
        }
        (
            Request::CreateDirectory { .. } | Request::CreateFile { .. } | Request::Rename { .. },
            Response::CheckingName { checked },
        ) => *checked > 0 && *checked % super::MAX_PAGE as u64 == 0,
        (Request::Rename { .. }, Response::Renamed { observed, parent }) => {
            observed.kind == super::Kind::Directory && parent.kind == super::Kind::Directory
        }
        (
            Request::RemoveFile { .. } | Request::RemoveDirectory { .. },
            Response::Removed { parent },
        ) => parent.kind == super::Kind::Directory,
        (
            Request::CreateDirectory { parent, .. } | Request::CreateFile { parent, .. },
            Response::Created {
                reference,
                observed,
                parent: after,
            },
        ) => {
            valid_reference(reference)
                && reference.connection == parent.connection
                && reference.id != parent.id
                && after.kind == super::Kind::Directory
                && match request {
                    Request::CreateDirectory { .. } => observed.kind == super::Kind::Directory,
                    _ => {
                        observed.kind == super::Kind::File
                            && observed.length == 0
                            && observed.links == 1
                    }
                }
        }
        (Request::Write { offset, bytes, .. }, Response::Written { offset: end }) => {
            offset.checked_add(bytes.len() as u64) == Some(*end)
        }
        (Request::Seal { .. }, Response::Sealing { verified }) => {
            *verified > 0 && *verified % super::MAX_READ as u64 == 0
        }
        (Request::Seal { .. }, Response::Sealed { observed }) => {
            observed.kind == super::Kind::File && observed.links == 1
        }
        (
            Request::DirectoryStatus { .. }
            | Request::CheckNamedDirectory { .. }
            | Request::SyncDirectory { .. },
            Response::DirectoryStatus { observed },
        ) => observed.kind == super::Kind::Directory,
        (Request::Close { .. }, Response::Closed) => true,
        _ => false,
    };
    if !valid {
        return Err(Error::Remote(
            "native media response does not match its operation",
        ));
    }
    Ok(())
}

struct Frame(Vec<u8>);

impl Write for Frame {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_FRAME - self.0.len() {
            return Err(std::io::Error::other(
                "native media frame exceeds its limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut frame = Frame(Vec::new());
    serde_json::to_writer(&mut frame, value).map_err(|_| {
        Error::InvalidInput("native media frame cannot be encoded within its limit".into())
    })?;
    Ok(frame.0)
}

fn certificates(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    if pem.len() > MAX_FRAME {
        return Err(Error::InvalidInput(
            "native media certificates exceed their limit".into(),
        ));
    }
    let certificates = rustls_pemfile::certs(&mut &pem[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Error::InvalidInput("native media certificate PEM is invalid".into()))?;
    if certificates.is_empty() || certificates.len() > 8 {
        return Err(Error::InvalidInput(
            "native media certificate chain must contain 1 through 8 certificates".into(),
        ));
    }
    Ok(certificates)
}

#[cfg(test)]
fn leaf(pem: &[u8]) -> Result<CertificateDer<'static>> {
    certificates(pem)?
        .into_iter()
        .next()
        .ok_or(Error::Internal("native media certificate disappeared"))
}

fn anchors(pem: &[u8]) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for certificate in certificates(pem)? {
        roots
            .add(certificate)
            .map_err(|_| Error::InvalidInput("native media trust anchor is invalid".into()))?;
    }
    Ok(roots)
}

fn key(pem: &[u8]) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    if pem.len() > 16384 {
        return Err(Error::InvalidInput(
            "native media private key exceeds its limit".into(),
        ));
    }
    rustls_pemfile::private_key(&mut &pem[..])
        .map_err(|_| Error::InvalidInput("native media private key PEM is invalid".into()))?
        .ok_or_else(|| Error::InvalidInput("native media private key is absent".into()))
}

// Pinning supplements WebPKI. It never replaces chain, validity, usage,
// hostname or handshake-signature verification.
#[derive(Debug)]
struct PinnedClient {
    inner: Arc<dyn ClientCertVerifier>,
    certificate: CertificateDer<'static>,
}

impl ClientCertVerifier for PinnedClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }
    fn verify_client_cert(
        &self,
        end: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        let verified = self.inner.verify_client_cert(end, intermediates, now)?;
        require_pin(end, &self.certificate)?;
        Ok(verified)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, signature)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[derive(Debug)]
struct PinnedServer {
    inner: Arc<dyn ServerCertVerifier>,
    certificate: CertificateDer<'static>,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let verified = self
            .inner
            .verify_server_cert(end, intermediates, name, ocsp, now)?;
        require_pin(end, &self.certificate)?;
        Ok(verified)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, signature)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

fn require_pin(
    actual: &CertificateDer<'_>,
    expected: &CertificateDer<'_>,
) -> std::result::Result<(), rustls::Error> {
    if actual != expected {
        return Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::ApplicationVerificationFailure,
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
