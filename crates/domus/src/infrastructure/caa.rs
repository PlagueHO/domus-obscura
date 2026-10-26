use crate::domain::identifiers::is_canonical_uuid;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

mod transport;
pub use transport::{CaaFetch, CaaHttpClient, CaaPayload, CaaRequest, CaaResponse};

pub const MAX_CAA_LISTING: usize = 2 * 1024 * 1024;
pub const MAX_CAA_IMAGES: usize = 200;

/// Validated listing metadata, not evidence that image bytes were downloaded.
/// Provider edit approval is neither user approval nor a rights grant.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct CaaListing {
    release_id: String,
    source: String,
    payload_sha256: String,
    images: Vec<CaaImage>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct CaaImage {
    pub id: String,
    pub source: String,
    pub request_source: String,
    pub edit_id: String,
    pub types: Vec<String>,
    pub front: bool,
    pub back: bool,
    pub provider_approved: bool,
    pub comment: String,
}

impl CaaListing {
    /// Bind an entire bounded response to one exact release. Never substitute
    /// release-group artwork or infer missing fields from a preferred image.
    pub fn parse(release_id: &str, body: &[u8]) -> Result<Self> {
        if !is_canonical_uuid(release_id) {
            return Err(Error::InvalidInput(
                "CAA requires a canonical exact-release ID".into(),
            ));
        }
        if body.is_empty() || body.len() > MAX_CAA_LISTING {
            return Err(bad("CAA listing exceeds its body limit or is empty"));
        }
        let raw: RawListing =
            serde_json::from_slice(body).map_err(|_| bad("CAA listing JSON is invalid"))?;
        if raw.release != format!("https://musicbrainz.org/release/{release_id}")
            && raw.release != format!("http://musicbrainz.org/release/{release_id}")
        {
            return Err(bad("CAA listing does not describe the exact release"));
        }
        if raw.images.len() > MAX_CAA_IMAGES {
            return Err(bad("CAA listing exceeds 200 images"));
        }
        let mut ids = HashSet::new();
        let mut images = Vec::with_capacity(raw.images.len());
        for image in raw.images {
            let id = image.id.validate()?;
            if !ids.insert(id.clone()) {
                return Err(bad("CAA listing has duplicate image IDs"));
            }
            let edit_id = image.edit.validate()?;
            let request_source = format!("https://coverartarchive.org/release/{release_id}/{id}");
            let source = image_source(release_id, &id, &image.image)?;
            let mut types = HashSet::new();
            if image.types.len() > 32
                || image.types.iter().any(|value| {
                    value.is_empty()
                        || value.len() > 128
                        || value.chars().any(char::is_control)
                        || !types.insert(value.as_str())
                })
            {
                return Err(bad("CAA image types are invalid or exceed their limit"));
            }
            if (image.front && !types.contains("Front")) || (image.back && !types.contains("Back"))
            {
                return Err(bad("CAA preferred image flags contradict its types"));
            }
            if image.comment.len() > 4096
                || image
                    .comment
                    .chars()
                    .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\r' | '\n'))
            {
                return Err(bad("CAA image comment is invalid or exceeds its limit"));
            }
            images.push(CaaImage {
                id,
                source,
                request_source,
                edit_id,
                types: image.types,
                front: image.front,
                back: image.back,
                provider_approved: image.approved,
                comment: image.comment,
            });
        }
        Ok(Self {
            release_id: release_id.into(),
            source: format!("https://coverartarchive.org/release/{release_id}/"),
            payload_sha256: format!("{:x}", Sha256::digest(body)),
            images,
        })
    }

    pub fn release_id(&self) -> &str {
        &self.release_id
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn payload_sha256(&self) -> &str {
        &self.payload_sha256
    }

    pub fn images(&self) -> &[CaaImage] {
        &self.images
    }

    pub fn image(&self, id: &str) -> Result<&CaaImage> {
        self.images
            .iter()
            .find(|image| image.id == id)
            .ok_or_else(|| Error::InvalidInput("CAA image is not in the release listing".into()))
    }
}

pub(crate) fn is_image_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn image_source(release: &str, id: &str, source: &str) -> Result<String> {
    let path = format!("coverartarchive.org/release/{release}/{id}.");
    let extension = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
        .and_then(|source| source.strip_prefix(&path))
        .ok_or_else(|| bad("CAA image source does not match its release and image ID"))?;
    if extension.is_empty()
        || extension.len() > 10
        || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(bad("CAA image source has an invalid filename extension"));
    }
    Ok(format!("https://{path}{extension}"))
}

fn bad(message: &'static str) -> Error {
    Error::Remote(message)
}

#[derive(Deserialize)]
struct RawListing {
    release: String,
    images: Vec<RawImage>,
}

#[derive(Deserialize)]
struct RawImage {
    id: RawId,
    edit: RawId,
    image: String,
    types: Vec<String>,
    front: bool,
    back: bool,
    approved: bool,
    comment: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawId {
    Text(String),
    Number(u64),
}

impl RawId {
    fn validate(self) -> Result<String> {
        let value = match self {
            Self::Text(value) => value,
            Self::Number(value) => value.to_string(),
        };
        if !is_image_id(&value) {
            return Err(bad("CAA identifier must be a bounded positive decimal"));
        }
        Ok(value)
    }
}
