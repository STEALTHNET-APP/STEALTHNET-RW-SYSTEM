use crate::error::ApiError;
use crate::state::AppState;
use crate::web::escape_html;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use sn_domain::image::ImageKind;
use sn_domain::module::ModuleId;
use sn_domain::settings::SiteImage;
use sn_services::ServiceResult;
use sn_services::branding::brand_initial;
use sn_services::own_css::StyleFile;
use sn_store::Db;
use std::sync::Arc;

const IMAGE_CACHE: &str = "public, max-age=31536000, immutable";
const ICON_CACHE: &str = "public, max-age=300";
const TAB_ICON_CACHE: &str = "public, max-age=3600";
const IMAGE_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'";
const ICON_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";
const LANDING_CACHE: &str = "no-cache";
const STYLE_POLICY: &str = "default-src 'none'; frame-ancestors 'none'; sandbox";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .route("/api/public/site", get(site))
        .route("/api/public/catalog", get(catalog))
        .route("/api/public/logo/{file}", get(logo))
        .route("/api/public/favicon/{file}", get(favicon))
        .route("/api/public/landing", get(landing))
        .route("/api/public/landing/{file}", get(landing_file))
        .route("/api/public/miniapp/{file}", get(miniapp_file))
        .route("/api/public/cabinet/{file}", get(cabinet_file))
        .route("/api/public/pages/{*path}", get(document_page))
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

async fn document_page(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Response {
    let path = format!("/{}", path.trim_matches('/'));
    let locale = state
        .services
        .settings()
        .languages
        .pick(sn_domain::lang::hint());
    match state.services.documents().page(&path, locale).await {
        Ok(Some(page)) => tagged(&headers, &serde_json::json!(page)),
        Ok(None) => ApiError::not_found().into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
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
    let email_confirm =
        email_auth == Some("password") && mail.signup_rule() != sn_services::mail::SignupRule::Open;
    let ctx = state.services.module_context();
    let widget = |captcha: sn_modules::catalog::antibot::captcha::Captcha| {
        serde_json::json!({
            "provider": captcha.provider,
            "site_key": captcha.site_key,
        })
    };
    let captcha = email_auth
        .and_then(|_| sn_modules::catalog::antibot::signup_captcha(&ctx))
        .map(widget);
    let captcha_spare = access
        .site_login
        .then(|| sn_modules::catalog::antibot::spare_captcha(&ctx))
        .flatten()
        .map(widget);
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
                "enabled": settings.trial.is_offered(),
                "days": settings
                    .trial
                    .shortest_days()
                    .or(settings.trial.first().map(|v| v.days))
                    .unwrap_or(0),
            },
            "documents": sn_services::documents::public_list(&state.services.bot_content().documents),
            "email_auth": email_auth,
            "email_reset": email_reset,
            "email_confirm": email_confirm,
            "captcha": captcha,
            "captcha_spare": captcha_spare,
            "oauth": state.services.oauth().availability(),
            "client_access": {
                "miniapp": access.miniapp,
                "miniapp_fullscreen": access.miniapp_fullscreen,
                "site_login": access.site_login,
                "devices": access.self_devices,
                "reissue": access.self_reissue,
            },
            "languages": {
                "list": settings.languages.enabled(),
                "default": settings.languages.fallback(),
            },
            "miniapp": state.services.miniapp().public().await,
            "cabinet": { "css": state.services.cabinet().style().map(|style| style.path()) },
            "maintenance": state.services.maintenance().public(),
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

fn style_file(style: Option<Arc<StyleFile>>, version: &str, headers: &HeaderMap) -> Response {
    let style = style.filter(|_| sn_domain::landing::valid_tag(version));
    let Some(style) = style else {
        return ApiError::not_found().into_response();
    };
    let etag = format!("\"{}\"", style.version);
    let Ok(tag) = HeaderValue::from_str(&etag) else {
        return ApiError::not_found().into_response();
    };
    let cache = HeaderValue::from_static(if style.version == version {
        IMAGE_CACHE
    } else {
        LANDING_CACHE
    });
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
                HeaderValue::from_static("text/css; charset=utf-8"),
            ),
            (header::ETAG, tag),
            (header::CACHE_CONTROL, cache),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(STYLE_POLICY),
            ),
        ],
        style.text.clone(),
    )
        .into_response()
}

fn image_response(found: ServiceResult<Option<(ImageKind, Vec<u8>)>>, extension: &str) -> Response {
    match found {
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

async fn landing_file(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some((tag, extension)) = file.split_once('.') else {
        return ApiError::not_found().into_response();
    };
    if extension == "css" {
        if !state.services.modules().is_enabled(ModuleId::Landing) {
            return sn_modules::gate::disabled_response(ModuleId::Landing);
        }
        return style_file(state.services.landing().style(), tag, &headers);
    }
    image_response(state.services.landing().image_file(tag).await, extension)
}

async fn miniapp_file(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some((tag, extension)) = file.split_once('.') else {
        return ApiError::not_found().into_response();
    };
    if extension == "css" {
        return style_file(state.services.miniapp().style(), tag, &headers);
    }
    image_response(state.services.miniapp().image_file(tag).await, extension)
}

async fn cabinet_file(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some((tag, extension)) = file.split_once('.') else {
        return ApiError::not_found().into_response();
    };
    if extension == "css" {
        return style_file(state.services.cabinet().style(), tag, &headers);
    }
    image_response(state.services.cabinet().image_file(tag).await, extension)
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
