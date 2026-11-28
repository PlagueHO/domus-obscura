use super::*;
use crate::infrastructure::server::configuration::{BrowseHostFolder, HostFolderPage};

pub(super) async fn host_folder_roots(State(api): State<Api>) -> ApiResult<Json<Vec<String>>> {
    api.require_configuration_write().await?;
    let source = api.configuration_service()?;
    Ok(Json(
        tokio::task::spawn_blocking(move || source.host_folder_roots())
            .await
            .map_err(|_| Error::Internal("host drive listing task failed"))??,
    ))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CloseHostFolder {
    cursor: String,
}

pub(super) async fn close_host_folder(
    State(api): State<Api>,
    Json(request): Json<CloseHostFolder>,
) -> ApiResult<StatusCode> {
    api.require_configuration_write().await?;
    let permit = api
        .0
        .folder_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy("Folder browsing is busy. Wait before trying again."))?;
    let source = api.configuration_service()?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        source.close_host_folder(&request.cursor)
    })
    .await
    .map_err(|_| Error::Internal("host folder closing task failed"))??;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn browse_host_folder(
    State(api): State<Api>,
    Json(request): Json<BrowseHostFolder>,
) -> ApiResult<Json<HostFolderPage>> {
    api.require_configuration_write().await?;
    let permit = api
        .0
        .folder_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy("Folder browsing is busy. Wait before trying again."))?;
    let source = api.configuration_service()?;
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        source.browse_host_folder(request)
    })
    .await
    .map_err(|_| Error::Internal("host folder browsing task failed"))?;
    match result {
        Ok(page) => Ok(Json(page)),
        Err(Error::Io(error)) => {
            eprintln!("Host folder browsing failed: {:?}", error.kind());
            Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "The application host could not read this folder. Check the path and the application account's access. For a network share, also check that the host can reach it.",
            ))
        }
        Err(error) => Err(error.into()),
    }
}
