//! In-process service ports shared by HTTP, CLI and the local stdio MCP adapter.
//! Provider identifiers are scoped, never interchangeable release evidence.
use crate::domain::CatalogReleaseFilters;
use crate::{Error, Result};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

pub mod acquisition;
pub mod cache;
pub mod images;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRef {
    pub provider: String,
    pub id: String,
}

impl EntityRef {
    pub fn validate(&self) -> Result<()> {
        provider_name(&self.provider)?;
        if self.id.is_empty() || self.id.len() > 512 || self.id.chars().any(char::is_control) {
            return Err(Error::InvalidInput("invalid provider entity ID".into()));
        }
        Ok(())
    }
}

fn provider_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(Error::InvalidInput("invalid provider name".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceConfiguration {
    pub discovery: Vec<String>,
    pub metadata: Vec<String>,
    pub artwork: Vec<String>,
    pub acquisition: Vec<String>,
}

impl Default for ServiceConfiguration {
    fn default() -> Self {
        Self {
            discovery: vec!["musicbrainz".into()],
            metadata: vec!["musicbrainz".into()],
            artwork: vec!["cover-art-archive".into()],
            acquisition: vec!["soulseek".into()],
        }
    }
}

impl ServiceConfiguration {
    pub fn validate(&self) -> Result<()> {
        for providers in [
            &self.discovery,
            &self.metadata,
            &self.artwork,
            &self.acquisition,
        ] {
            if providers.len() > 8 {
                return Err(Error::InvalidInput(
                    "a service supports at most eight configured providers".into(),
                ));
            }
            let mut seen = HashSet::new();
            for provider in providers {
                provider_name(provider)?;
                if !seen.insert(provider) {
                    return Err(Error::InvalidInput("duplicate service provider".into()));
                }
            }
        }
        Ok(())
    }

    /// Validate the installed built-ins before startup, claims or any network I/O.
    /// A future adapter must be installed and registered, not just named in JSON.
    pub fn validate_builtins(&self) -> Result<()> {
        self.validate()?;
        for (providers, installed) in [
            (&self.discovery, "musicbrainz"),
            (&self.metadata, "musicbrainz"),
            (&self.artwork, "cover-art-archive"),
            (&self.acquisition, "soulseek"),
        ] {
            if providers.iter().any(|provider| provider != installed) {
                return Err(Error::InvalidInput(
                    "configured provider is not installed for this service".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn require(&self, capability: Capability, provider: &str) -> Result<()> {
        self.validate()?;
        let configured = match capability {
            Capability::Discovery => &self.discovery,
            Capability::Metadata => &self.metadata,
            Capability::Artwork => &self.artwork,
            Capability::Acquisition => &self.acquisition,
        };
        if !configured.iter().any(|value| value == provider) {
            return Err(Error::Conflict(
                "provider is disabled for the requested service".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Capability {
    Discovery,
    Metadata,
    Artwork,
    Acquisition,
}

/// Provider-native pagination stays inside each provider result. No merged
/// cursor, deduplication, cross-provider ID substitution or hidden fallback.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryQuery {
    Artists {
        text: String,
        offset: u32,
        limit: u16,
    },
    Albums {
        text: String,
        artist: Option<EntityRef>,
        offset: u32,
        limit: u16,
        #[serde(default)]
        filters: CatalogReleaseFilters,
    },
    ArtistReleases {
        artist: EntityRef,
        offset: u32,
        limit: u16,
        #[serde(default)]
        filters: CatalogReleaseFilters,
    },
}

impl DiscoveryQuery {
    fn validate(&self) -> Result<()> {
        let (text, artist, limit) = match self {
            Self::Artists { text, limit, .. } => (Some(text), None, *limit),
            Self::Albums {
                text,
                artist,
                limit,
                ..
            } => (Some(text), artist.as_ref(), *limit),
            Self::ArtistReleases { artist, limit, .. } => (None, Some(artist), *limit),
        };
        if !(1..=25).contains(&limit)
            || text.is_some_and(|text| {
                text.trim().is_empty() || text.len() > 512 || text.chars().any(char::is_control)
            })
        {
            return Err(Error::InvalidInput(
                "discovery needs bounded text and a page limit of 1 through 25".into(),
            ));
        }
        if let Some(artist) = artist {
            artist.validate()?;
        }
        Ok(())
    }

    pub fn artist(&self) -> Option<&EntityRef> {
        match self {
            Self::Artists { .. } => None,
            Self::Albums { artist, .. } => artist.as_ref(),
            Self::ArtistReleases { artist, .. } => Some(artist),
        }
    }

    fn bounds(&self) -> (u32, u16) {
        match self {
            Self::Artists { offset, limit, .. }
            | Self::Albums { offset, limit, .. }
            | Self::ArtistReleases { offset, limit, .. } => (*offset, *limit),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryRequest {
    /// Omission selects the configured providers. An explicit empty list is invalid.
    pub providers: Option<Vec<String>>,
    pub query: DiscoveryQuery,
}

#[derive(Debug, Serialize)]
pub struct CatalogItem {
    pub entity: EntityRef,
    pub title: String,
    pub artist: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiscoveryPage {
    pub offset: u32,
    pub total: Option<u32>,
    pub next_offset: Option<u32>,
    pub items: Vec<CatalogItem>,
    pub source: String,
    pub fetched_at: i64,
}

#[derive(Debug, Serialize)]
pub struct MetadataTrack {
    pub title: String,
    pub artist: String,
    pub disc: Option<u16>,
    pub position: Option<u16>,
    pub duration_ms: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ReleaseMetadata {
    pub entity: EntityRef,
    pub title: String,
    pub artist: String,
    pub date: Option<String>,
    pub tracks: Vec<MetadataTrack>,
    pub source: String,
    pub fetched_at: i64,
}

pub trait CatalogProvider: Send + Sync {
    fn id(&self) -> &str;
    fn discover(&self, query: DiscoveryQuery) -> BoxFuture<'_, Result<DiscoveryPage>>;
    fn metadata(&self, release: EntityRef) -> BoxFuture<'_, Result<ReleaseMetadata>>;
}

#[derive(Debug, Serialize)]
pub struct ProviderPage {
    pub provider: String,
    pub page: DiscoveryPage,
}

#[derive(Debug, Serialize)]
pub struct ProviderFailure {
    pub provider: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct DiscoveryReport {
    pub results: Vec<ProviderPage>,
    pub failures: Vec<ProviderFailure>,
}

pub struct CatalogService {
    configuration: ServiceConfiguration,
    providers: Vec<Arc<dyn CatalogProvider>>,
}

impl CatalogService {
    pub fn new(
        configuration: ServiceConfiguration,
        providers: Vec<Arc<dyn CatalogProvider>>,
    ) -> Result<Self> {
        configuration.validate()?;
        let mut seen = HashSet::new();
        for provider in &providers {
            provider_name(provider.id())?;
            if !seen.insert(provider.id()) {
                return Err(Error::InvalidInput(
                    "duplicate installed catalog provider".into(),
                ));
            }
        }
        if configuration
            .discovery
            .iter()
            .chain(&configuration.metadata)
            .any(|id| !providers.iter().any(|provider| provider.id() == id))
        {
            return Err(Error::InvalidInput(
                "configured catalog provider is not installed".into(),
            ));
        }
        Ok(Self {
            configuration,
            providers,
        })
    }

    pub async fn discover(&self, request: DiscoveryRequest) -> Result<DiscoveryReport> {
        request.query.validate()?;
        let selected = request
            .providers
            .as_ref()
            .unwrap_or(&self.configuration.discovery);
        if selected.is_empty() || selected.len() > 8 {
            return Err(Error::InvalidInput(
                "select 1 through 8 enabled discovery providers".into(),
            ));
        }
        let mut seen = HashSet::new();
        for id in selected {
            self.configuration.require(Capability::Discovery, id)?;
            if !seen.insert(id) {
                return Err(Error::InvalidInput("duplicate requested provider".into()));
            }
            if request
                .query
                .artist()
                .is_some_and(|artist| artist.provider != *id)
            {
                return Err(Error::InvalidInput(
                    "map the artist identity explicitly before querying another provider".into(),
                ));
            }
        }
        let mut report = DiscoveryReport {
            results: Vec::new(),
            failures: Vec::new(),
        };
        for id in selected {
            let provider = self
                .providers
                .iter()
                .find(|provider| provider.id() == id)
                .ok_or(Error::Internal("configured catalog provider disappeared"))?;
            match provider.discover(request.query.clone()).await {
                Ok(page) => {
                    let (offset, limit) = request.query.bounds();
                    if page.items.len() > usize::from(limit)
                        || page.offset != offset
                        || (!page.items.is_empty()
                            && page.total.is_some_and(|total| {
                                u64::from(offset) + page.items.len() as u64 > u64::from(total)
                            }))
                        || page.next_offset.is_some_and(|next| {
                            next <= offset || page.total.is_some_and(|total| next >= total)
                        })
                        || page.items.iter().any(|item| {
                            item.entity.provider != *id || item.entity.validate().is_err()
                        })
                    {
                        report.failures.push(ProviderFailure {
                            provider: id.clone(),
                            error: "catalog provider returned invalid or unscoped results".into(),
                        });
                        continue;
                    }
                    report.results.push(ProviderPage {
                        provider: id.clone(),
                        page,
                    });
                }
                Err(error) => report.failures.push(ProviderFailure {
                    provider: id.clone(),
                    error: error.to_string(),
                }),
            }
        }
        Ok(report)
    }

    pub async fn metadata(&self, release: EntityRef) -> Result<ReleaseMetadata> {
        self.require_metadata(&release)?;
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.id() == release.provider)
            .ok_or(Error::Internal("configured metadata provider disappeared"))?;
        let result = provider.metadata(release.clone()).await?;
        if result.entity != release || result.tracks.len() > 1000 {
            return Err(Error::Internal(
                "metadata provider returned a different entity or excessive tracks",
            ));
        }
        Ok(result)
    }

    pub fn require_metadata(&self, release: &EntityRef) -> Result<()> {
        release.validate()?;
        self.configuration
            .require(Capability::Metadata, &release.provider)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtworkRequest {
    pub provider: String,
    pub release: EntityRef,
    pub image_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ArtworkImage {
    pub entity: EntityRef,
    pub source: String,
    pub front: bool,
    pub provider_approved: Option<bool>,
}

/// Fetched bytes are internal input to normalization, never an MCP JSON blob,
/// preview approval, rights grant or proof of an equivalent provider release.
pub enum ArtworkContent {
    Listing(Vec<ArtworkImage>),
    Image { bytes: Vec<u8>, sha256: String },
    Missing,
}

pub struct ArtworkResult {
    pub release: EntityRef,
    pub provider: String,
    pub sources: Vec<String>,
    pub content: ArtworkContent,
}

pub trait ArtworkProvider: Send + Sync {
    fn id(&self) -> &str;
    fn fetch(&self, request: ArtworkRequest) -> BoxFuture<'_, Result<ArtworkResult>>;
}

pub struct ArtworkService {
    configuration: ServiceConfiguration,
    providers: Vec<Arc<dyn ArtworkProvider>>,
}

impl ArtworkService {
    pub fn require_provider(&self, provider: &str) -> Result<()> {
        self.configuration.require(Capability::Artwork, provider)
    }
    pub fn new(
        configuration: ServiceConfiguration,
        providers: Vec<Arc<dyn ArtworkProvider>>,
    ) -> Result<Self> {
        configuration.validate()?;
        let mut seen = HashSet::new();
        for provider in &providers {
            provider_name(provider.id())?;
            if !seen.insert(provider.id()) {
                return Err(Error::InvalidInput(
                    "duplicate installed artwork provider".into(),
                ));
            }
        }
        if configuration
            .artwork
            .iter()
            .any(|id| !providers.iter().any(|provider| provider.id() == id))
        {
            return Err(Error::InvalidInput(
                "configured artwork provider is not installed".into(),
            ));
        }
        Ok(Self {
            configuration,
            providers,
        })
    }

    pub async fn fetch(&self, request: ArtworkRequest) -> Result<ArtworkResult> {
        request.release.validate()?;
        self.require_provider(&request.provider)?;
        if request
            .image_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 512 || id.chars().any(char::is_control))
        {
            return Err(Error::InvalidInput("invalid artwork image ID".into()));
        }
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.id() == request.provider)
            .ok_or(Error::Internal("configured artwork provider disappeared"))?;
        let expected = request.clone();
        let result = provider.fetch(request).await?;
        if result.provider != expected.provider || result.release != expected.release {
            return Err(Error::Internal(
                "artwork provider returned a different provider or release",
            ));
        }
        match &result.content {
            ArtworkContent::Image { bytes, sha256 } => {
                use sha2::{Digest, Sha256};
                if expected.image_id.is_none()
                    || bytes.is_empty()
                    || bytes.len() > 8388608
                    || *sha256 != format!("{:x}", Sha256::digest(bytes))
                {
                    return Err(Error::Internal(
                        "artwork provider returned invalid image bytes or digest",
                    ));
                }
            }
            ArtworkContent::Listing(images)
                if expected.image_id.is_some()
                    || images.len() > 1000
                    || images.iter().any(|image| {
                        image.entity.provider != expected.provider
                            || image.entity.validate().is_err()
                    }) =>
            {
                return Err(Error::Internal(
                    "artwork provider returned an invalid listing",
                ));
            }
            _ => {}
        }
        Ok(result)
    }
}

/// Read operations only. Host state, authentication and write approvals are
/// deliberately absent from tool arguments. An MCP adapter must still supply
/// its protocol schemas and authorization boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServiceRequest {
    Discover {
        request: DiscoveryRequest,
    },
    Metadata {
        release: EntityRef,
    },
    ArtworkList {
        provider: String,
        release: EntityRef,
    },
    ArtworkFetch {
        provider: String,
        release: EntityRef,
        image_id: String,
    },
}

impl ServiceRequest {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Discover { request } => request.query.validate(),
            Self::Metadata { release } => release.validate(),
            Self::ArtworkList { provider, release } => {
                provider_name(provider)?;
                release.validate()
            }
            Self::ArtworkFetch {
                provider,
                release,
                image_id,
            } => {
                provider_name(provider)?;
                release.validate()?;
                if image_id.is_empty()
                    || image_id.len() > 512
                    || image_id.chars().any(char::is_control)
                {
                    return Err(Error::InvalidInput("invalid artwork image ID".into()));
                }
                Ok(())
            }
        }
    }

    pub async fn execute(
        self,
        catalog: &CatalogService,
        artwork: &ArtworkService,
    ) -> Result<ServiceResponse> {
        self.validate()?;
        match self {
            Self::Discover { request } => Ok(ServiceResponse::Discovery {
                report: catalog.discover(request).await?,
            }),
            Self::Metadata { release } => Ok(ServiceResponse::Metadata {
                release: catalog.metadata(release).await?,
            }),
            Self::ArtworkList { provider, release } => {
                let result = artwork
                    .fetch(ArtworkRequest {
                        provider,
                        release,
                        image_id: None,
                    })
                    .await?;
                let images = match result.content {
                    ArtworkContent::Listing(images) => Some(images),
                    ArtworkContent::Missing => None,
                    ArtworkContent::Image { .. } => {
                        return Err(Error::Internal("artwork listing returned image bytes"));
                    }
                };
                Ok(ServiceResponse::Artwork {
                    provider: result.provider,
                    release: result.release,
                    sources: result.sources,
                    images,
                })
            }
            Self::ArtworkFetch {
                provider,
                release,
                image_id,
            } => {
                let result = artwork
                    .fetch(ArtworkRequest {
                        provider,
                        release,
                        image_id: Some(image_id),
                    })
                    .await?;
                let image = match result.content {
                    ArtworkContent::Image { bytes, sha256 } => Some(FetchedArtwork {
                        sha256,
                        bytes: bytes.len() as u64,
                    }),
                    ArtworkContent::Missing => None,
                    ArtworkContent::Listing(_) => {
                        return Err(Error::Internal("image fetch returned a listing"));
                    }
                };
                Ok(ServiceResponse::ArtworkFetched {
                    provider: result.provider,
                    release: result.release,
                    sources: result.sources,
                    image,
                })
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ServiceResponse {
    Discovery {
        report: DiscoveryReport,
    },
    Metadata {
        release: ReleaseMetadata,
    },
    Artwork {
        provider: String,
        release: EntityRef,
        sources: Vec<String>,
        /// Null means the provider explicitly reported missing artwork.
        images: Option<Vec<ArtworkImage>>,
    },
    ArtworkFetched {
        provider: String,
        release: EntityRef,
        sources: Vec<String>,
        image: Option<FetchedArtwork>,
    },
}

#[derive(Debug, Serialize)]
pub struct FetchedArtwork {
    pub sha256: String,
    pub bytes: u64,
}

#[cfg(test)]
mod tests;
