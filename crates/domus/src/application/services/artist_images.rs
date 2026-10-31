//! Explicit artist/file associations. Provider identity and image rights are not
//! release evidence, album-cover approval or a grant of personality rights.
use super::{EntityRef, ServiceConfiguration};
use crate::{Error, Result};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ImageRights {
    pub title: String,
    pub creator: String,
    pub credit: String,
    pub license: String,
    pub license_url: String,
    pub source_url: String,
    pub notices: String,
}

impl ImageRights {
    pub fn validate(&self) -> Result<()> {
        let url = match self.license.as_str() {
            "CC0 1.0" => "https://creativecommons.org/publicdomain/zero/1.0/",
            "CC BY 3.0" => "https://creativecommons.org/licenses/by/3.0/",
            "CC BY 4.0" => "https://creativecommons.org/licenses/by/4.0/",
            "CC BY-SA 3.0" => "https://creativecommons.org/licenses/by-sa/3.0/",
            "CC BY-SA 4.0" => "https://creativecommons.org/licenses/by-sa/4.0/",
            _ => {
                return Err(Error::InvalidInput(
                    "artist photo has unsupported rights".into(),
                ));
            }
        };
        if self.license_url != url
            || self.title.trim().is_empty()
            || self.creator.trim().is_empty()
            || self.source_url.is_empty()
            || reqwest::Url::parse(&self.source_url)
                .ok()
                .is_none_or(|url| {
                    url.scheme() != "https"
                        || url.host_str().is_none()
                        || !url.username().is_empty()
                        || url.password().is_some()
                })
            || [&self.title, &self.creator, &self.credit, &self.notices]
                .iter()
                .any(|value| {
                    value.len() > 8192 || value.chars().any(|c| c.is_control() && c != '\n')
                })
        {
            return Err(Error::InvalidInput(
                "artist photo rights or credit are incomplete".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ArtistImageSource {
    pub file: String,
    pub entity_id: String,
    pub revision: String,
    pub timestamp: String,
    pub sha1: Option<String>,
    pub bytes: u32,
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub original_url: String,
    pub rights: ImageRights,
}

impl ArtistImageSource {
    pub fn fingerprint(&self) -> Result<String> {
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }

    pub fn validate(&self) -> Result<()> {
        super::EntityRef {
            provider: "image".into(),
            id: self.file.clone(),
        }
        .validate()?;
        self.rights.validate()?;
        if self.entity_id.is_empty()
            || self.entity_id.len() > 512
            || self.revision.is_empty()
            || self.revision.len() > 512
            || self.timestamp.is_empty()
            || self.sha1.as_ref().is_some_and(|sha1| {
                sha1.len() != 40 || !sha1.bytes().all(|c| c.is_ascii_hexdigit())
            })
            || self.bytes == 0
            || self.bytes > 8 * 1024 * 1024
            || self.width == 0
            || self.height == 0
            || self.width > 8192
            || self.height > 8192
            || u64::from(self.width) * u64::from(self.height) > 16 * 1024 * 1024
            || !matches!(self.mime.as_str(), "image/jpeg" | "image/png")
        {
            return Err(Error::InvalidInput(
                "artist photo metadata exceeds supported bounds".into(),
            ));
        }
        Ok(())
    }
}

pub fn validate_file(file: &str) -> Result<()> {
    if !file.starts_with("File:")
        || file.len() <= 5
        || file.len() > 512
        || file
            .chars()
            .any(|c| c.is_control() || matches!(c, '|' | '#' | '\\' | '/' | '[' | ']' | '{' | '}'))
    {
        return Err(Error::InvalidInput(
            "use one explicit Commons File title, not a URL".into(),
        ));
    }
    Ok(())
}

pub fn validate_artist(artist: &EntityRef) -> Result<()> {
    artist.validate()?;
    if artist.provider != "musicbrainz" {
        return Err(Error::InvalidInput(
            "this host supports explicit MusicBrainz artist identities".into(),
        ));
    }
    if !crate::domain::identifiers::is_canonical_uuid(&artist.id) {
        return Err(Error::InvalidInput(
            "artist ID must be a canonical MusicBrainz UUID".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ArtistImageEvidence {
    pub source: ArtistImageSource,
    pub fetched_at: u64,
    pub expires_at: u64,
}

/// A replaceable original-source boundary. Implementations own transport,
/// file identity, freshness and rights parsing, never resizing or approval.
pub trait ArtistImageProvider: Send + Sync {
    fn id(&self) -> &str;
    fn matches_file(&self, requested: &str, source: &ArtistImageSource) -> bool {
        requested == source.file
    }
    fn inspect(&self, file: String) -> BoxFuture<'_, Result<Option<ArtistImageEvidence>>>;
    fn original(&self, source: ArtistImageSource) -> BoxFuture<'_, Result<Vec<u8>>>;
}

pub struct ArtistImageProviders {
    configuration: ServiceConfiguration,
    providers: Vec<Arc<dyn ArtistImageProvider>>,
}

impl ArtistImageProviders {
    pub fn new(
        configuration: ServiceConfiguration,
        providers: Vec<Arc<dyn ArtistImageProvider>>,
    ) -> Result<Self> {
        configuration.validate()?;
        let mut seen = std::collections::HashSet::new();
        for provider in &providers {
            super::provider_name(provider.id())?;
            if !seen.insert(provider.id()) {
                return Err(Error::InvalidInput(
                    "duplicate artist-image provider".into(),
                ));
            }
        }
        if configuration
            .artist_images
            .iter()
            .any(|id| !seen.contains(id.as_str()))
        {
            return Err(Error::InvalidInput(
                "artist-image provider is not installed".into(),
            ));
        }
        Ok(Self {
            configuration,
            providers,
        })
    }

    pub fn provider(&self, id: &str) -> Result<&dyn ArtistImageProvider> {
        self.configuration
            .require(super::Capability::ArtistImages, id)?;
        self.providers
            .iter()
            .find(|p| p.id() == id)
            .map(Arc::as_ref)
            .ok_or(Error::Internal("artist-image provider disappeared"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ArtistPhoto {
    pub artist: EntityRef,
    pub provider: String,
    pub source: ArtistImageSource,
    pub source_sha256: String,
    pub original_sha256: String,
    pub jpeg_sha256: String,
    pub width: u32,
    pub height: u32,
    pub fetched_at: u64,
    pub expires_at: u64,
}

impl ArtistPhoto {
    pub fn validate(&self) -> Result<()> {
        validate_artist(&self.artist)?;
        super::provider_name(&self.provider)?;
        self.source.validate()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::Internal("artist photo clock is invalid"))?
            .as_secs();
        if [
            &self.source_sha256,
            &self.original_sha256,
            &self.jpeg_sha256,
        ]
        .iter()
        .any(|value| {
            value.len() != 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) || self.source_sha256 != self.source.fingerprint()?
            || self.width == 0
            || self.height == 0
            || self.width.max(self.height) > 800
            || self.fetched_at > now
            || self.expires_at <= now
        {
            return Err(Error::InvalidInput(
                "artist photo confirmation or freshness is invalid".into(),
            ));
        }
        Ok(())
    }

    pub fn confirmation(&self) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                &self.artist,
                &self.provider,
                &self.source_sha256,
                &self.original_sha256,
                &self.jpeg_sha256,
            ))?)
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct ArtistPhotoBinding {
    pub id: i64,
    pub version: i64,
    pub photo: ArtistPhoto,
}

#[derive(Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct CheckArtistPhoto {
    pub artist: EntityRef,
    pub provider: String,
    pub file: String,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct SaveArtistPhoto {
    pub selection: CheckArtistPhoto,
    pub expected_version: Option<i64>,
    pub expected_confirmation: String,
    pub acknowledge_permitted_use: bool,
}

#[cfg(test)]
pub(crate) mod tests;
