use crate::error::ApiError;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sn_domain::lang;

const CLIENT: &str = "/api/client/";
const LOGOUT: &str = "/api/client/auth/logout";
const VERIFY: &str = "/api/public/verify";
const RETRY_SECONDS: &str = "300";

pub fn closed(method: &Method, path: &str) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return false;
    }
    if path == LOGOUT {
        return false;
    }
    path.starts_with(CLIENT)
        || path
            .strip_prefix(VERIFY)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

pub async fn gate(State(state): State<AppState>, request: Request<Body>, next: Next) -> Response {
    let ops = state.services.maintenance();
    if !ops.active() || !closed(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    let locale = state.services.settings().languages.pick(lang::hint());
    let mut response = ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        sn_services::maintenance::CODE,
        ops.text(locale),
    )
    .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static(RETRY_SECONDS));
    response
}

#[cfg(test)]
mod tests {
    use super::closed;
    use axum::http::Method;

    #[test]
    fn закрыты_только_изменения_клиента() {
        for (method, path) in [
            (Method::POST, "/api/client/checkout"),
            (Method::POST, "/api/client/topup"),
            (Method::POST, "/api/client/trial"),
            (Method::PUT, "/api/client/subscriptions/7/autorenew"),
            (Method::DELETE, "/api/client/subscriptions/7/devices/abc"),
            (Method::POST, "/api/client/auth/miniapp"),
            (Method::POST, "/api/client/m/promo/apply"),
            (Method::POST, "/api/client/m/gifts/buy"),
            (Method::POST, "/api/public/verify"),
            (Method::POST, "/api/public/verify/pass"),
        ] {
            assert!(closed(&method, path), "{method} {path}");
        }
        for (method, path) in [
            (Method::GET, "/api/client/me"),
            (Method::GET, "/api/client/subscriptions"),
            (Method::HEAD, "/api/client/me"),
            (Method::OPTIONS, "/api/client/checkout"),
            (Method::POST, "/api/client/auth/logout"),
            (Method::POST, "/api/webhooks/pay/cryptopay"),
            (Method::POST, "/api/public/pay/5/check"),
            (Method::POST, "/api/public/app-link"),
            (Method::POST, "/api/public/verifyx"),
            (Method::PUT, "/api/admin/settings/maintenance"),
            (Method::POST, "/api/core/balance"),
            (Method::POST, "/api/remnawave/webhook"),
        ] {
            assert!(!closed(&method, path), "{method} {path}");
        }
    }
}
