use super::*;
use crate::application::services::EntityRef;
use crate::application::services::artist_images::*;
use crate::application::services::images::ImageOptions;
use crate::domain::ArtworkSize;

#[cfg(test)]
mod tests;

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/api/system/artist-photos", get(list))
        .route(
            "/api/system/artist-photos/check",
            post(check).layer(DefaultBodyLimit::max(2048)),
        )
        .route(
            "/api/system/artist-photos/save",
            post(save).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/system/artist-photos/remove",
            post(remove).layer(DefaultBodyLimit::max(256)),
        )
        .route("/api/system/artist-photos/resume", post(resume))
        .route("/api/artist-photos", get(read))
        .route("/api/artist-photos/image", get(image))
        .route("/api/artist-photos/preview", get(preview))
}

#[derive(Serialize, ts_rs::TS)]
pub struct ArtistPhotoCheck {
    pub photo: ArtistPhoto,
    pub confirmation: String,
    pub expected_version: Option<i64>,
    pub preview_url: String,
}

#[derive(Serialize, ts_rs::TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ArtistPhotoState {
    Disabled,
    Unbound,
    Missing,
    Ready {
        binding: Box<ArtistPhotoBinding>,
        photo: Box<ArtistPhoto>,
        image_url: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhotoQuery {
    artist_provider: String,
    artist_id: String,
    #[serde(default = "default_photo_size")]
    size: u16,
}

fn default_photo_size() -> u16 {
    400
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhotoPage {
    #[serde(default)]
    after: i64,
    #[serde(default = "default_photo_limit")]
    limit: u16,
}

fn default_photo_limit() -> u16 {
    50
}

impl PhotoQuery {
    fn artist(&self) -> Result<EntityRef> {
        let artist = EntityRef {
            provider: self.artist_provider.clone(),
            id: self.artist_id.clone(),
        };
        validate_artist(&artist)?;
        Ok(artist)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageQuery {
    artist_provider: String,
    artist_id: String,
    provider: String,
    version: i64,
    source_sha256: String,
    jpeg_sha256: String,
    size: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewQuery {
    artist_provider: String,
    artist_id: String,
    provider: String,
    file: String,
    confirmation: String,
}

#[derive(Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct RemoveArtistPhoto {
    pub id: i64,
    pub expected_version: i64,
}

fn options(size: u16) -> Result<ImageOptions> {
    let size = match size {
        200 => ArtworkSize::Small,
        400 => ArtworkSize::Recommended,
        800 => ArtworkSize::Large,
        _ => {
            return Err(Error::InvalidInput(
                "artist photo size must be 200, 400 or 800".into(),
            ));
        }
    };
    Ok(ImageOptions {
        size: Some(size),
        background: None,
    })
}

fn encoded_query(pairs: &[(&str, String)]) -> Result<String> {
    let mut url = reqwest::Url::parse("https://localhost/")
        .map_err(|_| Error::Internal("photo query base is invalid"))?;
    url.query_pairs_mut()
        .extend_pairs(pairs.iter().map(|(k, v)| (*k, v.as_str())));
    Ok(url
        .query()
        .ok_or(Error::Internal("photo query is missing"))?
        .to_owned())
}

async fn list(
    State(api): State<Api>,
    Query(page): Query<PhotoPage>,
) -> ApiResult<Json<Vec<ArtistPhotoBinding>>> {
    Ok(Json(
        api.database(move |store| store.artist_photo_bindings(page.after, page.limit))
            .await?,
    ))
}

async fn check(
    State(api): State<Api>,
    Json(selection): Json<CheckArtistPhoto>,
) -> ApiResult<Json<ArtistPhotoCheck>> {
    let _reload = api.reload_permit()?;
    api.require_configuration_write().await?;
    let artist = selection.artist.clone();
    let provider = selection.provider.clone();
    let expected_version = api
        .database(move |store| store.artist_photo_version(&artist, &provider))
        .await?;
    let Some((photo, _)) = api
        .0
        .services
        .images
        .retrieve_artist(selection.clone(), options(400)?)
        .await?
    else {
        return Err(Error::Conflict("the selected Commons file is missing".into()).into());
    };
    let confirmation = photo.confirmation()?;
    let preview_url = format!(
        "/api/artist-photos/preview?{}",
        encoded_query(&[
            ("artist_provider", selection.artist.provider),
            ("artist_id", selection.artist.id),
            ("provider", selection.provider),
            ("file", selection.file),
            ("confirmation", confirmation.clone()),
        ])?
    );
    Ok(Json(ArtistPhotoCheck {
        photo,
        confirmation,
        expected_version,
        preview_url,
    }))
}

async fn save(
    State(api): State<Api>,
    Json(command): Json<SaveArtistPhoto>,
) -> ApiResult<Json<ArtistPhotoBinding>> {
    let _reload = api.reload_permit()?;
    api.require_configuration_write().await?;
    if !command.acknowledge_permitted_use {
        return Err(Error::InvalidInput(
            "confirm the exact artist association and permitted photo use".into(),
        )
        .into());
    }
    let Some((photo, _)) = api
        .0
        .services
        .images
        .retrieve_artist(command.selection, options(400)?)
        .await?
    else {
        return Err(Error::Conflict("the selected Commons file is missing".into()).into());
    };
    if photo.confirmation()? != command.expected_confirmation {
        return Err(
            Error::Conflict("photo or rights changed; check the photo again".into()).into(),
        );
    }
    api.require_configuration_write().await?;
    Ok(Json(
        api.database(move |store| store.save_artist_photo(&photo, command.expected_version))
            .await?,
    ))
}

async fn remove(
    State(api): State<Api>,
    Json(command): Json<RemoveArtistPhoto>,
) -> ApiResult<StatusCode> {
    let _reload = api.reload_permit()?;
    api.require_configuration_write().await?;
    api.database(move |store| store.remove_artist_photo(command.id, command.expected_version))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn selected(api: &Api) -> Option<String> {
    api.0.service_configuration.artist_images.first().cloned()
}

async fn resume(State(api): State<Api>) -> ApiResult<StatusCode> {
    let _reload = api.reload_permit()?;
    api.require_configuration_write().await?;
    api.database(|store| store.resume_commons()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn current(
    api: &Api,
    artist: EntityRef,
    provider: String,
) -> ApiResult<Option<ArtistPhotoBinding>> {
    api.database(move |store| store.artist_photo_binding(&artist, &provider))
        .await
}

async fn retrieve_bound(
    api: &Api,
    binding: &ArtistPhotoBinding,
    size: u16,
) -> ApiResult<
    Option<(
        ArtistPhoto,
        crate::application::services::images::ProcessedImage,
    )>,
> {
    let result = api
        .0
        .services
        .images
        .retrieve_artist(
            CheckArtistPhoto {
                artist: binding.photo.artist.clone(),
                provider: binding.photo.provider.clone(),
                file: binding.photo.source.file.clone(),
            },
            options(size)?,
        )
        .await?;
    if let Some((photo, _)) = &result
        && (photo.source_sha256 != binding.photo.source_sha256
            || photo.original_sha256 != binding.photo.original_sha256)
    {
        return Err(Error::Conflict(
            "artist photo source or rights changed; check and save the binding again".into(),
        )
        .into());
    }
    let active = current(
        api,
        binding.photo.artist.clone(),
        binding.photo.provider.clone(),
    )
    .await?;
    if active.as_ref().map(|v| (v.id, v.version)) != Some((binding.id, binding.version)) {
        return Err(
            Error::Conflict("artist photo binding changed; reload the photo".into()).into(),
        );
    }
    Ok(result)
}

async fn read(
    State(api): State<Api>,
    Query(query): Query<PhotoQuery>,
) -> ApiResult<Json<ArtistPhotoState>> {
    let _reload = api.reload_permit()?;
    options(query.size)?;
    let artist = query.artist()?;
    let Some(provider) = selected(&api) else {
        return Ok(Json(ArtistPhotoState::Disabled));
    };
    let Some(binding) = current(&api, artist, provider).await? else {
        return Ok(Json(ArtistPhotoState::Unbound));
    };
    let Some((photo, _)) = retrieve_bound(&api, &binding, query.size).await? else {
        return Ok(Json(ArtistPhotoState::Missing));
    };
    let image_url = format!(
        "/api/artist-photos/image?{}",
        encoded_query(&[
            ("artist_provider", photo.artist.provider.clone()),
            ("artist_id", photo.artist.id.clone()),
            ("provider", photo.provider.clone()),
            ("version", binding.version.to_string()),
            ("source_sha256", photo.source_sha256.clone()),
            ("jpeg_sha256", photo.jpeg_sha256.clone()),
            ("size", query.size.to_string()),
        ])?
    );
    Ok(Json(ArtistPhotoState::Ready {
        binding: Box::new(binding),
        photo: Box::new(photo),
        image_url,
    }))
}

fn jpeg_response(bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "private, no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

async fn image(
    State(api): State<Api>,
    headers: HeaderMap,
    Query(query): Query<ImageQuery>,
) -> ApiResult<Response> {
    let _reload = api.reload_permit()?;
    api.identity(&headers).await?;
    if selected(&api).as_deref() != Some(query.provider.as_str()) {
        return Err(Error::Conflict("artist image source is disabled".into()).into());
    }
    let artist = EntityRef {
        provider: query.artist_provider,
        id: query.artist_id,
    };
    let binding = current(&api, artist, query.provider)
        .await?
        .ok_or(Error::Conflict("artist photo is no longer bound".into()))?;
    if binding.version != query.version || binding.photo.source_sha256 != query.source_sha256 {
        return Err(Error::Conflict("artist photo URL is stale".into()).into());
    }
    let (photo, image) = retrieve_bound(&api, &binding, query.size)
        .await?
        .ok_or(Error::Conflict("artist photo is missing".into()))?;
    if photo.jpeg_sha256 != query.jpeg_sha256 {
        return Err(Error::Conflict("artist photo content differs from its URL".into()).into());
    }
    api.identity(&headers).await?;
    Ok(jpeg_response(image.jpeg))
}

async fn preview(
    State(api): State<Api>,
    headers: HeaderMap,
    Query(query): Query<PreviewQuery>,
) -> ApiResult<Response> {
    let _reload = api.reload_permit()?;
    api.identity(&headers).await?;
    api.require_configuration_write().await?;
    let (photo, image) = api
        .0
        .services
        .images
        .retrieve_artist(
            CheckArtistPhoto {
                artist: EntityRef {
                    provider: query.artist_provider,
                    id: query.artist_id,
                },
                provider: query.provider,
                file: query.file,
            },
            options(400)?,
        )
        .await?
        .ok_or(Error::Conflict("artist photo is missing".into()))?;
    if photo.confirmation()? != query.confirmation {
        return Err(
            Error::Conflict("artist photo preview changed; check the photo again".into()).into(),
        );
    }

    api.identity(&headers).await?;
    Ok(jpeg_response(image.jpeg))
}
