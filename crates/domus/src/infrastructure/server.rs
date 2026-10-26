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

pub mod configuration;
use configuration::{Configuration, Environment};

/// Plaintext credentials exist only in memory after bounded, local-file reads.
/// Configuration contains file references, not secret values.
pub struct PreparedServer {
    configuration: Configuration,
    store: Store,
    secret: Option<ClientSecret>,
    tls: RustlsConfig,
    hostname: String,
    configuration_source: configuration::ConfigurationService,
}

impl PreparedServer {
    pub fn load(path: &Path) -> Result<Self> {
        let (configuration_source, configuration, hostname) =
            configuration::ConfigurationService::load(path)?;
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

    pub async fn run(self) -> Result<()> {
        let Self {
            configuration,
            store,
            secret,
            tls,
            hostname,
            configuration_source,
        } = self;
        let cache = store.cache();
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
        api = api.with_backend_configuration(configuration_source)?;
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
    configuration::ConfigurationService::tls_material(configuration)
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
