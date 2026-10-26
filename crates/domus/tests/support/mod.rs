use domus::domain::{CatalogRelease, EditionSelection};
use domus::infrastructure::catalog::{CatalogQuery, CatalogResult};
use rusqlite::params;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Seed a generated, single-disc provider response, not an application bypass.
/// Production preparation must still check its raw body, digest and exact mapping.
pub fn catalog_snapshot(
    database: &Path,
    edition: &EditionSelection,
) -> Result<CatalogResult<CatalogRelease>, Box<dyn std::error::Error>> {
    if edition
        .tracks
        .iter()
        .any(|track| track.disc.is_some_and(|disc| disc != 1))
    {
        return Err("fixture accepts one generated disc only".into());
    }
    let credit = |name: &str| {
        json!([{"name":name,"artist":{
            "id":"10000000-0000-4000-8000-000000000001","name":name
        }}])
    };
    let tracks = edition
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| {
            json!({
                "id":format!("40000000-0000-4000-8000-{:012x}",index+1),
                "position":index+1,"number":(index+1).to_string(),"title":track.title,
                "artist-credit":credit(&track.artist),
                "recording":{"id":format!("50000000-0000-4000-8000-{:012x}",index+1),"video":false}
            })
        })
        .collect::<Vec<_>>();
    let body = serde_json::to_vec(&json!({
        "id":edition.release_id,"title":edition.album,"artist-credit":credit(&edition.album_artist),
        "release-group":{"id":"30000000-0000-4000-8000-000000000001"},
        "media":[{"position":1,"format":"CD","track-count":tracks.len(),"tracks":tracks}]
    }))?;
    let query = CatalogQuery::release(&edition.release_id)?;
    let data = query.parse_release(&body)?;
    let payload_sha256 = Sha256::digest(&body).to_vec();
    let connection = rusqlite::Connection::open(database)?;
    let fetched_at: i64 = connection.query_row("SELECT unixepoch()", [], |row| row.get(0))?;
    connection.execute(
        "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
         VALUES (?1,?2,?3,?4,?4+86400,?4*1000)",
        params![query.source(), body, payload_sha256, fetched_at],
    )?;
    Ok(CatalogResult {
        source: query.source().into(),
        fetched_at,
        cached: true,
        payload_sha256,
        data,
    })
}
