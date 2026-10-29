//! Fixture-only adapters for the shared Library interface. No production router.
use super::*;
use axum::extract::Path as RoutePath;
use domus::domain::library::LibraryMatchQuery;
use serde_json::{Value, json};
use std::io::Write;

pub(super) fn generate(folder: &Path) -> Result<()> {
    use id3::TagLike;
    for (name, artist, album) in [
        ("track.mp3", "Sample Artist", "Sample Album"),
        ("guest.mp3", "Guest Artist", "Shared Folder"),
    ] {
        let mut tag = id3::Tag::new();
        tag.set_title("Generated header fixture");
        tag.set_artist(artist);
        tag.set_album(album);
        tag.set_track(1);
        let mut file = std::fs::File::create(folder.join(name))?;
        tag.write_to(&mut file, id3::Version::Id3v24)
            .map_err(|_| Error::Internal("fixture tags could not be written"))?;
        for _ in 0..3 {
            let mut frame = vec![0; 417];
            frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0]);
            file.write_all(&frame)?;
        }
    }
    std::fs::write(
        folder.join("unsupported.m4a"),
        b"Generated unsupported-format fixture",
    )?;
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Browse {
    after: i64,
    limit: Option<u16>,
    artist: Option<i64>,
    folder: String,
}

fn require_location(fixture: &Fixture, location: i64) -> Result<()> {
    if location != fixture.location {
        return Err(Error::InvalidInput(
            "only the generated location is allowed".into(),
        ));
    }
    Ok(())
}

pub(super) async fn read(
    State(shared): State<Shared>,
    RoutePath((location, tail)): RoutePath<(i64, String)>,
    Query(query): Query<Browse>,
) -> ApiResult<Json<Value>> {
    database(shared, move |fixture| {
        require_location(fixture, location)?;
        let page = Page::new(query.after, query.limit.unwrap_or(50))?;
        let parts = tail.split('/').collect::<Vec<_>>();
        let entity = || {
            parts
                .get(1)
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| Error::InvalidInput("invalid fixture entity".into()))
        };
        Ok(match parts.as_slice() {
            ["artists"] => json!(fixture.store.library_artists(location, page)?),
            ["albums"] => json!(fixture.store.library_albums(location, query.artist, page)?),
            ["coverage"] => json!(
                fixture
                    .store
                    .library_coverage(location, &LibraryPath::parse(&query.folder)?)?
            ),
            ["albums", _] => json!(fixture.store.library_album(location, entity()?)?),
            ["albums", _, "files"] => json!(fixture.store.library_album_files(
                location,
                entity()?,
                page
            )?),
            ["albums", _, "assets"] => json!(fixture.store.library_album_assets(
                location,
                entity()?,
                page
            )?),
            ["files", _] => json!(fixture.store.library_file(location, entity()?)?),
            _ => return Err(Error::InvalidInput("unsupported fixture read".into())),
        })
    })
    .await
    .map(Json)
}

pub(super) async fn matches(
    State(shared): State<Shared>,
    RoutePath(location): RoutePath<i64>,
    Json(queries): Json<Vec<LibraryMatchQuery>>,
) -> ApiResult<Json<Value>> {
    database(shared, move |fixture| {
        require_location(fixture, location)?;
        Ok(json!(fixture.store.library_matches(location, &queries)?))
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Refresh {
    folder: String,
}

pub(super) async fn refresh(
    State(shared): State<Shared>,
    RoutePath(location): RoutePath<i64>,
    Json(request): Json<Refresh>,
) -> ApiResult<Json<Value>> {
    database(shared, move |fixture| {
        require_location(fixture, location)?;
        let folder = LibraryPath::parse(&request.folder)?;
        domus::application::refresh_library_folder(&mut fixture.store, location, &folder)?;
        Ok(json!({"coverage": fixture.store.library_coverage(location, &folder)?, "request": null}))
    })
    .await
    .map(Json)
}

pub(super) async fn availability(
    State(shared): State<Shared>,
    RoutePath(location): RoutePath<i64>,
) -> ApiResult<StatusCode> {
    database(shared, move |fixture| {
        require_location(fixture, location)?;
        domus::application::check_location_availability(&fixture.store, location)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn entries(
    State(shared): State<Shared>,
    RoutePath(location): RoutePath<i64>,
    Query(query): Query<Browse>,
) -> ApiResult<Json<Vec<Entry>>> {
    database(shared, move |fixture| {
        require_location(fixture, location)?;
        fixture.store.browse(
            location,
            &LibraryPath::parse(&query.folder)?,
            Page::new(query.after, query.limit.unwrap_or(50))?,
        )
    })
    .await
    .map(Json)
}
