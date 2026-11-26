pub mod admin;
pub mod client;
mod client_email;
mod client_oauth;
mod client_photo;
mod module_pages;
pub mod public_app;
mod public_pay;
mod public_verify;
pub mod support_files;
pub mod system;
pub mod webhooks;

use crate::extract::{api_key_scope, require_admin, require_client, with_client_address};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue, Method, Request, header};
use axum::middleware::from_fn_with_state;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

pub const REQUEST_ID: &str = "x-request-id";

fn cors(origins: &[String]) -> CorsLayer {
    let allowed: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static(REQUEST_ID),
            HeaderName::from_static(crate::lang::HEADER),
        ])
        .expose_headers([HeaderName::from_static(REQUEST_ID)])
        .allow_credentials(true)
}

pub fn request_span(request: &Request<Body>) -> tracing::Span {
    let request_id = request
        .headers()
        .get(REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(64).collect::<String>())
        .unwrap_or_default();
    tracing::info_span!(
        "http",
        method = %request.method(),
        path = %shown_path(request.uri().path()),
        request_id = %request_id,
    )
}

const SECRET_PATHS: [&str; 6] = [
    sn_services::bot_webhook::PATH_PREFIX,
    "/verify/",
    "/api/webhooks/pay/overpay/",
    "/api/public/support-files/",
    "/api/public/m/broadcast/unsubscribe/",
    "/api/public/m/broadcast/resubscribe/",
];

pub fn shown_path(path: &str) -> std::borrow::Cow<'_, str> {
    let pages = sn_services::ext_modules::proxy::PREFIX;
    if path.starts_with(pages) {
        return std::borrow::Cow::Owned(format!("{pages}…"));
    }
    SECRET_PATHS
        .iter()
        .find_map(|prefix| {
            path.split_once(prefix)
                .map(|(head, _)| std::borrow::Cow::Owned(format!("{head}{prefix}…")))
        })
        .unwrap_or(std::borrow::Cow::Borrowed(path))
}

const RESTORE_STATUS: &str = "/api/admin/backups/restore/status";

async fn restoring(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path();
    if !path.starts_with("/api/")
        || path == "/api/health"
        || path == "/api/version"
        || path == RESTORE_STATUS
    {
        return next.run(request).await;
    }
    match state.services.pause_gate().try_enter() {
        Some(work) => {
            let response = next.run(request).await;
            drop(work);
            response
        }
        None => {
            use axum::response::IntoResponse;
            let mut response = crate::error::ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "system.restoring",
                "Идёт восстановление из резервной копии. Панель заработает через несколько минут",
            )
            .into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
            response
        }
    }
}

pub fn router(state: AppState) -> Router {
    let ctx = state.services.module_context();
    let modules = state.services.modules().clone();
    let admin_modules = modules
        .admin_router(ctx.clone())
        .layer(from_fn_with_state(state.clone(), require_admin));
    let client_modules = modules
        .client_router(ctx.clone())
        .layer(from_fn_with_state(state.clone(), require_client));
    let public_modules = modules
        .public_router(ctx)
        .layer(from_fn_with_state(state.clone(), with_client_address));
    let request_id = HeaderName::from_static(REQUEST_ID);
    let pages = module_pages::routes()
        .layer(from_fn_with_state(
            state.clone(),
            crate::security::transport_security,
        ))
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        .layer(CatchPanicLayer::new())
        .with_state(state.clone());
    let main = Router::new()
        .merge(system::routes())
        .merge(public_app::routes())
        .merge(public_pay::routes())
        .merge(public_verify::routes())
        .merge(webhooks::routes())
        .merge(support_files::public_routes())
        .merge(crate::security::routes())
        .nest(
            "/api/admin",
            admin::routes()
                .merge(support_files::admin_routes())
                .layer(from_fn_with_state(state.clone(), api_key_scope)),
        )
        .nest(
            "/api/client",
            client::routes()
                .merge(client_photo::routes())
                .merge(client_oauth::routes())
                .merge(client_email::routes())
                .merge(support_files::client_routes()),
        )
        .nest_service("/api/admin/m", admin_modules)
        .nest_service("/api/client/m", client_modules)
        .nest_service("/api/public/m", public_modules)
        .fallback(crate::web::fallback)
        .layer(axum::middleware::from_fn(crate::error::client_facing))
        .layer(from_fn_with_state(state.clone(), crate::maintenance::gate))
        .layer(from_fn_with_state(
            state.clone(),
            crate::lang::client_language,
        ))
        .layer(from_fn_with_state(state.clone(), restoring))
        .layer(DefaultBodyLimit::max(state.config.body_limit_bytes))
        .layer(cors(&state.config.cors_origins))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(from_fn_with_state(
            state.clone(),
            crate::security::transport_security,
        ))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(CatchPanicLayer::new())
        .with_state(state);
    pages.merge(main)
}

#[cfg(test)]
mod tests {
    use super::shown_path;

    #[test]
    fn пути_с_секретами_скрываются_в_журнале() {
        for (path, shown) in [
            (
                "/api/webhooks/pay/overpay/9f1c2e7d4b5a60718293a4b5c6d7e8f9",
                "/api/webhooks/pay/overpay/…",
            ),
            (
                "/api/public/support-files/a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9",
                "/api/public/support-files/…",
            ),
            ("/verify/0123456789abcdef0123456789abcdef", "/verify/…"),
            (
                "/api/public/m/broadcast/unsubscribe/0b9d77aa1c2d4e3f",
                "/api/public/m/broadcast/unsubscribe/…",
            ),
            (
                "/api/public/m/broadcast/resubscribe/0b9d77aa1c2d4e3f",
                "/api/public/m/broadcast/resubscribe/…",
            ),
            ("/api/webhooks/telegram/abc", "/api/webhooks/telegram/…"),
            ("/m/nodes/AAAAAAAAAAE.c2VjcmV0/api/state", "/m/…"),
        ] {
            assert_eq!(shown_path(path), shown, "{path}");
        }
        for path in [
            "/api/webhooks/pay/cloudpayments/check",
            "/api/admin/clients/5",
            "/api/public/m/broadcast/images/k1",
        ] {
            assert_eq!(shown_path(path), path);
        }
    }
}
