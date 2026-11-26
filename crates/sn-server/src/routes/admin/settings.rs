use super::{decode, json};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;
use sn_domain::settings::SiteImage;
use sn_domain::settings::{
    Appearance, AutoRenewSettings, ClientAccess, ConnectSettings, MultiSubscriptions, TrialSettings,
};
use sn_modules::ModuleView;
use sn_services::bot_buttons::ButtonsView;
use sn_services::branding::{ImageView, SiteUpdate};
use sn_services::egress::{CheckView, ProxyCheckRequest, ProxyUpdate, ProxyView};
use sn_services::maintenance::MaintenanceUpdate;
use sn_services::settings::{
    BotUpdate, CoreUpdate, ModuleConfigUpdate, ModuleConfigView, PaymentsUpdate, ProviderDetails,
    ProviderUpdate, RemnaCheck, RemnaCheckRequest, RemnaUpdate, SubscriptionSettingsUpdate,
    WebhookSecret,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/{section}", get(read).put(write))
        .route("/settings/bot/restart", post(restart_bot))
        .route("/settings/bot/buttons", get(bot_buttons))
        .route("/settings/bot/buttons/menu", post(put_bot_menu))
        .route(
            "/settings/logo",
            post(upload_logo)
                .delete(delete_logo)
                .layer(DefaultBodyLimit::max(IMAGE_UPLOAD_LIMIT)),
        )
        .route(
            "/settings/favicon",
            post(upload_favicon)
                .delete(delete_favicon)
                .layer(DefaultBodyLimit::max(IMAGE_UPLOAD_LIMIT)),
        )
        .route("/settings/proxy/check", post(check_proxy))
        .route("/settings/proxy/reset", post(reset_proxy))
        .route("/settings/remnawave/check", post(check_remnawave))
        .route("/settings/remnawave/webhook-secret", post(webhook_secret))
        .route("/settings/remnawave/page-configs", get(page_configs))
        .route("/settings/remnawave/webhook-env", get(webhook_env))
        .route("/payments/providers", get(providers))
        .route(
            "/payments/providers/{id}",
            get(provider).put(update_provider),
        )
        .route("/payments/providers/{id}/check", post(check_provider))
        .route("/payments/providers/{id}/catalog", get(provider_catalog))
        .route("/modules", get(modules))
        .route("/modules/{id}", patch(toggle_module))
        .route(
            "/modules/{id}/config",
            get(module_config).put(update_module_config),
        )
}

async fn read(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(section): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let ops = state.services.settings_ops();
    let value = match section.as_str() {
        "core" => json(&ops.core())?,
        "appearance" => json(&ops.appearance())?,
        "connect" => json(&ops.connect())?,
        "payments" => json(&ops.payments().await?)?,
        "trial" => json(&ops.trial())?,
        "autorenew" => json(&ops.autorenew())?,
        "multi_subscriptions" => json(&ops.multi_subscriptions())?,
        "client_access" => json(&ops.client_access())?,
        "languages" => json(&ops.languages())?,
        "remnawave" => json(&ops.remnawave().await?)?,
        "subscription" => json(&ops.subscription_settings().await?)?,
        "bot" => json(&ops.bot().await?)?,
        "site" => json(&state.services.branding().site())?,
        "proxy" => json(&state.services.egress_ops().view().await?)?,
        "maintenance" => json(&state.services.maintenance().view(admin.0.admin_id).await?)?,
        _ => return Err(ApiError::not_found()),
    };
    Ok(Json(value))
}

async fn write(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(section): Path<String>,
    ApiJson(body): ApiJson<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let ops = state.services.settings_ops();
    let by = admin.0.admin_id;
    let value = match section.as_str() {
        "core" => json(&ops.update_core(&decode::<CoreUpdate>(body)?, by).await?)?,
        "appearance" => json(
            &ops.update_appearance(&decode::<Appearance>(body)?, by)
                .await?,
        )?,
        "connect" => json(
            &ops.update_connect(&decode::<ConnectSettings>(body)?, by)
                .await?,
        )?,
        "payments" => json(
            &ops.update_payments(&decode::<PaymentsUpdate>(body)?, by)
                .await?,
        )?,
        "trial" => json(
            &ops.update_trial(&decode::<TrialSettings>(body)?, by)
                .await?,
        )?,
        "autorenew" => json(
            &ops.update_autorenew(&decode::<AutoRenewSettings>(body)?, by)
                .await?,
        )?,
        "multi_subscriptions" => json(
            &ops.update_multi_subscriptions(&decode::<MultiSubscriptions>(body)?, by)
                .await?,
        )?,
        "client_access" => json(
            &ops.update_client_access(&decode::<ClientAccess>(body)?, by)
                .await?,
        )?,
        "languages" => json(
            &ops.update_languages(&decode::<sn_domain::client::Languages>(body)?, by)
                .await?,
        )?,
        "remnawave" => json(
            &ops.update_remnawave(&decode::<RemnaUpdate>(body)?, by)
                .await?,
        )?,
        "subscription" => json(
            &ops.update_subscription_settings(&decode::<SubscriptionSettingsUpdate>(body)?, by)
                .await?,
        )?,
        "bot" => json(&ops.update_bot(&decode::<BotUpdate>(body)?, by).await?)?,
        "proxy" => json(
            &state
                .services
                .egress_ops()
                .update(&decode::<ProxyUpdate>(body)?, by)
                .await?,
        )?,
        "site" => json(
            &state
                .services
                .branding()
                .update_site(&decode::<SiteUpdate>(body)?, by)
                .await?,
        )?,
        "maintenance" => json(
            &state
                .services
                .maintenance()
                .update(&decode::<MaintenanceUpdate>(body)?, by)
                .await?,
        )?,
        _ => return Err(ApiError::not_found()),
    };
    Ok(Json(value))
}

async fn check_proxy(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ProxyCheckRequest>,
) -> ApiResult<Json<CheckView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.egress_ops().check(&body).await?))
}

async fn reset_proxy(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<ProxyView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state.services.egress_ops().reset(admin.0.admin_id).await?,
    ))
}

#[derive(Debug, Default, Deserialize)]
struct RestartBot {
    #[serde(default)]
    take_over: bool,
}

async fn restart_bot(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    body: Option<ApiJson<RestartBot>>,
) -> ApiResult<Json<sn_services::settings::BotSettingsView>> {
    admin.require(Permission::Settings)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state
            .services
            .settings_ops()
            .restart_bot(body.take_over, admin.0.admin_id)
            .await?,
    ))
}

async fn bot_buttons(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<ButtonsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.bot_buttons().view().await?))
}

async fn put_bot_menu(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<ButtonsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .bot_buttons()
            .put_menu(admin.0.admin_id)
            .await?,
    ))
}

pub(super) const IMAGE_UPLOAD_LIMIT: usize = 4 * 1024 * 1024;

pub(super) fn declared_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

fn accepted(image: SiteImage, headers: &HeaderMap) -> Result<(), ApiError> {
    let declared = declared_type(headers);
    let (fits, code, message) = match image {
        SiteImage::Logo => (
            matches!(declared.as_str(), "image/png" | "image/jpeg" | "image/webp"),
            "logo.format",
            "Подходят только PNG, JPEG и WebP",
        ),
        SiteImage::Favicon => (
            matches!(
                declared.as_str(),
                "image/png" | "image/x-icon" | "image/vnd.microsoft.icon"
            ),
            "favicon.format",
            "Подходят только PNG и ICO",
        ),
    };
    if fits {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code,
            message,
        ))
    }
}

async fn upload_image(
    image: SiteImage,
    admin: CurrentAdmin,
    state: AppState,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<ImageView>> {
    admin.require(Permission::Settings)?;
    accepted(image, &headers)?;
    Ok(Json(
        state
            .services
            .branding()
            .upload(image, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn remove_image(
    image: SiteImage,
    admin: CurrentAdmin,
    state: AppState,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let removed = state
        .services
        .branding()
        .remove(image, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

async fn upload_logo(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<ImageView>> {
    upload_image(SiteImage::Logo, admin, state, headers, body).await
}

async fn delete_logo(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    remove_image(SiteImage::Logo, admin, state).await
}

async fn upload_favicon(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<ImageView>> {
    upload_image(SiteImage::Favicon, admin, state, headers, body).await
}

async fn delete_favicon(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    remove_image(SiteImage::Favicon, admin, state).await
}

async fn webhook_env(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        serde_json::json!({ "env": state.services.settings_ops().webhook_env_full() }),
    ))
}

async fn page_configs(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<sn_services::settings::PageConfig>>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.settings_ops().page_configs().await?))
}

async fn check_remnawave(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    body: Option<ApiJson<RemnaCheckRequest>>,
) -> ApiResult<Json<RemnaCheck>> {
    admin.require(Permission::Settings)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state.services.settings_ops().check_remnawave(&body).await?,
    ))
}

async fn webhook_secret(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<WebhookSecret>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .generate_remna_webhook_secret(admin.0.admin_id)
            .await?,
    ))
}

async fn providers(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let payments = state.services.settings_ops().payments().await?;
    Ok(Json(serde_json::json!({
        "currency": payments.currency,
        "order": payments.order,
        "providers": payments.providers,
        "balances": payments.balances,
    })))
}

#[derive(serde::Deserialize)]
struct InCurrency {
    #[serde(default)]
    currency: Option<sn_domain::money::Currency>,
}

async fn provider(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<InCurrency>,
) -> ApiResult<Json<ProviderDetails>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .provider_in(&id, query.currency)?,
    ))
}

async fn update_provider(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ProviderUpdate>,
) -> ApiResult<Json<ProviderDetails>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .update_provider(&id, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn provider_catalog(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state.services.settings_ops().provider_catalog(&id).await?,
    ))
}

async fn check_provider(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<sn_payments::CheckRecord>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .check_provider(&id, admin.0.admin_id)
            .await?,
    ))
}

async fn modules(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Modules)?;
    Ok(Json(serde_json::json!({
        "modules": state.services.modules().views(),
        "core": sn_domain::module::CoreFeature::ALL.map(|f| f.title_ru()),
    })))
}

fn module_id(id: &str) -> ApiResult<ModuleId> {
    ModuleId::parse(id).map_err(|_| ApiError::not_found())
}

#[derive(Deserialize)]
struct ToggleRequest {
    enabled: bool,
}

async fn toggle_module(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToggleRequest>,
) -> ApiResult<Json<ModuleView>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .toggle_module(module_id(&id)?, body.enabled, admin.0.admin_id)
            .await?,
    ))
}

async fn module_config(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ModuleConfigView>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state.services.settings_ops().module_config(module_id(&id)?),
    ))
}

async fn update_module_config(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ModuleConfigUpdate>,
) -> ApiResult<Json<ModuleConfigView>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .update_module_config(module_id(&id)?, &body, admin.0.admin_id)
            .await?,
    ))
}
