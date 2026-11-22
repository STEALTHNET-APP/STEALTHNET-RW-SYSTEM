use super::llm::{self, Llm, ModelOption, shortlist};
use super::models::{Catalog, Listing, Loaded};
use super::provider::Provider;
use super::store::{self, LastError, Usage};
use super::{Settings, day_start};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use crate::module::ModuleConfig;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/overview", get(overview))
        .with_state(ctx)
}

pub fn setup(ctx: ModuleContext) -> Router {
    Router::new().route("/models", post(models)).with_state(ctx)
}

#[derive(Serialize)]
struct Overview {
    ready: bool,
    provider: &'static str,
    model: String,
    today: Usage,
    daily_tokens: i64,
    client_daily: i64,
    last_error: Option<LastError>,
}

async fn overview(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<Overview>, WebError> {
    admin.require(Permission::Modules)?;
    let settings = Settings::from_config(&ctx.core.module_config(ModuleId::Ai));
    let since = day_start(ctx.core.now(), ctx.core.core_settings().remna_offset());
    let mut conn = ctx.store.conn().await?;
    Ok(Json(Overview {
        ready: settings.ready(),
        provider: settings.provider.label(),
        model: settings.model.clone(),
        today: store::usage(&mut conn, since).await?,
        daily_tokens: settings.daily_tokens,
        client_daily: settings.client_daily,
        last_error: store::last_error(&mut conn).await?,
    }))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ModelsRequest {
    pub provider: Option<String>,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub proxy_url: Option<String>,
    pub refresh: bool,
}

#[derive(Serialize)]
struct ModelsView {
    ok: bool,
    cached: bool,
    provider: &'static str,
    model: String,
    model_found: bool,
    models: Vec<ModelOption>,
    error: Option<String>,
}

impl ModelsView {
    fn failed(provider: &'static str, model: String, cached: bool, error: String) -> Self {
        ModelsView {
            ok: false,
            cached,
            provider,
            model,
            model_found: false,
            models: Vec::new(),
            error: Some(error),
        }
    }
}

fn filled(value: Option<&String>) -> Option<&str> {
    value.map(|v| v.trim()).filter(|v| !v.is_empty())
}

pub(super) fn form_settings(saved: &ModuleConfig, request: &ModelsRequest) -> Settings {
    let mut config = saved.clone();
    for (key, value) in [
        ("provider", &request.provider),
        ("api_url", &request.api_url),
        ("model", &request.model),
        ("proxy_url", &request.proxy_url),
    ] {
        if let Some(value) = value {
            config.0.insert(key.into(), serde_json::json!(value.trim()));
        }
    }
    Settings::from_config(&config)
}

pub(super) fn check_key(
    saved: &Settings,
    form: &Settings,
    typed: Option<&str>,
) -> Result<String, (&'static str, &'static str)> {
    match typed {
        Some(key) => Ok(key.to_string()),
        None if saved.api_key.trim().is_empty() => Err(("ai.no_key", "Сначала вставьте API-ключ")),
        None if !saved.same_address(form) => Err((
            "ai.key_needed",
            "Для нового провайдера или адреса API вставьте ключ: сохранённый ключ туда не отправляется",
        )),
        None => Ok(saved.api_key.clone()),
    }
}

async fn models(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(request): Json<ModelsRequest>,
) -> Result<Json<ModelsView>, WebError> {
    admin.require(Permission::Modules)?;
    let config = ctx.core.module_config(ModuleId::Ai);
    let saved = Settings::from_config(&config);
    let form = form_settings(&config, &request);
    if request
        .provider
        .as_deref()
        .is_some_and(|p| Provider::parse(p).is_none())
    {
        return Err(WebError::rejected(
            "ai.provider",
            "Выберите провайдера из списка",
        ));
    }
    let key = check_key(&saved, &form, filled(request.api_key.as_ref()))
        .map_err(|(code, text)| WebError::rejected(code, text))?;
    if form.api_url.is_empty() {
        return Err(WebError::rejected("ai.no_url", "Укажите адрес API"));
    }
    if !saved.same_address(&form)
        && let Err(problem) = llm::check_address(&form.api_url, true)
    {
        return Err(WebError::rejected(
            "ai.bad_url",
            format!("Адрес API не подходит: {problem}"),
        ));
    }
    let provider = form.provider.label();
    let llm = match Llm::new(form.provider, &form.api_url, &key, form.proxy.as_deref()) {
        Ok(llm) => llm,
        Err(err) => {
            return Ok(Json(ModelsView::failed(
                provider,
                form.model,
                false,
                err.to_string(),
            )));
        }
    };
    let (listing, cached) = match Catalog::shared()
        .load(admin.0.admin_id, &llm, request.refresh)
        .await
    {
        Loaded::Fresh(listing) => (listing, false),
        Loaded::Cached(listing) => (listing, true),
        Loaded::Limited => {
            return Err(WebError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "ai.models_rate",
                "Список моделей запрашивали слишком часто: подождите минуту и нажмите «Обновить список»",
            ));
        }
    };
    Ok(Json(match listing {
        Listing::Models(models) => ModelsView {
            ok: true,
            cached,
            provider,
            model_found: models.iter().any(|m| m.id == form.model),
            models: shortlist(models),
            model: form.model,
            error: None,
        },
        Listing::Failed(error) => ModelsView::failed(provider, form.model, cached, error),
    }))
}
