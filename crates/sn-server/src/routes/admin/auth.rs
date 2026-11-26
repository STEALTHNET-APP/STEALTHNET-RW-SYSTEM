use crate::cookies::{self, SameSite};
use crate::error::ApiResult;
use crate::extract::{ADMIN_COOKIE, ApiJson, CurrentAdmin, Meta};
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::maintenance::TelegramUpdate;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/password", post(change_password))
        .route("/auth/security", get(security))
        .route("/auth/2fa/setup", post(totp_setup))
        .route("/auth/2fa/enable", post(totp_enable))
        .route("/auth/2fa/disable", post(totp_disable))
        .route("/auth/telegram", put(set_telegram))
        .route("/me", get(me))
}

async fn set_telegram(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TelegramUpdate>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require_session()?;
    let telegram_id = state
        .services
        .maintenance()
        .set_telegram(admin.0.admin_id, &body)
        .await?;
    Ok(Json(serde_json::json!({ "telegram_id": telegram_id })))
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
    #[serde(default)]
    totp: Option<String>,
}

async fn login(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<LoginRequest>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .admin_login(&body.email, &body.password, body.totp.as_deref(), &meta)
        .await?;
    let cookie = cookies::session(
        &state,
        ADMIN_COOKIE,
        &login.token,
        Some(login.expires_at),
        SameSite::Strict,
    );
    Ok(cookies::with_cookie(cookie, Json(login)))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(token) = crate::extract::token(&headers, ADMIN_COOKIE) {
        state.services.auth().admin_logout(&token).await?;
    }
    let cookie = cookies::session(&state, ADMIN_COOKIE, "", None, SameSite::Strict);
    Ok(cookies::with_cookie(
        cookie,
        Json(serde_json::json!({ "ok": true })),
    ))
}

#[derive(Deserialize)]
struct PasswordRequest {
    current: String,
    new: String,
}

async fn change_password(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<PasswordRequest>,
) -> ApiResult<Response> {
    admin.require_session()?;
    let login = state
        .services
        .auth()
        .change_admin_password(admin.0.admin_id, &body.current, &body.new, &meta)
        .await?;
    let cookie = cookies::session(
        &state,
        ADMIN_COOKIE,
        &login.token,
        Some(login.expires_at),
        SameSite::Strict,
    );
    Ok(cookies::with_cookie(cookie, Json(login)))
}

async fn security(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::auth::AdminSecurity>> {
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .auth()
            .admin_security(admin.0.admin_id)
            .await?,
    ))
}

async fn totp_setup(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::auth::TotpSetup>> {
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .auth()
            .admin_totp_setup(admin.0.admin_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct TotpEnableRequest {
    secret: String,
    code: String,
    password: String,
}

async fn totp_enable(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TotpEnableRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require_session()?;
    state
        .services
        .auth()
        .admin_totp_enable_in(
            admin.0.admin_id,
            &body.secret,
            &body.code,
            &body.password,
            Some(admin.0.session_id),
        )
        .await?;
    Ok(Json(serde_json::json!({ "totp_enabled": true })))
}

#[derive(Deserialize)]
struct TotpDisableRequest {
    password: String,
    code: String,
}

async fn totp_disable(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TotpDisableRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require_session()?;
    state
        .services
        .auth()
        .admin_totp_disable(admin.0.admin_id, &body.password, &body.code)
        .await?;
    Ok(Json(serde_json::json!({ "totp_enabled": false })))
}

async fn me(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let permissions: Vec<Permission> = Permission::ALL
        .into_iter()
        .filter(|p| admin.can(*p))
        .collect();
    let settings = state.services.settings();
    let security = state
        .services
        .auth()
        .admin_security(admin.0.admin_id)
        .await
        .ok();
    let must_change_password = security.as_ref().is_some_and(|s| s.must_change_password);
    let email = security.map(|s| s.email);
    let pages = state
        .services
        .ext_modules()
        .menu(&admin.0)
        .await
        .unwrap_or_default();
    Ok(Json(serde_json::json!({
        "admin": admin.0,
        "email": email,
        "must_change_password": must_change_password,
        "permissions": permissions,
        "menu": state
            .services
            .modules()
            .menu()
            .into_iter()
            .filter(|item| admin.can(item.need))
            .collect::<Vec<_>>(),
        "pages": pages,
        "modules": state
            .services
            .modules()
            .enabled()
            .iter()
            .map(|m| m.id())
            .collect::<Vec<_>>(),
        "currency": settings.currency,
        "brand_name": settings.brand_name,
        "logo_url": settings.logo_url(),
        "appearance": settings.appearance,
        "remnawave": state.services.remna().is_ok(),
        "maintenance": settings.maintenance.enabled,
    })))
}
