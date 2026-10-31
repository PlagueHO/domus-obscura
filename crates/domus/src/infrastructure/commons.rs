//! Commons-only transport. No name search, implicit artist mapping, redirect
//! following, remote resizing or album-cover authority.
use crate::application::services::artist_images::*;
use crate::application::services::cache::{Cache, CacheKey, CacheLookup};
use crate::{Error, Result};
use futures_util::future::BoxFuture;
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PROVIDER: &str = "wikimedia-commons";
const USER_AGENT: &str = concat!(
    "DomusObscura/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/PlagueHO/domus-obscura)"
);

struct CancelPending(Arc<AtomicBool>);

impl Drop for CancelPending {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub struct CommonsClient {
    database: PathBuf,
    cache: Arc<dyn Cache>,
    http: reqwest::Client,
    slots: Arc<tokio::sync::Semaphore>,
}

impl CommonsClient {
    pub fn new(database: PathBuf, cache: Arc<dyn Cache>) -> Result<Self> {
        Self::from_builder(database, cache, reqwest::Client::builder())
    }

    /// Trusted private-CA fixtures may replace DNS and trust roots. The fixed
    /// HTTPS authorities and transport policy still apply to every request.
    pub fn from_builder(
        database: PathBuf,
        cache: Arc<dyn Cache>,
        builder: reqwest::ClientBuilder,
    ) -> Result<Self> {
        Ok(Self {
            database,
            cache,
            http: super::http::catalog_client(builder)?,
            slots: Arc::new(tokio::sync::Semaphore::new(2)),
        })
    }

    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(
            reqwest::Client,
            Arc<dyn Cache>,
            &mut super::store::Store,
            Arc<AtomicBool>,
            tokio::runtime::Handle,
        ) -> Result<T>
        + Send
        + 'static,
    ) -> BoxFuture<'_, Result<T>> {
        Box::pin(async move {
            let permit = self
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::Busy("artist photo capacity is full; retry later"))?;
            let path = self.database.clone();
            let cache = self.cache.clone();
            let http = self.http.clone();
            let runtime = tokio::runtime::Handle::current();
            let cancelled = Arc::new(AtomicBool::new(false));
            let _cancel = CancelPending(cancelled.clone());
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut store = super::store::Store::open_current_existing(&path)?;
                let result = work(http, cache, &mut store, cancelled, runtime);
                if let Err(error) = &result {
                    eprintln!("artist photo provider failed: {error}");
                }
                result
            })
            .await
            .map_err(|_| Error::Internal("Commons task failed"))?
        })
    }
}

#[cfg(test)]
mod tests;

impl ArtistImageProvider for CommonsClient {
    fn matches_file(&self, requested: &str, source: &ArtistImageSource) -> bool {
        requested.replace('_', " ") == source.file.replace('_', " ")
    }

    fn id(&self) -> &str {
        PROVIDER
    }

    fn inspect(&self, file: String) -> BoxFuture<'_, Result<Option<ArtistImageEvidence>>> {
        self.run(move |http, cache, store, cancelled, runtime| {
            validate_file(&file)?;
            let mut url = reqwest::Url::parse("https://commons.wikimedia.org/w/api.php")
                .map_err(|_| Error::Internal("Commons endpoint is invalid"))?;
            url.query_pairs_mut().extend_pairs([
                ("action", "query"), ("format", "json"), ("formatversion", "2"),
                ("prop", "imageinfo|info"), ("titles", file.as_str()), ("iilimit", "1"),
                ("iiprop", "timestamp|url|size|sha1|mime|extmetadata"),
                ("iiextmetadatalanguage", "en"),
                ("iiextmetadatafilter", "ObjectName|Artist|Credit|LicenseShortName|LicenseUrl|UsageTerms|AttributionRequired|Restrictions|Copyrighted|Attribution"),
                ("maxlag", "5"),
            ]);
            let key = CacheKey::new(PROVIDER, "file-metadata-v1", &file)?;
            let (body, fetched_at, expires_at) = runtime.block_on(fetch(
                &http, cache.as_ref(), store, cancelled.as_ref(), url, key, 256 * 1024, 86400,
                |bytes| parse_metadata(&file, bytes).map(|_| ()),
                |bytes| Ok(if parse_metadata(&file, bytes)?.is_none() { 3600 } else { 86400 }),
            ))?;
            Ok(parse_metadata(&file, &body)?.map(|source| ArtistImageEvidence { source, fetched_at, expires_at }))
        })
    }

    fn original(&self, source: ArtistImageSource) -> BoxFuture<'_, Result<Vec<u8>>> {
        self.run(move |http, cache, store, cancelled, runtime| {
            source.validate()?;
            let url = original_url(&source)?;
            let key = CacheKey::new(PROVIDER, "original-v1", &source.fingerprint()?)?;
            let (bytes, _, _) = runtime.block_on(fetch(
                &http,
                cache.as_ref(),
                store,
                cancelled.as_ref(),
                url,
                key,
                8 * 1024 * 1024,
                7 * 86400,
                |bytes| validate_original(&source, bytes),
                |_| Ok(7 * 86400),
            ))?;
            Ok(bytes)
        })
    }
}

fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Internal("Commons clock is invalid"))?
        .as_secs())
}

#[allow(clippy::too_many_arguments)]
async fn fetch(
    http: &reqwest::Client,
    cache: &dyn Cache,
    store: &mut super::store::Store,
    cancelled: &AtomicBool,
    url: reqwest::Url,
    key: CacheKey,
    maximum: usize,
    ttl: u64,
    validate: impl Fn(&[u8]) -> Result<()>,
    lifetime: impl Fn(&[u8]) -> Result<u64>,
) -> Result<(Vec<u8>, u64, u64)> {
    match cache.lookup(&key, now()?)? {
        CacheLookup::Hit(hit) => {
            if hit.bytes.len() > maximum {
                return Err(Error::Remote("Commons cache entry exceeds its bound"));
            }
            validate(&hit.bytes)?;
            return Ok((hit.bytes, hit.stored_at, hit.expires_at));
        }
        CacheLookup::Corrupt => return Err(Error::Remote("Commons cache entry is corrupt")),
        CacheLookup::Miss | CacheLookup::Expired => {}
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut permit = loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Busy("Commons request cancelled before submission"));
        }
        match store.claim_commons() {
            Ok(permit) => break permit,
            Err(Error::Catalog {
                retry_after_seconds,
                ..
            }) if retry_after_seconds <= 2
                && std::time::Instant::now()
                    + Duration::from_secs(u64::from(retry_after_seconds.max(1)))
                    < deadline =>
            {
                tokio::time::sleep(Duration::from_secs(u64::from(retry_after_seconds.max(1))))
                    .await;
            }
            Err(error) => return Err(error),
        }
    };
    let result = async {
        permit.start().await?;
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Busy("Commons request cancelled before submission"));
        }
        let metadata_response = url.host_str() == Some("commons.wikimedia.org");
        let mut response = http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .map_err(|_| Error::Remote("Commons HTTPS request failed"))?;
        if response.headers().len() > 64
            || response
                .headers()
                .iter()
                .map(|(k, v)| k.as_str().len() + v.len())
                .sum::<usize>()
                > 16384
        {
            return Err(Error::Remote("Commons response headers exceed their bound"));
        }
        let delay = match response.headers().get(reqwest::header::RETRY_AFTER) {
            None => 0,
            Some(value) => value
                .to_str()
                .ok()
                .and_then(|v| {
                    v.parse::<u32>().ok().or_else(|| {
                        httpdate::parse_http_date(v).ok().and_then(|time| {
                            time.duration_since(SystemTime::now())
                                .ok()
                                .and_then(|d| u32::try_from(d.as_secs().saturating_add(1)).ok())
                        })
                    })
                })
                .unwrap_or(u32::MAX),
        };
        permit.delay(store, delay)?;
        if !response.status().is_success() {
            return Err(if matches!(response.status().as_u16(), 429 | 503) {
                Error::Catalog {
                    reason: "Commons requested backoff",
                    retry_after_seconds: delay.max(60),
                }
            } else {
                Error::Remote("Commons returned an unsuccessful status; redirects are disabled")
            });
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next());
        if (metadata_response && content_type != Some("application/json"))
            || (!metadata_response && !matches!(content_type, Some("image/jpeg" | "image/png")))
        {
            return Err(Error::Remote(
                "Commons response has an unsupported content type",
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > maximum as u64)
            || response
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .is_some_and(|v| v != "identity")
        {
            return Err(Error::Remote("Commons body exceeds its supported envelope"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Remote("Commons response read failed"))?
        {
            if chunk.len() > maximum - bytes.len() {
                return Err(Error::Remote("Commons body exceeds its bound"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && let Some(error) = value.get("error")
        {
            let code = error.get("code").and_then(Value::as_str);
            if matches!(code, Some("maxlag" | "ratelimited")) {
                permit.delay(store, delay.max(60))?;
                return Err(Error::Catalog {
                    reason: "Commons API requested backoff",
                    retry_after_seconds: delay.max(60),
                });
            }
            return Err(Error::Remote("Commons API returned an error"));
        }
        validate(&bytes)?;
        let fetched = now()?;
        let mut seconds = ttl.min(lifetime(&bytes)?);
        let mut store_response = true;
        for value in response.headers().get_all(reqwest::header::CACHE_CONTROL) {
            for directive in value
                .to_str()
                .map_err(|_| Error::Remote("Commons cache policy is invalid"))?
                .split(',')
            {
                let directive = directive.trim().to_ascii_lowercase();
                if matches!(
                    directive.split('=').next(),
                    Some("no-store" | "no-cache" | "private")
                ) {
                    store_response = false;
                }
                if let Some(age) = directive.strip_prefix("max-age=") {
                    seconds = seconds.min(
                        age.trim_matches('"')
                            .parse::<u64>()
                            .map_err(|_| Error::Remote("Commons cache lifetime is invalid"))?,
                    );
                }
            }
        }
        if response
            .headers()
            .get_all(reqwest::header::VARY)
            .iter()
            .any(|value| {
                value
                    .to_str()
                    .map_or(true, |v| v.split(',').any(|v| v.trim() == "*"))
            })
        {
            store_response = false;
        }
        if let Some(expires) = response.headers().get(reqwest::header::EXPIRES) {
            let expires = expires
                .to_str()
                .ok()
                .and_then(|v| httpdate::parse_http_date(v).ok())
                .ok_or(Error::Remote("Commons expiry header is invalid"))?;
            let date = response
                .headers()
                .get(reqwest::header::DATE)
                .map(|v| {
                    v.to_str()
                        .ok()
                        .and_then(|v| httpdate::parse_http_date(v).ok())
                        .ok_or(Error::Remote("Commons date header is invalid"))
                })
                .transpose()?
                .unwrap_or(SystemTime::now());
            seconds = seconds.min(expires.duration_since(date).map_or(0, |d| d.as_secs()));
        }
        if let Some(age) = response.headers().get(reqwest::header::AGE) {
            seconds = seconds.saturating_sub(
                age.to_str()
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .ok_or(Error::Remote("Commons cache age is invalid"))?,
            );
        }
        let expires = fetched
            .checked_add(seconds)
            .ok_or(Error::Internal("Commons expiry overflow"))?;
        if !store_response || seconds == 0 {
            return Err(Error::Remote(
                "Commons cache policy does not permit the selected shared-cache workflow",
            ));
        }
        cache.put(&key, &bytes, fetched, expires)?;
        Ok((bytes, fetched, expires))
    }
    .await;
    store.complete_commons(permit, result.is_ok())?;
    result
}

fn text(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(Error::Remote(
            "Commons response is missing a required field",
        ))
}

fn number(value: &Value, field: &str) -> Result<i64> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or(Error::Remote(
            "Commons response is missing a required number",
        ))
}

fn metadata(value: &Value, field: &str) -> Result<String> {
    let raw = value
        .get(field)
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .ok_or(Error::InvalidInput(
            "Commons photo has incomplete rights metadata".into(),
        ))?;
    credit_text(raw)
}

fn parse_metadata(requested: &str, bytes: &[u8]) -> Result<Option<ArtistImageSource>> {
    let value: Value = serde_json::from_slice(bytes)?;
    if value.get("error").is_some() || value.get("warnings").is_some() {
        return Err(Error::Remote(
            "Commons metadata contains an error or warning",
        ));
    }
    let pages = value
        .pointer("/query/pages")
        .and_then(Value::as_array)
        .filter(|pages| pages.len() == 1)
        .ok_or(Error::Remote("Commons did not return one exact file"))?;
    let page = &pages[0];
    let file = text(page, "title")?;
    if file.replace('_', " ") != requested.replace('_', " ") || page.get("redirect").is_some() {
        return Err(Error::Remote("Commons returned a different file identity"));
    }
    if page.get("missing").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    if number(page, "ns")? != 6 || text(page, "imagerepository")? != "local" {
        return Err(Error::Remote(
            "Commons file is not in the local file namespace",
        ));
    }
    let infos = page
        .get("imageinfo")
        .and_then(Value::as_array)
        .filter(|v| v.len() == 1)
        .ok_or(Error::Remote("Commons file has no exact image revision"))?;
    let info = &infos[0];
    let ext = info.get("extmetadata").ok_or(Error::InvalidInput(
        "Commons rights metadata is missing".into(),
    ))?;
    let license = metadata(ext, "LicenseShortName")?;
    let license_url = metadata(ext, "LicenseUrl")?;
    let required = metadata(ext, "AttributionRequired")?;
    if (license.starts_with("CC BY") && required != "true")
        || (license == "CC0 1.0" && required != "false")
        || ext
            .get("Restrictions")
            .is_some_and(|_| metadata(ext, "Restrictions").map_or(true, |s| !s.trim().is_empty()))
    {
        return Err(Error::InvalidInput(
            "Commons rights are contradictory or restricted".into(),
        ));
    }
    let description = text(info, "descriptionurl")?;
    let mut expected = reqwest::Url::parse("https://commons.wikimedia.org/wiki/")
        .map_err(|_| Error::Internal("Commons description endpoint is invalid"))?;
    expected
        .path_segments_mut()
        .map_err(|_| Error::Internal("Commons URL cannot hold a title"))?
        .pop_if_empty()
        .push(&file.replace(' ', "_"));
    if reqwest::Url::parse(&description).ok().as_ref() != Some(&expected) {
        return Err(Error::Remote(
            "Commons description does not match the selected file",
        ));
    }
    let rights = ImageRights {
        title: metadata(ext, "ObjectName")?,
        creator: metadata(ext, "Artist")?,
        credit: metadata(ext, "Credit")?,
        license,
        license_url,
        source_url: description,
        notices: ["UsageTerms", "Attribution"]
            .into_iter()
            .filter(|field| ext.get(*field).is_some())
            .map(|field| metadata(ext, field))
            .collect::<Result<Vec<_>>>()?
            .join("\n"),
    };
    let source = ArtistImageSource {
        file,
        entity_id: number(page, "pageid")?.to_string(),
        revision: number(page, "lastrevid")?.to_string(),
        timestamp: text(info, "timestamp")?,
        sha1: Some(text(info, "sha1")?),
        bytes: u32::try_from(number(info, "size")?)
            .map_err(|_| Error::Remote("Commons byte size is invalid"))?,
        width: u32::try_from(number(info, "width")?)
            .map_err(|_| Error::Remote("Commons width is invalid"))?,
        height: u32::try_from(number(info, "height")?)
            .map_err(|_| Error::Remote("Commons height is invalid"))?,
        mime: text(info, "mime")?,
        original_url: text(info, "url")?,
        rights,
    };
    source.validate()?;
    if source
        .entity_id
        .parse::<i64>()
        .ok()
        .is_none_or(|id| id <= 0)
        || source
            .revision
            .parse::<i64>()
            .ok()
            .is_none_or(|revision| revision <= 0)
    {
        return Err(Error::Remote(
            "Commons returned an invalid page or revision identity",
        ));
    }
    original_url(&source)?;
    Ok(Some(source))
}

fn original_url(source: &ArtistImageSource) -> Result<reqwest::Url> {
    validate_file(&source.file)?;
    let url = reqwest::Url::parse(&source.original_url)
        .map_err(|_| Error::Remote("Commons original URL is invalid"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("upload.wikimedia.org")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Remote(
            "Commons original is outside the fixed upload authority",
        ));
    }
    let parts: Vec<_> = url
        .path_segments()
        .ok_or(Error::Remote("Commons original path is invalid"))?
        .collect();
    if parts.len() != 5
        || parts[0] != "wikipedia"
        || parts[1] != "commons"
        || parts[2].len() != 1
        || parts[3].len() != 2
        || !parts[3].starts_with(parts[2])
        || !parts[3]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(Error::Remote(
            "Commons original is not an allowed original-file path",
        ));
    }
    let mut expected = url.clone();
    expected
        .path_segments_mut()
        .map_err(|_| Error::Remote("Commons original path is invalid"))?
        .pop()
        .push(&source.file[5..].replace(' ', "_"));
    if expected != url {
        return Err(Error::Remote(
            "Commons original filename differs from the selected file",
        ));
    }
    Ok(url)
}

fn validate_original(source: &ArtistImageSource, bytes: &[u8]) -> Result<()> {
    if bytes.len() != source.bytes as usize
        || source.sha1.as_deref() != Some(format!("{:x}", Sha1::digest(bytes)).as_str())
    {
        return Err(Error::Remote(
            "Commons original differs from the exact file revision",
        ));
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let expected = match source.mime.as_str() {
        "image/jpeg" => image::ImageFormat::Jpeg,
        "image/png" => image::ImageFormat::Png,
        _ => return Err(Error::Remote("Commons original type is unsupported")),
    };
    if reader.format() != Some(expected)
        || reader
            .into_dimensions()
            .map_err(|_| Error::Remote("Commons original header is invalid"))?
            != (source.width, source.height)
    {
        return Err(Error::Remote(
            "Commons original geometry or type differs from its metadata",
        ));
    }
    Ok(())
}

/// Keep text and safe link destinations needed by credit, never provider markup.
/// Unknown markup/entities fail closed instead of silently losing attribution.
fn credit_text(raw: &str) -> Result<String> {
    if raw.len() > 16384 {
        return Err(Error::InvalidInput(
            "Commons credit exceeds its bound".into(),
        ));
    }
    let mut output = String::new();
    let mut rest = raw;
    while !rest.is_empty() {
        if let Some(tag) = rest.strip_prefix('<') {
            let end = tag.find('>').ok_or(Error::InvalidInput(
                "Commons credit markup is incomplete".into(),
            ))?;
            let body = &tag[..end];
            let name = body
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('/');
            if !matches!(
                name,
                "a" | "span" | "div" | "p" | "br" | "b" | "i" | "small" | "strong" | "em"
            ) {
                return Err(Error::InvalidInput(
                    "Commons credit markup is unsupported".into(),
                ));
            }
            if name == "a" && !body.starts_with('/') && !body.contains(" href=\"") {
                return Err(Error::InvalidInput(
                    "Commons credit link syntax is unsupported".into(),
                ));
            }
            if let Some(start) = body.find("href=\"") {
                let value = &body[start + 6..];
                let end = value
                    .find('"')
                    .ok_or(Error::InvalidInput("Commons credit link is invalid".into()))?;
                let href = value[..end].replace("&amp;", "&");
                let href = if href.starts_with("//") {
                    format!("https:{href}")
                } else if href.starts_with('/') {
                    format!("https://commons.wikimedia.org{href}")
                } else {
                    href
                };
                let url = reqwest::Url::parse(&href)
                    .map_err(|_| Error::InvalidInput("Commons credit link is invalid".into()))?;
                if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
                {
                    return Err(Error::InvalidInput(
                        "Commons credit link is unsupported".into(),
                    ));
                }
                output.push_str(&format!(" ({url}) "));
            }
            if matches!(name, "br" | "div" | "p") {
                output.push(' ');
            }
            rest = &tag[end + 1..];
        } else if let Some(entity) = rest.strip_prefix('&') {
            let end = entity
                .find(';')
                .filter(|end| *end <= 16)
                .ok_or(Error::InvalidInput(
                    "Commons credit entity is invalid".into(),
                ))?;
            let name = &entity[..end];
            let decoded = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" | "#39" => '\'',
                "nbsp" => ' ',
                _ => name
                    .strip_prefix("#x")
                    .and_then(|v| u32::from_str_radix(v, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|v| v.parse().ok()))
                    .and_then(char::from_u32)
                    .ok_or(Error::InvalidInput(
                        "Commons credit entity is unsupported".into(),
                    ))?,
            };
            output.push(decoded);
            rest = &entity[end + 1..];
        } else {
            let c = rest
                .chars()
                .next()
                .ok_or(Error::Internal("credit cursor is empty"))?;
            output.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    if output.len() > 8192 {
        return Err(Error::InvalidInput(
            "Commons credit exceeds its text bound".into(),
        ));
    }
    Ok(output.split_whitespace().collect::<Vec<_>>().join(" "))
}
