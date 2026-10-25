use super::*;
use crate::infrastructure::caa::CaaListing;
use serde_json::json;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
const RELEASE: &str = "20000000-0000-4000-8000-000000000001";

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
