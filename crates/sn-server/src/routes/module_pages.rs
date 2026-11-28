use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use sn_services::ext_modules::proxy::{self, BODY_MAX};

const SANDBOX: &str = "sandbox allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals; frame-ancestors 'self'";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/m/{id}/{cap}", any(bare))
        .route("/m/{id}/{cap}/", any(root))
        .route("/m/{id}/{cap}/{*rest}", any(page))
}

fn guarded(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CONTENT_SECURITY_POLICY, SANDBOX),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::CACHE_CONTROL, "no-store"),
        (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
    ] {
        headers.append(name, HeaderValue::from_static(value));
    }
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    response
}

fn text(status: StatusCode, message: &str) -> Response {
    guarded(
        (
            status,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            message.to_string(),
        )
            .into_response(),
    )
}

fn denied() -> Response {
    text(
        StatusCode::FORBIDDEN,
        "Ссылка на страницу модуля устарела или у вас нет к ней доступа. Откройте страницу из админки заново",
    )
}

async fn bare(Path((id, cap)): Path<(String, String)>, uri: Uri) -> Response {
    let target = match uri.query() {
        Some(query) => format!("{}{id}/{cap}/?{query}", proxy::PREFIX),
        None => format!("{}{id}/{cap}/", proxy::PREFIX),
    };
    match HeaderValue::from_str(&target) {
        Ok(location) => guarded(
            (
                StatusCode::PERMANENT_REDIRECT,
                [(header::LOCATION, location)],
            )
                .into_response(),
        ),
        Err(_) => denied(),
    }
}

struct Incoming {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
}

async fn root(
    State(state): State<AppState>,
    Path((id, cap)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let incoming = Incoming {
        method,
        uri,
        headers,
        body,
    };
    serve(state, id, cap, incoming).await
}

async fn page(
    State(state): State<AppState>,
    Path((id, cap, _)): Path<(String, String, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let incoming = Incoming {
        method,
        uri,
        headers,
        body,
    };
    serve(state, id, cap, incoming).await
}

fn raw_rest(uri: &Uri, id: &str, cap: &str) -> String {
    let prefix = format!("{}{id}/{cap}", proxy::PREFIX);
    uri.path().strip_prefix(&prefix).unwrap_or("/").to_string()
}

async fn serve(state: AppState, id: String, cap: String, incoming: Incoming) -> Response {
    let Incoming {
        method,
        uri,
        headers,
        body,
    } = incoming;
    if !matches!(
        method,
        Method::GET
            | Method::HEAD
            | Method::POST
            | Method::PUT
            | Method::PATCH
            | Method::DELETE
            | Method::OPTIONS
    ) {
        return text(
            StatusCode::METHOD_NOT_ALLOWED,
            "Такой запрос странице модуля не нужен",
        );
    }
    let modules = state.services.ext_modules();
    let entry = match modules.page_access(&id, &cap).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return denied(),
        Err(err) => {
            tracing::warn!(module = id.as_str(), error = %err, "страница модуля не открылась");
            return text(
                StatusCode::SERVICE_UNAVAILABLE,
                "Страница модуля сейчас недоступна. Попробуйте через минуту",
            );
        }
    };
    let rest = raw_rest(&uri, &id, &cap);
    if !proxy::within(entry.only.as_deref(), &rest) {
        return denied();
    }
    if method == Method::OPTIONS {
        let mut response = guarded(StatusCode::NO_CONTENT.into_response());
        let out = response.headers_mut();
        out.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, HEAD, POST, PUT, PATCH, DELETE"),
        );
        out.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("authorization, content-type, accept, accept-language"),
        );
        out.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        return response;
    }
    let body = match axum::body::to_bytes(body, BODY_MAX).await {
        Ok(body) => body,
        Err(_) => {
            return text(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Запрос к странице модуля больше 10 МБ",
            );
        }
    };
    let forwarded: Vec<(String, String)> = headers
        .iter()
        .filter(|(name, _)| proxy::request_header(name.as_str()))
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_string(), v.to_string()))
        })
        .collect();
    let public_url = state.services.settings().public_url.clone();
    let result = proxy::forward(
        &entry,
        proxy::Outgoing {
            method: method.as_str(),
            rest: &rest,
            query: uri.query(),
            headers: &forwarded,
            body,
        },
        state.services.now(),
        public_url.as_deref(),
    )
    .await;
    let forwarded = match result {
        Ok(forwarded) => forwarded,
        Err(err) => return text(StatusCode::BAD_GATEWAY, err.text()),
    };
    let status = StatusCode::from_u16(forwarded.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::new(Body::from_stream(forwarded.body));
    *response.status_mut() = status;
    let out = response.headers_mut();
    for (name, value) in forwarded.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            out.append(name, value);
        }
    }
    guarded(response)
}

#[cfg(test)]
mod tests {
    use super::raw_rest;
    use axum::http::Uri;

    #[test]
    fn путь_страницы_берётся_как_есть() {
        let uri: Uri = "/m/nodes/abc.def/api/installs/7?x=1".parse().unwrap();
        assert_eq!(raw_rest(&uri, "nodes", "abc.def"), "/api/installs/7");
        let uri: Uri = "/m/nodes/abc.def/".parse().unwrap();
        assert_eq!(raw_rest(&uri, "nodes", "abc.def"), "/");
        let uri: Uri = "/m/nodes/abc.def/a%2Fb".parse().unwrap();
        assert_eq!(raw_rest(&uri, "nodes", "abc.def"), "/a%2Fb");
    }
}
