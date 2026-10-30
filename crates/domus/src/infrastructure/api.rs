use super::catalog::{CatalogQuery, CatalogResult, MusicBrainzClient};
use super::entra::EntraBroker;
use super::identity::LoginChallenges;
use super::store::{
    ArtworkCandidate, ArtworkDecision, CatalogStatus, Entry, JobControl, JobControlAction,
    JobEvent, JobSummary, Location, PlanRequest, QuestionDetails, SettingsVersion, Store,
    StoredPlan, StoredQuestion, TransferFile,
};
pub use crate::application::catalog::CatalogPlanRequest;
use crate::domain::{
    CatalogArtist, CatalogPage, CatalogRelease, CatalogReleaseFilters, CatalogReleasePage,
};
use crate::domain::{LibraryPath, OutputSettings, Page};
use crate::{Error, Result};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};

const SESSION_COOKIE: &str = "__Host-domus-session";
mod library;
use library::*;
pub use library::{LibraryRefresh, RefreshLibraryFolder};
#[cfg(test)]
mod sharing_tests;
#[cfg(test)]
mod worker_tests;
const LOGIN_COOKIE: &str = "__Host-domus-login";

#[derive(Clone)]
pub struct Api(Arc<Shared>);

struct Shared {
    browser: Option<super::browser::BrowserAssets>,
    local: Option<super::local_runtime::LocalRuntime>,
    production_worker: Option<super::production_worker::WorkerConfiguration>,
    services: super::services::ServiceHost,
    service_configuration: crate::application::services::ServiceConfiguration,
    system_settings_read_only: bool,
    backend_configuration: Option<Arc<super::server::configuration::ConfigurationService>>,
    request_slots: Semaphore,
    event_slots: Arc<Semaphore>,
    broker: Option<RwLock<Arc<EntraBroker>>>,
    pending: Mutex<LoginChallenges>,
    login_times: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
    origin: String,
    authority: String,
}

#[derive(Debug)]
pub struct ApiError(StatusCode, String, Option<u32>);

type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Serialize, ts_rs::TS)]
pub struct ApiMessage {
    pub error: String,
}

#[derive(Serialize, ts_rs::TS)]
pub struct EventCursor {
    pub after: i64,
}

#[derive(Serialize, ts_rs::TS)]
pub struct SystemSettingsPolicy {
    pub read_only: bool,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self(status, message.into(), None)
    }
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        match error {
            Error::Authentication => Self::new(StatusCode::UNAUTHORIZED, "authentication required"),
            Error::InvalidInput(message) => Self::new(StatusCode::BAD_REQUEST, message),
            Error::ArtworkInput(issue) => Self::new(StatusCode::BAD_REQUEST, issue.to_string()),
            Error::ArtworkRequired(id) => Self::new(
                StatusCode::CONFLICT,
                format!("artwork needs user input; question {id} is saved"),
            ),
            Error::JobStopped(id) => Self::new(
                StatusCode::CONFLICT,
                format!("job stopped at a safe boundary; inspect job control {id}"),
            ),
            Error::Conflict(message) => Self::new(StatusCode::CONFLICT, message),
            Error::Busy(message) => Self::new(StatusCode::SERVICE_UNAVAILABLE, message),
            Error::WorkerStopping => {
                Self::new(StatusCode::SERVICE_UNAVAILABLE, "worker is stopping")
            }
            Error::WorkerWaiting => Self::new(
                StatusCode::CONFLICT,
                "worker request is waiting for an explicit decision",
            ),
            Error::Database(rusqlite::Error::QueryReturnedNoRows) => {
                Self::new(StatusCode::NOT_FOUND, "record not found")
            }
            Error::Remote(message) => {
                eprintln!("{message}");
                Self::new(StatusCode::BAD_GATEWAY, "remote service is unavailable")
            }
            Error::Catalog {
                reason,
                retry_after_seconds,
            } => {
                eprintln!("{reason}");
                Self(
                    StatusCode::SERVICE_UNAVAILABLE,
                    reason.into(),
                    Some(retry_after_seconds),
                )
            }
            Error::Internal(message) => {
                eprintln!("{message}");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "server operation failed")
            }
            Error::SoulseekTransport(_) => {
                Self::new(StatusCode::BAD_GATEWAY, "Soulseek connection failed")
            }
            Error::Database(_) | Error::Io(_) | Error::Json(_) => {
                eprintln!("API storage operation failed");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "storage operation failed",
                )
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.0, Json(ApiMessage { error: self.1 })).into_response();
        if let Some(seconds) = self.2 {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

impl Api {
    /// The caller must serve this router through validated direct TLS. No
    /// request can supply an identity verifier, issuer or callback destination.
    pub fn new(origin: &str, store: Store, broker: EntraBroker) -> Result<Self> {
        Self::new_with_catalog(origin, store, broker, MusicBrainzClient::new()?)
    }

    pub fn new_with_catalog(
        origin: &str,
        store: Store,
        broker: EntraBroker,
        catalog: MusicBrainzClient,
    ) -> Result<Self> {
        Self::build(origin, store, Some(broker), catalog)
    }

    /// Explicit local-only development identity. Never infer this mode from
    /// missing credentials, request headers or a failed production login.
    pub fn development(origin: &str, store: Store) -> Result<Self> {
        Self::build(origin, store, None, MusicBrainzClient::new()?)
    }

    pub fn new_with_services(
        origin: &str,
        store: Store,
        broker: EntraBroker,
        configuration: crate::application::services::ServiceConfiguration,
    ) -> Result<Self> {
        Self::build_services(
            origin,
            store,
            Some(broker),
            MusicBrainzClient::new()?,
            configuration,
        )
    }

    pub fn development_with_services(
        origin: &str,
        store: Store,
        configuration: crate::application::services::ServiceConfiguration,
    ) -> Result<Self> {
        Self::build_services(
            origin,
            store,
            None,
            MusicBrainzClient::new()?,
            configuration,
        )
    }

    fn build(
        origin: &str,
        store: Store,
        broker: Option<EntraBroker>,
        catalog: MusicBrainzClient,
    ) -> Result<Self> {
        Self::build_services(origin, store, broker, catalog, Default::default())
    }

    fn build_services(
        origin: &str,
        store: Store,
        broker: Option<EntraBroker>,
        catalog: MusicBrainzClient,
        configuration: crate::application::services::ServiceConfiguration,
    ) -> Result<Self> {
        let url = reqwest::Url::parse(origin)
            .map_err(|_| Error::InvalidInput("public origin is invalid".into()))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::InvalidInput(
                "API origin must be HTTPS without a path, credentials, query or fragment".into(),
            ));
        }
        if broker
            .as_ref()
            .is_some_and(|broker| url.origin() != broker.callback().url().origin())
        {
            return Err(Error::InvalidInput(
                "public origin must match the HTTPS Entra callback origin".into(),
            ));
        }
        if broker.is_none() && !Self::development_origin(&url) {
            return Err(Error::InvalidInput(
                "development origin must use localhost or a literal loopback IP address".into(),
            ));
        }
        let origin = url.origin().ascii_serialization();
        let authority = origin
            .strip_prefix("https://")
            .ok_or_else(|| Error::InvalidInput("public origin must use HTTPS".into()))?
            .into();
        Ok(Self(Arc::new(Shared {
            browser: None,
            local: None,
            production_worker: None,
            service_configuration: configuration.clone(),
            services: super::services::ServiceHost::with_catalog(store, configuration, catalog)?,
            system_settings_read_only: false,
            backend_configuration: None,
            request_slots: Semaphore::new(32),
            event_slots: Arc::new(Semaphore::new(4)),
            broker: broker.map(|broker| RwLock::new(Arc::new(broker))),
            pending: Mutex::new(LoginChallenges::default()),
            login_times: Mutex::new(HashMap::new()),
            origin,
            authority,
        })))
    }

    pub fn router(&self) -> Router {
        let protected = Router::new()
            .route("/", get(browser_index))
            .route("/assets/{name}", get(browser_asset))
            .route("/api/local/status", get(local_status))
            .route("/api/sharing/folders", get(inspect_share_folder))
            .route("/api/sharing/bindings", get(share_bindings))
            .route(
                "/api/sharing/folders/bind",
                post(bind_share_folder).layer(DefaultBodyLimit::max(16_384)),
            )
            .route("/api/worker/requests", get(worker_requests))
            .route("/api/worker/requests/{id}", get(worker_request))
            .route(
                "/api/worker/requests/{id}/retry",
                post(worker_retry).layer(DefaultBodyLimit::max(64)),
            )
            .route(
                "/api/worker/requests/{id}/dismiss",
                post(worker_dismiss).layer(DefaultBodyLimit::max(64)),
            )
            .route(
                "/api/local/source-search",
                post(local_search).layer(DefaultBodyLimit::max(512)),
            )
            .route(
                "/api/local/source-folder",
                post(local_folder).layer(DefaultBodyLimit::max(8192)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/run",
                post(local_run).layer(DefaultBodyLimit::max(512)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/publication/resume",
                post(local_publication_resume).layer(DefaultBodyLimit::max(512)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/publication/replacement-review",
                get(publication_replacement_review),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/publication/replacement-review/{location}",
                post(record_publication_replacement_decision).layer(DefaultBodyLimit::max(1024)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/publication/replacement/{location}/restore",
                post(restore_publication_replacement).layer(DefaultBodyLimit::max(1024)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork/local",
                post(upload_artwork).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
            )
            .route("/api/session", get(session))
            .route("/auth/logout", post(logout))
            .route(
                "/api/locations",
                get(locations).merge(post(register_location).layer(DefaultBodyLimit::max(16384))),
            )
            .route("/api/locations/{id}", get(location))
            .route(
                "/api/locations/{id}/availability",
                get(location_availability),
            )
            .route(
                "/api/locations/{id}/scan",
                post(scan_location).layer(DefaultBodyLimit::max(1024)),
            )
            .route("/api/locations/{id}/entries", get(entries))
            .route("/api/locations/{id}/library/artists", get(library_artists))
            .route("/api/locations/{id}/library/albums", get(library_albums))
            .route(
                "/api/locations/{id}/library/albums/{album}",
                get(library_album),
            )
            .route(
                "/api/locations/{id}/library/albums/{album}/files",
                get(library_album_files),
            )
            .route(
                "/api/locations/{id}/library/albums/{album}/assets",
                get(library_album_assets),
            )
            .route(
                "/api/locations/{id}/library/files/{entry}",
                get(library_file),
            )
            .route(
                "/api/locations/{id}/library/files/{entry}/preview",
                get(library_preview),
            )
            .route(
                "/api/locations/{id}/library/coverage",
                get(library_coverage),
            )
            .route(
                "/api/locations/{id}/library/matches",
                post(library_matches).layer(DefaultBodyLimit::max(131072)),
            )
            .route(
                "/api/locations/{id}/library/refresh",
                post(library_refresh).layer(DefaultBodyLimit::max(16384)),
            )
            .route(
                "/api/settings",
                get(settings).merge(put(save_settings).layer(DefaultBodyLimit::max(16384))),
            )
            .route("/api/system/settings-policy", get(settings_policy))
            .route("/api/system/configuration", get(backend_configuration))
            .route(
                "/api/system/configuration/validate",
                post(validate_backend_configuration).layer(DefaultBodyLimit::max(16384)),
            )
            .route(
                "/api/system/configuration/save",
                post(save_backend_configuration).layer(DefaultBodyLimit::max(16384)),
            )
            .route(
                "/api/system/configuration/sections/validate",
                post(validate_configuration_sections).layer(DefaultBodyLimit::max(16384)),
            )
            .route(
                "/api/system/configuration/sections/save",
                post(save_configuration_sections).layer(DefaultBodyLimit::max(16384)),
            )
            .route(
                "/api/system/configuration/reload",
                post(reload_configuration).layer(DefaultBodyLimit::max(1024)),
            )
            .route("/api/jobs", get(jobs).post(create_job))
            .route("/api/queue", get(queue))
            .route(
                "/api/queue/dismiss",
                post(dismiss_finished).layer(DefaultBodyLimit::max(8192)),
            )
            .route("/api/plans", post(prepare_plan))
            .route("/api/jobs/{id}/plans/{version}", get(plan))
            .route("/api/jobs/{id}/plans/{version}/transfers", get(transfers))
            .route(
                "/api/jobs/{id}/plans/{version}/transfers/summary",
                get(transfer_summary),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/source-candidates",
                get(source_candidates)
                    .post(prepare_source_candidate)
                    .layer(DefaultBodyLimit::max(1_048_576)),
            )
            .route("/api/source-candidates/{id}", get(source_candidate))
            .route(
                "/api/source-candidates/{id}/approve",
                post(approve_source_candidate).layer(DefaultBodyLimit::max(512)),
            )
            .route("/api/jobs/{id}/plans/{version}/approve", post(approve_plan))
            .route(
                "/api/jobs/{id}/plans/{version}/controls",
                get(job_controls)
                    .post(request_job_control)
                    .layer(DefaultBodyLimit::max(128)),
            )
            .route("/api/job-controls/{id}", get(job_control))
            .route(
                "/api/job-controls/{id}/continue",
                post(continue_job_control).layer(DefaultBodyLimit::max(128)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork",
                get(artwork_candidates),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork/{candidate}/image",
                get(artwork_image),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork/{candidate}/evidence",
                get(artwork_evidence),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork/caa",
                post(prepare_caa_artwork).layer(DefaultBodyLimit::max(1024)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork/caa/search",
                post(search_caa_artwork).layer(DefaultBodyLimit::max(128)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork/caa/download",
                post(download_caa_artwork).layer(DefaultBodyLimit::max(1024)),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork/approve",
                post(approve_artwork).layer(DefaultBodyLimit::max(512)),
            )
            .route("/api/questions", get(questions))
            .route("/api/inbox", get(inbox))
            .route("/api/questions/{id}", get(question_details))
            .route("/api/jobs/{id}/plans/{version}/decision", get(job_decision))
            .route(
                "/api/jobs/{id}/plans/{version}/publication",
                get(publication_status),
            )
            .route(
                "/api/jobs/{id}/plans/{version}/artwork/check",
                post(check_artwork),
            )
            .route("/api/questions/{id}/artwork", get(artwork_decision))
            .route(
                "/api/questions/{id}/artwork/resume",
                post(resume_artwork).layer(DefaultBodyLimit::max(128)),
            )
            .route("/api/questions/{id}/answer", post(answer))
            .route("/api/events", get(events))
            .route("/api/jobs/{id}/plans/{version}/events", get(plan_events))
            .route("/api/events/cursor", get(event_cursor))
            .route("/api/events/stream", get(event_stream))
            .route("/api/discovery/artists", get(search_artists))
            .route("/api/discovery/artists/{id}", get(artist_details))
            .route("/api/discovery/releases", get(search_albums))
            .route("/api/discovery/plans", post(prepare_catalog_plans))
            .route("/api/discovery/status", get(catalog_status))
            .route("/api/discovery/resume", post(resume_catalog))
            .route("/api/discovery/artists/{id}/releases", get(artist_releases))
            .route(
                "/api/discovery/release-groups/{id}/releases",
                get(group_releases),
            )
            .route("/api/discovery/releases/{id}", get(release_details))
            .route(
                "/api/discovery/releases/{id}/plans",
                post(prepare_catalog_plan),
            )
            .route_layer(middleware::from_fn_with_state(self.clone(), authenticated));
        Router::new()
            .route("/auth/login", get(login))
            .route("/auth/callback", get(callback))
            .merge(protected)
            .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
            .layer(middleware::from_fn_with_state(self.clone(), security))
            .with_state(self.clone())
    }

    pub fn with_browser(mut self, browser: super::browser::BrowserAssets) -> Result<Self> {
        Arc::get_mut(&mut self.0)
            .ok_or(Error::Internal("API already shared"))?
            .browser = Some(browser);
        Ok(self)
    }

    pub fn with_system_settings_read_only(mut self, read_only: bool) -> Result<Self> {
        Arc::get_mut(&mut self.0)
            .ok_or(Error::Internal("API already shared"))?
            .system_settings_read_only = read_only;
        Ok(self)
    }

    #[cfg(test)]
    pub(crate) fn with_backend_configuration(
        self,
        configuration: super::server::configuration::ConfigurationService,
    ) -> Result<Self> {
        self.with_shared_configuration(Arc::new(configuration))
    }

    pub(crate) fn with_shared_configuration(
        mut self,
        configuration: Arc<super::server::configuration::ConfigurationService>,
    ) -> Result<Self> {
        Arc::get_mut(&mut self.0)
            .ok_or(Error::Internal(
                "API configuration must finish before sharing",
            ))?
            .backend_configuration = Some(configuration);
        Ok(self)
    }

    /// Only explicit anonymous development composition can enable native local
    /// acquisition. Missing production credentials never activate these commands.
    pub fn with_local_runtime(mut self, local: super::local_runtime::LocalRuntime) -> Result<Self> {
        if self.0.broker.is_some() {
            return Err(Error::InvalidInput(
                "local acquisition is development-only".into(),
            ));
        }

        Arc::get_mut(&mut self.0)
            .ok_or(Error::Internal("API already shared"))?
            .local = Some(local);
        Ok(self)
    }

    pub(crate) fn with_production_worker(
        mut self,
        configuration: super::production_worker::WorkerConfiguration,
    ) -> Result<Self> {
        let shared = Arc::get_mut(&mut self.0).ok_or(Error::Internal("API is already shared"))?;
        if shared.local.is_some() {
            return Err(Error::Conflict(
                "local and production workers are mutually exclusive".into(),
            ));
        }
        shared.production_worker = Some(configuration);
        Ok(self)
    }

    pub(crate) fn stop_local_operations(&self) {
        if let Some(local) = &self.0.local {
            local.stop_accepting();
        }
    }

    pub(crate) async fn drain_local_operations(&self) -> Result<()> {
        if let Some(local) = &self.0.local {
            local.drain().await?;
        }
        Ok(())
    }

    pub(crate) fn reload_permit(&self) -> Result<Option<tokio::sync::OwnedSemaphorePermit>> {
        self.0
            .local
            .as_ref()
            .map(|local| local.reload_permit())
            .transpose()
    }

    fn local(&self) -> ApiResult<&super::local_runtime::LocalRuntime> {
        self.0.local.as_ref().ok_or_else(|| {
            ApiError::new(StatusCode::CONFLICT, "local acquisition is not configured")
        })
    }

    /// Run this supervised refresh loop beside the HTTPS listener. Failure is
    /// explicit; cached keys cannot authorize a new login after one hour.
    pub async fn refresh_identity(&self) {
        let Some(broker) = &self.0.broker else {
            std::future::pending::<()>().await;
            return;
        };
        let mut timer = tokio::time::interval(Duration::from_secs(600));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        timer.tick().await;
        loop {
            timer.tick().await;
            let current = broker.read().await.clone();
            match current.refresh().await {
                Ok(updated) => *broker.write().await = Arc::new(updated),
                Err(_) => eprintln!(
                    "Entra metadata refresh failed; new sign-in stops when cached keys expire"
                ),
            }
        }
    }

    async fn database<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Store) -> Result<T> + Send + 'static,
    ) -> ApiResult<T> {
        self.0
            .services
            .database
            .run(move |store| match operation(store) {
                Err(Error::ArtworkRequired(id)) => {
                    let decision = store.artwork_decision(id)?;
                    Err(Error::Conflict(format!(
                        "artwork needs user input; question {id} is saved: {}",
                        decision.issue
                    )))
                }
                result => result,
            })
            .await
            .map_err(ApiError::from)
    }

    async fn identity(&self, headers: &HeaderMap) -> ApiResult<super::store::SessionIdentity> {
        let Some(broker) = &self.0.broker else {
            return Ok(super::store::SessionIdentity {
                tenant_id: "_development".into(),
                object_id: "_anonymous".into(),
                client_id: "_development".into(),
                expires_at: 253_402_300_799,
            });
        };
        let credential = cookie_value(headers, SESSION_COOKIE)?.ok_or(Error::Authentication)?;
        let broker = broker.read().await.clone();
        self.database(move |store| {
            store
                .session(&credential, broker.verifier())?
                .ok_or(Error::Authentication)
        })
        .await
    }

    pub(crate) fn development_origin(url: &reqwest::Url) -> bool {
        url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
    }

    async fn catalog<T: Send + 'static>(
        &self,
        query: CatalogQuery,
        parse: fn(&CatalogQuery, &[u8]) -> Result<T>,
    ) -> ApiResult<CatalogResult<T>> {
        self.0
            .services
            .musicbrainz
            .query(query, parse)
            .await
            .map_err(ApiError::from)
    }
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(default, deny_unknown_fields)]
pub struct ArtistSearch {
    pub query: String,
    pub offset: u32,
    pub limit: u16,
}

impl Default for ArtistSearch {
    fn default() -> Self {
        Self {
            query: String::new(),
            offset: 0,
            limit: 25,
        }
    }
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogPageQuery {
    pub offset: u32,
    pub limit: u16,
    pub query: String,
    pub artist_id: Option<String>,
    pub include_singles: bool,
    pub include_live: bool,
    pub include_compilations: bool,
    pub include_unofficial: bool,
}

impl Default for CatalogPageQuery {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 25,
            query: String::new(),
            artist_id: None,
            include_singles: false,
            include_live: false,
            include_compilations: false,
            include_unofficial: false,
        }
    }
}

impl CatalogPageQuery {
    fn filters(&self) -> CatalogReleaseFilters {
        CatalogReleaseFilters {
            include_singles: self.include_singles,
            include_live: self.include_live,
            include_compilations: self.include_compilations,
            include_unofficial: self.include_unofficial,
        }
    }
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct CatalogPlanSelection {
    pub release_id: String,
    pub request: CatalogPlanRequest,
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct CatalogBatchPlanRequest {
    pub albums: Vec<CatalogPlanSelection>,
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct CatalogResumeRequest {
    pub acknowledge_pause: bool,
}

async fn catalog_status(State(api): State<Api>) -> ApiResult<Json<CatalogStatus>> {
    Ok(Json(api.database(|store| store.catalog_status()).await?))
}

async fn resume_catalog(
    State(api): State<Api>,
    Json(request): Json<CatalogResumeRequest>,
) -> ApiResult<StatusCode> {
    api.database(move |store| store.resume_catalog_requests(request.acknowledge_pause))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn search_artists(
    State(api): State<Api>,
    Query(search): Query<ArtistSearch>,
) -> ApiResult<Json<CatalogResult<CatalogPage<CatalogArtist>>>> {
    let query = CatalogQuery::artists(&search.query, search.offset, search.limit)?;
    Ok(Json(api.catalog(query, CatalogQuery::parse_artists).await?))
}

async fn artist_releases(
    State(api): State<Api>,
    Path(id): Path<String>,
    Query(page): Query<CatalogPageQuery>,
) -> ApiResult<Json<CatalogResult<CatalogReleasePage>>> {
    if page.artist_id.as_ref().is_some_and(|artist| artist != &id) {
        return Err(
            Error::InvalidInput("artist filter differs from the requested artist".into()).into(),
        );
    }
    let query = if page.query.is_empty() {
        CatalogQuery::filtered_releases(&id, page.offset, page.limit, page.filters())?
    } else {
        CatalogQuery::albums(
            &page.query,
            Some(&id),
            page.offset,
            page.limit,
            page.filters(),
        )?
    };
    Ok(Json(
        api.catalog(query, CatalogQuery::parse_releases).await?,
    ))
}

async fn artist_details(
    State(api): State<Api>,
    Path(id): Path<String>,
) -> ApiResult<Json<CatalogResult<CatalogArtist>>> {
    Ok(Json(
        api.catalog(CatalogQuery::artist(&id)?, CatalogQuery::parse_artist)
            .await?,
    ))
}

async fn group_releases(
    State(api): State<Api>,
    Path(id): Path<String>,
    Query(page): Query<CatalogPageQuery>,
) -> ApiResult<Json<CatalogResult<CatalogReleasePage>>> {
    if !page.query.is_empty() || page.artist_id.is_some() {
        return Err(Error::InvalidInput(
            "edition browsing accepts a release group, not search text or an artist filter".into(),
        )
        .into());
    }
    Ok(Json(
        api.catalog(
            CatalogQuery::group_releases(&id, page.offset, page.limit, page.filters())?,
            CatalogQuery::parse_releases,
        )
        .await?,
    ))
}

async fn search_albums(
    State(api): State<Api>,
    Query(page): Query<CatalogPageQuery>,
) -> ApiResult<Json<CatalogResult<CatalogReleasePage>>> {
    let query = CatalogQuery::albums(
        &page.query,
        page.artist_id.as_deref(),
        page.offset,
        page.limit,
        page.filters(),
    )?;
    Ok(Json(
        api.catalog(query, CatalogQuery::parse_releases).await?,
    ))
}

async fn release_details(
    State(api): State<Api>,
    Path(id): Path<String>,
) -> ApiResult<Json<CatalogResult<CatalogRelease>>> {
    Ok(Json(
        api.catalog(CatalogQuery::release(&id)?, CatalogQuery::parse_release)
            .await?,
    ))
}

async fn prepare_catalog_plan(
    State(api): State<Api>,
    Path(id): Path<String>,
    Json(request): Json<CatalogPlanRequest>,
) -> ApiResult<Json<StoredPlan>> {
    let catalog = api
        .catalog(CatalogQuery::release(&id)?, CatalogQuery::parse_release)
        .await?;
    let plan = api
        .database(move |store| {
            store.prepare_catalog_plan(request.into_plan_request(&catalog)?, catalog)
        })
        .await?;
    Ok(Json(plan))
}

async fn prepare_catalog_plans(
    State(api): State<Api>,
    Json(request): Json<CatalogBatchPlanRequest>,
) -> ApiResult<Json<Vec<StoredPlan>>> {
    if request.albums.is_empty() || request.albums.len() > 25 {
        return Err(Error::InvalidInput("a plan batch needs 1 through 25 albums".into()).into());
    }
    for album in &request.albums {
        api.0
            .services
            .catalog
            .require_metadata(&crate::application::services::EntityRef {
                provider: "musicbrainz".into(),
                id: album.release_id.clone(),
            })?;
    }
    let plans = api
        .database(move |store| {
            let requests = request
                .albums
                .into_iter()
                .map(|album| {
                    let query = CatalogQuery::release(&album.release_id)?;
                    let cache = store.catalog_cache(&query)?.ok_or_else(|| {
                        Error::Conflict(
                            "load every selected release before preparing a plan batch".into(),
                        )
                    })?;
                    let catalog = CatalogResult {
                        source: query.source().into(),
                        fetched_at: cache.fetched_at,
                        cached: true,
                        payload_sha256: cache.payload_sha256,
                        data: query.parse_release(&cache.body)?,
                    };
                    Ok((album.request.into_plan_request(&catalog)?, catalog))
                })
                .collect::<Result<Vec<_>>>()?;
            store.prepare_catalog_plans(requests)
        })
        .await?;
    Ok(Json(plans))
}

async fn security(State(api): State<Api>, request: Request, next: Next) -> Response {
    let timeout = if matches!(
        request.uri().path(),
        "/api/local/source-search" | "/api/local/source-folder"
    ) {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(25)
    };
    let mut response = match check_request(&api, &request) {
        Err(error) => error.into_response(),
        Ok(()) => match api.0.request_slots.try_acquire() {
            Err(_) => ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "server is busy; retry later",
            )
            .into_response(),
            Ok(_permit) => match tokio::time::timeout(timeout, next.run(request)).await {
                Ok(response) => response,
                Err(_) => ApiError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "request timed out; reload current state",
                )
                .into_response(),
            },
        },
    };
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        if response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some("application/json")
        {
            let message = match status {
                StatusCode::NOT_FOUND => "route not found",
                StatusCode::METHOD_NOT_ALLOWED => "method is not allowed",
                StatusCode::PAYLOAD_TOO_LARGE => "request body exceeds its size limit",
                StatusCode::UNSUPPORTED_MEDIA_TYPE => "JSON content type is required",
                StatusCode::UNPROCESSABLE_ENTITY => "request fields are invalid",
                _ => "request could not be processed",
            };
            let allow = response.headers().get(header::ALLOW).cloned();
            response = ApiError::new(status, message).into_response();
            if let Some(allow) = allow {
                response.headers_mut().insert(header::ALLOW, allow);
            }
        }
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
    let headers = response.headers_mut();
    for (name, value) in [
        ("cache-control", "no-store"),
        ("strict-transport-security", "max-age=31536000"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; media-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

fn check_request(api: &Api, request: &Request) -> ApiResult<()> {
    if api.0.broker.is_none()
        && !request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .is_some_and(|peer| peer.0.ip().is_loopback())
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "development access requires a direct loopback connection",
        ));
    }
    let headers = request.headers();
    if api.0.broker.is_none()
        && (headers
            .get_all(header::ORIGIN)
            .iter()
            .any(|origin| origin != api.0.origin.as_str())
            || headers
                .get_all("sec-fetch-site")
                .iter()
                .any(|site| site == "cross-site"))
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "development access requires the configured origin",
        ));
    }
    if request.uri().to_string().len() > 8192 {
        return Err(ApiError::new(
            StatusCode::URI_TOO_LONG,
            "request URL exceeds 8192 bytes",
        ));
    }
    if headers.len() > 64
        || headers
            .iter()
            .map(|(name, value)| name.as_str().len() + value.len())
            .sum::<usize>()
            > 16384
    {
        return Err(ApiError::new(
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "request headers exceed their limit",
        ));
    }
    if headers.get_all(header::HOST).iter().count() != 1
        || headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            != Some(api.0.authority.as_str())
    {
        return Err(ApiError::new(
            StatusCode::MISDIRECTED_REQUEST,
            "request host does not match the configured origin",
        ));
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD)
        && (headers.get_all(header::ORIGIN).iter().count() != 1
            || headers
                .get(header::ORIGIN)
                .and_then(|value| value.to_str().ok())
                != Some(api.0.origin.as_str())
            || headers.get_all("x-domus-csrf").iter().count() != 1
            || headers
                .get("x-domus-csrf")
                .and_then(|value| value.to_str().ok())
                != Some("1"))
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "same-origin command headers are required",
        ));
    }
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "content length is invalid"))
        })
        .transpose()?
        .unwrap_or(0);
    let segments = request.uri().path().split('/').skip(1).collect::<Vec<_>>();
    let upload = matches!(
        segments.as_slice(),
        ["api", "jobs", _, "plans", _, "artwork", "local"]
    );
    let maximum = if upload {
        8 * 1024 * 1024
    } else {
        2 * 1024 * 1024
    };
    if content_length > maximum {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body exceeds its size limit",
        ));
    }
    let body_free = matches!(*request.method(), Method::GET | Method::HEAD)
        || request.uri().path() == "/auth/logout"
        || matches!(
            segments.as_slice(),
            ["api", "jobs", _, "plans", _, "approve"]
        );
    if body_free && (content_length != 0 || headers.contains_key(header::TRANSFER_ENCODING)) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "this endpoint does not accept a request body",
        ));
    }
    Ok(())
}

async fn authenticated(State(api): State<Api>, request: Request, next: Next) -> Response {
    match api.identity(request.headers()).await {
        Ok(_) => next.run(request).await,
        Err(ApiError(StatusCode::UNAUTHORIZED, ..)) if request.uri().path() == "/" => {
            Redirect::to("/auth/login").into_response()
        }
        Err(error) => error.into_response(),
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> ApiResult<Option<String>> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        let header = header.to_str().map_err(|_| Error::Authentication)?;
        for part in header.split(';') {
            let cookie = Cookie::parse(part.trim()).map_err(|_| Error::Authentication)?;
            if cookie.name() == name {
                if found.is_some() || cookie.value().len() > 128 {
                    return Err(Error::Authentication.into());
                }
                found = Some(cookie.value().to_owned());
            }
        }
    }
    Ok(found)
}

fn set_cookie(
    response: &mut Response,
    name: &'static str,
    value: &str,
    seconds: i64,
) -> ApiResult<()> {
    let cookie = Cookie::build((name, value.to_owned()))
        .path("/")
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(seconds))
        .build();
    let mut header = HeaderValue::from_str(&cookie.to_string())
        .map_err(|_| Error::Internal("session cookie encoding failed"))?;
    header.set_sensitive(true);
    response.headers_mut().append(header::SET_COOKIE, header);
    Ok(())
}

async fn login(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> ApiResult<Response> {
    let Some(broker) = &api.0.broker else {
        return Ok(Redirect::to("/").into_response());
    };
    let login = {
        let now = Instant::now();
        let mut times = api
            .0
            .login_times
            .lock()
            .map_err(|_| Error::Internal("login rate lock failed"))?;
        times.retain(|_, events| {
            while events
                .front()
                .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(60))
            {
                events.pop_front();
            }
            !events.is_empty()
        });
        if !times.contains_key(&peer.ip()) && times.len() >= 256 {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "sign-in capacity reached; retry in one minute",
            ));
        }
        let events = times.entry(peer.ip()).or_default();
        if events.len() >= 12 {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "sign-in rate limit reached; retry in one minute",
            ));
        }
        events.push_back(now);
        api.0
            .pending
            .lock()
            .map_err(|_| Error::Internal("login challenge lock failed"))?
            .begin()?
    };
    let broker = broker.read().await.clone();
    let url = broker.authorization_url(&login)?;
    let mut response = Redirect::to(url.as_str()).into_response();
    set_cookie(
        &mut response,
        LOGIN_COOKIE,
        login.browser_cookie.secret(),
        300,
    )?;
    Ok(response)
}

#[derive(Deserialize)]
struct Callback {
    state: String,
    code: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(api): State<Api>,
    headers: HeaderMap,
    Query(query): Query<Callback>,
) -> ApiResult<Response> {
    let broker = api.0.broker.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "development mode has no identity callback",
        )
    })?;
    let browser = cookie_value(&headers, LOGIN_COOKIE)?.ok_or(Error::Authentication)?;
    let pending = api
        .0
        .pending
        .lock()
        .map_err(|_| Error::Internal("login challenge lock failed"))?
        .consume(&query.state, &browser)?;
    let outcome = async {
        if query.error.is_some() {
            return Err(Error::Authentication.into());
        }
        let code = query.code.ok_or(Error::Authentication)?;
        let broker = broker.read().await.clone();
        let identity = broker.exchange(code, pending).await?;
        let previous = cookie_value(&headers, SESSION_COOKIE)?;
        let session = api
            .database(move |store| store.rotate_session(&identity, 28800, previous.as_deref()))
            .await?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Internal("system clock precedes the Unix epoch"))?
            .as_secs();
        let seconds = session.expires_at
            - i64::try_from(now)
                .map_err(|_| Error::Internal("system clock exceeds the supported range"))?;
        if seconds <= 0 {
            return Err(Error::Authentication.into());
        }
        let mut response = Redirect::to("/").into_response();
        set_cookie(
            &mut response,
            SESSION_COOKIE,
            session.credential.secret(),
            seconds,
        )?;
        Ok(response)
    }
    .await;
    let mut response = match outcome {
        Ok(response) => response,
        Err(error) => ApiError::into_response(error),
    };
    set_cookie(&mut response, LOGIN_COOKIE, "", 0)?;
    Ok(response)
}

async fn logout(State(api): State<Api>, headers: HeaderMap) -> ApiResult<Response> {
    if api.0.broker.is_none() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "development mode always uses _anonymous; authentication is disabled",
        ));
    }
    let credential = cookie_value(&headers, SESSION_COOKIE)?.ok_or(Error::Authentication)?;
    api.database(move |store| store.revoke_session(&credential))
        .await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    set_cookie(&mut response, SESSION_COOKIE, "", 0)?;
    set_cookie(&mut response, LOGIN_COOKIE, "", 0)?;
    Ok(response)
}

async fn browser_index(State(api): State<Api>) -> Response {
    if let Some(browser) = &api.0.browser {
        return browser.response("/");
    }
    browser_not_installed().await.into_response()
}

async fn browser_asset(State(api): State<Api>, Path(name): Path<String>) -> Response {
    match &api.0.browser {
        Some(browser) => browser.response(&format!("/assets/{name}")),
        None => browser_not_installed().await.into_response(),
    }
}

async fn local_status(
    State(api): State<Api>,
) -> ApiResult<Json<super::local_runtime::LocalStatus>> {
    if let Some(worker) = &api.0.production_worker {
        let capacity = worker.queue_capacity;
        let status = api
            .database(move |store| store.worker_status(capacity))
            .await?;
        return Ok(Json(super::local_runtime::LocalStatus {
            enabled: worker.acquisition.is_some(),
            busy: status.unfinished >= capacity,
            last_error: None,
            worker: Some(status),
        }));
    }
    Ok(Json(match &api.0.local {
        Some(local) => local.status()?,
        None => super::local_runtime::LocalStatus {
            enabled: false,
            busy: false,
            last_error: None,
            worker: None,
        },
    }))
}

async fn local_search(
    State(api): State<Api>,
    Json(request): Json<super::worker::SourceSearchRequest>,
) -> ApiResult<Json<super::soulseek::search::SearchReport>> {
    Ok(Json(api.local()?.search(request.release_id).await?))
}

async fn local_folder(
    State(api): State<Api>,
    Json(request): Json<super::worker::SourceFolderRequest>,
) -> ApiResult<Json<super::worker::SourceFolderReport>> {
    Ok(Json(api.local()?.folder(request).await?))
}

async fn local_run(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
    Json(request): Json<super::worker::StartLocalDownload>,
) -> ApiResult<StatusCode> {
    if let Some(worker) = &api.0.production_worker {
        if worker.acquisition.is_none() {
            return Err(Error::Conflict("worker acquisition is not configured".into()).into());
        }
        let capacity = worker.queue_capacity;
        let binding = worker.binding(&api.0.service_configuration)?;
        api.database(move |store| {
            store.enqueue_worker(
                super::production_worker::WorkerOperation::Acquire {
                    job_id: job,
                    plan_version: version,
                    source_candidate: request.source_candidate,
                    acknowledge_new_only_publication: request.acknowledge_new_only_publication,
                },
                capacity,
                &binding,
            )
        })
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
    api.local()?.start(job, version, request)?;
    Ok(StatusCode::ACCEPTED)
}

async fn local_publication_resume(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
    Json(request): Json<super::worker::ResumePublication>,
) -> ApiResult<StatusCode> {
    if let Some(worker) = &api.0.production_worker {
        if worker.acquisition.is_none() {
            return Err(Error::Conflict("worker acquisition is not configured".into()).into());
        }
        let capacity = worker.queue_capacity;
        let binding = worker.binding(&api.0.service_configuration)?;
        api.database(move |store| {
            store.enqueue_worker(
                super::production_worker::WorkerOperation::Publish {
                    job_id: job,
                    plan_version: version,
                    normalization_run: request.normalization_run,
                    acknowledge_new_only_publication: request.acknowledge_new_only_publication,
                },
                capacity,
                &binding,
            )
        })
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
    api.local()?.resume_publication(job, version, request)?;
    Ok(StatusCode::ACCEPTED)
}

async fn publication_replacement_review(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
) -> ApiResult<Json<Vec<super::store::AlbumReplacementPreview>>> {
    Ok(Json(
        api.database(move |store| {
            crate::application::preview_album_replacements(store, job, version)
        })
        .await?,
    ))
}

async fn record_publication_replacement_decision(
    State(api): State<Api>,
    Path((job, version, location)): Path<(i64, i64, i64)>,
    Json(decision): Json<super::store::AlbumReplacementDecision>,
) -> ApiResult<Json<super::store::AlbumReplacementReview>> {
    Ok(Json(
        api.database(move |store| {
            let preview = crate::application::preview_album_replacements(store, job, version)?
                .into_iter()
                .find(|preview| preview.location_id == location)
                .ok_or_else(|| {
                    Error::Conflict("replacement destination is no longer planned".into())
                })?;
            if preview.album_identity.is_none() {
                return Err(Error::Conflict(
                    "replacement review requires an existing album folder".into(),
                ));
            }
            store.record_album_replacement_decision(&preview, decision)
        })
        .await?,
    ))
}

async fn restore_publication_replacement(
    State(api): State<Api>,
    Path((job, version, location)): Path<(i64, i64, i64)>,
    Json(decision): Json<super::store::AlbumReplacementRestoreDecision>,
) -> ApiResult<Json<super::store::PublicationDestination>> {
    Ok(Json(
        api.database(move |store| {
            crate::application::restore_album_replacement(store, job, version, location, decision)
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadArtworkQuery {
    acknowledge_permitted_use: bool,
}

async fn upload_artwork(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
    Query(query): Query<UploadArtworkQuery>,
    bytes: axum::body::Bytes,
) -> ApiResult<Json<ArtworkCandidate>> {
    Ok(Json(
        api.database(move |store| {
            store.prepare_uploaded_artwork_or_ask(
                job,
                version,
                &bytes,
                None,
                query.acknowledge_permitted_use,
            )
        })
        .await?,
    ))
}

async fn browser_not_installed() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "browser application is not installed",
    )
}

async fn session(
    State(api): State<Api>,
    headers: HeaderMap,
) -> ApiResult<Json<super::store::SessionIdentity>> {
    Ok(Json(api.identity(&headers).await?))
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(default, deny_unknown_fields)]
pub struct PageQuery {
    after: i64,
    limit: u16,
}
impl Default for PageQuery {
    fn default() -> Self {
        Self {
            after: 0,
            limit: 100,
        }
    }
}
impl PageQuery {
    fn page(&self) -> Result<Page> {
        Page::new(self.after, self.limit)
    }
}

async fn locations(
    State(api): State<Api>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<Location>>> {
    let page = query.page()?;
    Ok(Json(
        api.database(move |store| {
            super::server::configuration::ConfigurationService::library_roots(store, page)
        })
        .await?,
    ))
}

async fn inspect_share_folder(
    State(api): State<Api>,
    Query(request): Query<crate::application::sharing::InspectShareFolder>,
) -> ApiResult<Json<crate::application::sharing::ShareFolderInspection>> {
    Ok(Json(
        api.database(move |store| {
            crate::application::sharing::inspect_share_folder(store, &request)
        })
        .await?,
    ))
}

async fn bind_share_folder(
    State(api): State<Api>,
    Json(request): Json<crate::application::sharing::BindShareFolder>,
) -> ApiResult<Json<crate::application::sharing::ShareFolderInspection>> {
    api.require_configuration_write().await?;
    Ok(Json(
        api.database(move |store| crate::application::sharing::bind_share_folder(store, &request))
            .await?,
    ))
}

async fn share_bindings(
    State(api): State<Api>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<crate::application::sharing::ShareFolderInspection>>> {
    let page = query.page()?;
    Ok(Json(
        api.database(move |store| crate::application::sharing::share_bindings(store, page))
            .await?,
    ))
}
async fn location(State(api): State<Api>, Path(id): Path<i64>) -> ApiResult<Json<Location>> {
    Ok(Json(api.database(move |store| store.location(id)).await?))
}

async fn location_availability(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    api.database(move |store| crate::application::check_location_availability(store, id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct RegisterLocation {
    root: String,
    kind: String,
}

async fn register_location(
    State(api): State<Api>,
    Json(request): Json<RegisterLocation>,
) -> ApiResult<Json<Location>> {
    api.require_configuration_write().await?;
    Ok(Json(
        api.database(move |store| {
            super::server::configuration::ConfigurationService::register_library_root(
                store,
                &request.root,
                &request.kind,
            )
        })
        .await?,
    ))
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ScanLocation {}

async fn scan_location(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(_request): Json<ScanLocation>,
) -> ApiResult<(StatusCode, Json<Location>)> {
    api.require_configuration_write().await?;
    if let Some(worker) = &api.0.production_worker {
        let capacity = worker.queue_capacity;
        let binding = worker.binding(&api.0.service_configuration)?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(
                api.database(move |store| {
                    store.enqueue_worker(
                        super::production_worker::WorkerOperation::Scan { location_id: id },
                        capacity,
                        &binding,
                    )?;
                    store.location(id)
                })
                .await?,
            ),
        ));
    }
    Ok((
        StatusCode::OK,
        Json(
            api.database(move |store| {
                crate::application::scan_location(store, id)?;
                store.location(id)
            })
            .await?,
        ),
    ))
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WorkerPageQuery {
    after: i64,
    limit: u16,
    job_id: Option<i64>,
    unfinished_only: bool,
}

impl Default for WorkerPageQuery {
    fn default() -> Self {
        Self {
            after: 0,
            limit: 50,
            job_id: None,
            unfinished_only: false,
        }
    }
}

async fn worker_requests(
    State(api): State<Api>,
    Query(query): Query<WorkerPageQuery>,
) -> ApiResult<Json<Vec<super::production_worker::WorkerRequest>>> {
    let page = Page::new(query.after, query.limit)?;
    Ok(Json(
        api.database(move |store| {
            if query.unfinished_only {
                store.unfinished_worker_requests(page, query.job_id)
            } else {
                store.worker_requests_for_job(page, query.job_id)
            }
        })
        .await?,
    ))
}

async fn worker_retry(State(api): State<Api>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    if api.0.production_worker.is_none() {
        return Err(Error::Conflict("production worker is not configured".into()).into());
    }

    let request = api.database(move |store| store.worker_request(id)).await?;
    if matches!(
        request.operation,
        super::production_worker::WorkerOperation::Scan { .. }
            | super::production_worker::WorkerOperation::RefreshFolder { .. }
    ) {
        api.require_configuration_write().await?;
    }
    api.database(move |store| store.retry_worker(id)).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn worker_request(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<Json<super::production_worker::WorkerRequest>> {
    Ok(Json(
        api.database(move |store| store.worker_request(id)).await?,
    ))
}

async fn worker_dismiss(State(api): State<Api>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    api.database(move |store| store.dismiss_worker(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize, ts_rs::TS)]
#[serde(default, deny_unknown_fields)]
pub struct BrowseQuery {
    folder: String,
    after: i64,
    limit: u16,
}
impl Default for BrowseQuery {
    fn default() -> Self {
        Self {
            folder: String::new(),
            after: 0,
            limit: 100,
        }
    }
}
async fn entries(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Query(query): Query<BrowseQuery>,
) -> ApiResult<Json<Vec<Entry>>> {
    let page = Page::new(query.after, query.limit)?;
    let folder = LibraryPath::parse(&query.folder)?;
    Ok(Json(
        api.database(move |store| store.browse(id, &folder, page))
            .await?,
    ))
}
async fn settings(State(api): State<Api>) -> ApiResult<Json<Option<SettingsVersion>>> {
    Ok(Json(
        api.database(|store| {
            super::server::configuration::ConfigurationService::output_settings(store)
        })
        .await?,
    ))
}

async fn settings_policy(State(api): State<Api>) -> Json<SystemSettingsPolicy> {
    Json(SystemSettingsPolicy {
        read_only: api.0.system_settings_read_only,
    })
}

async fn backend_configuration(
    State(api): State<Api>,
) -> ApiResult<Json<super::server::configuration::BackendConfigurationState>> {
    let source = api.0.backend_configuration.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend configuration inspection is not attached to this host",
        )
    })?;
    let state = tokio::task::spawn_blocking(move || source.read())
        .await
        .map_err(|_| Error::Internal("backend configuration read task failed"))??;
    Ok(Json(state))
}

async fn validate_backend_configuration(
    State(api): State<Api>,
    request: std::result::Result<
        Json<super::server::configuration::BackendConfigurationCommand>,
        axum::extract::rejection::JsonRejection,
    >,
) -> ApiResult<Json<super::server::configuration::BackendConfigurationResult>> {
    let Json(request) = request.map_err(|error| {
        ApiError::new(error.status(), "backend configuration request is invalid")
    })?;
    let source = api.0.backend_configuration.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend configuration is not attached to this host",
        )
    })?;
    Ok(Json(
        tokio::task::spawn_blocking(move || source.validate(request))
            .await
            .map_err(|_| Error::Internal("configuration validation task failed"))??,
    ))
}

async fn save_backend_configuration(
    State(api): State<Api>,
    request: std::result::Result<
        Json<super::server::configuration::BackendConfigurationCommand>,
        axum::extract::rejection::JsonRejection,
    >,
) -> ApiResult<Json<super::server::configuration::BackendConfigurationResult>> {
    api.require_settings_write()?;
    let Json(request) = request.map_err(|error| {
        ApiError::new(error.status(), "backend configuration request is invalid")
    })?;
    let source = api.0.backend_configuration.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend configuration is not attached to this host",
        )
    })?;
    Ok(Json(
        tokio::task::spawn_blocking(move || source.save(request))
            .await
            .map_err(|_| Error::Internal("configuration save task failed"))??,
    ))
}

async fn validate_configuration_sections(
    State(api): State<Api>,
    request: std::result::Result<
        Json<super::server::configuration::ConfigurationSectionCommand>,
        axum::extract::rejection::JsonRejection,
    >,
) -> ApiResult<Json<super::server::configuration::BackendConfigurationResult>> {
    let Json(command) = request.map_err(|error| {
        ApiError::new(error.status(), "configuration section request is invalid")
    })?;
    let source = api.configuration_service()?;
    Ok(Json(
        tokio::task::spawn_blocking(move || source.validate_sections(command))
            .await
            .map_err(|_| Error::Internal("configuration validation task failed"))??,
    ))
}

async fn save_configuration_sections(
    State(api): State<Api>,
    request: std::result::Result<
        Json<super::server::configuration::ConfigurationSectionCommand>,
        axum::extract::rejection::JsonRejection,
    >,
) -> ApiResult<Json<super::server::configuration::BackendConfigurationResult>> {
    api.require_settings_write()?;
    let Json(command) = request.map_err(|error| {
        ApiError::new(error.status(), "configuration section request is invalid")
    })?;
    let source = api.configuration_service()?;
    Ok(Json(
        tokio::task::spawn_blocking(move || source.save_sections(command))
            .await
            .map_err(|_| Error::Internal("configuration save task failed"))??,
    ))
}

async fn reload_configuration(
    State(api): State<Api>,
    request: std::result::Result<
        Json<super::server::configuration::ConfigurationReloadCommand>,
        axum::extract::rejection::JsonRejection,
    >,
) -> ApiResult<Json<super::server::configuration::ConfigurationReloadState>> {
    api.require_settings_write()?;
    let Json(command) = request.map_err(|error| {
        ApiError::new(error.status(), "configuration reload request is invalid")
    })?;
    let source = api.configuration_service()?;
    drop(api.reload_permit()?);
    Ok(Json(
        tokio::task::spawn_blocking(move || source.request_reload(command))
            .await
            .map_err(|_| Error::Internal("configuration reload task failed"))??,
    ))
}

impl Api {
    fn configuration_service(
        &self,
    ) -> ApiResult<Arc<super::server::configuration::ConfigurationService>> {
        self.0.backend_configuration.clone().ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "backend configuration is not attached to this host",
            )
        })
    }
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct SaveSettings {
    expected_version: Option<i64>,
    settings: OutputSettings,
}
async fn save_settings(
    State(api): State<Api>,
    Json(request): Json<SaveSettings>,
) -> ApiResult<Json<i64>> {
    api.require_configuration_write().await?;
    Ok(Json(
        api.database(move |store| {
            super::server::configuration::ConfigurationService::save_output_settings(
                store,
                request.expected_version,
                &request.settings,
            )
        })
        .await?,
    ))
}

impl Api {
    async fn require_configuration_write(&self) -> ApiResult<()> {
        self.require_settings_write()?;
        if let Some(source) = self.0.backend_configuration.clone() {
            tokio::task::spawn_blocking(move || source.require_editable())
                .await
                .map_err(|_| Error::Internal("configuration policy task failed"))??;
        }
        Ok(())
    }

    fn require_settings_write(&self) -> ApiResult<()> {
        if self.0.system_settings_read_only {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "system settings are read-only in server configuration",
            ));
        }
        Ok(())
    }
}
async fn jobs(
    State(api): State<Api>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<JobSummary>>> {
    let page = query.page()?;
    Ok(Json(api.database(move |store| store.jobs(page)).await?))
}

async fn queue(
    State(api): State<Api>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<JobSummary>>> {
    let page = query.page()?;
    Ok(Json(api.database(move |store| store.queue(page)).await?))
}

async fn dismiss_finished(
    State(api): State<Api>,
    Json(selection): Json<Vec<super::store::QueueSelection>>,
) -> ApiResult<StatusCode> {
    api.database(move |store| store.dismiss_finished(&selection))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct CreateJob {}
#[derive(Serialize, ts_rs::TS)]
pub struct CreatedJob {
    job_id: i64,
    plan_version: i64,
}
async fn create_job(
    State(api): State<Api>,
    Json(_request): Json<CreateJob>,
) -> ApiResult<Json<CreatedJob>> {
    let job_id = api.database(|store| store.enqueue(1)).await?;
    Ok(Json(CreatedJob {
        job_id,
        plan_version: 1,
    }))
}
async fn prepare_plan(
    State(api): State<Api>,
    Json(request): Json<PlanRequest>,
) -> ApiResult<Json<StoredPlan>> {
    Ok(Json(
        api.database(move |store| store.prepare_plan(request))
            .await?,
    ))
}
async fn plan(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
) -> ApiResult<Json<StoredPlan>> {
    Ok(Json(
        api.database(move |store| store.plan(id, version)).await?,
    ))
}
async fn approve_plan(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
) -> ApiResult<StatusCode> {
    api.database(move |store| store.approve_plan(id, version))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn transfers(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<TransferFile>>> {
    let page = query.page()?;
    Ok(Json(
        api.database(move |store| {
            store.plan(id, version)?;
            store.transfers(id, version, page)
        })
        .await?,
    ))
}

async fn transfer_summary(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
) -> ApiResult<Json<super::store::TransferSummary>> {
    Ok(Json(
        api.database(move |store| store.transfer_summary(id, version))
            .await?,
    ))
}

async fn artwork_candidates(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<ArtworkCandidate>>> {
    let page = query.page()?;
    Ok(Json(
        api.database(move |store| store.artwork_candidates(id, version, page))
            .await?,
    ))
}

async fn artwork_image(
    State(api): State<Api>,
    Path((id, version, candidate)): Path<(i64, i64, i64)>,
) -> ApiResult<Response> {
    let bytes = api
        .database(move |store| Ok(store.artwork_jpeg(id, version, candidate)?.into_bytes()))
        .await?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg")], bytes).into_response())
}

async fn artwork_evidence(
    State(api): State<Api>,
    Path((id, version, candidate)): Path<(i64, i64, i64)>,
) -> ApiResult<Json<Option<super::store::CaaArtworkEvidence>>> {
    Ok(Json(
        api.database(move |store| {
            store.artwork_jpeg(id, version, candidate)?;
            store.artwork_caa_evidence(id, version, candidate)
        })
        .await?,
    ))
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct PrepareCaaArtwork {
    pub image_id: String,
    pub background: Option<[u8; 3]>,
    pub acknowledge_permitted_use: bool,
}

#[derive(Serialize, ts_rs::TS)]
pub struct CaaCoverOption {
    pub image_id: String,
    pub front: bool,
    pub provider_approved: Option<bool>,
}

#[derive(Serialize, ts_rs::TS)]
pub struct CaaCoverSearch {
    pub release_id: String,
    pub images: Vec<CaaCoverOption>,
}

async fn search_caa_artwork(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
) -> ApiResult<Json<CaaCoverSearch>> {
    use crate::application::services::{ArtworkContent, ArtworkRequest, EntityRef};
    api.0
        .services
        .artwork
        .require_provider("cover-art-archive")?;
    let release_id = api
        .database(move |store| store.artwork_provider_release(id, version))
        .await?;
    let result = api
        .0
        .services
        .artwork
        .fetch(ArtworkRequest {
            provider: "cover-art-archive".into(),
            release: EntityRef {
                provider: "musicbrainz".into(),
                id: release_id.clone(),
            },
            image_id: None,
        })
        .await?;
    api.database(move |store| store.artwork_provider_release(id, version))
        .await?;
    let images = match result.content {
        ArtworkContent::Listing(images) => images
            .into_iter()
            .map(|image| CaaCoverOption {
                image_id: image.entity.id,
                front: image.front,
                provider_approved: image.provider_approved,
            })
            .collect(),
        ArtworkContent::Missing => Vec::new(),
        ArtworkContent::Image { .. } => {
            return Err(Error::Internal("artwork search returned image bytes").into());
        }
    };
    Ok(Json(CaaCoverSearch { release_id, images }))
}

/// Fetch only a listing-bound image through the installed provider. Preparation
/// still uses retained CAA evidence and never approves the normalized preview.
async fn download_caa_artwork(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
    Json(request): Json<PrepareCaaArtwork>,
) -> ApiResult<Json<ArtworkCandidate>> {
    use crate::application::services::images::ImageOptions;
    use crate::application::services::{ArtworkRequest, EntityRef};
    api.0
        .services
        .artwork
        .require_provider("cover-art-archive")?;
    let (release_id, size) = api
        .database(move |store| store.artwork_provider_options(id, version))
        .await?;
    if !request.acknowledge_permitted_use {
        // Save the normal plan-bound permission question without network I/O.
        return prepare_caa_artwork(State(api), Path((id, version)), Json(request)).await;
    }
    let result = api
        .0
        .services
        .images
        .retrieve(
            ArtworkRequest {
                provider: "cover-art-archive".into(),
                release: EntityRef {
                    provider: "musicbrainz".into(),
                    id: release_id,
                },
                image_id: Some(request.image_id.clone()),
            },
            ImageOptions {
                size,
                background: request.background,
            },
        )
        .await;
    let retrieved = match result {
        Ok(Some(image)) => Ok(image),
        Ok(None) => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "CAA cover was not found",
            ));
        }
        Err(error) => Err(error),
    };
    Ok(Json(
        api.database(move |store| {
            store.prepare_retrieved_caa_artwork_or_ask(id, version, retrieved)
        })
        .await?,
    ))
}

async fn prepare_caa_artwork(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
    Json(request): Json<PrepareCaaArtwork>,
) -> ApiResult<Json<ArtworkCandidate>> {
    api.0
        .services
        .artwork
        .require_provider("cover-art-archive")?;
    Ok(Json(
        api.database(move |store| {
            store.prepare_cached_caa_artwork_or_ask(
                id,
                version,
                &request.image_id,
                request.background,
                request.acknowledge_permitted_use,
            )
        })
        .await?,
    ))
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ApproveArtwork {
    pub candidate_id: i64,
    pub sha256: String,
}

async fn approve_artwork(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
    Json(request): Json<ApproveArtwork>,
) -> ApiResult<StatusCode> {
    api.database(move |store| {
        store.approve_artwork(id, version, request.candidate_id, &request.sha256)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn questions(
    State(api): State<Api>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<StoredQuestion>>> {
    let page = query.page()?;
    Ok(Json(
        api.database(move |store| store.questions(page)).await?,
    ))
}

async fn inbox(
    State(api): State<Api>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<StoredQuestion>>> {
    let page = query.page()?;
    Ok(Json(api.database(move |store| store.inbox(page)).await?))
}

async fn question_details(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<Json<QuestionDetails>> {
    Ok(Json(
        api.database(move |store| store.question_details(id))
            .await?,
    ))
}

async fn job_decision(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
) -> ApiResult<Json<Option<QuestionDetails>>> {
    Ok(Json(
        api.database(move |store| store.job_decision(job, version))
            .await?,
    ))
}

async fn publication_status(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
) -> ApiResult<Json<Vec<super::store::PublicationDestination>>> {
    Ok(Json(
        api.database(move |store| store.publication_destinations(id, version))
            .await?,
    ))
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct RequestJobControl {
    pub action: JobControlAction,
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ContinueJobControl {
    pub plan_version: i64,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ApproveSourceCandidate {
    pub plan_version: i64,
    pub review_sha256: String,
    pub acknowledge_exact_edition_and_permitted_download: bool,
}

async fn prepare_source_candidate(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
    Json(listing): Json<crate::domain::sources::SourceFolder>,
) -> ApiResult<Json<super::store::SourceCandidate>> {
    Ok(Json(
        api.database(move |store| store.prepare_source_candidate(job, version, listing))
            .await?,
    ))
}

async fn source_candidates(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<Vec<super::store::SourceCandidate>>> {
    let page = page.page()?;
    Ok(Json(
        api.database(move |store| store.source_candidates(job, version, page))
            .await?,
    ))
}

async fn source_candidate(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<Json<super::store::SourceCandidate>> {
    Ok(Json(
        api.database(move |store| store.source_candidate(id))
            .await?,
    ))
}

async fn approve_source_candidate(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(request): Json<ApproveSourceCandidate>,
) -> ApiResult<Json<super::store::SourceCandidate>> {
    Ok(Json(
        api.database(move |store| {
            store.approve_source_candidate(
                id,
                request.plan_version,
                &request.review_sha256,
                request.acknowledge_exact_edition_and_permitted_download,
            )
        })
        .await?,
    ))
}

async fn request_job_control(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
    Json(request): Json<RequestJobControl>,
) -> ApiResult<Json<JobControl>> {
    Ok(Json(
        api.database(move |store| store.request_job_control(id, version, request.action))
            .await?,
    ))
}

async fn job_control(State(api): State<Api>, Path(id): Path<i64>) -> ApiResult<Json<JobControl>> {
    Ok(Json(
        api.database(move |store| store.job_control(id)).await?,
    ))
}

async fn job_controls(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<Vec<JobControl>>> {
    let page = page.page()?;
    Ok(Json(
        api.database(move |store| store.job_controls(job, version, page))
            .await?,
    ))
}

async fn continue_job_control(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(request): Json<ContinueJobControl>,
) -> ApiResult<Json<String>> {
    Ok(Json(
        api.database(move |store| store.continue_job_control(id, request.plan_version))
            .await?,
    ))
}

async fn check_artwork(
    State(api): State<Api>,
    Path((id, version)): Path<(i64, i64)>,
) -> ApiResult<Json<Option<ArtworkDecision>>> {
    Ok(Json(
        api.database(move |store| store.check_artwork(id, version))
            .await?,
    ))
}

async fn artwork_decision(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<Json<ArtworkDecision>> {
    Ok(Json(
        api.database(move |store| store.artwork_decision(id))
            .await?,
    ))
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ResumeArtwork {
    pub plan_version: i64,
}

async fn resume_artwork(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(request): Json<ResumeArtwork>,
) -> ApiResult<StatusCode> {
    api.database(move |store| store.resume_artwork(id, request.plan_version))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    plan_version: i64,
    choice: String,
}
async fn answer(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(request): Json<Answer>,
) -> ApiResult<StatusCode> {
    if request.choice.len() > 512 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "answer exceeds 512 bytes",
        ));
    }
    api.database(move |store| store.answer(id, request.plan_version, &request.choice))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn events(
    State(api): State<Api>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<JobEvent>>> {
    let page = query.page()?;
    Ok(Json(api.database(move |store| store.events(page)).await?))
}

async fn plan_events(
    State(api): State<Api>,
    Path((job, version)): Path<(i64, i64)>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<JobEvent>>> {
    let page = query.page()?;
    Ok(Json(
        api.database(move |store| store.plan_events(job, version, page))
            .await?,
    ))
}

async fn event_cursor(State(api): State<Api>) -> ApiResult<Json<EventCursor>> {
    Ok(Json(EventCursor {
        after: api.database(|store| store.last_event_id()).await?,
    }))
}

struct EventStream {
    api: Api,
    headers: HeaderMap,
    after: i64,
    limit: u16,
    pending: VecDeque<JobEvent>,
    done: bool,
    _permit: OwnedSemaphorePermit,
}

async fn event_stream(
    State(api): State<Api>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> ApiResult<Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>>> {
    let resume = headers.get_all("last-event-id");
    if resume.iter().count() > 1 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "one last-event-id is allowed",
        ));
    }
    let after = match resume.iter().next() {
        Some(value) => value
            .to_str()
            .ok()
            .filter(|value| value.len() <= 20)
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "last-event-id is invalid"))?,
        None => query.after,
    };
    let page = Page::new(after, query.limit)?;
    let head = api.database(|store| store.last_event_id()).await?;
    if after > head {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "event cursor is ahead of the database; reload read models and obtain a new cursor",
        ));
    }
    let permit = api
        .0
        .event_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "too many event streams"))?;
    let state = EventStream {
        api,
        headers,
        after,
        limit: page.limit(),
        pending: VecDeque::new(),
        done: false,
        _permit: permit,
    };
    let stream = futures_util::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        loop {
            // Recheck even buffered events. A slow reader must not retain access
            // after logout or expiry while it drains an earlier page.
            if let Err(error) = state.api.identity(&state.headers).await {
                state.done = true;
                return Some((Ok(stream_error(&error.1)), state));
            }
            if let Some(event) = state.pending.pop_front() {
                let output = Event::default()
                    .event("job")
                    .id(event.id.to_string())
                    .json_data(&event);
                match output {
                    Ok(output) => {
                        state.after = event.id;
                        return Some((Ok(output), state));
                    }
                    Err(_) => {
                        eprintln!("event serialization failed");
                        state.done = true;
                        return Some((Ok(stream_error("event serialization failed")), state));
                    }
                }
            }
            let after = state.after;
            let limit = state.limit;
            match state
                .api
                .database(move |store| store.events(Page::new(after, limit)?))
                .await
            {
                Ok(events) if events.is_empty() => tokio::time::sleep(Duration::from_secs(2)).await,
                Ok(events) => state.pending = events.into(),
                Err(error) => {
                    state.done = true;
                    return Some((Ok(stream_error(&error.1)), state));
                }
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keep-alive"),
    ))
}

fn stream_error(message: &str) -> Event {
    match Event::default().event("error").json_data(ApiMessage {
        error: message.into(),
    }) {
        Ok(event) => event,
        Err(_) => {
            eprintln!("event error serialization failed");
            Event::default()
                .event("error")
                .data("{\"error\":\"event serialization failed\"}")
        }
    }
}
