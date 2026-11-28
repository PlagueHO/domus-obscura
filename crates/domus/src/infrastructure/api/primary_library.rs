use super::*;
use crate::domain::library::PrimaryLibraryChange;
use crate::infrastructure::server::configuration::{
    ConfigurationService, InspectPrimaryLibrary, PrimaryLibraryControl, PrimaryLibraryPreview,
    SelectPrimaryLibrary,
};

pub(super) async fn control_status(api: &Api) -> ApiResult<PrimaryLibraryControl> {
    let pending = api.database(|store| store.pending_library_change()).await?;
    let pending_change_id = pending.map(|(id, _)| id);
    let pending_backup = pending.is_some_and(|(_, backup)| backup);
    let unavailable = |reason: &str| PrimaryLibraryControl {
        available: false,
        reason: Some(reason.into()),
        pending_change_id,
        pending_backup,
    };
    let Some(source) = api.0.backend_configuration.as_ref() else {
        return Ok(unavailable(
            "Library folder changes require the native application supervisor.",
        ));
    };
    if std::env::var_os("DOMUS_NATIVE_SUPERVISED").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Ok(unavailable(
            "Start the native application with dev-run to change library folders through Settings.",
        ));
    }
    let result = super::super::server::developer::control(
        source.primary_library_configuration_path(),
        "status",
    )
    .await;
    let status = match result {
        Ok(status) => status,
        Err(Error::Io(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(unavailable(
                "The native application supervisor is not connected.",
            ));
        }
        Err(error) => return Err(error.into()),
    };
    let owned = status
        .get("processes")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|processes| {
            processes.iter().any(|process| {
                process["role"] == "api"
                    && process["pid"].as_u64() == Some(u64::from(std::process::id()))
            })
        });
    if status["state"] != "ready"
        || !owned
        || status["revision"].as_str() != Some(source.active_revision())
    {
        return Ok(unavailable(
            "The application is restarting or its saved configuration changed. Check its status before changing folders.",
        ));
    }
    Ok(PrimaryLibraryControl {
        available: true,
        reason: None,
        pending_change_id,
        pending_backup,
    })
}

pub(super) async fn require_supervisor(api: &Api) -> ApiResult<Arc<ConfigurationService>> {
    let status = control_status(api).await?;
    if !status.available {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            status
                .reason
                .unwrap_or_else(|| "The application supervisor is unavailable.".into()),
        ));
    }
    api.configuration_service()
}

pub(super) async fn primary_library_control(
    State(api): State<Api>,
) -> ApiResult<Json<PrimaryLibraryControl>> {
    Ok(Json(control_status(&api).await?))
}

pub(super) async fn inspect_primary_library(
    State(api): State<Api>,
    Json(request): Json<InspectPrimaryLibrary>,
) -> ApiResult<Json<PrimaryLibraryPreview>> {
    api.require_configuration_write().await?;
    let source = require_supervisor(&api).await?;
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            source.inspect_primary_library(&request.path, request.backup.unwrap_or(false))
        })
        .await
        .map_err(|_| Error::Internal("primary library inspection task failed"))??,
    ))
}

pub(super) async fn select_primary_library(
    State(api): State<Api>,
    Json(request): Json<SelectPrimaryLibrary>,
) -> ApiResult<Json<PrimaryLibraryChange>> {
    api.require_configuration_write().await?;
    let source = require_supervisor(&api).await?;
    Ok(Json(
        tokio::task::spawn_blocking(move || source.approve_primary_library(request))
            .await
            .map_err(|_| Error::Internal("primary library approval task failed"))??,
    ))
}

pub(super) async fn primary_library_change(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<Json<PrimaryLibraryChange>> {
    Ok(Json(
        api.database(move |store| store.primary_library_change(id))
            .await?,
    ))
}

pub(super) async fn activate_primary_library(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(_request): Json<ScanLocation>,
) -> ApiResult<StatusCode> {
    api.require_configuration_write().await?;
    let source = require_supervisor(&api).await?;
    let change = api
        .database(move |store| store.primary_library_change(id))
        .await?;
    if change.state != "approved" {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "This library folder change is no longer waiting for activation. Check its saved result.",
        ));
    }
    super::super::server::developer::control(
        source.primary_library_configuration_path(),
        &format!("primary:{id}"),
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn cancel_primary_library_change(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(_request): Json<ScanLocation>,
) -> ApiResult<Json<PrimaryLibraryChange>> {
    api.require_configuration_write().await?;
    Ok(Json(
        api.database(move |store| store.cancel_primary_library_change(id))
            .await?,
    ))
}
