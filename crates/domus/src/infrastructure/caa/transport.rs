use super::{CaaListing, MAX_CAA_LISTING, bad};
use crate::infrastructure::store::{CatalogPermit, Store};
use crate::{Error, Result};
use reqwest::Url;
use std::time::Duration;

pub(crate) const MAX_IMAGE: usize = 8 * 1024 * 1024;
const MAX_REDIRECTS: usize = 4;

#[derive(Debug, Clone)]
enum Kind {
    Listing,
    Image { filename: String },
}

/// Only a release listing or an image from that validated listing can start
/// a request. Each redirect must retain the exact archive item and filename.
#[derive(Debug, Clone)]
pub struct CaaRequest {
    release: String,
    kind: Kind,
    chain: Vec<Url>,
}

impl CaaRequest {
    pub fn listing(release: &str) -> Result<Self> {
        if !crate::domain::identifiers::is_canonical_uuid(release) {
            return Err(Error::InvalidInput(
                "CAA requires a canonical exact-release ID".into(),
            ));
        }
        Self::new(
            release,
            Kind::Listing,
            &format!("https://coverartarchive.org/release/{release}/"),
        )
    }

    pub fn image(listing: &CaaListing, id: &str) -> Result<Self> {
        let image = listing.image(id)?;
        let extension = image
            .source
            .rsplit_once('.')
            .ok_or_else(|| Error::Internal("validated CAA image has no extension"))?
            .1;
        Self::new(
            listing.release_id(),
            Kind::Image {
                filename: format!("mbid-{}-{id}.{extension}", listing.release_id()),
            },
            &image.request_source,
        )
    }

    fn new(release: &str, kind: Kind, source: &str) -> Result<Self> {
        let url = Url::parse(source).map_err(|_| Error::Internal("CAA URL construction failed"))?;
        Ok(Self {
            release: release.into(),
            kind,
            chain: vec![url],
        })
    }

    pub fn source(&self) -> &str {
        self.url().as_str()
    }

    pub fn sources(&self) -> Vec<String> {
        self.chain.iter().map(ToString::to_string).collect()
    }

    pub(crate) fn cache_key(&self) -> String {
        match &self.kind {
            Kind::Listing => self.chain[0].to_string(),
            Kind::Image { filename } => format!("{}#{filename}", self.chain[0]),
        }
    }

    pub(crate) fn validate_sources(&self, sources: &[String]) -> Result<()> {
        if sources.is_empty()
            || sources.len() > MAX_REDIRECTS + 1
            || sources[0] != self.chain[0].as_str()
        {
            return Err(bad("CAA evidence has a different initial source"));
        }
        let mut expected = Self {
            release: self.release.clone(),
            kind: self.kind.clone(),
            chain: vec![self.chain[0].clone()],
        };
        for source in &sources[1..] {
            expected = expected.follow(source)?;
        }
        if expected.sources() != sources {
            return Err(bad("CAA evidence source chain is not canonical"));
        }
        Ok(())
    }

    pub(crate) fn restore_payload(&self, bytes: Vec<u8>) -> Result<CaaPayload> {
        match &self.kind {
            Kind::Listing => Ok(CaaPayload::Listing {
                listing: CaaListing::parse(&self.release, &bytes)?,
                raw: bytes,
            }),
            Kind::Image { .. } => {
                if bytes.is_empty() || bytes.len() > MAX_IMAGE {
                    return Err(bad("CAA image evidence exceeds its body limit"));
                }
                use sha2::{Digest, Sha256};
                Ok(CaaPayload::Image {
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    bytes,
                })
            }
        }
    }

    pub(crate) fn validate_payload<'a>(&self, payload: &'a CaaPayload) -> Result<Option<&'a [u8]>> {
        use sha2::{Digest, Sha256};
        match (&self.kind, payload) {
            (Kind::Listing, CaaPayload::Listing { listing, raw }) => {
                let restored = CaaListing::parse(&self.release, raw)?;
                if serde_json::to_vec(&restored)? != serde_json::to_vec(listing)? {
                    return Err(bad("CAA listing evidence differs from its original body"));
                }
                Ok(Some(raw))
            }
            (Kind::Image { .. }, CaaPayload::Image { bytes, sha256 })
                if !bytes.is_empty()
                    && bytes.len() <= MAX_IMAGE
                    && *sha256 == format!("{:x}", Sha256::digest(bytes)) =>
            {
                Ok(Some(bytes))
            }
            (_, CaaPayload::Missing) => Ok(None),
            _ => Err(bad("CAA response does not match its request")),
        }
    }

    fn url(&self) -> &Url {
        // All constructors insert the initial URL and never remove it.
        &self.chain[self.chain.len() - 1]
    }

    pub fn follow(&self, location: &str) -> Result<Self> {
        if self.chain.len() > MAX_REDIRECTS || location.len() > 4096 {
            return Err(bad("CAA redirect count or URL exceeds its limit"));
        }
        let target = self
            .url()
            .join(location)
            .map_err(|_| bad("CAA redirect URL is invalid"))?;
        if target.scheme() != "https"
            || target.port_or_known_default() != Some(443)
            || !target.username().is_empty()
            || target.password().is_some()
            || target.query().is_some()
            || target.fragment().is_some()
            || self.chain.contains(&target)
        {
            return Err(bad("CAA redirect is not an allowed HTTPS destination"));
        }
        let item = format!("mbid-{}", self.release);
        let filename = match &self.kind {
            Kind::Listing => "index.json",
            Kind::Image { filename } => filename,
        };
        let host = target.host_str().unwrap_or("");
        let allowed = match host {
            "archive.org" => target.path() == format!("/download/{item}/{filename}"),
            "s3.us.archive.org" => target.path() == format!("/{item}/{filename}"),
            host if archive_shard(host) => {
                let mut parts = target.path().split('/');
                matches!(
                    (parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
                    (Some(""), Some(disk), Some("items"), Some(actual_item), Some(actual_file), None)
                        if !disk.is_empty() && disk.len() <= 4
                        && disk.bytes().all(|byte| byte.is_ascii_digit())
                        && actual_item == item && actual_file == filename
                )
            }
            _ => false,
        };
        if !allowed {
            return Err(bad(
                "CAA redirect changes the archive item, file or authority",
            ));
        }
        let mut next = self.clone();
        next.chain.push(target);
        Ok(next)
    }
}

fn archive_shard(host: &str) -> bool {
    host.strip_prefix("ia")
        .and_then(|host| host.strip_suffix(".us.archive.org"))
        .is_some_and(|number| {
            (6..=12).contains(&number.len()) && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[derive(Debug)]
pub enum CaaPayload {
    Listing { listing: CaaListing, raw: Vec<u8> },
    Image { bytes: Vec<u8>, sha256: String },
    Missing,
}

/// An in-memory transport result. It is not a durable receipt or a rights grant.
#[derive(Debug)]
pub struct CaaFetch {
    pub sources: Vec<String>,
    pub payload: CaaPayload,
}

#[derive(Debug)]
pub struct CaaResponse {
    pub fetch: CaaFetch,
    pub fetched_at: i64,
    pub cached: bool,
}

enum Reply {
    Redirect(CaaRequest),
    Complete(CaaPayload),
}

#[derive(Clone)]
pub struct CaaHttpClient {
    client: reqwest::Client,
}

impl CaaHttpClient {
    pub fn new() -> Result<Self> {
        Self::from_builder(reqwest::Client::builder())
    }

    /// Trusted private-CA/DNS fixtures are allowed. Apply mandatory policy last.
    pub fn from_builder(builder: reqwest::ClientBuilder) -> Result<Self> {
        Ok(Self {
            client: crate::infrastructure::http::catalog_client(builder)?,
        })
    }

    /// Every hop uses the same durable catalog gate as MusicBrainz. A redirect
    /// never grants another request under an already-used capability.
    pub async fn fetch(&self, store: &mut Store, request: CaaRequest) -> Result<CaaFetch> {
        Ok(tokio::time::timeout(
            Duration::from_secs(30),
            self.fetch_inner(store, request, false),
        )
        .await
        .map_err(|_| bad("CAA operation exceeded 30 seconds"))??
        .fetch)
    }

    /// Only successful listing/image bodies are cached. Missing artwork and
    /// failed requests never become successful cache entries.
    pub async fn fetch_cached(
        &self,
        store: &mut Store,
        request: CaaRequest,
    ) -> Result<CaaResponse> {
        if let Some(cached) = store.caa_cache(&request)? {
            store.touch_caa_cache(&request)?;
            return Ok(cached);
        }
        tokio::time::timeout(
            Duration::from_secs(30),
            self.fetch_inner(store, request, true),
        )
        .await
        .map_err(|_| bad("CAA operation exceeded 30 seconds"))?
    }

    async fn fetch_inner(
        &self,
        store: &mut Store,
        mut request: CaaRequest,
        cache: bool,
    ) -> Result<CaaResponse> {
        loop {
            let mut permit = store.claim_catalog_request()?;
            let reply = self.execute(&request, &mut permit).await;
            let retry = match &reply {
                Ok((_, retry)) => *retry,
                Err(Error::Catalog {
                    retry_after_seconds,
                    ..
                }) => *retry_after_seconds,
                Err(_) => 0,
            };
            match reply {
                Err(error) => {
                    store.complete_uncached_catalog_request(permit, retry, false)?;
                    return Err(error);
                }
                Ok((Reply::Complete(payload), _)) => {
                    let fetch = CaaFetch {
                        sources: request.sources(),
                        payload,
                    };
                    let completion = if cache {
                        store.complete_caa_request(permit, retry, &request, &fetch)?
                    } else {
                        store.complete_uncached_catalog_request(permit, retry, true)?
                    };
                    return Ok(CaaResponse {
                        fetch,
                        fetched_at: completion.fetched_at,
                        cached: false,
                    });
                }
                Ok((Reply::Redirect(next), _)) => {
                    let completion =
                        store.complete_uncached_catalog_request(permit, retry, true)?;
                    if completion.retry_after_seconds > 15 {
                        return Err(Error::Catalog {
                            reason: "CAA redirect requires provider backoff before a new request",
                            retry_after_seconds: completion.retry_after_seconds,
                        });
                    }
                    tokio::time::sleep(Duration::from_secs(u64::from(
                        completion.retry_after_seconds,
                    )))
                    .await;
                    request = next;
                }
            }
        }
    }

    async fn execute(
        &self,
        request: &CaaRequest,
        permit: &mut CatalogPermit,
    ) -> Result<(Reply, u32)> {
        use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE, LOCATION};
        permit.start().await?;
        let mut response = self
            .client
            .get(request.url().clone())
            .header(
                "accept",
                match request.kind {
                    Kind::Listing => "application/json",
                    Kind::Image { .. } => "image/jpeg, image/png",
                },
            )
            .send()
            .await
            .map_err(|_| bad("CAA HTTPS request failed"))?;
        let retry = crate::infrastructure::catalog::response_retry_after(response.headers())
            .map_err(|error| match error {
                Error::Catalog {
                    retry_after_seconds,
                    ..
                } => Error::Catalog {
                    reason: "CAA Retry-After is unsupported; catalog requests are paused",
                    retry_after_seconds,
                },
                other => other,
            })?;
        let failure = |reason| Error::Catalog {
            reason,
            retry_after_seconds: retry.max(5),
        };
        let headers = response.headers();
        if headers.len() > 64
            || headers
                .iter()
                .map(|(name, value)| name.as_str().len() + value.len())
                .sum::<usize>()
                > 16384
        {
            return Err(failure("CAA response headers exceed their limit"));
        }
        if matches!(
            response.status(),
            reqwest::StatusCode::MOVED_PERMANENTLY
                | reqwest::StatusCode::FOUND
                | reqwest::StatusCode::SEE_OTHER
                | reqwest::StatusCode::TEMPORARY_REDIRECT
                | reqwest::StatusCode::PERMANENT_REDIRECT
        ) {
            let mut values = headers.get_all(LOCATION).iter();
            let location = values
                .next()
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| failure("CAA redirect has no valid Location"))?;
            if values.next().is_some() {
                return Err(failure("CAA redirect has ambiguous Location headers"));
            }
            let next = request.follow(location).map_err(|error| match error {
                Error::Remote(reason) => failure(reason),
                other => other,
            })?;
            return Ok((Reply::Redirect(next), retry));
        }
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok((Reply::Complete(CaaPayload::Missing), retry));
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(failure("CAA provider did not return an accepted status"));
        }
        let media_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        let supported = match request.kind {
            Kind::Listing => {
                media_type.is_some_and(|value| value.eq_ignore_ascii_case("application/json"))
            }
            Kind::Image { .. } => media_type.is_some_and(|value| {
                value.eq_ignore_ascii_case("image/jpeg") || value.eq_ignore_ascii_case("image/png")
            }),
        };
        if !supported
            || headers.get_all(CONTENT_TYPE).iter().count() != 1
            || headers
                .get_all(CONTENT_ENCODING)
                .iter()
                .any(|value| value != "identity")
        {
            return Err(failure(
                "CAA returned an unsupported content type or encoding",
            ));
        }
        let maximum = match request.kind {
            Kind::Listing => MAX_CAA_LISTING,
            Kind::Image { .. } => MAX_IMAGE,
        };
        if response
            .content_length()
            .is_some_and(|length| length > maximum as u64)
        {
            return Err(failure("CAA response exceeds its body limit"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| failure("CAA response could not be read"))?
        {
            if chunk.len() > maximum - bytes.len() {
                return Err(failure("CAA response exceeds its body limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(failure("CAA response is empty"));
        }
        let payload = match request.kind {
            Kind::Listing => CaaPayload::Listing {
                listing: CaaListing::parse(&request.release, &bytes).map_err(
                    |error| match error {
                        Error::Remote(reason) => failure(reason),
                        other => other,
                    },
                )?,
                raw: bytes,
            },
            Kind::Image { .. } => {
                use sha2::{Digest, Sha256};
                CaaPayload::Image {
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    bytes,
                }
            }
        };
        Ok((Reply::Complete(payload), retry))
    }
}
