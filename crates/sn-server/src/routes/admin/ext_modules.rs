use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_services::ServiceError;
use sn_services::ext_modules::package::PACKAGE_MAX_BYTES;
use sn_services::ext_modules::packages::{
    InstallInfo, LogsView, PackageInstall, PackageInstalled, PackagePreviewResult, Reissued,
    RemoveRequest, RuntimeView,
};
use sn_services::ext_modules::rewards::{RewardRequest, RewardResult, RewardState};
use sn_services::ext_modules::{
    Card, ConfigSaved, ConfigUpdate, ConfigView, InstallRequest, InstallResult, Journal, PageToken,
    PingResult, PreviewResult, SelfView, Summary,
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ext-modules", get(list).post(install))
        .route("/ext-modules/preview", post(preview))
        .route("/ext-modules/me", get(me))
        .route("/ext-modules/me/rewards", post(reward))
        .route("/ext-modules/me/rewards/{reference}", get(reward_state))
        .route("/ext-modules/install-info", get(install_info))
        .route(
            "/ext-modules/packages",
            post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/ext-modules/packages/{upload}",
            post(append)
                .delete(discard)
                .layer(DefaultBodyLimit::disable()),
        )
        .route("/ext-modules/packages/{upload}/check", post(check_package))
        .route(
            "/ext-modules/packages/{upload}/install",
            post(install_package),
        )
        .route("/ext-modules/{id}", get(card).patch(toggle).delete(remove))
        .route("/ext-modules/{id}/config", get(config).put(update_config))
        .route("/ext-modules/{id}/credentials", post(reissue))
        .route("/ext-modules/{id}/deliveries", get(journal))
        .route("/ext-modules/{id}/deliveries/{delivery}/retry", post(retry))
        .route(
            "/ext-modules/{id}/deliveries/retry-failed",
            post(retry_failed),
        )
        .route("/ext-modules/{id}/ping", post(ping))
        .route("/ext-modules/{id}/page-token", post(page_token))
        .route("/ext-modules/{id}/runtime", get(runtime))
        .route("/ext-modules/{id}/restart", post(restart))
        .route("/ext-modules/{id}/logs", get(logs).post(refresh_logs))
}

fn mapped(err: ServiceError) -> ApiError {
    let code = err.code();
    let mut api = ApiError::from(err);
    if code == sn_services::auth::TOTP_REQUIRED || code == sn_services::auth::TOTP_INVALID {
        api.status = StatusCode::UNPROCESSABLE_ENTITY;
    }
    api
}

async fn list(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<Vec<Summary>>> {
    admin.require(Permission::Modules)?;
    Ok(Json(state.services.ext_modules().list(&admin.0).await?))
}

#[derive(Deserialize)]
struct PreviewRequest {
    manifest: String,
    #[serde(default)]
    update: Option<String>,
}

async fn preview(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<PreviewRequest>,
) -> ApiResult<Json<PreviewResult>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .preview(&admin.0, &body.manifest, body.update.as_deref())
            .await?,
    ))
}

async fn install(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<InstallRequest>,
) -> ApiResult<Json<InstallResult>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .install(&admin.0, &body)
            .await?,
    ))
}

async fn install_info(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<InstallInfo>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().install_info(&admin.0).await?,
    ))
}

#[derive(Serialize)]
struct Uploaded {
    id: Uuid,
    size: u64,
}

fn too_big() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "ext_modules.too_large",
        "Архив больше 500 МБ",
    )
}

async fn save_part(path: &std::path::Path, body: Body, append: bool) -> ApiResult<u64> {
    let mut options = tokio::fs::OpenOptions::new();
    if append {
        options.append(true);
    } else {
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
    }
    let mut file = options.open(path).await.map_err(|_| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "ext_modules.upload_gone",
            "Загрузка не найдена: выберите архив ещё раз",
        )
    })?;
    let mut size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut stream = body.into_data_stream();
    let failed = loop {
        match stream.next().await {
            None => break None,
            Some(Ok(chunk)) => {
                size += chunk.len() as u64;
                if size > PACKAGE_MAX_BYTES {
                    break Some(too_big());
                }
                if let Err(err) = file.write_all(&chunk).await {
                    break Some(ApiError::from(ServiceError::Internal(format!(
                        "на сервере не хватает места для архива: {err}"
                    ))));
                }
            }
            Some(Err(_)) => {
                break Some(ApiError::bad_request(
                    "ext_modules.interrupted",
                    "Загрузка оборвалась. Попробуйте ещё раз",
                ));
            }
        }
    };
    let flushed = file.flush().await;
    drop(file);
    if let Some(err) = failed {
        let _ = tokio::fs::remove_file(path).await;
        return Err(err);
    }
    if flushed.is_err() {
        let _ = tokio::fs::remove_file(path).await;
        return Err(ApiError::from(ServiceError::Internal(
            "архив не сохранился".into(),
        )));
    }
    Ok(size)
}

fn declared_too_big(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|len| len > PACKAGE_MAX_BYTES)
}

async fn upload(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Body,
) -> ApiResult<Json<Uploaded>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    if declared_too_big(&headers) {
        return Err(too_big());
    }
    let modules = state.services.ext_modules();
    let id = Uuid::new_v4();
    let path = modules.upload_path(&admin.0, &id)?;
    modules.cleanup_uploads();
    let size = save_part(&path, body, false).await?;
    Ok(Json(Uploaded { id, size }))
}

async fn append(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(upload): Path<Uuid>,
    headers: axum::http::HeaderMap,
    body: Body,
) -> ApiResult<Json<Uploaded>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    if declared_too_big(&headers) {
        return Err(too_big());
    }
    let path = state
        .services
        .ext_modules()
        .upload_path(&admin.0, &upload)?;
    if !path.is_file() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "ext_modules.upload_gone",
            "Загрузка не найдена: выберите архив ещё раз",
        ));
    }
    let size = save_part(&path, body, true).await?;
    Ok(Json(Uploaded { id: upload, size }))
}

async fn discard(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(upload): Path<Uuid>,
) -> ApiResult<StatusCode> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    state
        .services
        .ext_modules()
        .discard_upload(&admin.0, &upload)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Default, Deserialize)]
struct CheckRequest {
    #[serde(default)]
    update: Option<String>,
}

async fn check_package(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(upload): Path<Uuid>,
    body: Option<ApiJson<CheckRequest>>,
) -> ApiResult<Json<PackagePreviewResult>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    let body = body.map(|ApiJson(b)| b).unwrap_or_default();
    Ok(Json(
        state
            .services
            .ext_modules()
            .check_package(&admin.0, &upload, body.update.as_deref())
            .await?,
    ))
}

async fn install_package(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(upload): Path<Uuid>,
    ApiJson(body): ApiJson<PackageInstall>,
) -> ApiResult<Json<PackageInstalled>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .install_package(&admin.0, &upload, &body)
            .await
            .map_err(mapped)?,
    ))
}

async fn me(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<SelfView>> {
    Ok(Json(state.services.ext_modules().me(&admin.0).await?))
}

async fn reward(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<RewardRequest>,
) -> ApiResult<Json<RewardResult>> {
    Ok(Json(
        state.services.ext_modules().reward(&admin.0, &body).await?,
    ))
}

async fn reward_state(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(reference): Path<String>,
) -> ApiResult<Json<RewardState>> {
    Ok(Json(
        state
            .services
            .ext_modules()
            .reward_state(&admin.0, &reference)
            .await?,
    ))
}

async fn card(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Card>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state.services.ext_modules().card_for(&admin.0, &id).await?,
    ))
}

#[derive(Deserialize)]
struct ToggleRequest {
    enabled: bool,
}

async fn toggle(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToggleRequest>,
) -> ApiResult<Json<Card>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .set_enabled(&admin.0, &id, body.enabled)
            .await?,
    ))
}

async fn remove(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<ApiJson<RemoveRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    let body = body.map(|ApiJson(b)| b).unwrap_or_default();
    state
        .services
        .ext_modules()
        .remove(&admin.0, &id, &body)
        .await
        .map_err(mapped)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn config(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ConfigView>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state.services.ext_modules().config(&admin.0, &id).await?,
    ))
}

async fn update_config(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ConfigUpdate>,
) -> ApiResult<Json<ConfigSaved>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .update_config(&admin.0, &id, &body)
            .await?,
    ))
}

async fn reissue(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Reissued>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().reissue(&admin.0, &id).await?,
    ))
}

async fn journal(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Journal>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().journal(&admin.0, &id).await?,
    ))
}

async fn retry(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path((id, delivery)): Path<(String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    let queued = state
        .services
        .ext_modules()
        .retry(&admin.0, &id, delivery)
        .await?;
    Ok(Json(serde_json::json!({ "queued": queued })))
}

async fn retry_failed(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    let queued = state
        .services
        .ext_modules()
        .retry_failed(&admin.0, &id)
        .await?;
    Ok(Json(serde_json::json!({ "queued": queued })))
}

async fn ping(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PingResult>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().ping(&admin.0, &id).await?,
    ))
}

async fn page_token(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PageToken>> {
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .page_token(&admin.0, &id)
            .await?,
    ))
}

async fn runtime(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RuntimeView>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().runtime(&admin.0, &id).await?,
    ))
}

async fn restart(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RuntimeView>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().restart(&admin.0, &id).await?,
    ))
}

async fn logs(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<LogsView>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().logs(&admin.0, &id).await?,
    ))
}

async fn refresh_logs(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RuntimeView>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().refresh(&admin.0, &id).await?,
    ))
}
