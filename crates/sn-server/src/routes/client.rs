use crate::cookies::{self, SameSite};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CLIENT_COOKIE, CurrentClient, Meta, request_credential};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::ClientChannel;
use sn_domain::connect::LinkSpot;
use sn_domain::ids::{ClientId, PeriodId, SubscriptionId, TicketFileId, TicketId};
use sn_domain::money::{Money, parse_units};
use sn_domain::support::TicketChannel;
use sn_modules::catalog::antibot;
use sn_services::ServiceError;
use sn_services::checkout::{
    CartRequest, Channel, CheckoutRequest, CheckoutResult, ItemCheckoutRequest, ItemQuoteView,
    ItemRequest,
};
use sn_services::login::TelegramLoginStatus;
use sn_services::signup::{Arrival, Registration, SignupStart};
use sn_services::support::{ClientThread, OpenTicket, ReplyRequest};
use sn_services::two_factor::{CabinetLogin, TwoFactor};
use sn_services::utm_tracking::site_utm;
use sn_store::{ClientTicket, Db};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/miniapp", post(miniapp_login))
        .route("/auth/telegram", post(telegram_start))
        .route("/auth/telegram/status", post(telegram_status))
        .route("/auth/email", post(email_start))
        .route("/auth/email/verify", post(email_verify))
        .route("/auth/email/register", post(email_register))
        .route("/auth/email/register/resend", post(email_signup_resend))
        .route("/auth/email/register/confirm", post(email_signup_confirm))
        .route("/auth/email/password", post(email_password))
        .route("/auth/email/reset", post(email_reset_start))
        .route("/auth/email/reset/confirm", post(email_reset_confirm))
        .route("/auth/totp", post(totp_login))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/locale", put(set_locale))
        .route("/security", get(security))
        .route("/security/password", post(change_password))
        .route("/security/totp/setup", post(totp_setup))
        .route("/security/totp/enable", post(totp_enable))
        .route("/security/totp/disable", post(totp_disable))
        .route("/catalog", get(catalog))
        .route("/quote", post(quote))
        .route("/checkout", post(checkout))
        .route("/items/quote", post(item_quote))
        .route("/items/checkout", post(item_checkout))
        .route("/topup", get(topup_options).post(topup))
        .route("/trial", get(trial_offer).post(trial))
        .route("/payments", get(payments))
        .route("/payments/{id}", get(payment))
        .route("/payments/{id}/telegram", post(gram_in_telegram))
        .route("/subscriptions", get(subscriptions))
        .route("/subscriptions/{id}", get(subscription))
        .route("/subscriptions/{id}/autorenew", put(autorenew))
        .route("/subscriptions/{id}/devices", get(devices))
        .route(
            "/subscriptions/{id}/devices/{hwid}",
            delete(disconnect_device),
        )
        .route("/subscriptions/{id}/reissue", post(reissue))
        .route("/subscriptions/{id}/app-link", post(app_link))
        .route("/support/unread", get(support_unread))
        .route("/support/tickets", get(tickets).post(open_ticket))
        .route("/support/tickets/{id}", get(ticket).post(reply))
        .route("/support/tickets/{id}/close", post(close_ticket))
        .route("/support/tickets/{id}/operator", post(ask_operator))
}

fn channel(client: &CurrentClient) -> Channel {
    match client.channel() {
        ClientChannel::Miniapp => Channel::Miniapp,
        ClientChannel::Bot => Channel::Bot,
        ClientChannel::Cabinet => Channel::Cabinet,
    }
}

fn session_response(
    state: &AppState,
    login: &sn_services::auth::ClientLogin,
    body: impl serde::Serialize,
) -> Response {
    let cookie = cookies::session(
        state,
        CLIENT_COOKIE,
        &login.token,
        Some(login.expires_at),
        SameSite::Lax,
    );
    cookies::with_cookie(cookie, Json(body))
}

#[derive(Deserialize)]
struct MiniappLogin {
    init_data: String,
}

async fn miniapp_login(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<MiniappLogin>,
) -> ApiResult<Json<sn_services::auth::ClientLogin>> {
    Ok(Json(
        state
            .services
            .auth()
            .miniapp_login(&body.init_data, &meta)
            .await?,
    ))
}

#[derive(Debug, Default, Deserialize)]
struct TelegramStart {
    #[serde(default, rename = "ref")]
    referral: Option<String>,
}

async fn telegram_start(
    State(state): State<AppState>,
    Meta(meta): Meta,
    body: Option<ApiJson<TelegramStart>>,
) -> ApiResult<Json<sn_services::login::TelegramLoginStart>> {
    let referral = body.and_then(|b| b.0.referral);
    Ok(Json(
        state
            .services
            .auth()
            .telegram_login_start_with(&meta, referral.as_deref())
            .await?,
    ))
}

#[derive(Deserialize)]
struct TelegramPoll {
    poll_token: String,
}

async fn telegram_status(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<TelegramPoll>,
) -> ApiResult<Response> {
    let status = state
        .services
        .auth()
        .telegram_login_status(&body.poll_token, &meta)
        .await?;
    Ok(match &status {
        TelegramLoginStatus::Confirmed { login } => session_response(&state, login, &status),
        _ => cookies::with_cookie(None, Json(&status)),
    })
}

#[derive(Deserialize)]
struct EmailStart {
    email: String,
    #[serde(default)]
    captcha: Option<String>,
}

async fn email_start(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<EmailStart>,
) -> ApiResult<Json<sn_services::login::EmailLoginStart>> {
    Ok(Json(
        state
            .services
            .auth()
            .email_login_start_with(&body.email, &meta, body.captcha.as_deref())
            .await?,
    ))
}

#[derive(Deserialize)]
struct EmailVerify {
    email: String,
    code: String,
    #[serde(default)]
    captcha: Option<String>,
    #[serde(default)]
    utm: Option<serde_json::Value>,
    #[serde(default, rename = "ref")]
    referral: Option<String>,
}

async fn email_verify(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<EmailVerify>,
) -> ApiResult<Response> {
    let utm = body.utm.as_ref().and_then(site_utm);
    let from = Arrival::site(utm.as_ref(), body.referral.as_deref());
    let login = state
        .services
        .auth()
        .email_login_verify_from(
            &body.email,
            &body.code,
            &meta,
            from,
            body.captcha.as_deref(),
        )
        .await?;
    Ok(match &login {
        CabinetLogin::Session(session) => session_response(&state, session, &login),
        CabinetLogin::TotpRequired(_) => cookies::with_cookie(None, Json(&login)),
    })
}

#[derive(Deserialize)]
struct EmailPassword {
    email: String,
    password: String,
    #[serde(default)]
    utm: Option<serde_json::Value>,
    #[serde(default, rename = "ref")]
    referral: Option<String>,
    #[serde(default)]
    captcha: Option<String>,
}

fn cabinet_response(state: &AppState, login: &CabinetLogin) -> Response {
    match login {
        CabinetLogin::Session(session) => session_response(state, session, login),
        CabinetLogin::TotpRequired(_) => cookies::with_cookie(None, Json(login)),
    }
}

async fn email_register(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<EmailPassword>,
) -> ApiResult<Response> {
    let utm = body.utm.as_ref().and_then(site_utm);
    let from = Arrival::site(utm.as_ref(), body.referral.as_deref());
    let registration = state
        .services
        .auth()
        .email_signup_with(
            &body.email,
            &body.password,
            &meta,
            from,
            body.captcha.as_deref(),
        )
        .await?;
    Ok(match &registration {
        Registration::Login(login) => cabinet_response(&state, login),
        Registration::CodeSent(_) => cookies::with_cookie(None, Json(&registration)),
    })
}

#[derive(Deserialize)]
struct SignupResend {
    email: String,
    signup_token: String,
}

async fn email_signup_resend(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<SignupResend>,
) -> ApiResult<Json<SignupStart>> {
    Ok(Json(
        state
            .services
            .auth()
            .email_signup_resend(&body.email, &body.signup_token, &meta)
            .await?,
    ))
}

#[derive(Deserialize)]
struct SignupConfirm {
    email: String,
    signup_token: String,
    code: String,
    #[serde(default)]
    captcha: Option<String>,
}

async fn email_signup_confirm(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<SignupConfirm>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .email_signup_confirm_with(
            &body.email,
            &body.signup_token,
            &body.code,
            &meta,
            body.captcha.as_deref(),
        )
        .await?;
    Ok(cabinet_response(&state, &login))
}

#[derive(Deserialize)]
struct ResetStart {
    email: String,
}

async fn email_reset_start(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<ResetStart>,
) -> ApiResult<Json<sn_services::login::EmailLoginStart>> {
    Ok(Json(
        state
            .services
            .auth()
            .email_reset_start(&body.email, &meta)
            .await?,
    ))
}

#[derive(Deserialize)]
struct ResetConfirm {
    email: String,
    code: String,
    password: String,
}

async fn email_reset_confirm(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<ResetConfirm>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .email_reset_confirm(&body.email, &body.code, &body.password, &meta)
        .await?;
    Ok(cabinet_response(&state, &login))
}

#[derive(Deserialize)]
struct PasswordChange {
    #[serde(default)]
    current: Option<String>,
    password: String,
}

async fn change_password(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<PasswordChange>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    state
        .services
        .auth()
        .change_password(&client.0, body.current.as_deref(), &body.password)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn email_password(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<EmailPassword>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .email_password_login(&body.email, &body.password, &meta)
        .await?;
    Ok(cabinet_response(&state, &login))
}

#[derive(Deserialize)]
struct TotpLogin {
    challenge: String,
    code: String,
}

async fn totp_login(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<TotpLogin>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .cabinet_totp_login(&body.challenge, &body.code, &meta)
        .await?;
    Ok(session_response(&state, &login, &login))
}

async fn security(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<TwoFactor>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .client_two_factor(client.0.client_id)
            .await?,
    ))
}

async fn totp_setup(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::auth::TotpSetup>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .client_totp_setup(client.0.client_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct TotpEnable {
    secret: String,
    code: String,
}

async fn totp_enable(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TotpEnable>,
) -> ApiResult<Json<TwoFactor>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .client_totp_enable_in(
                client.0.client_id,
                &body.secret,
                &body.code,
                client.0.session_id,
            )
            .await?,
    ))
}

#[derive(Deserialize)]
struct TotpDisable {
    code: String,
}

async fn totp_disable(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TotpDisable>,
) -> ApiResult<Json<TwoFactor>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .client_totp_disable(client.0.client_id, &body.code)
            .await?,
    ))
}

async fn logout(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Some(credential) = request_credential(&state, &method, &headers, CLIENT_COOKIE)? {
        state
            .services
            .auth()
            .client_logout(credential.token())
            .await?;
    }
    let cookie = cookies::session(&state, CLIENT_COOKIE, "", None, SameSite::Lax);
    Ok(cookies::with_cookie(
        cookie,
        Json(serde_json::json!({ "ok": true })),
    ))
}

async fn me(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let card = state.services.clients().card(client.0.client_id).await?;
    let email = card
        .identities
        .iter()
        .find(|i| i.kind == sn_domain::client::IdentityKind::Email);
    let email_change = state.services.auth().email_editable(&card.identities);
    let email_unconfirmed = email.is_some_and(|i| i.linked_without_code());
    let email = email.map(|i| i.value.clone());
    let has_password = state
        .services
        .auth()
        .has_password(client.0.client_id)
        .await?;
    let autorenew_card = state
        .services
        .autorenew()
        .has_saved_card(client.0.client_id)
        .await?;
    let photo = photo_version(&state, client.0.client_id).await?;
    let support_unread = state.services.support().unread(client.0.client_id).await?;
    let must_change_password = match client.0.session_id {
        Some(session) if client.0.channel == sn_domain::access::ClientChannel::Cabinet => {
            state
                .services
                .auth()
                .password_change_pending(session)
                .await?
        }
        _ => false,
    };
    Ok(Json(serde_json::json!({
        "locale": state.services.locale_of(card.client.locale),
        "client": card.client,
        "balance": card.balance,
        "balance_held": card.balance_held,
        "balance_enabled": state.services.payments().balance_enabled(),
        "multi_subscriptions": state.services.settings().multi_subscriptions,
        "promo_codes": state
            .services
            .modules()
            .is_enabled(sn_domain::module::ModuleId::Promo),
        "options": state
            .services
            .modules()
            .is_enabled(sn_domain::module::ModuleId::Options),
        "autorenew_card": autorenew_card,
        "referral_balance": card.referral_balance,
        "currency": state.services.settings().currency,
        "channel": client.0.channel,
        "full_access": client.0.full_access,
        "two_factor": card.two_factor,
        "payments_count": card.stats.payments_count,
        "email": email,
        "receipt_email": card.receipt_email,
        "email_change": email_change,
        "email_unconfirmed": email_unconfirmed && state.services.mail().available(),
        "has_password": has_password,
        "must_change_password": must_change_password,
        "photo": photo,
        "support_unread": support_unread,
    })))
}

async fn photo_version(state: &AppState, id: ClientId) -> ApiResult<Option<String>> {
    let mut conn = state
        .services
        .store()
        .conn()
        .await
        .map_err(ServiceError::from)?;
    let mut photos = conn.client_photos();
    let version = photos
        .versions(&[id])
        .await
        .map_err(ServiceError::from)?
        .into_iter()
        .next()
        .map(|(_, version)| version);
    if version.is_none()
        && let Err(err) = photos.request_missing(&[id]).await
    {
        tracing::debug!(error = %err, "запрос фото клиента не сохранён");
    }
    Ok(version)
}

#[derive(Deserialize)]
struct LocaleBody {
    locale: sn_domain::client::Locale,
}

async fn set_locale(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<LocaleBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let locale = state
        .services
        .clients()
        .set_locale(client.0.client_id, body.locale)
        .await?;
    Ok(Json(serde_json::json!({ "locale": locale })))
}

async fn catalog(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let offers = state
        .services
        .checkout()
        .catalog_in(Some(client.0.channel))
        .await?;
    Ok(Json(serde_json::json!({
        "currency": state.services.settings().currency,
        "tariffs": offers,
    })))
}

async fn quote(
    client: CurrentClient,
    Meta(meta): Meta,
    State(state): State<AppState>,
    ApiJson(cart): ApiJson<CartRequest>,
) -> ApiResult<Json<sn_services::checkout::QuoteView>> {
    let address = meta.ip.as_deref().and_then(|ip| ip.parse().ok());
    let mut view = state
        .services
        .checkout()
        .quote_for(client.0.client_id, &cart, address, Some(client.channel()))
        .await?;
    view.quote = view.quote.shown();
    Ok(Json(view))
}

#[derive(Deserialize)]
struct CheckoutBody {
    cart: CartRequest,
    provider: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    return_url: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

async fn keep_receipt_email(
    state: &AppState,
    client: &CurrentClient,
    email: Option<&str>,
) -> ApiResult<()> {
    let Some(raw) = email.map(str::trim).filter(|e| !e.is_empty()) else {
        return Ok(());
    };
    let id = client.0.client_id;
    if state
        .services
        .checkout()
        .billing_email(id)
        .await?
        .editable()
    {
        state
            .services
            .clients()
            .set_receipt_email(id, raw, sn_services::Actor::Client(id))
            .await?;
    }
    Ok(())
}

async fn checkout(
    client: CurrentClient,
    Meta(meta): Meta,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CheckoutBody>,
) -> ApiResult<Json<CheckoutResult>> {
    client.require_full()?;
    keep_receipt_email(&state, &client, body.email.as_deref()).await?;
    let request = CheckoutRequest {
        cart: body.cart,
        provider: body.provider,
        method: body.method,
        channel: channel(&client),
        return_url: body.return_url,
    };
    let address = meta.ip.as_deref().and_then(|ip| ip.parse().ok());
    Ok(Json(
        state
            .services
            .checkout()
            .start_from(client.0.client_id, &request, address)
            .await?,
    ))
}

async fn item_quote(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(item): ApiJson<ItemRequest>,
) -> ApiResult<Json<ItemQuoteView>> {
    let mut view = state
        .services
        .checkout()
        .quote_item_in(client.0.client_id, &item, Some(client.channel()))
        .await?;
    view.quote = view.quote.shown();
    Ok(Json(view))
}

#[derive(Deserialize)]
struct ItemCheckoutBody {
    item: ItemRequest,
    provider: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    return_url: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

async fn item_checkout(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ItemCheckoutBody>,
) -> ApiResult<Json<CheckoutResult>> {
    client.require_full()?;
    keep_receipt_email(&state, &client, body.email.as_deref()).await?;
    let request = ItemCheckoutRequest {
        item: body.item,
        provider: body.provider,
        method: body.method,
        channel: channel(&client),
        return_url: body.return_url,
    };
    Ok(Json(
        state
            .services
            .checkout()
            .start_item(client.0.client_id, &request)
            .await?,
    ))
}

#[derive(Deserialize)]
struct TopUpBody {
    amount: String,
    provider: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    return_url: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

async fn topup_options(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::checkout::TopUpOptions>> {
    Ok(Json(
        state
            .services
            .checkout()
            .topup_options(client.0.client_id, channel(&client))
            .await?,
    ))
}

async fn topup(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TopUpBody>,
) -> ApiResult<Json<CheckoutResult>> {
    client.require_full()?;
    state.services.checkout().topup_open()?;
    keep_receipt_email(&state, &client, body.email.as_deref()).await?;
    let currency = state.services.settings().currency;
    let minor = parse_units(body.amount.trim(), currency.into()).ok_or_else(|| {
        ApiError::bad_request(
            "validation.amount",
            "сумма пополнения указана неверно, например 10.00",
        )
    })?;
    let limits = state.services.settings().topup_limits();
    let amount = Money::new(minor, currency);
    if amount.minor < limits.min.minor || amount.minor > limits.max.minor {
        return Err(ApiError::bad_request(
            "balance.topup_out_of_range",
            format!(
                "сумма пополнения должна быть от {} до {}",
                limits.min, limits.max
            ),
        ));
    }
    Ok(Json(
        state
            .services
            .checkout()
            .topup_from(
                client.0.client_id,
                amount,
                body.provider.trim(),
                body.method.as_deref(),
                channel(&client),
                body.return_url,
            )
            .await?,
    ))
}

async fn gram_in_telegram(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    let chat_url = state
        .services
        .checkout()
        .gram_in_telegram(client.0.client_id, id)
        .await?;
    Ok(Json(serde_json::json!({ "chat_url": chat_url })))
}

#[derive(Debug, Default, Deserialize)]
struct TrialBody {
    #[serde(default)]
    confirm: Option<String>,
    #[serde(default)]
    variant: Option<u32>,
    #[serde(default)]
    proof: Option<String>,
    #[serde(default)]
    captcha: Option<String>,
}

async fn trial_offer(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::checkout::TrialOffer>> {
    if !client.0.full_access {
        return Ok(Json(sn_services::checkout::TrialOffer {
            available: false,
            variants: Vec::new(),
        }));
    }
    Ok(Json(
        state
            .services
            .checkout()
            .trial_offer(client.0.client_id)
            .await?,
    ))
}

fn again_reason(code: &str) -> Option<&'static str> {
    match code {
        antibot::CONFIRM_CODE => Some("ask"),
        antibot::TOO_FAST => Some("too_fast"),
        antibot::FAILED => Some("failed"),
        antibot::EXPIRED => Some("expired"),
        _ => None,
    }
}

async fn trial(
    client: CurrentClient,
    State(state): State<AppState>,
    Meta(meta): Meta,
    body: Option<ApiJson<TrialBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    let checkout = state.services.checkout();
    let id = client.0.client_id;
    let body = body.map(|ApiJson(b)| b).unwrap_or_default();
    let variant = body.variant;
    let result = match body
        .confirm
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(token) => {
            let answer = antibot::Answer::web(
                body.proof.as_deref(),
                body.captcha.as_deref(),
                meta.ip.as_deref(),
            );
            checkout.confirm_trial(id, token, answer, variant).await
        }
        None => checkout.start_trial(id, variant).await,
    };
    match result {
        Ok(order_id) => Ok(Json(serde_json::json!({ "order_id": order_id }))),
        Err(err) => match again_reason(err.code()) {
            Some(again) => {
                let challenge = checkout
                    .trial_challenge(id, antibot::Surface::Web, variant)
                    .await?;
                let text = if challenge.captcha() {
                    sn_domain::lang::pick(
                        "Чтобы получить пробный период, пройдите проверку «Я не робот».",
                        "To get the free trial, pass the check that you're not a robot.",
                    )
                } else {
                    sn_domain::lang::pick(
                        "Чтобы получить пробный период, подтвердите, что вы не робот.",
                        "To get the free trial, confirm you're not a robot.",
                    )
                };
                Ok(Json(serde_json::json!({
                    "confirm": {
                        "token": challenge.token,
                        "text": text,
                        "check": challenge.check,
                        "expires_at": challenge.expires_at,
                    },
                    "again": again,
                })))
            }
            None => Err(err.into()),
        },
    }
}

#[derive(Debug, Default, Deserialize)]
struct PaymentsQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    before: Option<Uuid>,
}

async fn payments(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<PaymentsQuery>,
) -> ApiResult<Json<sn_services::clients::ClientPayments>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .clients()
            .payments(client.0.client_id, query.before, query.limit)
            .await?,
    ))
}

async fn payment(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<sn_services::clients::PaymentStatusView>> {
    Ok(Json(
        state
            .services
            .clients()
            .payment_status(client.0.client_id, id)
            .await?,
    ))
}

async fn subscriptions(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<sn_services::subscriptions::SubscriptionView>>> {
    Ok(Json(
        state
            .services
            .subscriptions()
            .list_for_client(client.0.client_id)
            .await?,
    ))
}

async fn subscription(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<sn_services::subscriptions::SubscriptionView>> {
    Ok(Json(
        state
            .services
            .subscriptions()
            .view(SubscriptionId(id), client.actor(), true)
            .await?,
    ))
}

#[derive(Deserialize)]
struct AutorenewBody {
    enabled: bool,
    #[serde(default)]
    period_id: Option<PeriodId>,
}

async fn autorenew(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<AutorenewBody>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    state
        .services
        .subscriptions()
        .set_autorenew(
            SubscriptionId(id),
            body.enabled,
            body.period_id,
            client.actor(),
        )
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn devices(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<sn_services::subscriptions::DeviceView>>> {
    Ok(Json(
        state
            .services
            .subscriptions()
            .devices(SubscriptionId(id), client.actor())
            .await?,
    ))
}

async fn disconnect_device(
    client: CurrentClient,
    State(state): State<AppState>,
    Path((id, hwid)): Path<(i64, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    state
        .services
        .subscriptions()
        .disconnect_device(SubscriptionId(id), &hwid, client.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn reissue(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    let url = state
        .services
        .subscriptions()
        .reissue_link(SubscriptionId(id), client.actor())
        .await?;
    Ok(Json(serde_json::json!({ "subscription_url": url })))
}

async fn support_unread(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let tickets = state.services.support().unread(client.0.client_id).await?;
    Ok(Json(serde_json::json!({ "tickets": tickets })))
}

async fn app_link(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(spot): ApiJson<LinkSpot>,
) -> ApiResult<Json<sn_services::app_links::AppLink>> {
    Ok(Json(
        state
            .services
            .app_links()
            .issue(SubscriptionId(id), client.0.client_id, &spot)
            .await?,
    ))
}

async fn tickets(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ClientTicket>>> {
    Ok(Json(
        state
            .services
            .support()
            .for_client(client.0.client_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct OpenTicketBody {
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    subscription_id: Option<SubscriptionId>,
    #[serde(default)]
    files: Vec<TicketFileId>,
}

async fn open_ticket(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<OpenTicketBody>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    if !state.services.settings().support_tickets() {
        return Err(ApiError::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "support.tickets_off",
            "Обращения здесь не принимаются: напишите в поддержку по ссылке",
        ));
    }
    let channel = match client.channel() {
        ClientChannel::Cabinet => TicketChannel::Cabinet,
        ClientChannel::Miniapp => TicketChannel::Miniapp,
        ClientChannel::Bot => TicketChannel::Bot,
    };
    let id = state
        .services
        .support()
        .open_with_files(
            client.0.client_id,
            &OpenTicket {
                subscription_id: body.subscription_id,
                channel,
                subject: body.subject,
                body: body.body,
            },
            &body.files,
        )
        .await?;
    Ok(Json(serde_json::json!({ "ticket_id": id })))
}

async fn client_thread(
    state: &AppState,
    client: &CurrentClient,
    id: TicketId,
) -> ApiResult<Json<ClientThread>> {
    Ok(Json(
        state
            .services
            .support()
            .client_thread(id, client.0.client_id, client.0.full_access)
            .await?,
    ))
}

async fn ticket(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<ClientThread>> {
    client_thread(&state, &client, TicketId(id)).await
}

#[derive(Deserialize)]
struct ReplyBody {
    #[serde(default)]
    body: String,
    #[serde(default)]
    files: Vec<TicketFileId>,
}

async fn reply(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<ReplyBody>,
) -> ApiResult<Json<ClientThread>> {
    client.require_full()?;
    state
        .services
        .support()
        .reply_with_files(
            TicketId(id),
            &ReplyRequest {
                body: body.body,
                close: false,
            },
            &body.files,
            client.actor(),
        )
        .await?;
    client_thread(&state, &client, TicketId(id)).await
}

async fn ask_operator(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<ClientThread>> {
    client.require_full()?;
    state
        .services
        .support()
        .ask_operator(TicketId(id), client.actor())
        .await?;
    client_thread(&state, &client, TicketId(id)).await
}

async fn close_ticket(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    state
        .services
        .support()
        .close(TicketId(id), client.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use crate::{AppState, ServerConfig, router};
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{HeaderMap, Method, Request, StatusCode, header};
    use serde_json::{Value, json};
    use sn_domain::client::{RegistrationSource, TelegramProfile};
    use sn_domain::ids::{ClientId, TicketId};
    use sn_domain::settings::keys;
    use sn_domain::support::{TicketChannel, TicketStatus};
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_services::auth::{RequestMeta, sign_init_data};
    use sn_services::support::OpenTicket;
    use sn_services::{Actor, Clock, EnvFallbacks, FixedClock, ServiceDeps, Services, SystemClock};
    use sn_store::testing::{TestDb, test_store, wait_for_window_room};
    use sn_store::{Db, SessionChannel};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    const BOT: &str = "123456:TEST-token";
    const PANEL: &str = "https://panel.example.com";
    const PASSWORD: &str = "очень-надёжный-пароль";

    async fn state() -> Option<(TestDb, AppState)> {
        state_on(Arc::new(SystemClock)).await
    }

    async fn state_on(clock: Arc<dyn Clock>) -> Option<(TestDb, AppState)> {
        let db = test_store().await?;
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: None,
            clock,
            env: EnvFallbacks {
                bot_token: Some(BOT.into()),
                ..EnvFallbacks::default()
            },
        });
        services.reload().await.unwrap();
        let config = ServerConfig {
            public_url: Some(PANEL.into()),
            ..ServerConfig::default()
        };
        Some((db, AppState::new(config, services)))
    }

    async fn send(
        state: &AppState,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        send_from(state, None, method, path, headers, body).await
    }

    async fn send_from(
        state: &AppState,
        peer: Option<SocketAddr>,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let mut request = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        let response = router(state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn percent(value: &str) -> String {
        value
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }

    fn init_data(state: &AppState) -> String {
        let user = r#"{"id":5093118422,"first_name":"Алексей","username":"morozov_a"}"#;
        let auth_date = state.services.now().timestamp().to_string();
        let hash = sign_init_data(&[("auth_date", &auth_date), ("user", user)], BOT);
        format!("auth_date={auth_date}&user={}&hash={hash}", percent(user))
    }

    async fn cabinet_client(state: &AppState) -> (ClientId, String) {
        let (client, _) = state
            .services
            .clients()
            .ensure_telegram(
                &TelegramProfile {
                    id: 5_093_118_422,
                    username: Some("morozov_a".into()),
                    first_name: Some("Алексей".into()),
                    last_name: None,
                    language_code: Some("ru".into()),
                    is_premium: false,
                },
                RegistrationSource::Bot,
                None,
            )
            .await
            .unwrap();
        let login = state
            .services
            .auth()
            .client_session_for(
                client.id,
                SessionChannel::Cabinet,
                &RequestMeta::default(),
                false,
            )
            .await
            .unwrap();
        (client.id, login.token)
    }

    async fn lava_state(lava: &wiremock::MockServer) -> Option<(TestDb, AppState, i64, i64)> {
        use sn_domain::payment::providers;
        use sn_domain::settings::secret_keys;
        let db = test_store().await?;
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::new(vec![
                Arc::new(sn_payments::providers::Lava::with_api_base(lava.uri())),
                Arc::new(sn_payments::providers::Balance),
            ])),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: Some(
                sn_store::SecretBox::from_base64(&sn_store::SecretBox::generate_key_base64())
                    .unwrap(),
            ),
            clock: Arc::new(SystemClock),
            env: EnvFallbacks {
                bot_token: Some(BOT.into()),
                ..EnvFallbacks::default()
            },
        });
        let (tariff, period) = {
            let mut tx = db.store.begin().await.unwrap();
            tx.settings()
                .set(keys::PUBLIC_URL, &json!(PANEL), None)
                .await
                .unwrap();
            tx.settings()
                .set(
                    &keys::provider(providers::LAVA),
                    &json!({
                        "enabled": true,
                        "config": {"offer_id": "836b9fc5-7ae9-4a27-9642-592bc44072b7"},
                        "methods": [{"code": "card"}]
                    }),
                    None,
                )
                .await
                .unwrap();
            for (field, value) in [
                ("api_key", "lava-api-key"),
                ("webhook_key", "lava-webhook-key-123"),
            ] {
                sn_services::settings::write_secret(
                    &mut tx,
                    services.secrets(),
                    &secret_keys::provider(providers::LAVA, field),
                    Some(value),
                    None,
                )
                .await
                .unwrap();
            }
            let category = tx
                .tariffs()
                .create_category(&sn_store::NewCategory {
                    name: "Обычный".into(),
                    sort: 0,
                    visible: true,
                })
                .await
                .unwrap();
            let tariff = tx
                .tariffs()
                .create(&sn_store::NewTariff {
                    traffic_addon: None,
                    category_id: category.id,
                    name: "Стандарт".into(),
                    description: None,
                    on_sale: true,
                    sort: 0,
                    devices: sn_domain::tariff::DevicePolicy {
                        included: 3,
                        extra: None,
                    },
                    traffic: sn_domain::traffic::TrafficPolicy::UNLIMITED,
                    squads: vec![sn_domain::ids::SquadId(uuid::Uuid::from_u128(1))],
                    external_squad: None,
                    periods: vec![sn_store::NewPeriod {
                        days: 30,
                        prices: sn_domain::tariff::CurrencyPrices {
                            usd: Some(299),
                            rub: Some(19_900),
                        },
                        stars: None,
                        sort: 0,
                        active: true,
                    }],
                })
                .await
                .unwrap();
            tx.commit().await.unwrap();
            (tariff.id.0, tariff.periods[0].id.0)
        };
        services.reload().await.unwrap();
        let remna = Arc::new(sn_remnawave::fake::FakeRemnawave::new());
        remna.set_squads(vec![sn_remnawave::InternalSquad {
            uuid: uuid::Uuid::from_u128(1),
            name: "Германия".into(),
            view_position: 0,
            info: sn_remnawave::SquadInfo::default(),
            inbounds: Vec::new(),
        }]);
        services.set_remna(Some(remna));
        let config = ServerConfig {
            public_url: Some(PANEL.into()),
            ..ServerConfig::default()
        };
        Some((db, AppState::new(config, services), tariff, period))
    }

    async fn telegram_session(state: &AppState, id: i64, channel: SessionChannel) -> String {
        let (client, _) = state
            .services
            .clients()
            .ensure_telegram(
                &TelegramProfile {
                    id,
                    username: None,
                    first_name: Some("Аня".into()),
                    last_name: None,
                    language_code: Some("ru".into()),
                    is_premium: false,
                },
                RegistrationSource::Bot,
                None,
            )
            .await
            .unwrap();
        state
            .services
            .auth()
            .client_session_for(client.id, channel, &RequestMeta::default(), false)
            .await
            .unwrap()
            .token
    }

    #[tokio::test]
    async fn кабинет_и_мини_приложение_принимают_почту_для_чека_при_оплате() {
        use wiremock::matchers::{method as http, path};
        let lava = wiremock::MockServer::start().await;
        wiremock::Mock::given(http("POST"))
            .and(path("/api/v3/invoice"))
            .respond_with(|req: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let tail = if body["email"] == "pay@example.com" {
                    1
                } else {
                    2
                };
                wiremock::ResponseTemplate::new(201).set_body_json(json!({
                    "id": format!("7ea82675-4ded-4133-95a7-{tail:012}"),
                    "status": "in-progress",
                    "amountTotal": {"currency": "USD", "amount": 2.99},
                    "paymentUrl": "https://app.lava.top/payment/7ea82675"
                }))
            })
            .mount(&lava)
            .await;
        let Some((db, state, tariff, period)) = lava_state(&lava).await else {
            return;
        };
        let cart = json!({"tariff_id": tariff, "period_id": period, "extra_devices": 0});
        for (id, channel, email) in [
            (5_093_118_501, SessionChannel::Cabinet, "Pay@Example.com"),
            (5_093_118_502, SessionChannel::Miniapp, "mini@example.com"),
        ] {
            let token = telegram_session(&state, id, channel).await;
            let auth = format!("Bearer {token}");
            let headers = [("authorization", auth.as_str())];
            let (status, _, me) = send(&state, Method::GET, "/api/client/me", &headers, None).await;
            assert_eq!(status, StatusCode::OK, "{me}");
            assert_eq!(me["receipt_email"], Value::Null);
            let (status, _, quote) = send(
                &state,
                Method::POST,
                "/api/client/quote",
                &headers,
                Some(cart.clone()),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{quote}");
            let lava_method = quote["methods"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["provider"] == "lava")
                .cloned()
                .unwrap();
            assert_eq!(lava_method["needs_email"], true, "{quote}");
            let (status, _, refused) = send(
                &state,
                Method::POST,
                "/api/client/checkout",
                &headers,
                Some(json!({"cart": cart, "provider": "lava", "method": "card"})),
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
            assert_eq!(refused["error"]["code"], "payment.email_required");
            let (status, _, typo) = send(
                &state,
                Method::POST,
                "/api/client/checkout",
                &headers,
                Some(json!({"cart": cart, "provider": "lava", "method": "card", "email": "pay@"})),
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{typo}");
            assert_eq!(typo["error"]["code"], "receipt.email_invalid");
            let (status, _, paid) = send(
                &state,
                Method::POST,
                "/api/client/checkout",
                &headers,
                Some(json!({"cart": cart, "provider": "lava", "method": "card", "email": email})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{paid}");
            assert_eq!(paid["type"], "redirect", "{paid}");
            let (_, _, me) = send(&state, Method::GET, "/api/client/me", &headers, None).await;
            assert_eq!(me["receipt_email"], email.to_lowercase());
        }
        let sent: Vec<Value> = lava
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| serde_json::from_slice(&r.body).ok())
            .collect();
        let emails: Vec<&str> = sent.iter().filter_map(|b| b["email"].as_str()).collect();
        assert_eq!(emails, vec!["pay@example.com", "mini@example.com"]);
        db.cleanup().await;
    }

    async fn open_ticket(state: &AppState, client: ClientId) -> TicketId {
        state
            .services
            .support()
            .open(
                client,
                &OpenTicket {
                    subscription_id: None,
                    channel: TicketChannel::Cabinet,
                    subject: Some("Чек".into()),
                    body: "Не пришёл чек об оплате".into(),
                },
            )
            .await
            .unwrap()
    }

    async fn ticket_status(state: &AppState, client: ClientId, ticket: TicketId) -> TicketStatus {
        state
            .services
            .support()
            .thread(ticket, Actor::Client(client))
            .await
            .unwrap()
            .ticket
            .status
    }

    async fn promo_state() -> Option<(TestDb, AppState)> {
        let db = test_store().await?;
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::new(sn_modules::catalog::all()).unwrap()),
            secrets: None,
            clock: Arc::new(SystemClock),
            env: EnvFallbacks {
                bot_token: Some(BOT.into()),
                ..EnvFallbacks::default()
            },
        });
        services.reload().await.unwrap();
        let config = ServerConfig {
            public_url: Some(PANEL.into()),
            ..ServerConfig::default()
        };
        Some((db, AppState::new(config, services)))
    }

    async fn promo_window_room(state: &AppState) {
        let minutes = sn_modules::catalog::promo::FAILURE_WINDOW_MINUTES.unsigned_abs();
        wait_for_window_room(state.services.store(), Duration::from_secs(minutes * 60)).await;
    }

    async fn session(state: &AppState, telegram_id: i64) -> String {
        let (client, _) = state
            .services
            .clients()
            .ensure_telegram(
                &TelegramProfile {
                    id: telegram_id,
                    username: None,
                    first_name: Some("Клиент".into()),
                    last_name: None,
                    language_code: Some("ru".into()),
                    is_premium: false,
                },
                RegistrationSource::Bot,
                None,
            )
            .await
            .unwrap();
        state
            .services
            .auth()
            .client_session_for(
                client.id,
                SessionChannel::Cabinet,
                &RequestMeta::default(),
                false,
            )
            .await
            .unwrap()
            .token
    }

    #[tokio::test]
    async fn перебор_промокодов_с_одного_адреса_останавливается_для_новых_аккаунтов() {
        let Some((db, state)) = promo_state().await else {
            return;
        };
        sn_modules::catalog::promo::create(
            &state.services.module_context(),
            &serde_json::from_value(json!({"code": "REAL20", "kind": "percent", "percent": 20}))
                .unwrap(),
            None,
        )
        .await
        .unwrap();
        promo_window_room(&state).await;
        let attacker: SocketAddr = "203.0.113.7:40000".parse().unwrap();
        let other: SocketAddr = "203.0.113.8:40000".parse().unwrap();
        for account in 0..3 {
            let token = session(&state, 9_000 + account).await;
            let bearer = format!("Bearer {token}");
            for n in 0..10 {
                let (status, _, body) = send_from(
                    &state,
                    Some(attacker),
                    Method::POST,
                    "/api/client/m/promo/check",
                    &[("authorization", &bearer)],
                    Some(json!({ "code": format!("GUESS{account}X{n}") })),
                )
                .await;
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            }
        }
        let token = session(&state, 9_100).await;
        let bearer = format!("Bearer {token}");
        let (status, _, body) = send_from(
            &state,
            Some(attacker),
            Method::POST,
            "/api/client/m/promo/check",
            &[("authorization", &bearer)],
            Some(json!({"code": "REAL20"})),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
        let (status, _, body) = send_from(
            &state,
            Some(other),
            Method::POST,
            "/api/client/m/promo/check",
            &[("authorization", &bearer)],
            Some(json!({"code": "REAL20"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        db.cleanup().await;
    }

    async fn real_code(state: &AppState) {
        sn_modules::catalog::promo::create(
            &state.services.module_context(),
            &serde_json::from_value(json!({"code": "REAL20", "kind": "percent", "percent": 20}))
                .unwrap(),
            None,
        )
        .await
        .unwrap();
    }

    async fn guess_from(
        state: &AppState,
        peer: SocketAddr,
        forwarded: Option<&str>,
        bearer: &str,
        code: &str,
    ) -> StatusCode {
        let mut headers = vec![("authorization", bearer)];
        if let Some(forwarded) = forwarded {
            headers.push(("x-forwarded-for", forwarded));
        }
        send_from(
            state,
            Some(peer),
            Method::POST,
            "/api/client/m/promo/check",
            &headers,
            Some(json!({ "code": code })),
        )
        .await
        .0
    }

    async fn exhaust_address(
        state: &AppState,
        peer: SocketAddr,
        forwarded: Option<&str>,
        first_account: i64,
    ) {
        promo_window_room(state).await;
        let accounts = sn_modules::catalog::promo::ADDRESS_FAILURE_LIMIT
            / sn_modules::catalog::promo::FAILURE_LIMIT;
        for account in 0..i64::from(accounts) {
            let bearer = format!("Bearer {}", session(state, first_account + account).await);
            for n in 0..sn_modules::catalog::promo::FAILURE_LIMIT {
                assert_eq!(
                    guess_from(
                        state,
                        peer,
                        forwarded,
                        &bearer,
                        &format!("GUESS{account}X{n}")
                    )
                    .await,
                    StatusCode::UNPROCESSABLE_ENTITY
                );
            }
        }
    }

    #[tokio::test]
    async fn лимит_промокодов_считается_по_адресу_клиента_за_доверенным_прокси() {
        let Some((db, state)) = promo_state().await else {
            return;
        };
        real_code(&state).await;
        let proxy: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        exhaust_address(&state, proxy, Some("203.0.113.7"), 9_200).await;
        let bearer = format!("Bearer {}", session(&state, 9_300).await);
        assert_eq!(
            guess_from(&state, proxy, Some("203.0.113.7"), &bearer, "REAL20").await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            guess_from(&state, proxy, Some("203.0.113.8"), &bearer, "REAL20").await,
            StatusCode::OK,
            "сосед за тем же прокси не ограничен"
        );
        let direct: SocketAddr = "203.0.113.7:40000".parse().unwrap();
        assert_eq!(
            guess_from(&state, direct, Some("198.51.100.77"), &bearer, "REAL20").await,
            StatusCode::TOO_MANY_REQUESTS,
            "заголовок от недоверенного адреса не меняет адрес клиента"
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn расчёт_корзины_считает_неверные_промокоды_по_адресу_клиента() {
        use sn_domain::money::Stars;
        use sn_domain::tariff::{CurrencyPrices, DevicePolicy};
        use sn_domain::traffic::TrafficPolicy;
        use sn_store::{NewCategory, NewPeriod, NewTariff};
        let Some((db, state)) = promo_state().await else {
            return;
        };
        real_code(&state).await;
        let tariff = {
            let mut conn = state.services.store().conn().await.unwrap();
            let category = conn
                .tariffs()
                .create_category(&NewCategory {
                    name: "Стандарт".into(),
                    sort: 0,
                    visible: true,
                })
                .await
                .unwrap();
            conn.tariffs()
                .create(&NewTariff {
                    traffic_addon: None,
                    category_id: category.id,
                    name: "Стандарт".into(),
                    description: None,
                    on_sale: true,
                    sort: 0,
                    devices: DevicePolicy {
                        included: 3,
                        extra: None,
                    },
                    traffic: TrafficPolicy::UNLIMITED,
                    squads: Vec::new(),
                    external_squad: None,
                    periods: vec![NewPeriod {
                        days: 30,
                        prices: CurrencyPrices {
                            usd: Some(299),
                            rub: Some(19_900),
                        },
                        stars: Some(Stars(150)),
                        sort: 0,
                        active: true,
                    }],
                })
                .await
                .unwrap()
        };
        let proxy: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        exhaust_address(&state, proxy, Some("203.0.113.40"), 9_400).await;
        let bearer = format!("Bearer {}", session(&state, 9_500).await);
        let cart = json!({
            "tariff_id": tariff.id,
            "period_id": tariff.periods[0].id,
            "promo_code": "REAL20",
        });
        let quote = |forwarded: &'static str| {
            let state = state.clone();
            let bearer = bearer.clone();
            let cart = cart.clone();
            async move {
                send_from(
                    &state,
                    Some(proxy),
                    Method::POST,
                    "/api/client/quote",
                    &[("authorization", &bearer), ("x-forwarded-for", forwarded)],
                    Some(cart),
                )
                .await
            }
        };
        let (status, _, body) = quote("203.0.113.40").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("Слишком много")),
            "{body}"
        );
        let (status, _, body) = quote("203.0.113.41").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["quote"]["total"]["minor"], json!(239), "{body}");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn клиенты_за_прокси_на_loopback_не_делят_лимит_входа() {
        let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
        let Some((db, state)) = state_on(clock).await else {
            return;
        };
        {
            let mut conn = state.services.store().conn().await.unwrap();
            conn.settings()
                .set_as(keys::BOT_USERNAME, &"stealth_vpn_bot", None)
                .await
                .unwrap();
        }
        let proxy: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        let limit = sn_services::limits::rules::TELEGRAM_START_IP.limit;
        for i in 0..=limit {
            let forwarded = format!("203.0.113.{i}");
            let (status, _, body) = send_from(
                &state,
                Some(proxy),
                Method::POST,
                "/api/client/auth/telegram",
                &[("x-forwarded-for", &forwarded)],
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{forwarded}: {body}");
        }
        for attempt in 0..=limit {
            let (status, _, body) = send_from(
                &state,
                Some(proxy),
                Method::POST,
                "/api/client/auth/telegram",
                &[("x-forwarded-for", "198.51.100.7")],
                None,
            )
            .await;
            let expected = if attempt < limit {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            };
            assert_eq!(status, expected, "{attempt}: {body}");
        }
        db.cleanup().await;
    }

    #[tokio::test]
    async fn мини_приложение_получает_токен_без_куки() {
        let Some((db, state)) = state().await else {
            return;
        };
        let (status, headers, body) = send(
            &state,
            Method::POST,
            "/api/client/auth/miniapp",
            &[],
            Some(json!({ "init_data": init_data(&state) })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            headers.get(header::SET_COOKIE).is_none(),
            "мини-приложение не должно получать куку: {:?}",
            headers.get(header::SET_COOKIE)
        );
        let token = body["token"].as_str().unwrap().to_string();
        let bearer = format!("Bearer {token}");
        let (status, _, me) = send(
            &state,
            Method::GET,
            "/api/client/me",
            &[("authorization", &bearer)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{me}");
        assert_eq!(me["channel"], "miniapp");
        let cookie = format!("sn_client={token}");
        let (status, _, _) = send(
            &state,
            Method::GET,
            "/api/client/me",
            &[("cookie", &cookie)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn кука_кабинета_не_закрывает_обращение_по_запросу_с_чужого_сайта() {
        let Some((db, state)) = state().await else {
            return;
        };
        let (client, token) = cabinet_client(&state).await;
        let ticket = open_ticket(&state, client).await;
        let path = format!("/api/client/support/tickets/{}/close", ticket.0);
        let cookie = format!("sn_client={token}");
        for headers in [
            vec![
                ("cookie", cookie.as_str()),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
            vec![("cookie", cookie.as_str()), ("origin", "null")],
            vec![
                ("cookie", cookie.as_str()),
                ("origin", "https://shop.example.com"),
                ("sec-fetch-site", "same-site"),
            ],
        ] {
            let (status, _, body) = send(&state, Method::POST, &path, &headers, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}: {body}");
            assert_eq!(body["error"]["code"], "auth.cross_site");
            assert_eq!(
                ticket_status(&state, client, ticket).await,
                TicketStatus::Open
            );
        }
        let (status, _, body) = send(
            &state,
            Method::POST,
            &path,
            &[
                ("cookie", &cookie),
                ("origin", PANEL),
                ("sec-fetch-site", "same-origin"),
            ],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            ticket_status(&state, client, ticket).await,
            TicketStatus::Closed
        );
        let second = open_ticket(&state, client).await;
        let bearer = format!("Bearer {token}");
        let (status, _, body) = send(
            &state,
            Method::POST,
            &format!("/api/client/support/tickets/{}/close", second.0),
            &[
                ("authorization", &bearer),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _, body) = send(
            &state,
            Method::GET,
            "/api/client/support/tickets",
            &[("cookie", &cookie), ("sec-fetch-site", "cross-site")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn выход_по_куке_с_чужого_сайта_не_закрывает_сессию() {
        let Some((db, state)) = state().await else {
            return;
        };
        let (_, token) = cabinet_client(&state).await;
        let cookie = format!("sn_client={token}");
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/client/auth/logout",
            &[
                ("cookie", &cookie),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(state.services.auth().client_session(&token).await.is_ok());
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/client/auth/logout",
            &[("cookie", &cookie), ("sec-fetch-site", "same-origin")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(state.services.auth().client_session(&token).await.is_err());
        db.cleanup().await;
    }

    #[tokio::test]
    async fn кука_админа_не_работает_для_изменений_с_чужого_сайта() {
        let Some((db, state)) = state().await else {
            return;
        };
        state
            .services
            .auth()
            .upsert_owner("owner@example.com", PASSWORD)
            .await
            .unwrap();
        let login = state
            .services
            .auth()
            .admin_login("owner@example.com", PASSWORD, None, &RequestMeta::default())
            .await
            .unwrap();
        let cookie = format!("sn_admin={}", login.token);
        let (status, _, body) = send(
            &state,
            Method::POST,
            "/api/admin/auth/2fa/setup",
            &[
                ("cookie", &cookie),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"]["code"], "auth.cross_site");
        let (status, _, body) = send(
            &state,
            Method::POST,
            "/api/admin/auth/2fa/setup",
            &[("cookie", &cookie), ("sec-fetch-site", "same-origin")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _, body) = send(
            &state,
            Method::GET,
            "/api/admin/me",
            &[("cookie", &cookie), ("sec-fetch-site", "cross-site")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        db.cleanup().await;
    }
    #[tokio::test]
    async fn ии_чат_виден_сайту_и_клиент_зовёт_оператора() {
        let Some((db, state)) = promo_state().await else {
            return;
        };
        state
            .services
            .modules()
            .apply_rows(&[sn_store::ModuleStateRow {
                module: "ai".into(),
                enabled: true,
                config: json!({ "api_key": "gsk_server_test", "miniapp": false }),
                updated_at: chrono::Utc::now(),
            }]);
        let (status, _, site) = send(&state, Method::GET, "/api/public/site", &[], None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            site["support_ai"],
            json!({ "cabinet": true, "miniapp": false })
        );
        let (_, token) = cabinet_client(&state).await;
        let bearer = format!("Bearer {token}");
        let (status, _, body) = send(
            &state,
            Method::POST,
            "/api/client/support/tickets",
            &[("authorization", &bearer)],
            Some(json!({ "body": "Не подключается ноутбук" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["ticket_id"].as_i64().unwrap();
        let path = format!("/api/client/support/tickets/{id}");
        let (_, _, thread) = send(
            &state,
            Method::GET,
            &path,
            &[("authorization", &bearer)],
            None,
        )
        .await;
        assert_eq!(thread["ai"]["state"], "active", "{thread}");
        let (status, _, thread) = send(
            &state,
            Method::POST,
            &format!("{path}/operator"),
            &[("authorization", &bearer)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{thread}");
        assert_eq!(thread["ai"]["state"], "handed_off");
        assert!(thread["ai"].get("reason").is_none(), "{thread}");
        let full = state
            .services
            .support()
            .thread(TicketId(id), Actor::System)
            .await
            .unwrap();
        assert_eq!(
            full.ai.and_then(|a| a.reason),
            Some(sn_domain::support::HandoffReason::Human)
        );
        db.cleanup().await;
    }
}
