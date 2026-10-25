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
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

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
}

pub struct MusicBrainzService {
    database: ServiceStore,
    client: MusicBrainzClient,
    configuration: ServiceConfiguration,
}

impl MusicBrainzService {
    pub(crate) fn new(
        database: ServiceStore,
        client: MusicBrainzClient,
        configuration: ServiceConfiguration,
    ) -> Self {
        Self {
            database,
            client,
            configuration,
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
        let mut permit = self.database.run(Store::claim_catalog_request).await?;
        let outcome = self.client.execute(&query, &mut permit).await;
        self.database
            .run(move |store| {
                crate::application::catalog::complete_catalog(store, query, permit, outcome, parse)
            })
            .await
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
}

impl CaaArtworkService {
    pub fn new(database: &std::path::Path, client: super::caa::CaaHttpClient) -> Self {
        Self {
            database: database.into(),
            client,
            slots: Arc::new(Semaphore::new(4)),
        }
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
            let permit = self
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::Busy("artwork service is busy; retry later"))?;
            let path = self.database.clone();
            let client = self.client.clone();
            // CAA's existing redirect/budget orchestration includes SQLite.
            // Keep that work on a blocking thread with an independent connection.
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut store = Store::open_existing(&path)?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(async move {
                    use super::caa::{CaaPayload, CaaRequest};
                    let listing = client.fetch_cached(&mut store, listing_request).await?.fetch;
                    let mut sources = listing.sources;
                    let content = match listing.payload {
                        CaaPayload::Listing { listing, .. } => {
                            if let Some(id) = request.image_id {
                                let image_request = CaaRequest::image(&listing, &id)?;
                                let delay = if store.caa_cache(&image_request)?.is_some() { 0 }
                                    else { store.catalog_status()?.retry_after_seconds };
                                if delay > 15 {
                                    return Err(Error::Catalog {
                                        reason: "CAA image requires provider backoff before a new request",
                                        retry_after_seconds: delay,
                                    });
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(u64::from(delay))).await;
                                let fetched = client
                                    .fetch_cached(&mut store, image_request)
                                    .await?.fetch;
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
                        }
                        CaaPayload::Missing => ArtworkContent::Missing,
                        _ => {
                            return Err(Error::Internal("CAA listing request returned image bytes"));
                        }
                    };
                    Ok(ArtworkResult {
                        release: request.release,
                        provider: request.provider,
                        sources,
                        content,
                    })
                })
            })
            .await
            .map_err(|_| Error::Internal("artwork service task failed"))?
        })
    }
}

/// A host owns private state. Tool requests carry entity IDs, not database
/// paths, provider credentials, arbitrary URLs or filesystem destinations.
pub struct ServiceHost {
    pub catalog: CatalogService,
    pub artwork: ArtworkService,
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
        let artwork = Arc::new(CaaArtworkService::new(
            store.database_path(),
            super::caa::CaaHttpClient::new()?,
        ));
        let database = ServiceStore::new(store);
        let musicbrainz = Arc::new(MusicBrainzService::new(
            database.clone(),
            client,
            configuration.clone(),
        ));
        Ok(Self {
            catalog: CatalogService::new(configuration.clone(), vec![musicbrainz.clone()])?,
            artwork: ArtworkService::new(configuration, vec![artwork])?,
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
    ServiceHost::new(Store::open_current_existing(&host.database)?, host.services)
}

/// Open current operator-owned state without creating a database or migrating it.
/// The host file must not overlap a configured media root.
pub fn load_host(configuration: &std::path::Path) -> Result<ServiceHost> {
    open_host(read_host(configuration)?, &[configuration])
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
