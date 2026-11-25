use crate::cookies::{self, SameSite};
use crate::error::{ApiError, ApiResult};
use crate::extract::{
    ApiJson, CLIENT_COOKIE, CurrentClient, Meta, allowed_origins, cookie, cross_site_rejected,
    cross_site_request,
};
use crate::state::AppState;
use axum::extract::rejection::FormRejection;
use axum::extract::{Form, Path, Query, State};
use axum::http::header::{LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::ClientChannel;
use sn_modules::catalog::antibot;
use sn_modules::catalog::oauth::Provider;
use sn_services::limits::RATE_LIMITED;
use sn_services::oauth::{
    COOKIE, COOKIE_PATH, CallbackParams, LoginMethods, OauthFinish, OauthStart, Purpose,
    REQUEST_TTL_SECONDS, UNFINISHED,
};
use sn_services::telegram_link::{TelegramLinkStart, TelegramLinkStatus};
use sn_services::utm_tracking::site_utm;

const FIELD_MAX: usize = 4096;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/oauth/finish", post(finish))
        .route("/auth/oauth/{provider}", post(start))
        .route(
            "/auth/oauth/{provider}/callback",
            get(callback_query).post(callback_form),
        )
        .route("/security/logins", get(logins))
        .route("/security/logins/telegram", post(link_telegram))
        .route(
            "/security/logins/telegram/status",
            post(link_telegram_status),
        )
        .route("/security/logins/telegram/cancel", post(stop_telegram))
        .route("/security/logins/{provider}", post(link).delete(unlink))
}

fn provider(raw: &str) -> ApiResult<Provider> {
    Provider::parse(raw).ok_or_else(ApiError::not_found)
}

fn same_site(provider: Provider) -> SameSite {
    match provider {
        Provider::Google => SameSite::Lax,
        Provider::Apple => SameSite::None,
    }
}

fn started(state: &AppState, provider: Provider, start: &OauthStart) -> Response {
    let cookie = cookies::scoped(
        state,
        COOKIE,
        &start.browser,
        COOKIE_PATH,
        REQUEST_TTL_SECONDS,
        same_site(provider),
    );
    cookies::with_cookie(cookie, Json(start))
}

fn cleared(state: &AppState) -> Option<HeaderValue> {
    cookies::scoped(state, COOKIE, "", COOKIE_PATH, 0, SameSite::Lax)
}

fn same_origin(state: &AppState, method: &Method, headers: &HeaderMap) -> ApiResult<()> {
    if cross_site_request(method, headers, &allowed_origins(state)) {
        Err(cross_site_rejected())
    } else {
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
struct StartBody {
    #[serde(default, rename = "ref")]
    referral: Option<String>,
    #[serde(default)]
    utm: Option<serde_json::Value>,
}

async fn start(
    State(state): State<AppState>,
    Meta(meta): Meta,
    method: Method,
    headers: HeaderMap,
    Path(raw): Path<String>,
    body: Option<ApiJson<StartBody>>,
) -> ApiResult<Response> {
    same_origin(&state, &method, &headers)?;
    let provider = provider(&raw)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    let utm = body.utm.as_ref().and_then(site_utm);
    let start = state
        .services
        .oauth()
        .start_login_with(provider, body.referral.as_deref(), utm.as_ref(), &meta)
        .await?;
    Ok(started(&state, provider, &start))
}

#[derive(Debug, Default, Deserialize)]
struct Reply {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    user: Option<String>,
}

fn bounded(value: Option<String>) -> Option<String> {
    value.filter(|v| v.len() <= FIELD_MAX)
}

fn see_other(target: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(location) = HeaderValue::from_str(target) {
        response.headers_mut().insert(LOCATION, location);
    }
    response
}

async fn returned(state: &AppState, headers: &HeaderMap, raw: &str, reply: Reply) -> Response {
    let Some(provider) = Provider::parse(raw) else {
        return see_other("/cabinet/login");
    };
    let params = CallbackParams {
        code: bounded(reply.code),
        state: bounded(reply.state),
        error: bounded(reply.error),
        user: bounded(reply.user),
    };
    let browser = cookie(headers, COOKIE);
    let purpose = match state
        .services
        .oauth()
        .callback(provider, &params, browser.as_deref())
        .await
    {
        Ok(outcome) => outcome.purpose,
        Err(err) => {
            tracing::error!(error = %err, provider = provider.as_str(), "ответ провайдера входа не обработан");
            Purpose::Login
        }
    };
    let page = match purpose {
        Purpose::Login => "login",
        Purpose::Link => "security",
    };
    see_other(&format!("/cabinet/{page}?oauth={}", provider.as_str()))
}

async fn callback_query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(reply): Query<Reply>,
) -> Response {
    returned(&state, &headers, &raw, reply).await
}

async fn callback_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    form: Result<Form<Reply>, FormRejection>,
) -> Response {
    let reply = form.map(|f| f.0).unwrap_or_default();
    returned(&state, &headers, &raw, reply).await
}

#[derive(Debug, Default, Deserialize)]
struct FinishBody {
    #[serde(default)]
    captcha: Option<String>,
}

fn may_retry(code: &str) -> bool {
    matches!(
        code,
        RATE_LIMITED
            | UNFINISHED
            | antibot::CAPTCHA_REQUIRED
            | antibot::FAILED
            | antibot::UNAVAILABLE
    )
}

async fn finish(
    State(state): State<AppState>,
    Meta(meta): Meta,
    method: Method,
    headers: HeaderMap,
    body: Option<ApiJson<FinishBody>>,
) -> ApiResult<Response> {
    same_origin(&state, &method, &headers)?;
    let browser = cookie(&headers, COOKIE);
    let body = body.map(|b| b.0).unwrap_or_default();
    let result = state
        .services
        .oauth()
        .finish_with(browser.as_deref(), &meta, body.captcha.as_deref())
        .await;
    let mut response = match result {
        Ok(finished) => {
            let session = match &finished {
                OauthFinish::Confirmed { login } => cookies::session(
                    &state,
                    CLIENT_COOKIE,
                    &login.token,
                    Some(login.expires_at),
                    SameSite::Lax,
                ),
                _ => None,
            };
            cookies::with_cookie(session, Json(&finished))
        }
        Err(err) if may_retry(err.code()) => return Err(err.into()),
        Err(err) => ApiError::from(err).into_response(),
    };
    if let Some(clear) = cleared(&state) {
        response.headers_mut().append(SET_COOKIE, clear);
    }
    Ok(response)
}

fn cabinet(client: &CurrentClient) -> ApiResult<()> {
    client.require_full()?;
    if client.channel() == ClientChannel::Cabinet {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

async fn logins(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<LoginMethods>> {
    client.require_full()?;
    Ok(Json(
        state.services.oauth().logins(client.0.client_id).await?,
    ))
}

async fn link(
    client: CurrentClient,
    State(state): State<AppState>,
    Meta(meta): Meta,
    Path(raw): Path<String>,
) -> ApiResult<Response> {
    cabinet(&client)?;
    let provider = provider(&raw)?;
    let start = state
        .services
        .oauth()
        .start_link(&client.0, provider, &meta)
        .await?;
    Ok(started(&state, provider, &start))
}

async fn link_telegram(
    client: CurrentClient,
    State(state): State<AppState>,
    Meta(meta): Meta,
) -> ApiResult<Json<TelegramLinkStart>> {
    cabinet(&client)?;
    Ok(Json(
        state
            .services
            .telegram_links()
            .start(&client.0, &meta)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct LinkRequest {
    poll_token: String,
}

async fn link_telegram_status(
    client: CurrentClient,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<LinkRequest>,
) -> ApiResult<Json<TelegramLinkStatus>> {
    cabinet(&client)?;
    Ok(Json(
        state
            .services
            .telegram_links()
            .status(&client.0, &body.poll_token, &meta)
            .await?,
    ))
}

async fn stop_telegram(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<LinkRequest>,
) -> ApiResult<StatusCode> {
    cabinet(&client)?;
    state
        .services
        .telegram_links()
        .stop(&client.0, &body.poll_token)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unlink(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> ApiResult<Json<LoginMethods>> {
    cabinet(&client)?;
    let provider = provider(&raw)?;
    Ok(Json(
        state.services.oauth().unlink(&client.0, provider).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn кука_входа_остаётся_пока_вход_можно_продолжить() {
        for code in [
            RATE_LIMITED,
            UNFINISHED,
            antibot::CAPTCHA_REQUIRED,
            antibot::FAILED,
            antibot::UNAVAILABLE,
        ] {
            assert!(may_retry(code), "{code}");
        }
        for code in ["oauth.no_request", "oauth.refused", "oauth.expired"] {
            assert!(!may_retry(code), "{code}");
        }
    }
}
