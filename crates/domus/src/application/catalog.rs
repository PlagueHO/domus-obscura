use crate::domain::{AudioFormat, CatalogRelease};
use crate::infrastructure::catalog::{CatalogQuery, CatalogReply, CatalogResult};
use crate::infrastructure::store::{CatalogPermit, PlanRequest, Store, StoredPlan};
use crate::{Error, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct CatalogPlanRequest {
    pub job_id: i64,
    pub expected_version: i64,
    pub settings_version: i64,
    pub format: Option<AudioFormat>,
    pub locations: Vec<i64>,
    pub accepts_incomplete_coverage: bool,
    pub bucket: Option<String>,
}

impl CatalogPlanRequest {
    pub fn into_plan_request(self, catalog: &CatalogResult<CatalogRelease>) -> Result<PlanRequest> {
        Ok(PlanRequest {
            job_id: self.job_id,
            expected_version: self.expected_version,
            settings_version: self.settings_version,
            format: self.format,
            edition: catalog.data.selection(self.bucket)?,
            locations: self.locations,
            accepts_incomplete_coverage: self.accepts_incomplete_coverage,
        })
    }
}

/// Prepare from retained provider evidence only. This never makes a network
/// request, substitutes caller track text, approves a plan or starts a transfer.
pub fn prepare_cached_catalog_plan(
    store: &mut Store,
    release: &str,
    request: CatalogPlanRequest,
) -> Result<StoredPlan> {
    let query = CatalogQuery::release(release)?;
    let catalog = cached_catalog(store, &query, CatalogQuery::parse_release)?.ok_or_else(|| {
        Error::Conflict("load the exact release before preparing a catalog plan".into())
    })?;
    store.prepare_catalog_plan(request.into_plan_request(&catalog)?, catalog)
}

pub fn cached_catalog<T>(
    store: &mut Store,
    query: &CatalogQuery,
    parse: fn(&CatalogQuery, &[u8]) -> Result<T>,
) -> Result<Option<CatalogResult<T>>> {
    store
        .catalog_cache(query)?
        .map(|cache| {
            let data = parse(query, &cache.body)?;
            Ok(CatalogResult {
                source: query.source().into(),
                fetched_at: cache.fetched_at,
                cached: true,
                payload_sha256: cache.payload_sha256,
                data,
            })
        })
        .transpose()
}

/// Persist success or failure while retaining the OS-backed provider permit.
/// Both CLI and API use this path so validation failures cannot erase backoff.
pub fn complete_catalog<T>(
    store: &mut Store,
    query: CatalogQuery,
    permit: CatalogPermit,
    outcome: Result<CatalogReply>,
    parse: fn(&CatalogQuery, &[u8]) -> Result<T>,
) -> Result<CatalogResult<T>> {
    let retry = match &outcome {
        Ok(reply) => reply.retry_after_seconds.max(1),
        Err(Error::Catalog {
            retry_after_seconds,
            ..
        }) => *retry_after_seconds,
        Err(_) => 5,
    };
    let parsed = outcome.and_then(|reply| {
        if reply.status == 200 {
            parse(&query, &reply.body).map(|data| (data, reply.body, reply.retry_after_seconds))
        } else {
            let reason = match reply.status {
                404 => "MusicBrainz record was not found",
                429 | 503 => "MusicBrainz is rate limited or temporarily unavailable",
                _ => "MusicBrainz returned an unsupported response status",
            };
            Err(Error::Catalog {
                reason,
                retry_after_seconds: retry,
            })
        }
    });
    match parsed {
        Ok((data, body, cooldown)) => {
            let cache = store
                .complete_catalog_request(permit, cooldown, Some((&query, &body)))?
                .cache
                .ok_or(Error::Internal("catalog response was not cached"))?;
            Ok(CatalogResult {
                source: query.source().into(),
                fetched_at: cache.fetched_at,
                cached: false,
                payload_sha256: cache.payload_sha256,
                data,
            })
        }
        Err(error) => {
            let completion = store.complete_catalog_request(permit, retry, None)?;
            match error {
                Error::Catalog { reason, .. } => Err(Error::Catalog {
                    reason,
                    retry_after_seconds: completion.retry_after_seconds,
                }),
                error => Err(error),
            }
        }
    }
}
