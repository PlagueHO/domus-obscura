use super::*;
use crate::infrastructure::server::configuration::{SaveSoulseekAccount, SoulseekAccountStatus};
use crate::infrastructure::store::SoulseekAccountChange;

pub(super) async fn soulseek_account_status(
    State(api): State<Api>,
) -> ApiResult<Json<SoulseekAccountStatus>> {
    let Some(source) = api.0.backend_configuration.clone() else {
        return Ok(Json(SoulseekAccountStatus {
            available: false,
            reason: Some(
                "Soulseek account setup requires a configured native application host.".into(),
            ),
            configured: false,
            active_revision: String::new(),
            pending_change_id: None,
        }));
    };
    let mut status = tokio::task::spawn_blocking(move || source.soulseek_account_status())
        .await
        .map_err(|_| Error::Internal("account status task failed"))??;
    let supervisor = super::primary_library::control_status(&api).await?;
    if !supervisor.available {
        status.available = false;
        status.reason = Some("Start the native application with dev-run to save and apply an account. Installed services and Docker use the operator procedure.".into());
    }
    Ok(Json(status))
}

pub(super) async fn save_soulseek_account(
    State(api): State<Api>,
    Json(request): Json<SaveSoulseekAccount>,
) -> ApiResult<Json<SoulseekAccountChange>> {
    api.require_configuration_write().await?;
    let source = super::primary_library::require_supervisor(&api).await?;
    Ok(Json(
        tokio::task::spawn_blocking(move || source.save_soulseek_account(request))
            .await
            .map_err(|_| Error::Internal("account save task failed"))??,
    ))
}

pub(super) async fn soulseek_account_change(
    State(api): State<Api>,
    Path(id): Path<i64>,
) -> ApiResult<Json<SoulseekAccountChange>> {
    Ok(Json(
        api.database(move |store| store.soulseek_account_change(id))
            .await?,
    ))
}

pub(super) async fn activate_soulseek_account(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(_request): Json<ScanLocation>,
) -> ApiResult<StatusCode> {
    api.require_configuration_write().await?;
    let source = super::primary_library::require_supervisor(&api).await?;
    let change = api
        .database(move |store| store.soulseek_account_change(id))
        .await?;
    if change.state != "approved" {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "This account change is not waiting for activation. Check its saved result.",
        ));
    }
    super::super::server::developer::control(
        source.primary_library_configuration_path(),
        &format!("account:{id}"),
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn cancel_soulseek_account(
    State(api): State<Api>,
    Path(id): Path<i64>,
    Json(_request): Json<ScanLocation>,
) -> ApiResult<Json<SoulseekAccountChange>> {
    api.require_configuration_write().await?;
    Ok(Json(
        api.database(move |store| store.cancel_soulseek_account(id))
            .await?,
    ))
}
