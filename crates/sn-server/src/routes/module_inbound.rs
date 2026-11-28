use crate::extract::client_ip;
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Extension, Router};
use sn_services::ext_modules::INBOUND_PREFIX;
use sn_services::ext_modules::inbound::{self, BODY_MAX, InboundRequest};
use std::net::SocketAddr;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/ext/{id}",
            any(inbound_root).layer(DefaultBodyLimit::max(BODY_MAX)),
        )
        .route(
            "/api/ext/{id}/{*rest}",
            any(inbound_path).layer(DefaultBodyLimit::max(BODY_MAX)),
        )
}

fn plain(status: StatusCode, message: &str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        message.to_string(),
    )
        .into_response()
}

struct Incoming {
    peer: Option<SocketAddr>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
}

async fn inbound_root(
    State(state): State<AppState>,
    Path(id): Path<String>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let incoming = Incoming {
        peer: peer.map(|p| p.0.0),
        method,
        uri,
        headers,
        body,
    };
    serve(state, id, incoming).await
}

async fn inbound_path(
    State(state): State<AppState>,
    Path((id, _)): Path<(String, String)>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let incoming = Incoming {
        peer: peer.map(|p| p.0.0),
        method,
        uri,
        headers,
        body,
    };
    serve(state, id, incoming).await
}

async fn serve(state: AppState, id: String, incoming: Incoming) -> Response {
    let Incoming {
        peer,
        method,
        uri,
        headers,
        body,
    } = incoming;
    if !matches!(
        method,
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return plain(
            StatusCode::METHOD_NOT_ALLOWED,
            "Такой запрос модулю не нужен",
        );
    }
    let base = match state.services.ext_modules().inbound_base(&id).await {
        Ok(Some(base)) => base,
        Ok(None) => {
            return plain(
                StatusCode::NOT_FOUND,
                "Модуля с таким адресом нет или он выключен",
            );
        }
        Err(err) => {
            tracing::warn!(module = id.as_str(), error = %err, "входящий запрос модулю не принят");
            return plain(
                StatusCode::SERVICE_UNAVAILABLE,
                "Модуль сейчас недоступен. Попробуйте через минуту",
            );
        }
    };
    let prefix = format!("{INBOUND_PREFIX}{id}");
    let rest = uri
        .path()
        .strip_prefix(&prefix)
        .unwrap_or_default()
        .to_string();
    if !sn_services::ext_modules::proxy::plain_path(&rest) {
        return plain(StatusCode::NOT_FOUND, "Такого адреса у модуля нет");
    }
    let forwarded: Vec<(String, String)> = headers
        .iter()
        .filter(|(name, _)| inbound::forwarded_header(name.as_str()))
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_string(), v.to_string()))
        })
        .collect();
    let ip = client_ip(
        peer.map(|p| p.ip()),
        &headers,
        &state.config.trusted_proxies,
    );
    let public_url = state.services.settings().public_url.clone();
    let result = inbound::forward(
        &base,
        InboundRequest {
            module: &id,
            method: method.as_str(),
            rest: &rest,
            query: uri.query(),
            headers: &forwarded,
            body,
            client_ip: ip,
            public_url: public_url.as_deref(),
        },
    )
    .await;
    let reply = match result {
        Ok(reply) => reply,
        Err(err) => {
            tracing::warn!(
                module = id.as_str(),
                error = err.text(),
                "входящий запрос модулю не дошёл"
            );
            let status = match err {
                inbound::InboundError::Timeout => StatusCode::GATEWAY_TIMEOUT,
                _ => StatusCode::BAD_GATEWAY,
            };
            return plain(status, err.text());
        }
    };
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::new(Body::from(reply.body));
    *response.status_mut() = status;
    let out = response.headers_mut();
    for (name, value) in reply.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            out.append(name, value);
        }
    }
    out.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
