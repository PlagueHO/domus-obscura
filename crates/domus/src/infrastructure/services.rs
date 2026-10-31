//! Composition of installed adapters. This module creates no listener and
//! performs no provider requests during construction.
use crate::application::services::{
    ArtworkContent, ArtworkImage, ArtworkProvider, ArtworkRequest, ArtworkResult, ArtworkService,
    Capability, CatalogItem, CatalogProvider, CatalogService, DiscoveryPage, DiscoveryQuery,
    EntityRef, MetadataTrack, ReleaseMetadata, ServiceConfiguration,
};
use crate::domain::credit_text;
use crate::infrastructure::catalog::{CatalogQuery, CatalogResult, MusicBrainzClient};
use crate::infrastructure::store::Store;
use crate::{Error, Result};
use futures_util::future::BoxFuture;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// One operation has a total bounded admission wait, including CAA redirects.
/// This does not grant network authority; only CatalogPermit can do that.
pub(crate) struct ProviderWait {
    remaining: Duration,
    pub(crate) cancelled: Arc<AtomicBool>,
}

impl ProviderWait {
    pub(crate) fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            remaining: Duration::from_secs(15),
            cancelled,
        }
    }

    pub(crate) fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Error::Busy("metadata request cancelled before submission"));
        }
        Ok(())
    }

    pub(crate) async fn pause(&mut self, status: super::store::CatalogStatus) -> Result<()> {
        use super::store::CatalogProviderState;
        self.check()?;
        if matches!(status.state, CatalogProviderState::Paused) {
            return Err(Error::Catalog {
                reason: "provider requests are paused; explicit review is required",
                retry_after_seconds: u32::MAX,
            });
        }
        let delay = status
            .known_retry_after_seconds
            .map(|seconds| Duration::from_secs(u64::from(seconds)).max(Duration::from_millis(100)))
            .unwrap_or(Duration::from_millis(100));
        if delay.saturating_add(Duration::from_secs(1)) > self.remaining {
            return Err(match status.known_retry_after_seconds {
                Some(seconds) => Error::Catalog {
                    reason: "provider admission wait ended; retry metadata manually later",
                    retry_after_seconds: seconds,
                },
                None => Error::ProviderActive,
            });
        }
        let started = Instant::now();
        tokio::time::sleep(delay).await;
        self.remaining = self.remaining.saturating_sub(started.elapsed());
        self.check()
    }

    pub(crate) fn reserve_start(&mut self) {
        self.remaining = self.remaining.saturating_sub(Duration::from_secs(1));
    }
}

struct CancelPending(Arc<AtomicBool>);

impl Drop for CancelPending {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
pub(crate) struct ServiceStore {
    store: Arc<Mutex<Store>>,
    slots: Arc<Semaphore>,
}

impl ServiceStore {
    pub(crate) fn new(store: Store) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            slots: Arc::new(Semaphore::new(4)),
        }
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy("database is busy; retry later"))?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut store = store
                .lock()
                .map_err(|_| Error::Internal("database lock failed"))?;
            operation(&mut store)
        })
        .await
        .map_err(|_| Error::Internal("database task failed"))?
    }

    /// Started provider work must persist even when normal database admission
    /// is full. Provider admission bounds the number of these completion tasks.
    async fn complete<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = store
                .lock()
                .map_err(|_| Error::Internal("database lock failed"))?;
            operation(&mut store)
        })
        .await
        .map_err(|_| Error::Internal("provider completion task failed"))?
    }
}

pub struct MusicBrainzService {
    database: ServiceStore,
    client: MusicBrainzClient,
    configuration: ServiceConfiguration,
    slots: Arc<Semaphore>,
}

impl MusicBrainzService {
    pub(crate) fn new(
        database: ServiceStore,
        client: MusicBrainzClient,
        configuration: ServiceConfiguration,
        slots: Arc<Semaphore>,
    ) -> Self {
        Self {
            database,
            client,
            configuration,
            slots,
        }
    }

    /// Retains the exact-release lane used by the existing API and frozen plans.
    /// General provider results cannot be passed here as MusicBrainz evidence.
    pub async fn query<T: Send + 'static>(
        &self,
        query: CatalogQuery,
        parse: fn(&CatalogQuery, &[u8]) -> Result<T>,
    ) -> Result<CatalogResult<T>> {
        self.configuration.require(
            if query.is_exact_release() {
                Capability::Metadata
            } else {
                Capability::Discovery
            },
            "musicbrainz",
        )?;
        let cached_query = query.clone();
        let cached = self
            .database
            .run(move |store| {
                crate::application::catalog::cached_catalog(store, &cached_query, parse)
            })
            .await?;
        if let Some(cached) = cached {
            return Ok(cached);
        }
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::ProviderSaturated)?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel = CancelPending(cancelled.clone());
        let mut wait = ProviderWait::new(cancelled.clone());
        enum Admission<T> {
            Cached(CatalogResult<T>),
            Permit(super::store::CatalogPermit),
        }
        let mut permit = loop {
            wait.check()?;
            let candidate = query.clone();
            let admitted = self
                .database
                .run(move |store| {
                    if let Some(cached) =
                        crate::application::catalog::cached_catalog(store, &candidate, parse)?
                    {
                        return Ok(Admission::Cached(cached));
                    }
                    store.claim_catalog_request().map(Admission::Permit)
                })
                .await;
            match admitted {
                Ok(Admission::Cached(cached)) => return Ok(cached),
                Ok(Admission::Permit(permit)) => break permit,
                Err(Error::Catalog { .. }) => {
                    wait.pause(self.database.run(|store| store.catalog_status()).await?)
                        .await?;
                }
                Err(error) => return Err(error),
            }
        };
        wait.reserve_start();
        permit.cancel_before_start(cancelled);
        let database = self.database.clone();
        let client = self.client.clone();
        // A dropped browser future cannot abandon a started response or its
        // durable backoff. Cancellation is checked immediately before send.
        tokio::spawn(async move {
            let _slot = slot;
            let outcome = client.execute(&query, &mut permit).await;
            let result = database
                .complete(move |store| {
                    crate::application::catalog::complete_catalog(
                        store, query, permit, outcome, parse,
                    )
                })
                .await;
            if let Err(error) = &result {
                eprintln!("provider operation failed: {error}");
            }
            result
        })
        .await
        .map_err(|_| Error::Internal("provider operation task failed"))?
    }
}

fn musicbrainz_id(entity: &EntityRef) -> Result<&str> {
    entity.validate()?;
    if entity.provider != "musicbrainz" {
        return Err(Error::InvalidInput(
            "MusicBrainz needs an explicitly mapped MusicBrainz entity".into(),
        ));
    }
    Ok(&entity.id)
}

fn entity(id: String) -> EntityRef {
    EntityRef {
        provider: "musicbrainz".into(),
        id,
    }
}

impl CatalogProvider for MusicBrainzService {
    fn id(&self) -> &str {
        "musicbrainz"
    }

    fn discover(&self, query: DiscoveryQuery) -> BoxFuture<'_, Result<DiscoveryPage>> {
        Box::pin(async move {
            match query {
                DiscoveryQuery::Artists {
                    text,
                    offset,
                    limit,
                } => {
                    let result = self
                        .query(
                            CatalogQuery::artists(&text, offset, limit)?,
                            CatalogQuery::parse_artists,
                        )
                        .await?;
                    let next = offset
                        .checked_add(result.data.items.len() as u32)
                        .filter(|next| *next < result.data.count);
                    Ok(DiscoveryPage {
                        offset,
                        total: Some(result.data.count),
                        next_offset: next,
                        items: result
                            .data
                            .items
                            .into_iter()
                            .map(|artist| CatalogItem {
                                entity: entity(artist.id),
                                title: artist.name,
                                artist: None,
                                date: None,
                            })
                            .collect(),
                        source: result.source,
                        fetched_at: result.fetched_at,
                    })
                }
                query => {
                    let request = match query {
                        DiscoveryQuery::Albums {
                            text,
                            artist,
                            offset,
                            limit,
                            filters,
                        } => {
                            let artist = artist.as_ref().map(musicbrainz_id).transpose()?;
                            CatalogQuery::albums(&text, artist, offset, limit, filters)?
                        }
                        DiscoveryQuery::ArtistReleases {
                            artist,
                            offset,
                            limit,
                            filters,
                        } => CatalogQuery::filtered_releases(
                            musicbrainz_id(&artist)?,
                            offset,
                            limit,
                            filters,
                        )?,
                        DiscoveryQuery::Artists { .. } => {
                            return Err(Error::Internal(
                                "artist query dispatched as an album query",
                            ));
                        }
                    };
                    let result = self.query(request, CatalogQuery::parse_releases).await?;
                    Ok(DiscoveryPage {
                        offset: result.data.offset,
                        total: Some(result.data.count),
                        next_offset: result.data.next_offset,
                        items: result
                            .data
                            .items
                            .into_iter()
                            .map(|release| CatalogItem {
                                entity: entity(release.id),
                                title: release.title,
                                artist: Some(release.artist),
                                date: release.date,
                            })
                            .collect(),
                        source: result.source,
                        fetched_at: result.fetched_at,
                    })
                }
            }
        })
    }

    fn metadata(&self, release: EntityRef) -> BoxFuture<'_, Result<ReleaseMetadata>> {
        Box::pin(async move {
            let result = self
                .query(
                    CatalogQuery::release(musicbrainz_id(&release)?)?,
                    CatalogQuery::parse_release,
                )
                .await?;
            Ok(ReleaseMetadata {
                entity: release,
                title: result.data.summary.title,
                artist: result.data.summary.artist,
                date: result.data.summary.date,
                tracks: result
                    .data
                    .media
                    .into_iter()
                    .flat_map(|medium| {
                        medium.tracks.into_iter().map(move |track| MetadataTrack {
                            title: track.title,
                            artist: credit_text(&track.credits),
                            disc: Some(medium.position),
                            position: Some(track.position),
                            duration_ms: track.duration_ms,
                        })
                    })
                    .collect(),
                source: result.source,
                fetched_at: result.fetched_at,
            })
        })
    }
}

pub struct CaaArtworkService {
    database: std::path::PathBuf,
    client: super::caa::CaaHttpClient,
    slots: Arc<Semaphore>,
    cache_slots: Arc<Semaphore>,
    cache: Option<Arc<dyn crate::application::services::cache::Cache>>,
}

impl CaaArtworkService {
    pub fn new(database: &std::path::Path, client: super::caa::CaaHttpClient) -> Self {
        Self {
            database: database.into(),
            client,
            slots: Arc::new(Semaphore::new(4)),
            cache_slots: Arc::new(Semaphore::new(4)),
            cache: None,
        }
    }

    pub fn with_cache(
        mut self,
        cache: Arc<dyn crate::application::services::cache::Cache>,
    ) -> Self {
        self.cache = Some(cache);
        self
    }
}

impl ArtworkProvider for CaaArtworkService {
    fn id(&self) -> &str {
        "cover-art-archive"
    }

    fn fetch(&self, request: ArtworkRequest) -> BoxFuture<'_, Result<ArtworkResult>> {
        Box::pin(async move {
            if request.provider != self.id() {
                return Err(Error::InvalidInput(
                    "CAA request selects a different provider".into(),
                ));
            }
            let release_id = musicbrainz_id(&request.release)?.to_owned();
            let listing_request = super::caa::CaaRequest::listing(&release_id)?;
            if request
                .image_id
                .as_deref()
                .is_some_and(|id| !super::caa::is_image_id(id))
            {
                return Err(Error::InvalidInput("CAA image ID is invalid".into()));
            }
            let cache_slot = self
                .cache_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::Busy("artwork cache reads are busy; retry later"))?;
            let path = self.database.clone();
            let cache = self.cache.clone();
            let cached_request = request.clone();
            let cached_listing = listing_request.clone();
            let cached = tokio::task::spawn_blocking(move || {
                let _slot = cache_slot;
                let mut store = Store::open_current_existing(&path)?;
                if let Some(cache) = cache {
                    store = store.with_cache(cache);
                }
                cached_artwork(&mut store, &cached_listing, cached_request)
            })
            .await
            .map_err(|_| Error::Internal("artwork cache read task failed"))??;
            if let Some(cached) = cached {
                return Ok(cached);
            }
            let permit = self
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::ProviderSaturated)?;
            let cancelled = Arc::new(AtomicBool::new(false));
            let _cancel = CancelPending(cancelled.clone());
            let path = self.database.clone();
            let client = self.client.clone();
            let cache = self.cache.clone();
            // CAA's existing redirect/budget orchestration includes SQLite.
            // Keep that work on a blocking thread with an independent connection.
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut store = Store::open_current_existing(&path)?;
                if let Some(cache) = cache {
                    store = store.with_cache(cache);
                }
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let result = runtime.block_on(async move {
                    use super::caa::{CaaPayload, CaaRequest};
                    let mut wait = ProviderWait::new(cancelled);
                    let listing = client
                        .fetch_cached_with_wait(&mut store, listing_request, &mut wait)
                        .await?
                        .fetch;
                    let mut sources = listing.sources;
                    let content = match listing.payload {
                        CaaPayload::Listing { listing, .. } => {
                            if let Some(id) = request.image_id {
                                let image_request = CaaRequest::image(&listing, &id)?;
                                let fetched = client
                                    .fetch_cached_with_wait(&mut store, image_request, &mut wait)
                                    .await?
                                    .fetch;
                                sources.extend(fetched.sources);
                                match fetched.payload {
                                    CaaPayload::Image { bytes, sha256 } => {
                                        ArtworkContent::Image { bytes, sha256 }
                                    }
                                    CaaPayload::Missing => ArtworkContent::Missing,
                                    _ => {
                                        return Err(Error::Internal(
                                            "CAA image request returned a listing",
                                        ));
                                    }
                                }
                            } else {
                                artwork_listing(&listing)
                            }
                        }
                        CaaPayload::Missing => ArtworkContent::Missing,
                        _ => {
                            return Err(Error::Internal(
                                "CAA listing request returned image bytes",
                            ));
                        }
                    };
                    Ok(ArtworkResult {
                        release: request.release,
                        provider: request.provider,
                        sources,
                        content,
                    })
                });
                if let Err(error) = &result {
                    eprintln!("artwork provider operation failed: {error}");
                }
                result
            })
            .await
            .map_err(|_| Error::Internal("artwork service task failed"))?
        })
    }
}

fn artwork_listing(listing: &super::caa::CaaListing) -> ArtworkContent {
    ArtworkContent::Listing(
        listing
            .images()
            .iter()
            .map(|image| ArtworkImage {
                entity: EntityRef {
                    provider: "cover-art-archive".into(),
                    id: image.id.clone(),
                },
                source: image.source.clone(),
                front: image.front,
                provider_approved: Some(image.provider_approved),
            })
            .collect(),
    )
}

fn cached_artwork(
    store: &mut Store,
    listing_request: &super::caa::CaaRequest,
    request: ArtworkRequest,
) -> Result<Option<ArtworkResult>> {
    use super::caa::{CaaPayload, CaaRequest};
    let Some(cached) = store.caa_cache(listing_request)? else {
        return Ok(None);
    };
    let CaaPayload::Listing { listing, .. } = cached.fetch.payload else {
        return Err(Error::Internal(
            "CAA listing cache contains a different payload",
        ));
    };
    let mut sources = cached.fetch.sources;
    let content = if let Some(id) = &request.image_id {
        let image_request = CaaRequest::image(&listing, id)?;
        let Some(image) = store.caa_cache(&image_request)? else {
            return Ok(None);
        };
        let CaaPayload::Image { bytes, sha256 } = image.fetch.payload else {
            return Err(Error::Internal(
                "CAA image cache contains a different payload",
            ));
        };
        store.touch_caa_cache(&image_request)?;
        sources.extend(image.fetch.sources);
        ArtworkContent::Image { bytes, sha256 }
    } else {
        artwork_listing(&listing)
    };
    store.touch_caa_cache(listing_request)?;
    Ok(Some(ArtworkResult {
        release: request.release,
        provider: request.provider,
        sources,
        content,
    }))
}

/// A host owns private state. Tool requests carry entity IDs, not database
/// paths, provider credentials, arbitrary URLs or filesystem destinations.
pub struct ServiceHost {
    pub catalog: CatalogService,
    pub artwork: Arc<ArtworkService>,
    pub images: crate::application::services::images::ImageService,
    pub musicbrainz: Arc<MusicBrainzService>,
    pub(crate) database: ServiceStore,
}

impl ServiceHost {
    pub fn new(store: Store, configuration: ServiceConfiguration) -> Result<Self> {
        configuration.validate_builtins()?;
        Self::with_catalog(store, configuration, MusicBrainzClient::new()?)
    }

    pub async fn call(
        &self,
        request: crate::application::services::ServiceRequest,
    ) -> Result<crate::application::services::ServiceResponse> {
        request.execute(&self.catalog, &self.artwork).await
    }

    pub(crate) fn with_catalog(
        store: Store,
        configuration: ServiceConfiguration,
        client: MusicBrainzClient,
    ) -> Result<Self> {
        configuration.validate_builtins()?;
        if !configuration.artist_images.is_empty() && store.cache().is_none() {
            return Err(Error::InvalidInput(
                "artist photos require the shared file cache".into(),
            ));
        }
        let processor = Arc::new(super::artwork::NativeImageProcessor::new(store.cache()));
        let mut artist_providers: Vec<
            Arc<dyn crate::application::services::artist_images::ArtistImageProvider>,
        > = Vec::new();
        if !configuration.artist_images.is_empty() {
            artist_providers.push(Arc::new(super::commons::CommonsClient::new(
                store.database_path().to_owned(),
                store
                    .cache()
                    .ok_or(Error::Internal("artist photo cache disappeared"))?,
            )?));
        }
        let artists = crate::application::services::artist_images::ArtistImageProviders::new(
            configuration.clone(),
            artist_providers,
        )?;
        let mut artwork =
            CaaArtworkService::new(store.database_path(), super::caa::CaaHttpClient::new()?);
        let slots = Arc::new(Semaphore::new(4));
        artwork.slots = slots.clone();
        if let Some(cache) = store.cache() {
            artwork = artwork.with_cache(cache);
        }
        let artwork = Arc::new(artwork);
        let database = ServiceStore::new(store);
        let musicbrainz = Arc::new(MusicBrainzService::new(
            database.clone(),
            client,
            configuration.clone(),
            slots,
        ));
        let artwork = Arc::new(ArtworkService::new(configuration.clone(), vec![artwork])?);
        let images =
            crate::application::services::images::ImageService::new(artwork.clone(), processor)
                .with_artists(artists);
        Ok(Self {
            catalog: CatalogService::new(configuration.clone(), vec![musicbrainz.clone()])?,
            artwork,
            images,
            musicbrainz,
            database,
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HostConfiguration {
    database: std::path::PathBuf,
    #[serde(default)]
    services: ServiceConfiguration,
    cache: Option<super::cache::CacheConfiguration>,
}

/// CLI host configuration is local operator input, not part of tool arguments.
fn read_host(configuration: &std::path::Path) -> Result<HostConfiguration> {
    let host: HostConfiguration =
        serde_json::from_slice(&super::local_file::read_local(configuration, 16384, false)?)
            .map_err(|_| Error::InvalidInput("invalid service host configuration".into()))?;
    host.services.validate_builtins()?;
    if !host.database.is_absolute() {
        return Err(Error::InvalidInput(
            "service database must be an absolute local path".into(),
        ));
    }
    Ok(host)
}

fn open_host(host: HostConfiguration, files: &[&std::path::Path]) -> Result<ServiceHost> {
    let store = Store::open_read_snapshot(&host.database)?;
    super::local_file::protect_files(&store, files)?;
    drop(store);
    let mut store = Store::open_current_existing(&host.database)?;
    if let Some(configuration) = host.cache {
        let cache = configuration.open(&mut store, files)?;
        store = store.with_cache(cache);
    }
    ServiceHost::new(store, host.services)
}

/// Open current operator-owned state without creating a database or migrating it.
/// The host file must not overlap a configured media root.
pub fn load_host(configuration: &std::path::Path) -> Result<ServiceHost> {
    open_host(read_host(configuration)?, &[configuration])
}

/// Open selected CLI state without migration or cache-root initialization.
/// Extra request files receive the same media/cache overlap checks as the host.
pub fn load_cli_store(
    configuration: &std::path::Path,
    protected: &[&std::path::Path],
) -> Result<(Store, ServiceConfiguration)> {
    open_cli_store(read_host(configuration)?, configuration, protected, false)
}

/// Prepare a version-bound plan from the selected cache, without provider I/O.
pub fn prepare_cli_catalog_plan(
    configuration: &std::path::Path,
    release: &str,
    request_file: &std::path::Path,
) -> Result<super::store::StoredPlan> {
    let request =
        serde_json::from_slice(&super::local_file::read_local(request_file, 16384, false)?)
            .map_err(|_| Error::InvalidInput("invalid catalog plan request".into()))?;
    let (mut store, services) = load_cli_store(configuration, &[request_file])?;
    services.require(Capability::Metadata, "musicbrainz")?;
    crate::application::catalog::prepare_cached_catalog_plan(&mut store, release, request)
}

pub(crate) fn load_cli_source_store(
    configuration: &std::path::Path,
    database: &std::path::Path,
    protected: &[&std::path::Path],
) -> Result<Store> {
    let host = read_host(configuration)?;
    host.services.require(Capability::Acquisition, "soulseek")?;
    let identity = |path: &std::path::Path| -> Result<String> {
        let parent = super::filesystem::LibraryDirectory::open(
            path.parent()
                .ok_or_else(|| Error::InvalidInput("database parent is missing".into()))?,
        )?;
        let file = parent.open_file(
            path.file_name()
                .ok_or_else(|| Error::InvalidInput("database name is missing".into()))?,
        )?;
        super::filesystem::native_file_identity(&file)
    };
    if !database.is_absolute() || identity(database)? != identity(&host.database)? {
        return Err(Error::Conflict(
            "source and host configurations must identify the same database".into(),
        ));
    }
    Ok(open_cli_store(host, configuration, protected, true)?.0)
}

fn open_cli_store(
    host: HostConfiguration,
    configuration: &std::path::Path,
    protected: &[&std::path::Path],
    read_only: bool,
) -> Result<(Store, ServiceConfiguration)> {
    let mut files = vec![configuration];
    files.extend_from_slice(protected);
    let mut store = Store::open_read_current(&host.database)?;
    super::local_file::protect_files(&store, &files)?;
    let cache = host
        .cache
        .map(|cache| cache.open_bound(&mut store, &files))
        .transpose()?;
    if !read_only {
        drop(store);
        store = Store::open_current_existing(&host.database)?;
    }
    if let Some(cache) = cache {
        store = store.with_cache(cache);
    }
    Ok((store, host.services))
}

pub fn load_call(
    configuration: &std::path::Path,
    request_file: &std::path::Path,
) -> Result<(ServiceHost, crate::application::services::ServiceRequest)> {
    use super::local_file::read_local;
    let host = read_host(configuration)?;
    let request: crate::application::services::ServiceRequest =
        serde_json::from_slice(&read_local(request_file, 16384, false)?)
            .map_err(|_| Error::InvalidInput("invalid service request".into()))?;
    request.validate()?;
    Ok((open_host(host, &[configuration, request_file])?, request))
}
/// Credentials and server connection setup stay in the private worker host.
pub struct SoulseekAcquisition {
    pub(crate) session: super::soulseek::SoulseekSession,
    pub(crate) peer: String,
}

impl crate::application::services::acquisition::AcquisitionProvider for SoulseekAcquisition {
    fn id(&self) -> &str {
        "soulseek"
    }

    fn acquire(
        &mut self,
        store: &mut Store,
        claim: &super::store::JobClaim,
        staging: &super::staging::StagingRoot,
    ) -> Result<i64> {
        let mut peer = self.session.connect_peer(&self.peer)?;
        crate::application::acquire_album(store, claim, staging, &mut peer)
    }
}

#[cfg(test)]
mod provider_wait_tests {
    use super::*;
    use crate::infrastructure::store::{CatalogProviderState, CatalogStatus};

    fn status(state: CatalogProviderState, seconds: Option<u32>) -> CatalogStatus {
        CatalogStatus {
            state,
            retry_after_seconds: seconds.unwrap_or(1),
            known_retry_after_seconds: seconds,
            failures: 0,
        }
    }

    #[tokio::test]
    async fn provider_wait_never_shortens_long_backoff_or_clears_a_pause() -> Result<()> {
        let mut wait = ProviderWait::new(Arc::new(AtomicBool::new(false)));
        assert!(matches!(
            wait.pause(status(CatalogProviderState::Backoff, Some(120)))
                .await,
            Err(Error::Catalog {
                retry_after_seconds: 120,
                ..
            })
        ));
        assert!(matches!(
            wait.pause(status(CatalogProviderState::Paused, None)).await,
            Err(Error::Catalog {
                retry_after_seconds: u32::MAX,
                ..
            })
        ));
        assert_eq!(wait.remaining, Duration::from_secs(15));
        Ok(())
    }

    #[tokio::test]
    async fn provider_wait_spends_one_shared_allowance_and_cancels_pending_work() -> Result<()> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut wait = ProviderWait::new(cancelled.clone());
        wait.remaining = Duration::from_millis(1150);
        wait.pause(status(CatalogProviderState::Active, None))
            .await?;
        assert!(wait.remaining <= Duration::from_millis(1050));
        assert!(matches!(
            wait.pause(status(CatalogProviderState::Active, None)).await,
            Err(Error::ProviderActive)
        ));
        cancelled.store(true, Ordering::Release);
        assert!(matches!(wait.check(), Err(Error::Busy(_))));
        Ok(())
    }

    #[tokio::test]
    async fn provider_completion_is_not_rejected_by_database_admission() -> Result<()> {
        let root = tempfile::tempdir()?;
        let database = ServiceStore::new(Store::open(&root.path().join("state.db"))?);
        let _slots = database
            .slots
            .acquire_many(4)
            .await
            .map_err(|_| Error::Internal("test semaphore closed"))?;
        assert!(matches!(
            database.run(|_| Ok(())).await,
            Err(Error::Busy(_))
        ));
        database
            .complete(|store| {
                store.catalog_status()?;
                Ok(())
            })
            .await
    }

    #[tokio::test]
    async fn provider_host_shares_capacity_between_musicbrainz_and_caa() -> Result<()> {
        let root = tempfile::tempdir()?;
        let host = ServiceHost::new(
            Store::open(&root.path().join("state.db"))?,
            Default::default(),
        )?;
        let _slots = host
            .musicbrainz
            .slots
            .acquire_many(4)
            .await
            .map_err(|_| Error::Internal("test semaphore closed"))?;
        let result = host
            .artwork
            .fetch(ArtworkRequest {
                release: EntityRef {
                    provider: "musicbrainz".into(),
                    id: "20000000-0000-4000-8000-000000000001".into(),
                },
                provider: "cover-art-archive".into(),
                image_id: None,
            })
            .await;
        assert!(matches!(result, Err(Error::ProviderSaturated)));
        Ok(())
    }
}
