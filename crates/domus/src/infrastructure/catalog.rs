use crate::domain::credit_text;
use crate::domain::identifiers::is_canonical_uuid;
use crate::domain::{
    CatalogArtist, CatalogCredit, CatalogMedium, CatalogPage, CatalogRelease,
    CatalogReleaseFilters, CatalogReleaseGroup, CatalogReleasePage, CatalogReleaseSummary,
    CatalogTrack,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

pub const MAX_CATALOG_BODY: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct CatalogResult<T> {
    pub source: String,
    pub fetched_at: i64,
    pub cached: bool,
    pub payload_sha256: Vec<u8>,
    pub data: T,
}

#[derive(Clone)]
pub struct CatalogQuery {
    url: reqwest::Url,
    kind: QueryKind,
}

#[derive(Clone)]
enum QueryKind {
    Artist {
        id: String,
    },
    Artists {
        offset: u32,
        limit: u16,
    },
    Releases {
        offset: u32,
        limit: u16,
        artist: String,
        filters: CatalogReleaseFilters,
    },
    Albums {
        offset: u32,
        limit: u16,
        artist: Option<String>,
        filters: CatalogReleaseFilters,
    },
    GroupReleases {
        offset: u32,
        limit: u16,
        group: String,
        filters: CatalogReleaseFilters,
    },
    Release {
        id: String,
    },
}

impl CatalogQuery {
    pub(crate) fn is_exact_release(&self) -> bool {
        matches!(self.kind, QueryKind::Release { .. })
    }

    pub fn artists(text: &str, offset: u32, limit: u16) -> Result<Self> {
        let literal = literal(text)?;
        let mut query = Self::page(
            "artist",
            offset,
            limit,
            QueryKind::Artists { offset, limit },
        )?;
        query
            .url
            .query_pairs_mut()
            // MusicBrainz's default artist fields include aliases and sort names.
            .append_pair("query", &format!("\"{literal}\""));
        Ok(query)
    }

    pub fn artist(id: &str) -> Result<Self> {
        input_id(id)?;
        let mut url = reqwest::Url::parse(&format!("https://musicbrainz.org/ws/2/artist/{id}"))
            .map_err(|_| Error::Internal("catalog URL construction failed"))?;
        url.query_pairs_mut().append_pair("fmt", "json");
        Ok(Self {
            url,
            kind: QueryKind::Artist { id: id.into() },
        })
    }

    pub fn releases(artist: &str, offset: u32, limit: u16) -> Result<Self> {
        Self::filtered_releases(artist, offset, limit, CatalogReleaseFilters::default())
    }

    pub fn filtered_releases(
        artist: &str,
        offset: u32,
        limit: u16,
        filters: CatalogReleaseFilters,
    ) -> Result<Self> {
        input_id(artist)?;
        let mut query = Self::page(
            "release",
            offset,
            limit,
            QueryKind::Releases {
                offset,
                limit,
                artist: artist.into(),
                filters,
            },
        )?;
        query
            .url
            .query_pairs_mut()
            .append_pair("artist", artist)
            .append_pair("inc", "artist-credits release-groups media")
            .append_pair(
                "type",
                if filters.include_singles {
                    "album|ep|single"
                } else {
                    "album|ep"
                },
            );
        if !filters.include_unofficial {
            query
                .url
                .query_pairs_mut()
                .append_pair("status", "official");
        }
        Ok(query)
    }

    pub fn albums(
        title: &str,
        artist: Option<&str>,
        offset: u32,
        limit: u16,
        filters: CatalogReleaseFilters,
    ) -> Result<Self> {
        let mut expression = format!("release:\"{}\"", literal(title)?);
        if let Some(artist) = artist {
            input_id(artist)?;
            expression.push_str(&format!(" AND arid:{artist}"));
        }
        expression.push_str(" AND (primarytype:album OR primarytype:ep");
        if filters.include_singles {
            expression.push_str(" OR primarytype:single");
        }
        expression.push(')');
        if !filters.include_unofficial {
            expression.push_str(" AND status:official");
        }
        if !filters.include_live {
            expression.push_str(" AND NOT secondarytype:live");
        }
        if !filters.include_compilations {
            expression.push_str(" AND NOT secondarytype:compilation");
        }
        let mut query = Self::page(
            "release",
            offset,
            limit,
            QueryKind::Albums {
                offset,
                limit,
                artist: artist.map(str::to_owned),
                filters,
            },
        )?;
        query
            .url
            .query_pairs_mut()
            .append_pair("query", &expression);
        Ok(query)
    }

    pub fn group_releases(
        group: &str,
        offset: u32,
        limit: u16,
        filters: CatalogReleaseFilters,
    ) -> Result<Self> {
        input_id(group)?;
        let mut query = Self::page(
            "release",
            offset,
            limit,
            QueryKind::GroupReleases {
                offset,
                limit,
                group: group.into(),
                filters,
            },
        )?;
        query
            .url
            .query_pairs_mut()
            .append_pair("release-group", group)
            .append_pair("inc", "artist-credits release-groups media");
        if !filters.include_unofficial {
            query
                .url
                .query_pairs_mut()
                .append_pair("status", "official");
        }
        Ok(query)
    }

    pub fn release(id: &str) -> Result<Self> {
        input_id(id)?;
        let mut url = reqwest::Url::parse(&format!("https://musicbrainz.org/ws/2/release/{id}"))
            .map_err(|_| Error::Internal("catalog URL construction failed"))?;
        url.query_pairs_mut()
            .append_pair("fmt", "json")
            .append_pair("inc", "recordings artist-credits release-groups");
        Ok(Self {
            url,
            kind: QueryKind::Release { id: id.into() },
        })
    }

    fn page(entity: &str, offset: u32, limit: u16, kind: QueryKind) -> Result<Self> {
        if !(1..=25).contains(&limit) {
            return Err(Error::InvalidInput(
                "catalog page limit must be 1 through 25".into(),
            ));
        }
        let mut url = reqwest::Url::parse(&format!("https://musicbrainz.org/ws/2/{entity}"))
            .map_err(|_| Error::Internal("catalog URL construction failed"))?;
        url.query_pairs_mut()
            .append_pair("fmt", "json")
            .append_pair("offset", &offset.to_string())
            .append_pair("limit", &limit.to_string());
        Ok(Self { url, kind })
    }

    pub fn source(&self) -> &str {
        self.url.as_str()
    }

    pub fn validate_response(&self, body: &[u8]) -> Result<()> {
        match self.kind {
            QueryKind::Artist { .. } => self.parse_artist(body).map(|_| ()),
            QueryKind::Artists { .. } => self.parse_artists(body).map(|_| ()),
            QueryKind::Releases { .. }
            | QueryKind::Albums { .. }
            | QueryKind::GroupReleases { .. } => self.parse_releases(body).map(|_| ()),
            QueryKind::Release { .. } => self.parse_release(body).map(|_| ()),
        }
    }

    pub fn parse_artist(&self, body: &[u8]) -> Result<CatalogArtist> {
        let QueryKind::Artist { ref id } = self.kind else {
            return Err(Error::Internal("catalog query does not request an artist"));
        };
        let artist: Artist = json(body)?;
        if &artist.id != id {
            return Err(bad(
                "MusicBrainz artist differs from the requested identity",
            ));
        }
        artist.into_catalog()
    }

    pub fn parse_artists(&self, body: &[u8]) -> Result<CatalogPage<CatalogArtist>> {
        let QueryKind::Artists { offset, limit } = self.kind else {
            return Err(Error::Internal("catalog query does not request artists"));
        };
        let page: ArtistPage = json(body)?;
        page_bounds(offset, limit, page.offset, page.count, page.artists.len())?;
        let mut ids = HashSet::new();
        let mut items = Vec::with_capacity(page.artists.len());
        for artist in page.artists {
            provider_id(&artist.id)?;
            if !ids.insert(artist.id.clone()) {
                return Err(bad("MusicBrainz artist page has duplicate IDs"));
            }
            items.push(artist.into_catalog()?);
        }
        Ok(CatalogPage {
            offset,
            count: page.count,
            items,
        })
    }

    pub fn parse_releases(&self, body: &[u8]) -> Result<CatalogReleasePage> {
        let (offset, limit, filters, artist, group, search) = match &self.kind {
            QueryKind::Releases {
                offset,
                limit,
                filters,
                artist,
            } => (
                *offset,
                *limit,
                *filters,
                Some(artist.as_str()),
                None,
                false,
            ),
            QueryKind::Albums {
                offset,
                limit,
                filters,
                artist,
            } => (*offset, *limit, *filters, artist.as_deref(), None, true),
            QueryKind::GroupReleases {
                offset,
                limit,
                filters,
                group,
            } => (*offset, *limit, *filters, None, Some(group.as_str()), false),
            _ => return Err(Error::Internal("catalog query does not request releases")),
        };
        let page: ReleasePage = if search {
            let page: ReleaseSearchPage = json(body)?;
            ReleasePage {
                offset: page.offset,
                count: page.count,
                releases: page.releases,
            }
        } else {
            json(body)?
        };
        page_bounds(offset, limit, page.offset, page.count, page.releases.len())?;
        let scanned = u16::try_from(page.releases.len())
            .map_err(|_| bad("MusicBrainz page exceeds its bound"))?;
        let next = offset
            .checked_add(u32::from(scanned))
            .ok_or_else(|| bad("MusicBrainz page cursor overflowed"))?;
        let mut ids = HashSet::new();
        let mut items = Vec::with_capacity(page.releases.len());
        for release in page.releases {
            if artist.is_some_and(|artist| {
                !release
                    .credits
                    .iter()
                    .any(|credit| credit.artist.id == artist)
            }) {
                return Err(bad("MusicBrainz returned a release by a different artist"));
            }
            let summary = summary(release)?;
            if group.is_some_and(|group| summary.group.as_ref().is_none_or(|item| item.id != group))
            {
                return Err(bad(
                    "MusicBrainz returned a release from a different release group",
                ));
            }
            if !ids.insert(summary.id.clone()) {
                return Err(bad("MusicBrainz release page has duplicate IDs"));
            }
            if filters.includes(&summary) {
                items.push(summary);
            }
        }
        Ok(CatalogReleasePage {
            offset,
            count: page.count,
            scanned,
            next_offset: (next < page.count).then_some(next),
            filters,
            items,
        })
    }

    pub fn parse_release(&self, body: &[u8]) -> Result<CatalogRelease> {
        let QueryKind::Release { ref id } = self.kind else {
            return Err(Error::Internal(
                "catalog query does not request an exact release",
            ));
        };
        let raw: ReleaseDetail = json(body)?;
        if &raw.release.id != id {
            return Err(bad("MusicBrainz returned a different release"));
        }
        let release_group_id = raw
            .release
            .group
            .as_ref()
            .ok_or_else(|| bad("MusicBrainz release group is missing"))?
            .id
            .clone();
        let album_credits = credits(&raw.release.credits)?;
        let summary = summary(raw.release)?;
        if raw.media.is_empty() || raw.media.len() > 100 {
            return Err(bad("MusicBrainz release has an unsupported medium count"));
        }
        let mut media = raw.media;
        media.sort_by_key(|medium| medium.position);
        let mut result = Vec::with_capacity(media.len());
        let mut track_ids = HashSet::new();
        let mut total = 0usize;
        for (index, mut medium) in media.into_iter().enumerate() {
            if usize::from(medium.position) != index + 1
                || medium.tracks.len() != usize::from(medium.track_count)
                || medium.tracks.is_empty()
            {
                return Err(bad("MusicBrainz release has incomplete or unordered media"));
            }
            total += medium.tracks.len();
            if total > 1000 {
                return Err(bad("MusicBrainz release exceeds 1000 tracks"));
            }
            medium.tracks.sort_by_key(|track| track.position);
            let mut tracks = Vec::with_capacity(medium.tracks.len());
            for (index, track) in medium.tracks.into_iter().enumerate() {
                if usize::from(track.position) != index + 1 || !track_ids.insert(track.id.clone()) {
                    return Err(bad(
                        "MusicBrainz release has incomplete or duplicate tracks",
                    ));
                }
                provider_id(&track.id)?;
                provider_id(&track.recording.id)?;
                text(&track.title)?;
                text(&track.number)?;
                let track_credits = track
                    .credits
                    .as_deref()
                    .or(track.recording.credits.as_deref())
                    .ok_or_else(|| bad("MusicBrainz track artist credits are missing"))?;
                tracks.push(CatalogTrack {
                    track_id: track.id,
                    recording_id: track.recording.id,
                    position: track.position,
                    number: track.number,
                    title: track.title,
                    credits: credits(track_credits)?,
                    duration_ms: track.length,
                    video: track.recording.video,
                });
            }
            result.push(CatalogMedium {
                position: medium.position,
                format: optional_text(medium.format)?,
                title: optional_text(medium.title)?,
                has_pregap: medium.pregap.is_some(),
                has_data_tracks: !medium.data_tracks.is_empty()
                    || medium.data_track_count.is_some_and(|count| count > 0),
                tracks,
            });
        }
        Ok(CatalogRelease {
            summary,
            release_group_id,
            credits: album_credits,
            media: result,
        })
    }
}

#[derive(Clone)]
pub struct MusicBrainzClient {
    client: reqwest::Client,
}

pub struct CatalogReply {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after_seconds: u32,
}

impl MusicBrainzClient {
    pub fn new() -> Result<Self> {
        Self::from_builder(reqwest::Client::builder())
    }

    /// Private CA/DNS fixture options are permitted; mandatory policy is last.
    /// Runtime configuration cannot change the HTTPS authority or redirect rule.
    pub fn from_builder(builder: reqwest::ClientBuilder) -> Result<Self> {
        let client = super::http::catalog_client(builder)
            .map_err(|_| bad("MusicBrainz HTTP client initialization failed"))?;
        Ok(Self { client })
    }

    /// The application must hold its database-backed request lease before
    /// calling this transport, including for retries and in other processes.
    pub async fn execute(
        &self,
        query: &CatalogQuery,
        permit: &mut super::store::CatalogPermit,
    ) -> Result<CatalogReply> {
        permit.start().await?;
        let mut response = self
            .client
            .get(query.url.clone())
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|_| bad("MusicBrainz HTTPS request failed"))?;
        let retry = response_retry_after(response.headers());
        let observed = match &retry {
            Ok(seconds) => *seconds,
            Err(Error::Catalog {
                retry_after_seconds,
                ..
            }) => *retry_after_seconds,
            Err(_) => 0,
        };
        permit.record_retry_after(observed).await?;
        let retry_after_seconds = retry?;
        let failure = |reason| Error::Catalog {
            reason,
            retry_after_seconds: retry_after_seconds.max(5),
        };
        if response.headers().len() > 64
            || response
                .headers()
                .iter()
                .map(|(name, value)| name.as_str().len() + value.len())
                .sum::<usize>()
                > 16384
        {
            return Err(failure("MusicBrainz response headers exceed their limit"));
        }
        if response.status() == reqwest::StatusCode::OK
            && (response
                .headers()
                .get_all(reqwest::header::CONTENT_TYPE)
                .iter()
                .count()
                != 1
                || !response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(';').next())
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
                || response
                    .headers()
                    .get_all(reqwest::header::CONTENT_ENCODING)
                    .iter()
                    .any(|value| value != "identity"))
        {
            return Err(failure(
                "MusicBrainz returned an unsupported content type or encoding",
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_CATALOG_BODY as u64)
        {
            return Err(failure("MusicBrainz response exceeds 2 MiB"));
        }
        let status = response.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| failure("MusicBrainz response could not be read"))?
        {
            if chunk.len() > MAX_CATALOG_BODY - body.len() {
                return Err(failure("MusicBrainz response exceeds 2 MiB"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(CatalogReply {
            status,
            body,
            retry_after_seconds,
        })
    }
}

pub(super) fn response_retry_after(headers: &reqwest::header::HeaderMap) -> Result<u32> {
    let mut values = headers.get_all(reqwest::header::RETRY_AFTER).iter();
    let Some(value) = values.next() else {
        return Ok(0);
    };
    if values.next().is_some() {
        return Err(unsupported_retry());
    }
    let value = value.to_str().map_err(|_| unsupported_retry())?;
    if value.len() > 128 {
        return Err(unsupported_retry());
    }
    let value = value.trim_matches([' ', '\t']);
    let numeric = !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    let reference = if numeric {
        SystemTime::UNIX_EPOCH
    } else {
        let mut dates = headers.get_all(reqwest::header::DATE).iter();
        let date = dates
            .next()
            .and_then(|value| value.to_str().ok())
            .filter(|date| date.len() <= 128)
            .ok_or_else(unsupported_retry)?;
        if dates.next().is_some() {
            return Err(unsupported_retry());
        }
        httpdate::parse_http_date(date.trim_matches([' ', '\t']))
            .map_err(|_| unsupported_retry())?
    };
    let seconds = retry_after(value, reference)?;
    if !numeric && seconds != 0 {
        // HTTP dates do not carry fractional seconds.
        seconds
            .checked_add(1)
            .filter(|seconds| *seconds < u32::MAX)
            .ok_or_else(unsupported_retry)
    } else {
        Ok(seconds)
    }
}

pub fn retry_after(value: &str, now: SystemTime) -> Result<u32> {
    if value.len() > 128 {
        return Err(unsupported_retry());
    }
    let value = value.trim_matches([' ', '\t']);
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value
            .parse::<u32>()
            .ok()
            .filter(|seconds| *seconds < u32::MAX)
            .ok_or_else(unsupported_retry);
    }
    let date = httpdate::parse_http_date(value).map_err(|_| unsupported_retry())?;
    let duration = date.duration_since(now).unwrap_or(Duration::ZERO);
    let seconds = duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0));
    u32::try_from(seconds)
        .ok()
        .filter(|seconds| *seconds < u32::MAX)
        .ok_or_else(unsupported_retry)
}

fn unsupported_retry() -> Error {
    Error::Catalog {
        reason: "MusicBrainz Retry-After is unsupported; provider requests are paused",
        retry_after_seconds: u32::MAX,
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use reqwest::header::{DATE, HeaderMap, HeaderValue, RETRY_AFTER};

    #[test]
    fn catalog_http_date_delay_uses_provider_date_and_rejects_ambiguous_references() -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert(
            DATE,
            HeaderValue::from_static("Tue, 01 Jan 2030 00:00:00 GMT"),
        );
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Tue, 01 Jan 2030 00:02:00 GMT"),
        );
        assert_eq!(response_retry_after(&headers)?, 121);
        headers.append(
            DATE,
            HeaderValue::from_static("Tue, 01 Jan 2030 00:00:00 GMT"),
        );
        assert!(matches!(
            response_retry_after(&headers),
            Err(Error::Catalog {
                retry_after_seconds: u32::MAX,
                ..
            })
        ));
        headers.remove(DATE);
        assert!(matches!(
            response_retry_after(&headers),
            Err(Error::Catalog {
                retry_after_seconds: u32::MAX,
                ..
            })
        ));
        headers.insert(DATE, HeaderValue::from_static("invalid"));
        assert!(response_retry_after(&headers).is_err());
        headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
        assert_eq!(response_retry_after(&headers)?, 120);
        Ok(())
    }
}
pub(crate) fn bad(reason: &'static str) -> Error {
    Error::Catalog {
        reason,
        retry_after_seconds: 5,
    }
}

fn input_id(id: &str) -> Result<()> {
    if !is_canonical_uuid(id) {
        return Err(Error::InvalidInput(
            "MusicBrainz ID must be a canonical lowercase UUID".into(),
        ));
    }
    Ok(())
}

fn literal(value: &str) -> Result<String> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "catalog search needs 1 through 512 bytes without controls".into(),
        ));
    }
    let mut literal = String::new();
    for character in value.trim().chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(character) {
            literal.push('\\');
        }
        literal.push(character);
    }
    Ok(literal)
}

fn provider_id(id: &str) -> Result<()> {
    if !is_canonical_uuid(id) {
        return Err(bad("MusicBrainz returned an invalid identifier"));
    }
    Ok(())
}

fn text(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(bad("MusicBrainz returned invalid text"));
    }
    Ok(())
}

fn optional_text(value: Option<String>) -> Result<Option<String>> {
    if let Some(value) = &value {
        if value.is_empty() {
            return Ok(None);
        }
        text(value)?;
    }
    Ok(value)
}

fn json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
    if body.len() > MAX_CATALOG_BODY {
        return Err(bad("MusicBrainz response exceeds 2 MiB"));
    }
    serde_json::from_slice(body).map_err(|_| bad("MusicBrainz response JSON is invalid"))
}

fn page_bounds(offset: u32, limit: u16, actual: u32, count: u32, length: usize) -> Result<()> {
    if actual != offset
        || length > usize::from(limit)
        || (length > 0 && u64::from(offset) + length as u64 > u64::from(count))
        || (length == 0 && offset < count)
    {
        return Err(bad("MusicBrainz returned an inconsistent page"));
    }
    Ok(())
}

fn credits(raw: &[Credit]) -> Result<Vec<CatalogCredit>> {
    if raw.is_empty() || raw.len() > 32 {
        return Err(bad("MusicBrainz artist credits are incomplete"));
    }
    let mut result = Vec::with_capacity(raw.len());
    for credit in raw {
        provider_id(&credit.artist.id)?;
        let name = credit.name.as_ref().unwrap_or(&credit.artist.name);
        text(name)?;
        if credit.joinphrase.len() > 512 || credit.joinphrase.chars().any(char::is_control) {
            return Err(bad("MusicBrainz artist join phrase is invalid"));
        }
        result.push(CatalogCredit {
            artist_id: credit.artist.id.clone(),
            name: name.clone(),
            joinphrase: credit.joinphrase.clone(),
        });
    }
    text(&credit_text(&result))?;
    Ok(result)
}

fn summary(raw: ReleaseSummary) -> Result<CatalogReleaseSummary> {
    provider_id(&raw.id)?;
    text(&raw.title)?;
    let date = optional_text(raw.date)?;
    if let Some(date) = &date {
        validate_date(date)?;
    }
    let group = raw
        .group
        .map(|group| -> Result<CatalogReleaseGroup> {
            provider_id(&group.id)?;
            if group.secondary_types.len() > 32 {
                return Err(bad("MusicBrainz release has too many secondary types"));
            }
            let mut unique = HashSet::new();
            for kind in &group.secondary_types {
                text(kind)?;
                if !unique.insert(kind.to_lowercase()) {
                    return Err(bad("MusicBrainz release has duplicate secondary types"));
                }
            }
            Ok(CatalogReleaseGroup {
                id: group.id,
                primary_type: optional_text(group.primary_type)?,
                secondary_types: group.secondary_types,
            })
        })
        .transpose()?;
    Ok(CatalogReleaseSummary {
        id: raw.id,
        title: raw.title,
        artist: credit_text(&credits(&raw.credits)?),
        date,
        country: optional_text(raw.country)?,
        disambiguation: optional_text(raw.disambiguation)?,
        status: optional_text(raw.status)?,
        group,
    })
}

fn validate_date(date: &str) -> Result<()> {
    if !crate::domain::valid_release_date(date) {
        return Err(bad("MusicBrainz release date is invalid"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct ArtistPage {
    offset: u32,
    count: u32,
    artists: Vec<Artist>,
}
#[derive(Deserialize)]
struct Artist {
    id: String,
    name: String,
    #[serde(rename = "sort-name")]
    sort_name: Option<String>,
    disambiguation: Option<String>,
    country: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}
impl Artist {
    fn into_catalog(self) -> Result<CatalogArtist> {
        provider_id(&self.id)?;
        text(&self.name)?;
        Ok(CatalogArtist {
            id: self.id,
            name: self.name,
            sort_name: optional_text(self.sort_name)?,
            disambiguation: optional_text(self.disambiguation)?,
            country: optional_text(self.country)?,
            kind: optional_text(self.kind)?,
        })
    }
}
#[derive(Deserialize)]
struct ReleasePage {
    #[serde(rename = "release-offset")]
    offset: u32,
    #[serde(rename = "release-count")]
    count: u32,
    releases: Vec<ReleaseSummary>,
}
#[derive(Deserialize)]
struct ReleaseSearchPage {
    offset: u32,
    count: u32,
    releases: Vec<ReleaseSummary>,
}
#[derive(Deserialize)]
struct Credit {
    name: Option<String>,
    #[serde(default)]
    joinphrase: String,
    artist: CreditArtist,
}
#[derive(Deserialize)]
struct CreditArtist {
    id: String,
    name: String,
}
#[derive(Deserialize)]
struct ReleaseSummary {
    id: String,
    title: String,
    date: Option<String>,
    country: Option<String>,
    disambiguation: Option<String>,
    status: Option<String>,
    #[serde(rename = "artist-credit")]
    credits: Vec<Credit>,
    #[serde(rename = "release-group")]
    group: Option<ReleaseGroup>,
}
#[derive(Deserialize)]
struct ReleaseDetail {
    #[serde(flatten)]
    release: ReleaseSummary,
    media: Vec<Medium>,
}
#[derive(Deserialize)]
struct ReleaseGroup {
    id: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(default, rename = "secondary-types")]
    secondary_types: Vec<String>,
}
#[derive(Deserialize)]
struct Medium {
    position: u16,
    format: Option<String>,
    title: Option<String>,
    #[serde(rename = "track-count")]
    track_count: u16,
    tracks: Vec<Track>,
    pregap: Option<serde_json::Value>,
    #[serde(default, rename = "data-tracks")]
    data_tracks: Vec<serde_json::Value>,
    #[serde(rename = "data-track-count")]
    data_track_count: Option<u16>,
}
#[derive(Deserialize)]
struct Track {
    id: String,
    position: u16,
    number: String,
    title: String,
    length: Option<u32>,
    #[serde(rename = "artist-credit")]
    credits: Option<Vec<Credit>>,
    recording: Recording,
}
#[derive(Deserialize)]
struct Recording {
    id: String,
    video: Option<bool>,
    #[serde(rename = "artist-credit")]
    credits: Option<Vec<Credit>>,
}
