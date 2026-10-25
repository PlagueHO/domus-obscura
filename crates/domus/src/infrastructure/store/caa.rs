use super::Store;
use crate::infrastructure::caa::{CaaFetch, CaaImage, CaaPayload, CaaRequest, CaaResponse};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

fn invalid() -> Error {
    Error::Conflict("saved CAA evidence is invalid or does not match the exact release".into())
}

pub(super) fn persist_cache(
    connection: &Connection,
    request: &CaaRequest,
    fetched: &CaaFetch,
    now_ms: i64,
) -> Result<()> {
    let Some(body) = request.validate_payload(&fetched.payload)? else {
        return Ok(());
    };
    request.validate_sources(&fetched.sources)?;
    let sources = serde_json::to_string(&fetched.sources)?;
    if sources.len() > 24576 {
        return Err(invalid());
    }
    connection.execute("DELETE FROM caa_cache WHERE expires_at <= unixepoch()", [])?;
    connection.execute(
        "INSERT INTO caa_cache(cache_key,body,sha256,sources,fetched_at,expires_at,accessed_at_ms)
         VALUES (?1,?2,?3,?4,?5,?5+86400,?6)
         ON CONFLICT(cache_key) DO UPDATE SET body=excluded.body,sha256=excluded.sha256,
         sources=excluded.sources,fetched_at=excluded.fetched_at,expires_at=excluded.expires_at,
         accessed_at_ms=excluded.accessed_at_ms",
        params![
            request.cache_key(),
            body,
            &Sha256::digest(body)[..],
            sources,
            now_ms / 1000,
            now_ms
        ],
    )?;
    connection.execute(
        "DELETE FROM caa_cache WHERE cache_key NOT IN (
          SELECT cache_key FROM (
            SELECT cache_key, row_number() OVER (ORDER BY accessed_at_ms DESC,cache_key) AS rank,
              sum(length(body)+length(sources)) OVER (ORDER BY accessed_at_ms DESC,cache_key) AS bytes
            FROM caa_cache)
          WHERE rank <= 32 AND bytes <= 67108864)",
        [],
    )?;
    Ok(())
}

impl Store {
    /// A corrupt fresh entry fails explicitly. Expired entries cannot authorize
    /// new cover preparation. Frozen evidence has a separate lifetime.
    pub fn caa_cache(&self, request: &CaaRequest) -> Result<Option<CaaResponse>> {
        let saved = self
            .connection
            .query_row(
                "SELECT body,sha256,sources,fetched_at FROM caa_cache WHERE cache_key=?1
             AND fetched_at <= unixepoch() AND expires_at > unixepoch()
             AND length(body) BETWEEN 1 AND 8388608 AND length(sources) BETWEEN 2 AND 24576",
                [request.cache_key()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((body, hash, sources, fetched_at)) = saved else {
            return Ok(None);
        };
        if Sha256::digest(&body)[..] != hash {
            return Err(invalid());
        }
        let sources: Vec<String> = serde_json::from_str(&sources)?;
        request.validate_sources(&sources)?;
        Ok(Some(CaaResponse {
            fetch: CaaFetch {
                sources,
                payload: request.restore_payload(body)?,
            },
            fetched_at,
            cached: true,
        }))
    }
}

#[derive(Debug, Serialize, ts_rs::TS)]
pub struct CaaArtworkEvidence {
    pub candidate_id: i64,
    pub release_id: String,
    pub image: CaaImage,
    pub listing_sha256: String,
    pub listing_sources: Vec<String>,
    pub listing_fetched_at: i64,
    pub image_sha256: String,
    pub image_sources: Vec<String>,
    pub image_fetched_at: i64,
    pub permitted_use_acknowledged: bool,
}

pub(super) struct PreparedCaa {
    pub release: String,
    pub image: String,
    pub listing: Vec<u8>,
    pub listing_sources: Vec<String>,
    pub listing_fetched_at: i64,
    pub bytes: Vec<u8>,
    pub image_sources: Vec<String>,
    pub image_fetched_at: i64,
}

impl Store {
    pub(super) fn cached_caa_cover(&self, release: &str, image: &str) -> Result<PreparedCaa> {
        let listing = self
            .caa_cache(&CaaRequest::listing(release)?)?
            .ok_or_else(|| {
                Error::Conflict("fetch a fresh exact-release CAA listing first".into())
            })?;
        let CaaPayload::Listing {
            listing: parsed,
            raw,
        } = listing.fetch.payload
        else {
            return Err(invalid());
        };
        let fetched = self
            .caa_cache(&CaaRequest::image(&parsed, image)?)?
            .ok_or_else(|| Error::Conflict("fetch the selected CAA image first".into()))?;
        let CaaPayload::Image { bytes, .. } = fetched.fetch.payload else {
            return Err(invalid());
        };
        Ok(PreparedCaa {
            release: release.into(),
            image: image.into(),
            listing: raw,
            listing_sources: listing.fetch.sources,
            listing_fetched_at: listing.fetched_at,
            bytes,
            image_sources: fetched.fetch.sources,
            image_fetched_at: fetched.fetched_at,
        })
    }

    /// None means legacy trusted input or a permitted local cover. Never invent
    /// provider evidence for candidates created before this boundary existed.
    pub fn artwork_caa_evidence(
        &self,
        job: i64,
        version: i64,
        candidate: i64,
    ) -> Result<Option<CaaArtworkEvidence>> {
        self.connection.query_row(
            "SELECT id FROM artwork_candidates WHERE id=?1 AND job_id=?2 AND plan_version=?3",
            params![candidate, job, version],
            |row| row.get::<_, i64>(0),
        )?;
        read_evidence(&self.connection, job, version, candidate)
    }

    pub(crate) fn touch_caa_cache(&mut self, request: &CaaRequest) -> Result<()> {
        self.connection.execute(
            "UPDATE caa_cache SET accessed_at_ms=CAST(unixepoch('subsec')*1000 AS INTEGER)
             WHERE cache_key=?1",
            [request.cache_key()],
        )?;
        Ok(())
    }
}

impl PreparedCaa {
    pub(super) fn freeze(&self, connection: &Connection, candidate: i64) -> Result<()> {
        let now: i64 = connection.query_row("SELECT unixepoch()", [], |row| row.get(0))?;
        if [self.listing_fetched_at, self.image_fetched_at]
            .iter()
            .any(|time| *time <= 0 || *time > now || now - time >= 86400)
        {
            return Err(Error::Conflict(
                "CAA cache evidence expired before preview preparation".into(),
            ));
        }
        connection.execute(
            "INSERT INTO artwork_caa_evidence(candidate_id,release_id,image_id,listing,listing_sha256,
              listing_sources,listing_fetched_at,image_sha256,image_sources,image_fetched_at,
              permitted_use_acknowledged) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1)",
            params![candidate, self.release,self.image,self.listing,
                &Sha256::digest(&self.listing)[..],serde_json::to_string(&self.listing_sources)?,
                self.listing_fetched_at,&Sha256::digest(&self.bytes)[..],
                serde_json::to_string(&self.image_sources)?,self.image_fetched_at],
        )?;
        Ok(())
    }
}

pub(super) fn read_evidence(
    connection: &Connection,
    job: i64,
    version: i64,
    candidate: i64,
) -> Result<Option<CaaArtworkEvidence>> {
    let saved = connection
        .query_row(
            "SELECT e.release_id,e.image_id,e.listing,e.listing_sha256,e.listing_sources,
           e.listing_fetched_at,e.image_sha256,e.image_sources,e.image_fetched_at,
           c.image_id,c.input_sha256,c.source
         FROM artwork_caa_evidence e JOIN artwork_candidates c ON c.id=e.candidate_id
         WHERE c.id=?1 AND c.job_id=?2 AND c.plan_version=?3
           AND length(e.listing) BETWEEN 1 AND 2097152
           AND length(e.listing_sources) <= 24576 AND length(e.image_sources) <= 24576",
            params![candidate, job, version],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Vec<u8>>(10)?,
                    row.get::<_, String>(11)?,
                ))
            },
        )
        .optional()?;
    let Some((
        release,
        image,
        raw,
        listing_hash,
        listing_sources,
        listing_at,
        image_hash,
        image_sources,
        image_at,
        candidate_image,
        input_hash,
        source,
    )) = saved
    else {
        return Ok(None);
    };
    let listing = crate::infrastructure::caa::CaaListing::parse(&release, &raw)?;
    let plan = super::planning::read_plan(connection, job, version)?;
    if image != candidate_image
        || image_hash != input_hash
        || Sha256::digest(&raw)[..] != listing_hash
        || plan
            .plan
            .catalog
            .as_ref()
            .is_none_or(|catalog| catalog.data.summary.id != release)
        || source != listing.image(&image)?.request_source
    {
        return Err(invalid());
    }
    let listing_sources: Vec<String> = serde_json::from_str(&listing_sources)?;
    let image_sources: Vec<String> = serde_json::from_str(&image_sources)?;
    CaaRequest::listing(&release)?.validate_sources(&listing_sources)?;
    CaaRequest::image(&listing, &image)?.validate_sources(&image_sources)?;
    Ok(Some(CaaArtworkEvidence {
        candidate_id: candidate,
        release_id: release,
        image: listing.image(&image)?.clone(),
        listing_sha256: listing.payload_sha256().into(),
        listing_sources,
        listing_fetched_at: listing_at,
        image_sha256: hex(&image_hash),
        image_sources,
        image_fetched_at: image_at,
        permitted_use_acknowledged: true,
    }))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
