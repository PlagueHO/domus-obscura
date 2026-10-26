use super::api::Api;
use super::entra::EntraBroker;
use super::filesystem::require_local_disk;
use super::http::EntraHttpClient;
use super::local_file::read_local;
use super::store::Store;
use super::tls;
use crate::domain::Page;
use crate::{Error, Result};
use axum::Router;
use axum_server::accept::Accept;
use axum_server::tls_rustls::RustlsConfig;
use hyper_util::rt::TokioTimer;
use openidconnect::{ClientSecret, RedirectUrl};
use serde::Deserialize;
use std::future::Future;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const MAX_CONNECTIONS: usize = 64;

/// A permit covers both TLS negotiation and the complete connection lifetime,
/// including idle keep-alive and any future streaming response.
#[derive(Clone)]
pub struct ConnectionLimit<A> {
    inner: A,
    slots: Arc<Semaphore>,
}

impl<A> ConnectionLimit<A> {
    pub fn new(inner: A) -> Self {
        Self {
            inner,
            slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        }
    }

    pub fn active_connections(&self) -> usize {
        MAX_CONNECTIONS - self.slots.available_permits()
    }
}

pub struct LimitedStream<T> {
    inner: T,
    _permit: OwnedSemaphorePermit,
}

impl<I, S, A> Accept<I, S> for ConnectionLimit<A>
where
    I: Send + 'static,
    S: Send + 'static,
    A: Accept<I, S> + Clone + Send + Sync + 'static,
    A::Future: Send + 'static,
    A::Stream: Send + 'static,
    A::Service: Send + 'static,
{
    type Stream = LimitedStream<A::Stream>;
    type Service = A::Service;
    type Future = Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let permit = self.slots.clone().try_acquire_owned();
        let inner = self.inner.clone();
        Box::pin(async move {
            let permit = permit.map_err(|_| {
                io::Error::new(io::ErrorKind::WouldBlock, "connection limit reached")
            })?;
            let (inner, service) = inner.accept(stream, service).await?;
            Ok((
                LimitedStream {
                    inner,
                    _permit: permit,
                },
                service,
            ))
        })
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for LimitedStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buffer)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for LimitedStream<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buffer)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// The foreground application and isolated TLS fixtures use the same parser,
/// handshake and connection limits. The application supplies its guarded router.
pub async fn serve_listener(
    listener: TcpListener,
    config: RustlsConfig,
    router: Router,
    handle: axum_server::Handle<SocketAddr>,
) -> io::Result<()> {
    let _shutdown = ShutdownOnDrop(handle.clone());
    listener.set_nonblocking(true)?;
    let acceptor = axum_server::tls_rustls::RustlsAcceptor::new(config);
    let mut server = axum_server::from_tcp(listener)?
        .acceptor(ConnectionLimit::new(acceptor))
        .http1_only()
        .handle(handle);
    server
        .http_builder()
        .http1()
        .max_headers(64)
        .max_buf_size(32768)
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(10));
    server
        .serve(router.into_make_service_with_connect_info::<SocketAddr>())
        .await
}

struct ShutdownOnDrop(axum_server::Handle<SocketAddr>);
impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Environment {
    #[default]
    Production,
    Development,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    #[serde(default)]
    environment: Environment,
    #[serde(default = "system_settings_read_only_by_default")]
    system_settings_read_only: bool,
    #[serde(default)]
    services: crate::application::services::ServiceConfiguration,
    database: PathBuf,
    listen: SocketAddr,
    origin: String,
    tenant_id: Option<String>,
    object_id: Option<String>,
    client_id: Option<String>,
    client_secret_file: Option<PathBuf>,
    certificate_file: PathBuf,
    private_key_file: PathBuf,
    browser_assets: Option<PathBuf>,
    local_acquisition: Option<super::worker::LocalAcquisition>,
}

fn system_settings_read_only_by_default() -> bool {
    true
}

/// Plaintext credentials exist only in memory after bounded, local-file reads.
/// Configuration contains file references, not secret values.
pub struct PreparedServer {
    configuration: Configuration,
    store: Store,
    secret: Option<ClientSecret>,
    tls: RustlsConfig,
    hostname: String,
}

impl PreparedServer {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = read_local(path, 16384, false)?;
        let mut configuration: Configuration = serde_json::from_slice(&bytes)
            .map_err(|_| Error::InvalidInput("service configuration JSON is invalid".into()))?;
        configuration.services.validate_builtins()?;
        match configuration.environment {
            Environment::Production => {
                if configuration.local_acquisition.is_some() {
                    return Err(Error::InvalidInput(
                        "local acquisition is development-only".into(),
                    ));
                }
                if ![
                    &configuration.tenant_id,
                    &configuration.object_id,
                    &configuration.client_id,
                ]
                .into_iter()
                .all(|value| {
                    value
                        .as_deref()
                        .is_some_and(crate::domain::identifiers::is_canonical_uuid)
                }) || configuration.client_secret_file.is_none()
                {
                    return Err(Error::InvalidInput(
                        "production requires canonical Entra IDs and a client secret file".into(),
                    ));
                }
            }

            Environment::Development => {
                if configuration.tenant_id.is_some()
                    || configuration.object_id.is_some()
                    || configuration.client_id.is_some()
                    || configuration.client_secret_file.is_some()
                {
                    return Err(Error::InvalidInput(
                        "development mode must not contain Entra configuration".into(),
                    ));
                }
            }
        }
        if !configuration.listen.ip().is_loopback() || configuration.listen.port() == 0 {
            return Err(Error::InvalidInput("API preview requires a loopback address and an explicit nonzero port; public operation is not enabled".into()));
        }
        let origin = reqwest::Url::parse(&configuration.origin)
            .map_err(|_| Error::InvalidInput("service origin is invalid".into()))?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.port_or_known_default() != Some(configuration.listen.port())
        {
            return Err(Error::InvalidInput("service origin must be HTTPS with the listener port and no path, credentials or query".into()));
        }
        configuration.origin = origin.origin().ascii_serialization();
        if matches!(configuration.environment, Environment::Development)
            && !Api::development_origin(&origin)
        {
            return Err(Error::InvalidInput(
                "development origin must use localhost or a literal loopback IP address".into(),
            ));
        }
        let hostname = origin
            .host_str()
            .ok_or_else(|| Error::InvalidInput("service hostname is missing".into()))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        for file in [
            &configuration.database,
            &configuration.certificate_file,
            &configuration.private_key_file,
        ]
        .into_iter()
        .chain(configuration.client_secret_file.iter())
        {
            if !file.is_absolute() {
                return Err(Error::InvalidInput(
                    "service file references must be absolute".into(),
                ));
            }
            require_local_disk(&std::fs::canonicalize(file)?)?;
        }
        let store = Store::open_existing(&configuration.database)?;
        let mut protected = vec![
            path.to_owned(),
            configuration.certificate_file.clone(),
            configuration.private_key_file.clone(),
        ];
        protected.extend(configuration.client_secret_file.iter().cloned());
        protected.extend(configuration.browser_assets.iter().cloned());
        if let Some(local) = &configuration.local_acquisition {
            local.validate(&store, &configuration.services)?;
            protected.push(local.credentials_file.clone());
        }
        let protected = protected
            .iter()
            .map(std::fs::canonicalize)
            .collect::<io::Result<Vec<_>>>()?;
        let mut after = 0;
        loop {
            let locations = store.locations(Page::new(after, 200)?)?;
            if locations.is_empty() {
                break;
            }
            for location in locations {
                after = location.id;
                if protected
                    .iter()
                    .any(|file| file.starts_with(&location.root))
                {
                    return Err(Error::InvalidInput("service configuration, secrets and TLS files must remain outside all media roots".into()));
                }
            }
        }
        let secret = configuration
            .client_secret_file
            .as_ref()
            .map(|path| -> Result<ClientSecret> {
                let secret = read_local(path, 4096, true)?;
                let secret = String::from_utf8(secret)
                    .map_err(|_| Error::InvalidInput("client secret file is not UTF-8".into()))?;
                let secret = secret.trim_end_matches(['\r', '\n']);
                if secret.trim().is_empty() || secret.chars().any(char::is_control) {
                    return Err(Error::InvalidInput(
                        "client secret file must contain one nonempty line".into(),
                    ));
                }
                Ok(ClientSecret::new(secret.into()))
            })
            .transpose()?;
        let tls = load_tls(&configuration, &hostname)?;
        Ok(Self {
            configuration,
            store,
            secret,
            tls,
            hostname,
        })
    }

    pub async fn run(self) -> Result<()> {
        let Self {
            configuration,
            store,
            secret,
            tls,
            hostname,
        } = self;
        let mut api = match configuration.environment {
            Environment::Development => Api::development_with_services(
                &configuration.origin,
                store,
                configuration.services.clone(),
            )?,
            Environment::Production => {
                let broker = EntraBroker::discover(
                    configuration
                        .tenant_id
                        .as_deref()
                        .ok_or(Error::Internal("validated tenant ID is missing"))?,
                    configuration
                        .object_id
                        .as_deref()
                        .ok_or(Error::Internal("validated object ID is missing"))?,
                    configuration
                        .client_id
                        .as_deref()
                        .ok_or(Error::Internal("validated client ID is missing"))?,
                    secret.ok_or(Error::Internal("validated client secret is missing"))?,
                    RedirectUrl::new(format!("{}/auth/callback", configuration.origin))
                        .map_err(|_| Error::InvalidInput("service callback is invalid".into()))?,
                    EntraHttpClient::new()?,
                )
                .await?;
                Api::new_with_services(
                    &configuration.origin,
                    store,
                    broker,
                    configuration.services.clone(),
                )?
            }
        };
        api = api.with_system_settings_read_only(configuration.system_settings_read_only)?;
        if let Some(root) = &configuration.browser_assets {
            let root = root.clone();
            let browser =
                tokio::task::spawn_blocking(move || super::browser::BrowserAssets::load(&root))
                    .await
                    .map_err(|_| Error::Internal("browser asset task failed"))??;
            api = api.with_browser(browser)?;
        }
        if let Some(local) = configuration.local_acquisition.clone() {
            let database = configuration.database.clone();
            let services = configuration.services.clone();
            let runtime = tokio::task::spawn_blocking(move || {
                super::local_runtime::LocalRuntime::new(database, local, services)
            })
            .await
            .map_err(|_| Error::Internal("local runtime task failed"))??;
            api = api.with_local_runtime(runtime)?;
        }
        let listener = TcpListener::bind(configuration.listen)?;
        let handle = axum_server::Handle::new();
        let serving = serve_listener(listener, tls.clone(), api.router(), handle.clone());
        tokio::pin!(serving);
        eprintln!(
            "{} API preview listening on {}",
            if matches!(configuration.environment, Environment::Development) {
                "WARNING: development _anonymous (authentication disabled)"
            } else {
                "Authenticated production"
            },
            configuration.listen,
        );
        let result = tokio::select! {
            result = &mut serving => result.map_err(Error::from),
            result = shutdown_signal() => {
                api.stop_local_operations();
                handle.graceful_shutdown(Some(Duration::from_secs(10)));
                let served = serving.await.map_err(Error::from);
                result.map_err(Error::from).and(served)
            },
            _ = api.refresh_identity() => Err(Error::Internal("identity refresh stopped unexpectedly")),
            _ = watch_tls(configuration, hostname, tls) => Err(Error::Internal("TLS refresh stopped unexpectedly")),
        };
        api.stop_local_operations();
        handle.shutdown();
        eprintln!("API stopped accepting work; waiting for local operations and saved results");
        api.drain_local_operations().await?;
        result
    }
}

async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            received = terminate.recv() => received.ok_or_else(|| io::Error::other("termination signal stream closed")),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

fn load_tls(configuration: &Configuration, hostname: &str) -> Result<RustlsConfig> {
    let (certificate, key) = tls_material(configuration)?;
    tls::configuration(hostname, &certificate, &key)
}

fn tls_material(configuration: &Configuration) -> Result<(Vec<u8>, Vec<u8>)> {
    Ok((
        read_local(&configuration.certificate_file, 262144, false)?,
        read_local(&configuration.private_key_file, 16384, true)?,
    ))
}

async fn watch_tls(configuration: Configuration, hostname: String, active: RustlsConfig) {
    let configuration = Arc::new(configuration);
    let mut timer = tokio::time::interval(Duration::from_secs(60));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    timer.tick().await;
    loop {
        timer.tick().await;
        let configuration = configuration.clone();
        let hostname = hostname.clone();
        let active = active.clone();
        match tokio::task::spawn_blocking(move || {
            let (certificate, key) = tls_material(&configuration)?;
            tls::reload(&active, &hostname, &certificate, &key)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(_)) => eprintln!("TLS reload rejected; the previous certificate remains active"),
            Err(_) => eprintln!("TLS reload task failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_server::accept::DefaultAcceptor;

    #[tokio::test]
    async fn connection_permits_cover_the_complete_stream_lifetime() -> io::Result<()> {
        let acceptor = ConnectionLimit::new(DefaultAcceptor);
        let mut streams = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            let (stream, _peer) = tokio::io::duplex(1);
            streams.push(acceptor.accept(stream, ()).await?.0);
        }
        assert_eq!(acceptor.active_connections(), 64);
        let (stream, _peer) = tokio::io::duplex(1);
        let rejected = acceptor.accept(stream, ()).await;
        assert!(matches!(rejected, Err(error) if error.kind() == io::ErrorKind::WouldBlock));
        streams.pop();
        assert_eq!(acceptor.active_connections(), 63);
        let (stream, _peer) = tokio::io::duplex(1);
        streams.push(acceptor.accept(stream, ()).await?.0);
        assert_eq!(acceptor.active_connections(), 64);
        drop(streams);
        assert_eq!(acceptor.active_connections(), 0);
        Ok(())
    }

    #[test]
    fn system_settings_read_only_defaults_to_true() -> std::result::Result<(), serde_json::Error> {
        let configuration: Configuration = serde_json::from_value(serde_json::json!({
            "database": "C:\\DomusDev\\state.db",
            "listen": "127.0.0.1:8443",
            "origin": "https://localhost:8443",
            "certificate_file": "C:\\DomusDev\\localhost.pem",
            "private_key_file": "C:\\DomusDev\\localhost-key.pem"
        }))?;
        assert!(configuration.system_settings_read_only);
        Ok(())
    }

    #[test]
    fn system_settings_read_only_can_be_disabled_by_the_operator()
    -> std::result::Result<(), serde_json::Error> {
        let configuration: Configuration = serde_json::from_value(serde_json::json!({
            "database": "C:\\DomusDev\\state.db",
            "listen": "127.0.0.1:8443",
            "origin": "https://localhost:8443",
            "certificate_file": "C:\\DomusDev\\localhost.pem",
            "private_key_file": "C:\\DomusDev\\localhost-key.pem",
            "system_settings_read_only": false
        }))?;
        assert!(!configuration.system_settings_read_only);
        Ok(())
    }
}
