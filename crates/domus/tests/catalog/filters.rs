use super::*;
use domus::domain::CatalogReleaseFilters;

#[test]
fn catalog_album_search_is_literal_artist_scoped_and_uses_documented_fields() -> TestResult {
    let query = CatalogQuery::albums(
        "A+B \" OR status:*",
        Some(ARTIST),
        0,
        25,
        CatalogReleaseFilters::default(),
    )?;
    let url = reqwest::Url::parse(query.source())?;
    let expression = url
        .query_pairs()
        .find(|(key, _)| key == "query")
        .ok_or("query missing")?
        .1;
    assert_eq!(
        expression,
        format!(
            "release:\"A\\+B \\\" OR status\\:\\*\" AND arid:{ARTIST} AND (primarytype:album OR primarytype:ep) AND status:official AND NOT secondarytype:live AND NOT secondarytype:compilation"
        )
    );
    let all = CatalogReleaseFilters {
        include_singles: true,
        include_live: true,
        include_compilations: true,
        include_unofficial: true,
    };
    let query = CatalogQuery::albums("Fixture", None, 0, 25, all)?;
    let url = reqwest::Url::parse(query.source())?;
    let expression = url
        .query_pairs()
        .find(|(key, _)| key == "query")
        .ok_or("query missing")?
        .1;
    assert_eq!(
        expression,
        "release:\"Fixture\" AND (primarytype:album OR primarytype:ep OR primarytype:single)"
    );
    for title in ["", " ", "\n", &"x".repeat(513)] {
        assert!(CatalogQuery::albums(title, None, 0, 25, all).is_err());
    }
    assert!(CatalogQuery::albums("Fixture", Some("../artist"), 0, 25, all).is_err());
    assert!(CatalogQuery::albums("Fixture", None, 0, 26, all).is_err());
    assert!(
        CatalogQuery::albums(&"\\".repeat(512), Some(ARTIST), 0, 25, all)?
            .source()
            .len()
            <= 4096
    );
    let query = CatalogQuery::filtered_releases(ARTIST, 0, 25, all)?;
    let url = reqwest::Url::parse(query.source())?;
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "type")
            .ok_or("type missing")?
            .1,
        "album|ep|single"
    );
    assert!(!url.query_pairs().any(|(key, _)| key == "status"));
    Ok(())
}

#[test]
fn catalog_release_filters_cover_all_combinations_without_losing_provider_cursors() -> TestResult {
    let cases = [
        (json!("Album"), json!([]), json!("Official"), Some(0)),
        (json!("EP"), json!([]), json!("Official"), Some(0)),
        (json!("Single"), json!([]), json!("Official"), Some(1)),
        (json!("Album"), json!(["Live"]), json!("Official"), Some(2)),
        (
            json!("Album"),
            json!(["Compilation"]),
            json!("Official"),
            Some(4),
        ),
        (json!("Album"), json!([]), json!("Bootleg"), Some(8)),
        (json!("Album"), json!([]), Value::Null, Some(8)),
        (
            json!("Album"),
            json!(["Live", "Compilation"]),
            json!("Official"),
            Some(6),
        ),
        (json!("Single"), json!(["Live"]), json!("Bootleg"), Some(11)),
        (Value::Null, json!([]), json!("Official"), None),
        (json!("Other"), json!([]), json!("Official"), None),
    ];
    for (primary, secondary, status, required) in cases {
        for mask in 0..16 {
            let filters = CatalogReleaseFilters {
                include_singles: mask & 1 != 0,
                include_live: mask & 2 != 0,
                include_compilations: mask & 4 != 0,
                include_unofficial: mask & 8 != 0,
            };
            let mut raw = release();
            raw["release-group"]["primary-type"] = primary.clone();
            raw["release-group"]["secondary-types"] = secondary.clone();
            raw["status"] = status.clone();
            let body = serde_json::to_vec(
                &json!({"release-offset":0,"release-count":3,"releases":[raw.clone()]}),
            )?;
            let page =
                CatalogQuery::filtered_releases(ARTIST, 0, 25, filters)?.parse_releases(&body)?;
            assert_eq!(
                page.items.len(),
                usize::from(required.is_some_and(|required| mask & required == required))
            );
            assert_eq!(page.scanned, 1);
            assert_eq!(page.count, 3);
            assert_eq!(page.next_offset, Some(1));
            assert_eq!(page.filters, filters);
            let body = serde_json::to_vec(&json!({"offset":2,"count":3,"releases":[raw]}))?;
            let page = CatalogQuery::albums("Fixture", Some(ARTIST), 2, 25, filters)?
                .parse_releases(&body)?;
            assert_eq!(page.next_offset, None);
            assert_eq!(page.scanned, 1);
        }
    }
    Ok(())
}

#[test]
fn catalog_hidden_releases_still_require_valid_distinct_artist_bound_evidence() -> TestResult {
    let query = CatalogQuery::releases(ARTIST, 0, 25)?;
    let mut hidden = release();
    hidden["release-group"]["primary-type"] = json!("Single");
    assert!(
        query
            .parse_releases(&serde_json::to_vec(&json!({
                "release-offset":0,"release-count":2,"releases":[hidden.clone(),hidden.clone()]
            }))?)
            .is_err()
    );
    hidden["artist-credit"][0]["artist"]["id"] = json!(GROUP);
    assert!(
        query
            .parse_releases(&serde_json::to_vec(&json!({
                "release-offset":0,"release-count":1,"releases":[hidden]
            }))?)
            .is_err()
    );
    let mut raw = release();
    raw["release-group"]["secondary-types"] = json!(["Live", "live"]);
    assert!(
        CatalogQuery::release(RELEASE)?
            .parse_release(&serde_json::to_vec(&raw)?)
            .is_err()
    );
    raw["release-group"]["secondary-types"] = json!([]);
    raw["release-group"]["id"] = json!("../group");
    assert!(
        CatalogQuery::release(RELEASE)?
            .parse_release(&serde_json::to_vec(&raw)?)
            .is_err()
    );
    Ok(())
}
