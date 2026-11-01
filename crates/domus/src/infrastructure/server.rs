use super::api::Api;
use super::entra::EntraBroker;
use super::http::EntraHttpClient;
use super::store::Store;
use super::tls;
use crate::{Error, Result};
use axum::Router;
use axum_server::accept::Accept;
use axum_server::tls_rustls::RustlsConfig;
use hyper_util::rt::TokioTimer;
use openidconnect::{ClientSecret, RedirectUrl};
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
    configure_http_limits(server.http_builder());
    server
        .serve(router.into_make_service_with_connect_info::<SocketAddr>())
        .await
}

/// The anonymous application uses the same connection and HTTP parser limits,
/// without TLS. Both the bind and the guarded router require direct loopback.
pub async fn serve_plain_listener(
    listener: TcpListener,
    router: Router,
    handle: axum_server::Handle<SocketAddr>,
) -> io::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "anonymous HTTP requires loopback",
        ));
    }
    let _shutdown = ShutdownOnDrop(handle.clone());
    listener.set_nonblocking(true)?;
    let mut server = axum_server::from_tcp(listener)?
        .acceptor(ConnectionLimit::new(axum_server::accept::DefaultAcceptor))
        .http1_only()
        .handle(handle);
    configure_http_limits(server.http_builder());
    server
        .serve(router.into_make_service_with_connect_info::<SocketAddr>())
        .await
}

fn configure_http_limits(
    builder: &mut hyper_util::server::conn::auto::Builder<hyper_util::rt::TokioExecutor>,
) {
    builder
        .http1()
        .max_headers(64)
        .max_buf_size(32768)
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(10));
}

struct ShutdownOnDrop(axum_server::Handle<SocketAddr>);
impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

mod bootstrap;
pub mod configuration;
use configuration::{Configuration, Environment};

/// Plaintext credentials exist only in memory after bounded, local-file reads.
/// Configuration contains file references, not secret values.
pub struct PreparedServer {
    configuration: Configuration,
    store: Store,
    secret: Option<ClientSecret>,
    tls: Option<RustlsConfig>,
    hostname: String,
    configuration_source: Arc<configuration::ConfigurationService>,
}

impl PreparedServer {
    /// No configuration selects only the explicit loopback single-user mode.
    /// An explicitly supplied configuration never falls back after an error.
    pub fn local() -> Result<Self> {
        bootstrap::prepare()
    }

    pub fn load(path: &Path) -> Result<Self> {
        let (configuration_source, configuration, hostname) =
            configuration::ConfigurationService::load(path)?;
        Self::from_source(Arc::new(configuration_source), configuration, hostname)
    }

    fn from_source(
        configuration_source: Arc<configuration::ConfigurationService>,
        configuration: Configuration,
        hostname: String,
    ) -> Result<Self> {
        let mut store = Store::open_existing(&configuration.database)?;
        let protected = configuration_source.protected(&configuration);
        let (secret, tls) = configuration_source.startup_material(&store, &hostname)?;
        if let Some(cache) = &configuration.cache {
            let protected = protected.iter().map(PathBuf::as_path).collect::<Vec<_>>();
            let cache = cache.open(&mut store, &protected)?;
            store = store.with_cache(cache);
        }
        Ok(Self {
            configuration_source,
            configuration,
            store,
            secret,
            tls,
            hostname,
        })
    }

    async fn prepare(self) -> Result<Host> {
        self.prepare_with_certificate_guard(None).await
    }

    async fn prepare_with_certificate_guard(
        self,
        previous: Option<Arc<crate::infrastructure::certificates::HostGuard>>,
    ) -> Result<Host> {
        let Self {
            configuration,
            store,
            secret,
            tls,
            hostname,
            configuration_source,
        } = self;
        let certificate_guard = if configuration.tls_mode
            == crate::domain::certificates::TlsMode::AzureDns
            && tls.is_some()
        {
            let path = configuration
                .managed_tls
                .as_ref()
                .ok_or(Error::Internal("managed TLS configuration is missing"))?
                .state_directory
                .clone();
            Some(
                tokio::task::spawn_blocking(move || {
                    crate::infrastructure::certificates::HostGuard::acquire(&path, previous)
                })
                .await
                .map_err(|_| Error::Internal("certificate host gate task failed"))??,
            )
        } else {
            None
        };
        let cache = store.cache();
        let mut api = match configuration.environment {
            Environment::SingleUser => {
                Api::single_user(&configuration.origin, store, configuration.services.clone())?
            }
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
        api = api.with_shared_configuration(configuration_source.clone())?;
        if let Some(worker) = configuration.production_worker.clone() {
            api = api.with_production_worker(worker)?;
        }
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
                Ok::<_, Error>(
                    super::local_runtime::LocalRuntime::new(database, local, services)?
                        .with_cache(cache),
                )
            })
            .await
            .map_err(|_| Error::Internal("local runtime task failed"))??;
            api = api.with_local_runtime(runtime)?;
        }
        Ok(Host {
            configuration,
            api,
            tls,
            hostname,
            source: configuration_source,
            certificate_guard,
        })
    }

    pub async fn run(self) -> Result<()> {
        let (sender, reloads) = tokio::sync::mpsc::channel(1);
        self.configuration_source.attach_reload(sender)?;
        let host = self.prepare().await?;
        let mut renewals = renewal_owner(&host).await?;
        let result = Self::run_host(host, reloads, &mut renewals).await;
        if let Some(owner) = &mut renewals {
            owner.drain().await?;
        }
        result
    }

    async fn run_host(
        mut host: Host,
        mut reloads: tokio::sync::mpsc::Receiver<String>,
        renewals: &mut Option<crate::infrastructure::certificates::RenewalOwner>,
    ) -> Result<()> {
        let mut listener = TcpListener::bind(host.configuration.listen)?;
        let (mut handle, mut serving) = host.start(&listener).await?;
        let mut shutdown = ShutdownOnDrop(handle.clone());
        eprintln!(
            "{} API preview listening on {}",
            match host.configuration.environment {
                Environment::Development =>
                    "WARNING: development _anonymous (authentication disabled)",
                Environment::SingleUser => "Single-user _anonymous HTTP (loopback only)",
                Environment::Production => "Authenticated production",
            },
            host.configuration.listen,
        );
        let result = loop {
            enum Event {
                Reload(String),
                Stop(Result<()>),
            }
            let event = tokio::select! {
                result = &mut serving => Event::Stop(join_listener(result)),
                result = shutdown_signal() => Event::Stop(result.map_err(Error::from)),
                _ = host.api.refresh_identity() => Event::Stop(Err(Error::Internal("identity refresh stopped unexpectedly"))),
                _ = watch_tls(host.configuration.clone(), host.hostname.clone(), host.tls.clone()) => Event::Stop(Err(Error::Internal("TLS refresh stopped unexpectedly"))),
                request = reloads.recv() => match request {
                    Some(revision) => Event::Reload(revision),
                    None => Event::Stop(Err(Error::Internal("configuration reload channel closed"))),
                },
            };
            let revision = match event {
                Event::Stop(result) => break result,
                Event::Reload(revision) => revision,
            };
            if let Some(owner) = renewals {
                owner.drain().await?;
            }
            *renewals = None;
            let prepared = tokio::time::timeout(
                Duration::from_secs(30),
                host.replacement(&revision, &listener),
            )
            .await;
            let (candidate, candidate_listener, _idle) = match prepared {
                Ok(Ok(candidate)) => candidate,
                Ok(Err(_)) | Err(_) => {
                    eprintln!("API reload preparation failed; the previous host remains active");
                    host.source.finish_reload(
                        revision,
                        Err(Error::Internal("reload preparation failed")),
                    )?;
                    match renewal_owner(&host).await {
                        Ok(owner) => *renewals = owner,
                        Err(_) => eprintln!(
                            "Certificate renewal could not restart after rejected reload; the previous listener remains active. Stop the API before local recovery."
                        ),
                    }
                    continue;
                }
            };
            // Keep a bound copy of the old socket until the new generation is
            // listening. Failed handover can restore the immutable old host.
            host.api.stop_local_operations();
            drop(_idle);
            handle.graceful_shutdown(Some(Duration::from_secs(10)));
            if let Err(error) = join_listener((&mut serving).await) {
                eprintln!("API listener drain failed during reload: {error}");
            }
            host.api.drain_local_operations().await?;
            let source = host.source.clone();
            let expected = revision.clone();
            let verified = tokio::task::spawn_blocking(move || source.verify_reload(&expected))
                .await
                .unwrap_or(Err(Error::Internal("API activation revision task failed")));
            let started = match verified {
                Ok(()) => match renewal_owner(&candidate).await {
                    Ok(owner) => {
                        *renewals = owner;
                        candidate.start(&candidate_listener).await
                    }
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };
            match started {
                Ok((next_handle, next_serving)) => {
                    host = candidate;
                    listener = candidate_listener;
                    handle = next_handle;
                    serving = next_serving;
                    shutdown.0 = handle.clone();
                    host.source.finish_reload(revision, Ok(()))?;
                    eprintln!("API configuration reload activated");
                }
                Err(_) => {
                    if let Some(owner) = renewals {
                        owner.drain().await?;
                    }
                    *renewals = None;
                    eprintln!("API reload handover failed; restoring the previous host");
                    let source = host.source.clone();
                    let configuration = host.configuration.clone();
                    let hostname = host.hostname.clone();
                    let previous_guard = host.certificate_guard.clone();
                    host = tokio::task::spawn_blocking(move || {
                        Self::from_source(source, configuration, hostname)
                    })
                    .await
                    .map_err(|_| Error::Internal("API restoration task failed"))??
                    .prepare_with_certificate_guard(previous_guard)
                    .await?;
                    *renewals = renewal_owner(&host).await?;
                    (handle, serving) = host.start(&listener).await?;
                    shutdown.0 = handle.clone();
                    host.source
                        .finish_reload(revision, Err(Error::Internal("reload handover failed")))?;
                }
            }
        };
        host.api.stop_local_operations();
        handle.shutdown();
        eprintln!("API stopped accepting work; waiting for local operations and saved results");
        host.api.drain_local_operations().await?;
        result
    }
}

struct Host {
    configuration: Configuration,
    api: Api,
    tls: Option<RustlsConfig>,
    hostname: String,
    source: Arc<configuration::ConfigurationService>,
    certificate_guard: Option<Arc<crate::infrastructure::certificates::HostGuard>>,
}

type ListenerTask = tokio::task::JoinHandle<io::Result<()>>;

impl Host {
    async fn start(
        &self,
        listener: &TcpListener,
    ) -> Result<(axum_server::Handle<SocketAddr>, ListenerTask)> {
        let listener = listener.try_clone()?;
        listener.set_nonblocking(true)?;
        let handle = axum_server::Handle::new();
        let serving_handle = handle.clone();
        let tls = self.tls.clone();
        let router = self.api.router();
        let mut serving = tokio::spawn(async move {
            match tls {
                Some(tls) => serve_listener(listener, tls, router, serving_handle).await,
                None => serve_plain_listener(listener, router, serving_handle).await,
            }
        });
        tokio::select! {
            ready = handle.listening() => {
                if ready.is_none() {
                    handle.shutdown();
                    join_listener(serving.await)?;
                    return Err(Error::Internal("API listener did not become ready"));
                }
            },
            result = &mut serving => {
                join_listener(result)?;
                return Err(Error::Internal("API listener stopped before readiness"));
            },
        }
        Ok((handle, serving))
    }

    async fn replacement(
        &self,
        revision: &str,
        listener: &TcpListener,
    ) -> Result<(Host, TcpListener, super::api::ReloadPermit)> {
        let idle = self.api.reload_permit()?;
        let source = self.source.clone();
        let expected = revision.to_owned();
        // Blocking resource preparation can outlive its timeout. Keep media
        // operations excluded until that preparation actually finishes.
        let (prepared, idle) = tokio::task::spawn_blocking(move || {
            let candidate = source.reload_candidate(&expected)?;
            let configuration = candidate.active.clone();
            let hostname = reqwest::Url::parse(&configuration.origin)
                .map_err(|_| Error::InvalidInput("service origin is invalid".into()))?
                .host_str()
                .ok_or(Error::Internal("hostname is missing"))?
                .trim_matches(['[', ']'])
                .to_owned();
            let prepared =
                PreparedServer::from_source(Arc::new(candidate), configuration, hostname)?;
            Ok::<_, Error>((prepared, idle))
        })
        .await
        .map_err(|_| Error::Internal("API reload preparation task failed"))??;
        let candidate = prepared
            .prepare_with_certificate_guard(self.certificate_guard.clone())
            .await?;
        let listener = if candidate.configuration.listen == self.configuration.listen {
            listener.try_clone()?
        } else {
            TcpListener::bind(candidate.configuration.listen)?
        };
        listener.set_nonblocking(true)?;
        let source = self.source.clone();
        let expected = revision.to_owned();
        tokio::task::spawn_blocking(move || source.verify_reload(&expected))
            .await
            .map_err(|_| Error::Internal("API reload revision task failed"))??;
        Ok((candidate, listener, idle))
    }
}

fn join_listener(
    result: std::result::Result<io::Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    result
        .map_err(|_| Error::Internal("API listener task failed"))?
        .map_err(Error::from)
}

pub(crate) async fn shutdown_signal() -> io::Result<()> {
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

fn tls_material(configuration: &Configuration) -> Result<(Vec<u8>, Vec<u8>)> {
    configuration::ConfigurationService::tls_material(configuration)
}

fn load_tls(configuration: &Configuration, hostname: &str) -> Result<RustlsConfig> {
    if configuration.tls_mode == crate::domain::certificates::TlsMode::AzureDns {
        let managed = configuration
            .managed_tls
            .as_ref()
            .ok_or_else(|| Error::InvalidInput("managed TLS configuration is missing".into()))?;
        return crate::infrastructure::certificates::state::StateStore::load_active(
            &managed.state_directory,
            &managed.binding(hostname)?,
            hostname,
        )
        .map(|(_, _, configuration)| configuration);
    }
    let (certificate, key) = tls_material(configuration)?;
    tls::configuration(hostname, &certificate, &key)
}

async fn watch_tls(configuration: Configuration, hostname: String, active: Option<RustlsConfig>) {
    if configuration.tls_mode == crate::domain::certificates::TlsMode::AzureDns {
        std::future::pending::<()>().await;
        return;
    }
    let Some(active) = active else {
        std::future::pending::<()>().await;
        return;
    };
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

async fn renewal_owner(
    host: &Host,
) -> Result<Option<crate::infrastructure::certificates::RenewalOwner>> {
    let Some(tls) = host.tls.clone() else {
        return Ok(None);
    };
    if host.configuration.tls_mode != crate::domain::certificates::TlsMode::AzureDns {
        return Ok(None);
    }
    let managed = host
        .configuration
        .managed_tls
        .clone()
        .ok_or(Error::Internal("managed TLS configuration is missing"))?;
    let hostname = host.hostname.clone();
    let database = host.configuration.database.clone();
    let selection = host.source.certificate_selection();
    let recovery = host.source.certificate_recovery();
    let runtime_recovery = recovery.clone();
    let started = tokio::task::spawn_blocking(move || {
        crate::infrastructure::certificates::RenewalOwner::start(
            managed, hostname, database, tls, selection, recovery,
        )
    })
    .await
    .map_err(|_| Error::Internal("certificate owner start task failed"));
    match started {
        Ok(Ok(owner)) => Ok(Some(owner)),
        Ok(Err(error)) | Err(error) => {
            runtime_recovery.store(true, std::sync::atomic::Ordering::Release);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_server::accept::DefaultAcceptor;

    #[tokio::test]
    async fn single_user_http_serves_guarded_anonymous_api_without_tls()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = crate::infrastructure::permissions::private_test_directory()?;
        let store = Store::open(&directory.path().join("state.db"))?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let origin = format!("http://{}", listener.local_addr()?);
        let api = Api::single_user(&origin, store, Default::default())?;
        let handle = axum_server::Handle::new();
        let serving_handle = handle.clone();
        let serving = tokio::spawn(serve_plain_listener(listener, api.router(), serving_handle));
        assert!(handle.listening().await.is_some());
        let client = reqwest::Client::builder().no_proxy().build()?;
        let response = client.get(format!("{origin}/api/session")).send().await?;
        assert!(response.status().is_success());
        assert!(!response.headers().contains_key("strict-transport-security"));
        assert!(!response.headers().contains_key("set-cookie"));
        let body: serde_json::Value = response.json().await?;
        assert!(body.to_string().contains("_anonymous"));
        let rejected = client
            .get(format!("{origin}/api/session"))
            .header("Origin", "http://other.invalid")
            .send()
            .await?;
        assert_eq!(rejected.status(), 403);
        let rejected = client
            .get(format!("{origin}/api/session"))
            .header("Host", "other.invalid")
            .send()
            .await?;
        assert_eq!(rejected.status(), 421);
        let rejected = client
            .post(format!("{origin}/api/jobs"))
            .json(&serde_json::json!({}))
            .send()
            .await?;
        assert_eq!(rejected.status(), 403);
        handle.shutdown();
        serving.await??;
        Ok(())
    }

    #[test]
    fn single_user_http_cannot_be_selected_by_missing_production_credentials()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = crate::infrastructure::permissions::private_test_directory()?;
        let file = directory.path().join("service.json");
        let database = directory.path().join("state.db");
        Store::open(&database)?;
        std::fs::write(
            &file,
            serde_json::to_vec(&serde_json::json!({
                "database":database, "origin":"http://127.0.0.1:8080", "listen":"127.0.0.1:8080"
            }))?,
        )?;
        assert!(PreparedServer::load(&file).is_err());
        assert!(PreparedServer::load(&directory.path().join("absent.json")).is_err());
        Ok(())
    }

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
