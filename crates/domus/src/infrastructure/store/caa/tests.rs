use super::*;
use crate::infrastructure::caa::CaaListing;
use serde_json::json;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
const RELEASE: &str = "20000000-0000-4000-8000-000000000001";

#[test]
fn shared_cache_caa_preserves_provenance_expiry_and_exact_filename_without_legacy_fallback()
-> TestResult {
    use crate::application::services::cache::testing::MemoryCache;
    use std::sync::Arc;
    let state = tempfile::tempdir()?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let (parsed, raw) = listing(1, "png")?;
    let request = CaaRequest::listing(RELEASE)?;
    cache(
        &mut store,
        &request,
        CaaPayload::Listing {
            listing: parsed.clone(),
            raw: raw.clone(),
        },
    )?;
    let shared = Arc::new(MemoryCache::default());
    let store = store.with_cache(shared.clone());
    assert!(store.caa_cache(&request)?.is_none());
    let now: i64 = store
        .connection
        .query_row("SELECT unixepoch()", [], |row| row.get(0))?;
    let fetch = CaaFetch {
        sources: request.sources(),
        payload: CaaPayload::Listing {
            listing: parsed.clone(),
            raw,
        },
    };
    persist_shared(shared.as_ref(), &request, &fetch, now)?;
    let saved = store.caa_cache(&request)?.ok_or("shared listing missing")?;
    assert_eq!(saved.fetched_at, now);
    assert_eq!(saved.fetch.sources, fetch.sources);
    assert!(saved.cached);
    assert!(shared_response(shared.as_ref(), &request, now as u64 + 86399)?.is_some());
    assert!(shared_response(shared.as_ref(), &request, now as u64 + 86400)?.is_none());
    assert!(shared_response(shared.as_ref(), &request, now as u64 - 1)?.is_none());
    let image = CaaRequest::image(&parsed, "1")?;
    let bytes = b"transport hashes alone are not image validity".to_vec();
    persist_shared(
        shared.as_ref(),
        &image,
        &CaaFetch {
            sources: image.sources(),
            payload: CaaPayload::Image {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                bytes,
            },
        },
        now,
    )?;
    let changed = CaaRequest::image(&listing(1, "jpg")?.0, "1")?;
    assert!(store.caa_cache(&changed)?.is_none());
    assert!(
        store
            .caa_cache(&CaaRequest::listing(
                "20000000-0000-4000-8000-000000000002"
            )?)?
            .is_none()
    );
    assert!(shared.invalidate(&shared_key(&request)?)?);
    assert!(store.caa_cache(&request)?.is_none());
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM caa_cache", [], |row| row
                .get::<_, i64>(0))?,
        1
    );
    persist_shared(
        shared.as_ref(),
        &request,
        &CaaFetch {
            sources: request.sources(),
            payload: CaaPayload::Missing,
        },
        now,
    )?;
    assert!(store.caa_cache(&request)?.is_none());
    persist_shared(shared.as_ref(), &request, &fetch, now)?;
    {
        let mut entries = shared.entries.lock().map_err(|_| "cache lock")?;
        let entry = entries
            .get_mut(shared_key(&request)?.digest())
            .ok_or("shared listing missing")?;
        entry.bytes[0] ^= 1;
    }
    assert!(store.caa_cache(&request).is_err());
    Ok(())
}

#[test]
fn shared_cache_caa_rejects_malformed_envelopes_and_transactional_reads() -> TestResult {
    use crate::application::services::cache::testing::MemoryCache;
    use std::sync::Arc;
    let state = tempfile::tempdir()?;
    let shared = Arc::new(MemoryCache::default());
    let store = Store::open(&state.path().join("state.db"))?.with_cache(shared.clone());
    let request = CaaRequest::listing(RELEASE)?;
    let now: i64 = store
        .connection
        .query_row("SELECT unixepoch()", [], |row| row.get(0))?;
    let (parsed, raw) = listing(1, "png")?;
    let fetch = CaaFetch {
        sources: request.sources(),
        payload: CaaPayload::Listing {
            listing: parsed,
            raw,
        },
    };
    for mutation in 0..6 {
        persist_shared(shared.as_ref(), &request, &fetch, now)?;
        {
            let mut entries = shared.entries.lock().map_err(|_| "cache lock")?;
            let entry = entries
                .get_mut(shared_key(&request)?.digest())
                .ok_or("shared listing missing")?;
            match mutation {
                0 => entry.bytes[0] ^= 1,
                1 => entry.bytes.truncate(11),
                2 => entry.bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes()),
                3 => entry.expires_at += 1,
                4 => {
                    let sources = serde_json::to_vec(&vec!["https://example.invalid/"])?;
                    let mut bytes = SHARED_PREFIX.to_vec();
                    bytes.extend_from_slice(&(sources.len() as u32).to_le_bytes());
                    bytes.extend_from_slice(&sources);
                    bytes.extend_from_slice(b"{}");
                    entry.bytes = bytes;
                }
                _ => entry.bytes[12] = b'!',
            }
            entry.sha256 = Sha256::digest(&entry.bytes).into();
        }
        assert!(store.caa_cache(&request).is_err(), "mutation {mutation}");
    }
    persist_shared(shared.as_ref(), &request, &fetch, now)?;
    store.connection.execute_batch("BEGIN DEFERRED")?;
    assert!(store.caa_cache(&request).is_err());
    store.connection.execute_batch("ROLLBACK")?;
    assert!(store.caa_cache(&request)?.is_some());
    Ok(())
}

struct TransactionProbe {
    cache: crate::application::services::cache::testing::MemoryCache,
    database: std::path::PathBuf,
}

impl Cache for TransactionProbe {
    fn lookup(&self, key: &CacheKey, now: u64) -> Result<CacheLookup> {
        Connection::open(&self.database)?.execute_batch("BEGIN IMMEDIATE; ROLLBACK;")?;
        self.cache.lookup(key, now)
    }
    fn put(&self, key: &CacheKey, bytes: &[u8], now: u64, expires: u64) -> Result<[u8; 32]> {
        Connection::open(&self.database)?.execute_batch("BEGIN IMMEDIATE; ROLLBACK;")?;
        self.cache.put(key, bytes, now, expires)
    }
    fn invalidate(&self, key: &CacheKey) -> Result<bool> {
        self.cache.invalidate(key)
    }
    fn evict_expired(&self, now: u64, limit: u16) -> Result<u16> {
        self.cache.evict_expired(now, limit)
    }
}

#[tokio::test]
async fn shared_cache_caa_commits_budget_before_storage_and_never_rolls_it_back_on_failure()
-> TestResult {
    use std::sync::{Arc, atomic::Ordering};
    for fail in [false, true] {
        let state = tempfile::tempdir()?;
        let database = state.path().join("state.db");
        let shared = Arc::new(TransactionProbe {
            cache: Default::default(),
            database: database.clone(),
        });
        shared.cache.fail_put.store(fail, Ordering::SeqCst);
        let mut store = Store::open(&database)?.with_cache(shared);
        let (parsed, raw) = listing(1, "png")?;
        let request = CaaRequest::listing(RELEASE)?;
        let fetched = CaaFetch {
            sources: request.sources(),
            payload: CaaPayload::Listing {
                listing: parsed,
                raw,
            },
        };
        let mut permit = store.claim_catalog_request()?;
        permit.start().await?;
        let result = store.complete_caa_request(permit, 9, &request, &fetched);
        assert_eq!(result.is_err(), fail);
        let (lease, delay, count): (i64, u32, i64) = store.connection.query_row(
            "SELECT lease_until_ms,delay_seconds,(SELECT count(*) FROM caa_cache) FROM catalog_gate WHERE id=1", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(lease, 0);
        assert_eq!(delay, 9);
        assert_eq!(count, 0);
        assert_eq!(store.caa_cache(&request)?.is_none(), fail);
        assert!(store.claim_catalog_request().is_err());
    }
    Ok(())
}

fn listing(id: u32, extension: &str) -> Result<(CaaListing, Vec<u8>)> {
    let raw = serde_json::to_vec(&json!({
        "release":format!("https://musicbrainz.org/release/{RELEASE}"),
        "images":[{
            "id":id,"edit":1,"image":format!("https://coverartarchive.org/release/{RELEASE}/{id}.{extension}"),
            "types":["Front"],"front":true,"back":false,"approved":false,"comment":"Generated fixture"
        }]
    }))?;
    Ok((CaaListing::parse(RELEASE, &raw)?, raw))
}

fn cache(store: &mut Store, request: &CaaRequest, payload: CaaPayload) -> Result<()> {
    let now: i64 = store.connection.query_row(
        "SELECT CAST(unixepoch('subsec') * 1000 AS INTEGER)",
        [],
        |row| row.get(0),
    )?;
    let tx = store.connection.transaction()?;
    persist_cache(
        &tx,
        request,
        &CaaFetch {
            sources: request.sources(),
            payload,
        },
        now,
    )?;
    tx.commit()?;
    Ok(())
}

#[test]
fn caa_cache_checks_hashes_expiry_scope_and_image_format_before_reuse() -> TestResult {
    let state = tempfile::tempdir()?;
    let path = state.path().join("state.db");
    let mut store = Store::open(&path)?;
    let request = CaaRequest::listing(RELEASE)?;
    let (parsed, raw) = listing(1, "png")?;
    cache(
        &mut store,
        &request,
        CaaPayload::Listing {
            listing: parsed.clone(),
            raw: raw.clone(),
        },
    )?;
    let cached = store.caa_cache(&request)?.ok_or("cache missing")?;
    assert!(cached.cached);
    assert_eq!(cached.fetch.sources, request.sources());
    assert!(cached.fetched_at > 0);
    let image = CaaRequest::image(&parsed, "1")?;
    let body = b"transport bytes are not valid-image proof".to_vec();
    cache(
        &mut store,
        &image,
        CaaPayload::Image {
            sha256: format!("{:x}", Sha256::digest(&body)),
            bytes: body,
        },
    )?;
    let (changed, _) = listing(1, "jpg")?;
    assert!(
        store
            .caa_cache(&CaaRequest::image(&changed, "1")?)?
            .is_none()
    );
    drop(store);
    let store = Store::open_existing(&path)?;
    assert!(store.caa_cache(&request)?.is_some());
    assert!(store.caa_cache(&image)?.is_some());
    store.connection.execute(
        "UPDATE caa_cache SET sha256=zeroblob(32) WHERE cache_key=?1",
        [request.cache_key()],
    )?;
    assert!(store.caa_cache(&request).is_err());
    store.connection.execute(
        "UPDATE caa_cache SET sha256=?1,sources=?2 WHERE cache_key=?3",
        params![
            &Sha256::digest(&raw)[..],
            r#"["https://example.invalid/"]"#,
            request.cache_key()
        ],
    )?;
    assert!(store.caa_cache(&request).is_err());
    store.connection.execute(
        "UPDATE caa_cache SET sources=?1,fetched_at=unixepoch()-86401,expires_at=unixepoch()-1 WHERE cache_key=?2",
        params![serde_json::to_string(&request.sources())?,request.cache_key()],
    )?;
    assert!(store.caa_cache(&request)?.is_none());
    store.connection.execute(
        "UPDATE caa_cache SET fetched_at=unixepoch()+60,expires_at=unixepoch()+86460 WHERE cache_key=?1",
        [request.cache_key()],
    )?;
    assert!(store.caa_cache(&request)?.is_none());
    Ok(())
}

#[test]
fn caa_cache_enforces_exact_entry_byte_bounds_and_no_missing_or_invalid_entries() -> TestResult {
    let state = tempfile::tempdir()?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let request = CaaRequest::listing(RELEASE)?;
    cache(&mut store, &request, CaaPayload::Missing)?;
    assert!(store.caa_cache(&request)?.is_none());
    let (parsed, raw) = listing(1, "png")?;
    assert!(
        persist_cache(
            &store.connection,
            &request,
            &CaaFetch {
                sources: vec!["https://example.invalid/".into()],
                payload: CaaPayload::Listing {
                    listing: parsed.clone(),
                    raw
                },
            },
            1_000
        )
        .is_err()
    );
    let image = CaaRequest::image(&parsed, "1")?;
    assert!(
        cache(
            &mut store,
            &image,
            CaaPayload::Image {
                bytes: vec![1; 8388609],
                sha256: "bad".into()
            }
        )
        .is_err()
    );
    assert!(
        cache(
            &mut store,
            &image,
            CaaPayload::Image {
                bytes: vec![1],
                sha256: "bad".into()
            }
        )
        .is_err()
    );
    for id in 1..=33 {
        let (parsed, raw) = listing(id, "png")?;
        let release = format!("20000000-0000-4000-8000-{id:012}");
        let raw = String::from_utf8(raw)?
            .replace(RELEASE, &release)
            .into_bytes();
        cache(
            &mut store,
            &CaaRequest::listing(&release)?,
            CaaPayload::Listing {
                listing: CaaListing::parse(&release, &raw)?,
                raw,
            },
        )?;
        assert!(parsed.image(&id.to_string()).is_ok());
    }

    let count: i64 = store
        .connection
        .query_row("SELECT count(*) FROM caa_cache", [], |row| row.get(0))?;
    assert_eq!(count, 32);
    store.connection.execute("DELETE FROM caa_cache", [])?;
    for id in 1..=9 {
        let (parsed, _) = listing(id, "png")?;
        let bytes = vec![id as u8; 8388608];
        cache(
            &mut store,
            &CaaRequest::image(&parsed, &id.to_string())?,
            CaaPayload::Image {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                bytes,
            },
        )?;
    }
    let (count, bytes): (i64, i64) = store.connection.query_row(
        "SELECT count(*),sum(length(body)+length(sources)) FROM caa_cache",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(count, 7);
    assert!(bytes <= 67108864);
    assert!(
        store
            .caa_cache(&CaaRequest::image(&listing(9, "png")?.0, "9")?)?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn caa_cache_and_provider_completion_commit_atomically_or_keep_uncertain_lease() -> TestResult
{
    for fail in [false, true] {
        let state = tempfile::tempdir()?;
        let mut store = Store::open(&state.path().join("state.db"))?;
        let (listing, raw) = listing(1, "png")?;
        let request = CaaRequest::listing(RELEASE)?;
        let fetch = CaaFetch {
            sources: request.sources(),
            payload: CaaPayload::Listing { listing, raw },
        };
        let mut permit = store.claim_catalog_request()?;
        permit.start().await?;
        if fail {
            store.connection.execute_batch(
                "CREATE TRIGGER fail_cache BEFORE INSERT ON caa_cache BEGIN SELECT RAISE(ABORT,'injected'); END;"
            )?;
        }
        let completed = store.complete_caa_request(permit, 1, &request, &fetch);
        let (lease, count): (i64, i64) = store.connection.query_row(
            "SELECT lease_until_ms,(SELECT count(*) FROM caa_cache) FROM catalog_gate WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if fail {
            assert!(completed.is_err());
            assert!(lease > 0);
            assert_eq!(count, 0);
            assert!(store.claim_catalog_request().is_err());
        } else {
            assert!(completed.is_ok());
            assert_eq!(lease, 0);
            assert_eq!(count, 1);
            assert!(store.caa_cache(&request)?.is_some());
        }
    }
    Ok(())
}
