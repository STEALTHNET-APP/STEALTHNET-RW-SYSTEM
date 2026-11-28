use super::page::{Form, page};
use super::{
    Audience, AutoReach, Broadcast, BroadcastError, ButtonSpec, Channel, Draft, IMAGE_MAX_BYTES,
    ImageMeta, ListItem, MediaKind, MediaPart, Options, Reach, ReachQuery, Rule, RuleDraft,
    RuleItem, TestReport, TestTarget, Unsubscribe, UnsubscribeView, VIDEO_MAX_BYTES, Wanted,
};
use crate::catalog::web::{Admin, WebError};
use crate::context::{ModuleContext, RateLimit};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sn_domain::access::Permission;
use sn_domain::client::Locale;
use sn_domain::ids::ClientId;
use sn_domain::lang;
use sn_domain::paging::{Page, PageRequest};
use sn_store::ClientConditions;
use uuid::Uuid;

const UPLOAD_LIMIT: usize = if IMAGE_MAX_BYTES > VIDEO_MAX_BYTES {
    IMAGE_MAX_BYTES
} else {
    VIDEO_MAX_BYTES
} + 64 * 1024;

pub const TEST_LETTERS: RateLimit = RateLimit {
    scope: "broadcast_test_letters",
    limit: 20,
    window_seconds: 3600,
};

impl From<BroadcastError> for WebError {
    fn from(err: BroadcastError) -> Self {
        match err {
            BroadcastError::Rejected { code, message } => WebError::rejected(code, message),
            BroadcastError::Module(err) => err.into(),
        }
    }
}

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/options", get(options))
        .route("/broadcasts", get(list).post(create))
        .route("/broadcasts/{id}", get(view).put(update).delete(remove))
        .route("/broadcasts/{id}/send", post(send))
        .route("/broadcasts/{id}/cancel", post(cancel))
        .route("/broadcasts/{id}/copy", post(copy))
        .route("/audience", post(audience))
        .route("/test", post(test))
        .route(
            "/images",
            post(upload).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route("/images/{id}", get(image))
        .route("/auto", get(auto_list).post(auto_create))
        .route("/auto/reach", post(auto_reach))
        .route("/auto/presets/enable", post(auto_presets))
        .route(
            "/auto/{id}",
            get(auto_view).put(auto_update).delete(auto_remove),
        )
        .route("/auto/{id}/switch", post(auto_switch))
        .with_state(ctx)
}

fn money_conditions(admin: &Admin, audience: Option<&ClientConditions>) -> Result<(), WebError> {
    if audience.is_some_and(ClientConditions::uses_money) && !admin.sees_money() {
        return Err(WebError::new(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "Условия про оплаты и баланс доступны с правом «Деньги»",
        ));
    }
    Ok(())
}

fn money_allowed(admin: &Admin, audience: &Audience) -> Result<(), WebError> {
    if audience.uses_money() && !admin.sees_money() {
        return Err(WebError::new(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "Условия про оплаты и баланс доступны с правом «Деньги»",
        ));
    }
    Ok(())
}

fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, WebError> {
    serde_json::from_slice(body).map_err(|err| {
        tracing::debug!(error = %err, "рассылки: тело запроса не разобрано");
        WebError::rejected(
            "validation.invalid_value",
            "Запрос не разобран: обновите страницу и попробуйте ещё раз",
        )
    })
}

async fn options(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<Options>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::options(&ctx).await?))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn list(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<ListItem>>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::list(&ctx, PageRequest::new(query.limit, query.after)).await?,
    ))
}

async fn create(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    body: Bytes,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    let draft: Draft = parse(&body)?;
    money_allowed(&admin, &draft.audience)?;
    Ok(Json(
        super::create(&ctx, &draft, Some(admin.0.admin_id)).await?,
    ))
}

async fn view(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::view(&ctx, id).await?))
}

async fn update(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    let draft: Draft = parse(&body)?;
    money_allowed(&admin, &draft.audience)?;
    Ok(Json(super::update(&ctx, id, &draft).await?))
}

async fn remove(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::Marketing)?;
    super::delete(&ctx, id).await?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

#[derive(Deserialize)]
struct SendBody {
    #[serde(default)]
    at: Option<DateTime<Utc>>,
}

async fn send(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    let body: SendBody = if body.is_empty() {
        SendBody { at: None }
    } else {
        parse(&body)?
    };
    Ok(Json(
        super::start(&ctx, id, body.at, Some(admin.0.admin_id)).await?,
    ))
}

async fn cancel(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::cancel(&ctx, id, Some(admin.0.admin_id)).await?))
}

async fn copy(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::copy(&ctx, id, Some(admin.0.admin_id)).await?))
}

#[derive(serde::Serialize)]
struct RuleList {
    items: Vec<RuleItem>,
}

async fn auto_list(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<RuleList>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(RuleList {
        items: super::rules(&ctx).await?,
    }))
}

async fn auto_create(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    body: Bytes,
) -> Result<Json<Rule>, WebError> {
    admin.require(Permission::Marketing)?;
    let draft: RuleDraft = parse(&body)?;
    money_conditions(&admin, draft.audience.as_ref())?;
    Ok(Json(
        super::create_rule(&ctx, &draft, Some(admin.0.admin_id)).await?,
    ))
}

async fn auto_view(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Rule>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::rule(&ctx, id).await?))
}

async fn auto_update(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Rule>, WebError> {
    admin.require(Permission::Marketing)?;
    let draft: RuleDraft = parse(&body)?;
    money_conditions(&admin, draft.audience.as_ref())?;
    Ok(Json(
        super::update_rule(&ctx, id, &draft, Some(admin.0.admin_id)).await?,
    ))
}

async fn auto_remove(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::Marketing)?;
    super::delete_rule(&ctx, id, Some(admin.0.admin_id)).await?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

#[derive(Deserialize)]
struct SwitchBody {
    enabled: bool,
}

async fn auto_switch(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Rule>, WebError> {
    admin.require(Permission::Marketing)?;
    let body: SwitchBody = parse(&body)?;
    Ok(Json(
        super::switch_rule(&ctx, id, body.enabled, Some(admin.0.admin_id)).await?,
    ))
}

async fn auto_presets(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::Marketing)?;
    let enabled = super::enable_presets(&ctx, Some(admin.0.admin_id)).await?;
    Ok(Json(serde_json::json!({ "enabled": enabled })))
}

async fn auto_reach(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    body: Bytes,
) -> Result<Json<AutoReach>, WebError> {
    admin.require(Permission::Marketing)?;
    let query: ReachQuery = parse(&body)?;
    money_conditions(&admin, query.audience.as_ref())?;
    Ok(Json(super::rule_reach(&ctx, &query).await?))
}

#[derive(Deserialize)]
struct AudienceBody {
    audience: Audience,
}

async fn audience(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    body: Bytes,
) -> Result<Json<Reach>, WebError> {
    admin.require(Permission::Marketing)?;
    let body: AudienceBody = parse(&body)?;
    money_allowed(&admin, &body.audience)?;
    Ok(Json(super::reach(&ctx, body.audience).await?))
}

#[derive(Deserialize)]
struct TestBody {
    #[serde(default)]
    client_id: Option<ClientId>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    image_id: Option<i64>,
    #[serde(default)]
    buttons: Vec<ButtonSpec>,
}

async fn test(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    body: Bytes,
) -> Result<Json<TestReport>, WebError> {
    admin.require(Permission::Marketing)?;
    let body: TestBody = parse(&body)?;
    let target = match (body.email, body.client_id) {
        (Some(email), _) => TestTarget::Address(email),
        (None, Some(client)) => TestTarget::Client(client),
        (None, None) => {
            return Err(WebError::rejected(
                "validation.invalid_value",
                "Выберите клиента или укажите адрес для проверки",
            ));
        }
    };
    if matches!(target, TestTarget::Address(_))
        && !ctx
            .core
            .within_rate_limit(TEST_LETTERS, &admin.0.admin_id.0.to_string())
            .await?
    {
        return Err(WebError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "broadcast.rate_limited",
            "Проверочных писем на адрес — не больше 20 в час. Попробуйте позже или проверьте рассылку на клиенте в боте",
        ));
    }
    let draft = Draft {
        channel: match target {
            TestTarget::Address(_) => Channel::Email,
            TestTarget::Client(_) => Channel::Bot,
        },
        subject: body.subject,
        text: body.text,
        image_id: body.image_id,
        buttons: body.buttons,
        audience: Audience::All,
    };
    Ok(Json(
        super::send_test(&ctx, &draft, target, Some(admin.0.admin_id)).await?,
    ))
}

async fn upload(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ImageMeta>, WebError> {
    admin.require(Permission::Marketing)?;
    let declared = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let refused = |message: &str| {
        WebError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "broadcast.image",
            message.to_string(),
        )
    };
    let uploaded = match declared.as_str() {
        "image/png" | "image/jpeg" => {
            super::upload_image(&ctx, &body, Some(admin.0.admin_id)).await?
        }
        "video/mp4" => super::upload_video(&ctx, &body, Some(admin.0.admin_id)).await?,
        "video/quicktime" => {
            return Err(refused(
                "Это ролик MOV, а видео с проигрывателем Telegram показывает из MP4. Сохраните ролик в MP4 и загрузите снова",
            ));
        }
        _ => return Err(refused("Нужна картинка JPEG или PNG или видео MP4")),
    };
    Ok(Json(uploaded))
}

async fn image(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    admin.require(Permission::Marketing)?;
    let wanted = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(Wanted::parse);
    let (status, content_type, range, bytes, video) =
        match super::media_part(&ctx, id, wanted).await? {
            MediaPart::Whole {
                content_type,
                kind,
                bytes,
            } => (
                StatusCode::OK,
                content_type,
                None,
                bytes,
                kind == MediaKind::Video,
            ),
            MediaPart::Part {
                content_type,
                start,
                end,
                total,
                bytes,
            } => (
                StatusCode::PARTIAL_CONTENT,
                content_type,
                Some(format!("bytes {start}-{end}/{total}")),
                bytes,
                true,
            ),
            MediaPart::Outside { total } => {
                let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                if let Ok(value) = HeaderValue::from_str(&format!("bytes */{total}")) {
                    response.headers_mut().insert(header::CONTENT_RANGE, value);
                }
                return Ok(response);
            }
        };
    let mut response = (status, bytes).into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Some(range) = range.and_then(|range| HeaderValue::from_str(&range).ok()) {
        headers.insert(header::CONTENT_RANGE, range);
    }
    if video {
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400, immutable"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    Ok(response)
}

pub fn public(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/images/{key}", get(public_image))
        .route(
            "/unsubscribe/{token}",
            get(unsubscribe_page).post(unsubscribe_now),
        )
        .route("/resubscribe/{token}", post(resubscribe_now))
        .with_state(ctx)
}

async fn public_image(
    State(ctx): State<ModuleContext>,
    Path(key): Path<Uuid>,
) -> Result<Response, WebError> {
    let Some((content_type, bytes)) = super::public_image(&ctx, key).await? else {
        return Err(WebError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Картинка не найдена",
        ));
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    Ok(response)
}

fn unsubscribe_html(view: &UnsubscribeView, token: Uuid) -> (StatusCode, String) {
    let email = view.email.as_deref().unwrap_or_default();
    let unsubscribe = format!("/api/public/m/broadcast/unsubscribe/{token}");
    let resubscribe = format!("/api/public/m/broadcast/resubscribe/{token}");
    let english = lang::current() == Locale::En;
    match view.state {
        Unsubscribe::Unknown => (
            StatusCode::NOT_FOUND,
            page(
                &view.brand,
                lang::pick("Ссылка не найдена", "Link not found"),
                lang::pick(
                    "Возможно, она устарела. Отписаться можно по ссылке из любого нового письма с рассылкой.",
                    "It may be out of date. You can unsubscribe with the link in any new newsletter email.",
                ),
                None,
            ),
        ),
        Unsubscribe::Preview => (
            StatusCode::OK,
            page(
                &view.brand,
                lang::pick("Это проверочное письмо", "This is a test email"),
                lang::pick(
                    "В настоящей рассылке по этой ссылке получатель сможет отписаться от писем.",
                    "In a real newsletter, this link lets the recipient unsubscribe.",
                ),
                None,
            ),
        ),
        Unsubscribe::Asked => (
            StatusCode::OK,
            page(
                &view.brand,
                lang::pick("Отписаться от рассылок?", "Unsubscribe from newsletters?"),
                &if english {
                    format!(
                        "News and offers won't be sent to {email} anymore. Service emails, such as sign-in codes, will keep coming."
                    )
                } else {
                    format!(
                        "Письма с новостями и предложениями на {email} больше не придут. Служебные письма, например коды входа, будут приходить как раньше."
                    )
                },
                Some(Form {
                    action: &unsubscribe,
                    button: lang::pick("Отписаться", "Unsubscribe"),
                    quiet: false,
                }),
            ),
        ),
        Unsubscribe::Done => (
            StatusCode::OK,
            page(
                &view.brand,
                lang::pick("Вы отписаны", "You're unsubscribed"),
                &if english {
                    format!("Newsletters won't be sent to {email} anymore.")
                } else {
                    format!("Рассылки на {email} больше не придут.")
                },
                Some(Form {
                    action: &resubscribe,
                    button: lang::pick("Вернуть рассылки", "Subscribe again"),
                    quiet: true,
                }),
            ),
        ),
        Unsubscribe::Restored => (
            StatusCode::OK,
            page(
                &view.brand,
                lang::pick("Рассылки вернутся", "Newsletters are back"),
                &if english {
                    format!("News emails will come to {email} again.")
                } else {
                    format!("Письма с новостями снова будут приходить на {email}.")
                },
                None,
            ),
        ),
    }
}

fn rendered(view: &UnsubscribeView, token: Uuid) -> Response {
    let (status, html) = lang::sync_scope(view.locale, || unsubscribe_html(view, token));
    let mut response = (status, Html(html)).into_response();
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    response
}

async fn unsubscribe_page(
    State(ctx): State<ModuleContext>,
    Path(token): Path<Uuid>,
) -> Result<Response, WebError> {
    let view = super::unsubscribe_view(&ctx, token).await?;
    Ok(rendered(&view, token))
}

async fn unsubscribe_now(
    State(ctx): State<ModuleContext>,
    Path(token): Path<Uuid>,
) -> Result<Response, WebError> {
    let view = super::unsubscribe(&ctx, token).await?;
    Ok(rendered(&view, token))
}

async fn resubscribe_now(
    State(ctx): State<ModuleContext>,
    Path(token): Path<Uuid>,
) -> Result<Response, WebError> {
    let view = super::resubscribe(&ctx, token).await?;
    Ok(rendered(&view, token))
}
