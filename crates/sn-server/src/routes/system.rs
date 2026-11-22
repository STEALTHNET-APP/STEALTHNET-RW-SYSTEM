use crate::error::ApiError;
use crate::state::AppState;
use crate::web::escape_html;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use sn_domain::module::ModuleId;
use sn_domain::settings::SiteImage;
use sn_services::branding::brand_initial;
use sn_store::Db;

const IMAGE_CACHE: &str = "public, max-age=31536000, immutable";
const ICON_CACHE: &str = "public, max-age=300";
const TAB_ICON_CACHE: &str = "public, max-age=3600";
const IMAGE_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'";
const ICON_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";
const LANDING_CACHE: &str = "no-cache";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .route("/api/public/site", get(site))
        .route("/api/public/catalog", get(catalog))
        .route("/api/public/logo/{file}", get(logo))
        .route("/api/public/favicon/{file}", get(favicon))
        .route("/api/public/landing", get(landing))
        .route("/api/public/landing/{file}", get(landing_image))
        .route("/api/public/connect-apps", get(connect_apps))
        .route("/api/public/app-icon/{file}", get(app_icon))
        .route("/api/public/icon.svg", get(icon))
        .route("/favicon.ico", get(favicon_ico))
        .route("/robots.txt", get(robots))
}

async fn robots(State(state): State<AppState>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        crate::web::robots_txt(state.services.settings().site.indexing),
    )
        .into_response()
}

fn tagged(headers: &HeaderMap, value: &serde_json::Value) -> Response {
    let Ok(body) = serde_json::to_vec(value) else {
        return Json(value).into_response();
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&body, &mut hasher);
    let etag = format!(
        "\"{:x}-{:016x}\"",
        body.len(),
        std::hash::Hasher::finish(&hasher)
    );
    let Ok(tag) = HeaderValue::from_str(&etag) else {
        return ApiError::not_found().into_response();
    };
    let cache = HeaderValue::from_static(LANDING_CACHE);
    if etag_matches(headers, &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, tag), (header::CACHE_CONTROL, cache)],
        )
            .into_response();
    }
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (header::ETAG, tag),
            (header::CACHE_CONTROL, cache),
        ],
        body,
    )
        .into_response()
}

async fn catalog(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match state.services.checkout().public_catalog().await {
        Ok(offers) => tagged(
            &headers,
            &serde_json::json!({
                "currency": state.services.settings().currency,
                "tariffs": offers,
            }),
        ),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn site(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let settings = state.services.settings();
    let trial_plans = settings.trial.active_plans();
    let bot_username = match state.services.store().conn().await {
        Ok(mut conn) => conn
            .settings()
            .get(sn_domain::settings::keys::BOT_USERNAME)
            .await
            .ok()
            .flatten()
            .and_then(|value: serde_json::Value| value.as_str().map(str::to_string)),
        Err(_) => None,
    };
    let access = settings.client_access;
    let mail = state.services.mail();
    let email_auth = if !access.site_login {
        None
    } else if mail.code_login() {
        Some("code")
    } else {
        Some("password")
    };
    let email_reset = email_auth == Some("password") && mail.available();
    tagged(
        &headers,
        &serde_json::json!({
            "brand_name": settings.brand_name,
            "title": settings.site.title.as_deref().unwrap_or(&settings.brand_name),
            "logo_url": settings.logo_url(),
            "currency": settings.currency,
            "support_url": settings.support_link(),
            "support_tickets": settings.support_tickets(),
            "support_ai": if settings.support_tickets() {
                sn_modules::catalog::ai::channels(state.services.modules())
            } else {
                sn_modules::catalog::ai::Channels::default()
            },
            "support_files": if settings.support_tickets() {
                state.services.support().file_limits()
            } else {
                None
            },
            "bot_username": bot_username,
            "appearance": settings.appearance,
            "connect": settings.connect,
            "autorenew_days_before": settings.autorenew.charge_days_before,
            "trial": {
                "enabled": !trial_plans.is_empty(),
                "days": settings.trial.days,
                "plans": trial_plans.iter().map(|plan| serde_json::json!({
                    "id": plan.id,
                    "name": plan.name,
                    "days": plan.days,
                    "devices": plan.devices,
                    "squads": plan.squads,
                })).collect::<Vec<_>>(),
            },
            "documents": state.services.bot_content().documents,
            "email_auth": email_auth,
            "email_reset": email_reset,
            "oauth": state.services.oauth().availability(),
            "client_access": {
                "miniapp": access.miniapp,
                "site_login": access.site_login,
                "devices": access.self_devices,
                "reissue": access.self_reissue,
            },
            "languages": {
                "list": settings.languages.enabled(),
                "default": settings.languages.fallback(),
            },
        }),
    )
}

async fn image_file(state: &AppState, image: SiteImage, file: &str) -> Response {
    let Some((tag, extension)) = file.split_once('.') else {
        return ApiError::not_found().into_response();
    };
    match state.services.branding().file(image, tag).await {
        Ok(Some((kind, bytes))) if kind.extension() == extension => (
            [
                (header::CONTENT_TYPE, kind.content_type()),
                (header::CACHE_CONTROL, IMAGE_CACHE),
                (header::CONTENT_SECURITY_POLICY, IMAGE_POLICY),
            ],
            bytes,
        )
            .into_response(),
        Ok(_) => ApiError::not_found().into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn logo(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    image_file(&state, SiteImage::Logo, &file).await
}

async fn favicon(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    image_file(&state, SiteImage::Favicon, &file).await
}

fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').map(str::trim).any(|candidate| {
                candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
            })
        })
}

async fn landing(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.services.modules().is_enabled(ModuleId::Landing) {
        return sn_modules::gate::disabled_response(ModuleId::Landing);
    }
    let public = state.services.landing().public();
    let etag = format!("\"{}\"", public.version);
    let Ok(tag) = HeaderValue::from_str(&etag) else {
        return ApiError::not_found().into_response();
    };
    let cache = HeaderValue::from_static(LANDING_CACHE);
    if etag_matches(&headers, &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, tag), (header::CACHE_CONTROL, cache)],
        )
            .into_response();
    }
    (
        [(header::ETAG, tag), (header::CACHE_CONTROL, cache)],
        Json(public),
    )
        .into_response()
}

async fn landing_image(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    let Some((tag, extension)) = file.split_once('.') else {
        return ApiError::not_found().into_response();
    };
    match state.services.landing().image_file(tag).await {
        Ok(Some((kind, bytes))) if kind.extension() == extension => (
            [
                (header::CONTENT_TYPE, kind.content_type()),
                (header::CACHE_CONTROL, IMAGE_CACHE),
                (header::CONTENT_SECURITY_POLICY, IMAGE_POLICY),
            ],
            bytes,
        )
            .into_response(),
        Ok(_) => ApiError::not_found().into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn connect_apps(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let public = state.services.connect_apps().public();
    let etag = format!("\"{}\"", public.version);
    let Ok(tag) = HeaderValue::from_str(&etag) else {
        return ApiError::not_found().into_response();
    };
    let cache = HeaderValue::from_static(LANDING_CACHE);
    if etag_matches(&headers, &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, tag), (header::CACHE_CONTROL, cache)],
        )
            .into_response();
    }
    (
        [(header::ETAG, tag), (header::CACHE_CONTROL, cache)],
        Json(public),
    )
        .into_response()
}

async fn app_icon(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    let Some((tag, extension)) = file.split_once('.') else {
        return ApiError::not_found().into_response();
    };
    match state
        .services
        .connect_apps()
        .icon_file(tag, extension)
        .await
    {
        Ok(Some((kind, bytes))) => (
            [
                (header::CONTENT_TYPE, kind.content_type()),
                (header::CACHE_CONTROL, IMAGE_CACHE),
                (header::CONTENT_SECURITY_POLICY, ICON_POLICY),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => ApiError::not_found().into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn favicon_ico(State(state): State<AppState>) -> Response {
    match state.services.branding().tab_icon_file().await {
        Ok(Some((kind, bytes))) => (
            [
                (header::CONTENT_TYPE, kind.content_type()),
                (header::CACHE_CONTROL, TAB_ICON_CACHE),
                (header::CONTENT_SECURITY_POLICY, IMAGE_POLICY),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => mark(&state, TAB_ICON_CACHE),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn icon(State(state): State<AppState>) -> Response {
    mark(&state, ICON_CACHE)
}

fn mark(state: &AppState, cache: &'static str) -> Response {
    let letter = escape_html(&brand_initial(&state.services.settings().brand_name));
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\">\
         <rect width=\"64\" height=\"64\" rx=\"16\" fill=\"#0b0b0c\"/>\
         <text x=\"32\" y=\"44\" text-anchor=\"middle\" fill=\"#ffffff\" font-size=\"36\" font-weight=\"800\" \
         font-family=\"system-ui, -apple-system, 'Segoe UI', Roboto, Arial, sans-serif\">{letter}</text></svg>"
    );
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, cache),
            (header::CONTENT_SECURITY_POLICY, IMAGE_POLICY),
        ],
        svg,
    )
        .into_response()
}

async fn health(State(state): State<AppState>) -> Response {
    match state.services.store().ping().await {
        Ok(()) => Json(serde_json::json!({ "status": "ok" })).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "база недоступна");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "db_unavailable" })),
            )
                .into_response()
        }
    }
}

async fn version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "stealthnet",
        "version": env!("CARGO_PKG_VERSION"),
        "remnawave": sn_remnawave::SUPPORTED_VERSION,
    }))
}
