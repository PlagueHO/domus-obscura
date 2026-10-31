use super::*;
use crate::application::services::artist_images::tests::fixture;
use crate::application::services::cache::testing::MemoryCache;
use crate::infrastructure::store::Store;

pub(crate) fn metadata_body(source: &ArtistImageSource) -> Value {
    let rights = &source.rights;
    serde_json::json!({"query":{"pages":[{
        "pageid":1, "ns":6, "title":source.file, "lastrevid":2,
        "imagerepository":"local", "imageinfo":[{
            "timestamp":source.timestamp, "sha1":source.sha1, "size":source.bytes,
            "width":source.width, "height":source.height, "mime":source.mime,
            "url":source.original_url, "descriptionurl":rights.source_url,
            "extmetadata": {
                "ObjectName":{"value":rights.title}, "Artist":{"value":rights.creator},
                "Credit":{"value":rights.credit}, "LicenseShortName":{"value":rights.license},
                "LicenseUrl":{"value":rights.license_url}, "AttributionRequired":{"value":"true"},
                "UsageTerms":{"value":rights.license}
            }
        }]
    }]}})
}

#[test]
fn artist_images_commons_parser_preserves_exact_source_and_safe_credit() -> Result<()> {
    let (source, _) = fixture()?;
    let mut body = metadata_body(&source);
    body["query"]["pages"][0]["imageinfo"][0]["extmetadata"]["Artist"]["value"] =
        Value::String("<a href=\"https://example.org/creator\">Fixture &amp; co</a>".into());
    let parsed = parse_metadata(&source.file, &serde_json::to_vec(&body)?)?
        .ok_or(Error::Internal("fixture file missing"))?;
    assert!(
        parsed
            .rights
            .creator
            .contains("https://example.org/creator")
    );
    assert!(parsed.rights.creator.contains("Fixture & co"));
    assert!(!parsed.rights.creator.contains('<'));
    assert!(parse_metadata("File:Other.jpg", &serde_json::to_vec(&body)?).is_err());
    body["query"]["pages"][0]["imageinfo"][0]["extmetadata"]["LicenseUrl"]["value"] =
        Value::String("https://example.org/not-a-license".into());
    assert!(parse_metadata(&source.file, &serde_json::to_vec(&body)?).is_err());
    for raw in [
        "<script>bad</script>",
        "<img src=x>",
        "Name &unknown;",
        "<a href=\"http://example.org\">Name</a>",
    ] {
        assert!(credit_text(raw).is_err());
    }
    assert_eq!(
        credit_text("Fixture &#x41; &#66; &quot;credit&quot;")?,
        "Fixture A B \"credit\""
    );
    Ok(())
}

#[test]
fn artist_images_commons_parser_rejects_restricted_missing_and_unbounded_metadata() -> Result<()> {
    let (source, _) = fixture()?;
    for field in [
        "Artist",
        "Credit",
        "LicenseShortName",
        "LicenseUrl",
        "AttributionRequired",
    ] {
        let mut body = metadata_body(&source);
        body["query"]["pages"][0]["imageinfo"][0]["extmetadata"]
            .as_object_mut()
            .ok_or(Error::Internal("fixture extmetadata missing"))?
            .remove(field);
        assert!(parse_metadata(&source.file, &serde_json::to_vec(&body)?).is_err());
    }
    let mut body = metadata_body(&source);
    body["query"]["pages"][0]["imageinfo"][0]["extmetadata"]["Restrictions"] =
        serde_json::json!({"value":"Personality rights"});
    assert!(parse_metadata(&source.file, &serde_json::to_vec(&body)?).is_err());
    let missing =
        serde_json::json!({"query":{"pages":[{"ns":6,"title":source.file,"missing":true}]}});
    assert!(parse_metadata(&source.file, &serde_json::to_vec(&missing)?)?.is_none());
    body = metadata_body(&source);
    body["query"]["pages"][0]["imageinfo"][0]["width"] = serde_json::json!(9000);
    assert!(parse_metadata(&source.file, &serde_json::to_vec(&body)?).is_err());
    body = metadata_body(&source);
    body["error"] = serde_json::json!({"code":"maxlag"});
    assert!(parse_metadata(&source.file, &serde_json::to_vec(&body)?).is_err());
    Ok(())
}

#[test]
fn artist_images_commons_rejects_arbitrary_original_urls_and_hash_mismatches() -> Result<()> {
    let (mut source, original) = fixture()?;
    original_url(&source)?;
    validate_original(&source, &original)?;
    assert!(validate_original(&source, &[1, 2, 3]).is_err());
    for url in [
        "http://upload.wikimedia.org/wikipedia/commons/a/ab/Generated.jpg",
        "https://example.org/wikipedia/commons/a/ab/Generated.jpg",
        "https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Generated.jpg",
        "https://upload.wikimedia.org/wikipedia/commons/a/ab/Other.jpg",
        "https://upload.wikimedia.org/wikipedia/commons/a/ab/Generated.jpg?x=1",
        "https://user@upload.wikimedia.org/wikipedia/commons/a/ab/Generated.jpg",
        "https://upload.wikimedia.org:8443/wikipedia/commons/a/ab/Generated.jpg",
    ] {
        source.original_url = url.into();
        assert!(original_url(&source).is_err(), "{url}");
    }
    Ok(())
}

#[tokio::test]
async fn artist_images_commons_cache_reuses_original_without_any_network_and_reports_corruption()
-> Result<()> {
    let state = super::super::store::artist_images::tests::state()?;
    let database = state.path().join("state.db");
    drop(Store::open(&database)?);
    let (source, original) = fixture()?;
    let cache = Arc::new(MemoryCache::default());
    let now = now()?;
    let metadata_key = CacheKey::new(PROVIDER, "file-metadata-v1", &source.file)?;
    cache.put(
        &metadata_key,
        &serde_json::to_vec(&metadata_body(&source))?,
        now,
        now + 86400,
    )?;
    let evidence_source =
        parse_metadata(&source.file, &serde_json::to_vec(&metadata_body(&source))?)?
            .ok_or(Error::Internal("fixture metadata missing"))?;
    let original_key = CacheKey::new(PROVIDER, "original-v1", &evidence_source.fingerprint()?)?;
    cache.put(&original_key, &original, now, now + 86400)?;
    let client = CommonsClient::new(database.clone(), cache.clone())?;
    for _ in 0..2 {
        let evidence = client
            .inspect(source.file.clone())
            .await?
            .ok_or(Error::Internal("fixture missing"))?;
        assert_eq!(client.original(evidence.source).await?, original);
    }
    let count: i64 = rusqlite::Connection::open(&database)?.query_row(
        "SELECT generation FROM commons_gate WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count, 0);
    cache
        .entries
        .lock()
        .map_err(|_| Error::Internal("fixture cache lock"))?
        .get_mut(original_key.digest())
        .ok_or(Error::Internal("fixture cache missing"))?
        .bytes[0] ^= 1;
    assert!(client.original(evidence_source).await.is_err());
    Ok(())
}
