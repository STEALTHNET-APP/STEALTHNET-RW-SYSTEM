use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::{EnvFallbacks, RemnaConnection, Services};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sn_domain::cabinet_css::CabinetCss;
use sn_domain::client::Languages;
use sn_domain::format;
use sn_domain::ids::AdminId;
use sn_domain::landing::LandingConfig;
use sn_domain::ledger::Wallet;
use sn_domain::miniapp::MiniAppConfig;
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money};
use sn_domain::payment::{
    ButtonLabel, LABEL_HINT_MAX, METHOD_TITLE_MAX, MethodChoice, PaymentChannels, TopupBonus,
    providers,
};
use sn_domain::pricing::TOPUP_BONUS_MAX;
use sn_domain::settings::{
    Appearance, AutoRenewSettings, ClientAccess, ConnectSettings, CoreSettings, FieldKind,
    ImageMeta, MultiSubscriptions, PaymentOrder, SettingField, SiteSettings, SupportMode,
    TRIAL_NAME_MAX, TRIAL_VARIANTS_MAX, TopUpSettings, TrialSettings, TrialVariant, keys,
    secret_keys,
};
use sn_modules::catalog::antibot::{AntibotSettings, captcha};
use sn_modules::{ModuleConfig, ModuleView};
use sn_payments::providers::STARS_TOKEN_FIELD;
use sn_payments::{
    CheckContext, CheckRecord, CheckStatus, ConnectionCheck, InboxTrace, PaymentProvider,
    ProviderConfig, ProviderKind, ProviderRegistry, ProviderState, ProviderView, RegistrySnapshot,
};
use sn_remnawave::client::{normalize_api_key, normalize_cookie};
use sn_remnawave::{
    CustomRemarks, HwidSettings, RemnaError, RemnaResult, RemnawaveApi, RemnawaveClient,
    SubscriptionSettings, UpdateSubscriptionSettingsRequest,
};
use sn_store::{Db, ModuleStateRow, SecretBox};
use std::collections::HashMap;
use std::sync::Arc;

pub const SETTINGS_CHANNEL: &str = "settings_changed";
pub const REMNA_WEBHOOK_PATH: &str = "/api/remnawave/webhook";
pub const MAX_TEXT_FIELD: usize = 2_000;
pub const MAX_LARGE_FIELD: usize = 20_000;
pub const MAX_REMARK_CHARS: usize = 200;
pub const PROVIDER_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);
pub const CHECK_UNCONFIGURED: &str = "settings.provider_check_unconfigured";
pub const SECRETS_AGAIN: &str = "settings.secrets_again";
pub const TOPUP_BONUS_LIMIT: i64 = 100_000_000;
pub const INBOX_TRACE_DAYS: i64 = 30;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderSettings {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: serde_json::Map<String, Value>,
    #[serde(default)]
    pub channels: PaymentChannels,
    #[serde(default)]
    pub methods: Vec<MethodChoice>,
    #[serde(default)]
    pub label: ButtonLabel,
    #[serde(default)]
    pub topup_bonus: TopupBonus,
}

fn parse<T: serde::de::DeserializeOwned>(key: &str, value: Value) -> Option<T> {
    match serde_json::from_value(value) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            tracing::warn!(key, error = %err, "настройка не разобрана, взято значение по умолчанию");
            None
        }
    }
}

pub async fn load_core(db: &mut impl Db) -> ServiceResult<CoreSettings> {
    let rows = db.settings().with_prefix("").await?;
    let mut settings = CoreSettings::default();
    let mut topup: Option<TopUpSettings> = None;
    let mut support_mode: Option<SupportMode> = None;
    for (key, value) in rows {
        match key.as_str() {
            keys::CURRENCY => {
                if let Some(currency) = parse::<Currency>(&key, value) {
                    settings.currency = currency;
                }
            }
            keys::BRAND_NAME => {
                if let Some(name) = parse::<String>(&key, value) {
                    settings.brand_name = name;
                }
            }
            keys::PUBLIC_URL => settings.public_url = parse(&key, value),
            keys::BOT_UPDATES => {
                if let Some(v) = parse::<sn_domain::settings::BotDelivery>(&key, value) {
                    settings.bot_updates = v;
                }
            }
            keys::BOT_FLOOD => {
                if let Some(v) = parse::<sn_domain::settings::FloodSettings>(&key, value) {
                    settings.bot_flood = v;
                }
            }
            keys::SUPPORT_URL => settings.support_url = parse(&key, value),
            keys::SUPPORT_MODE => support_mode = parse(&key, value),
            keys::REMNA_UTC_OFFSET => {
                if let Some(minutes) = parse::<i32>(&key, value) {
                    settings.remna_utc_offset_minutes = minutes;
                }
            }
            keys::AUTORENEW => {
                if let Some(v) = parse::<AutoRenewSettings>(&key, value) {
                    settings.autorenew = v;
                }
            }
            keys::TRIAL => {
                if let Some(v) = parse::<TrialSettings>(&key, value) {
                    settings.trial = v;
                }
            }
            keys::TOPUP => topup = parse(&key, value),
            keys::PAYMENT_ORDER => {
                if let Some(v) = parse::<PaymentOrder>(&key, value) {
                    settings.payment_order = v;
                }
            }
            keys::INVOICE_TTL => {
                if let Some(v) = parse::<u32>(&key, value) {
                    settings.invoice_ttl_minutes = v.clamp(5, 24 * 60);
                }
            }
            keys::APPEARANCE => {
                if let Some(v) = parse::<Appearance>(&key, value) {
                    settings.appearance = v;
                }
            }
            keys::CONNECT => {
                if let Some(v) = parse::<ConnectSettings>(&key, value) {
                    settings.connect = v;
                }
            }
            keys::CONNECT_APPS => {
                settings.connect_apps = parse::<sn_domain::connect::ConnectApps>(&key, value)
                    .and_then(|apps| match apps.checked() {
                        Ok(apps) => Some(apps),
                        Err(err) => {
                            tracing::warn!(key, error = %err, "приложения для подключения не прошли проверку, взяты по умолчанию");
                            None
                        }
                    });
            }
            keys::SITE => {
                if let Some(v) = parse::<SiteSettings>(&key, value) {
                    settings.site = v;
                }
            }
            keys::CLIENT_ACCESS => {
                if let Some(v) = parse::<ClientAccess>(&key, value) {
                    settings.client_access = v;
                }
            }
            keys::MULTI_SUBSCRIPTIONS => {
                if let Some(v) = parse::<MultiSubscriptions>(&key, value) {
                    settings.multi_subscriptions = v.enabled;
                }
            }
            keys::LOGO => settings.logo = parse::<ImageMeta>(&key, value),
            keys::FAVICON => settings.favicon = parse::<ImageMeta>(&key, value),
            keys::LANDING => {
                if let Some(v) = parse::<LandingConfig>(&key, value) {
                    settings.landing = v.arranged();
                }
            }
            keys::MINIAPP => match MiniAppConfig::stored(value) {
                Some(v) => settings.miniapp = v,
                None => {
                    tracing::warn!(key, "настройка не разобрана, взято значение по умолчанию");
                }
            },
            keys::CABINET_CSS => {
                if let Some(v) = parse::<CabinetCss>(&key, value) {
                    settings.cabinet_css = v;
                }
            }
            keys::LANGUAGES => {
                if let Some(v) = parse::<Languages>(&key, value) {
                    settings.languages = v;
                }
            }
            _ => {}
        }
    }
    settings.topup = topup.unwrap_or_else(|| TopUpSettings::default_for(settings.currency));
    settings.support_mode = support_mode.unwrap_or(if settings.support_url.is_some() {
        SupportMode::Both
    } else {
        SupportMode::Tickets
    });
    Ok(settings)
}

pub async fn read_secret(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    key: &str,
) -> ServiceResult<Option<String>> {
    let Some(sealed) = db.secrets().get(key).await? else {
        return Ok(None);
    };
    let Some(secrets) = secrets else {
        tracing::warn!(key, "секрет есть в базе, но SN_SECRET_KEY не задан");
        return Ok(None);
    };
    match secrets.open_string(key, &sealed) {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            tracing::error!(key, error = %err, "секрет не расшифрован: SN_SECRET_KEY сменился?");
            Ok(None)
        }
    }
}

pub async fn write_secret(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    key: &str,
    value: Option<&str>,
    by: Option<AdminId>,
) -> ServiceResult<()> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => {
            let secrets = secrets.ok_or(ServiceError::NoSecretKey)?;
            let sealed = secrets.seal_str(key, value)?;
            db.secrets().set(key, &sealed, by).await?;
        }
        None => {
            db.secrets().delete(key).await?;
        }
    }
    Ok(())
}

pub async fn secret_or(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    key: &str,
    fallback: Option<String>,
) -> ServiceResult<Option<String>> {
    Ok(read_secret(db, secrets, key).await?.or(fallback))
}

pub async fn remna_connection(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    env: &EnvFallbacks,
) -> ServiceResult<RemnaConnection> {
    let fallback = env.remna.clone().unwrap_or_default();
    let guard = stored_guard(db, secrets, &fallback).await?;
    let url = db
        .settings()
        .get_as::<String>(keys::REMNA_URL)
        .await?
        .filter(|u| !u.trim().is_empty())
        .unwrap_or(fallback.url);
    let token = read_secret(db, secrets, secret_keys::REMNA_TOKEN)
        .await?
        .unwrap_or(fallback.token);
    let webhook_secret = secret_or(
        db,
        secrets,
        secret_keys::REMNA_WEBHOOK_SECRET,
        fallback.webhook_secret,
    )
    .await?;
    Ok(RemnaConnection {
        url,
        token,
        cookie: guard.cookie,
        api_key: guard.api_key,
        webhook_secret,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelGuard {
    #[default]
    #[serde(rename = "none")]
    Off,
    Cookie,
    Tinyauth,
    Portal,
}

impl PanelGuard {
    pub fn of(cookie: Option<&str>, api_key: Option<&str>) -> Self {
        match (cookie, api_key) {
            (_, Some(key)) if basic_key(key) => PanelGuard::Tinyauth,
            (_, Some(_)) => PanelGuard::Portal,
            (Some(_), None) => PanelGuard::Cookie,
            (None, None) => PanelGuard::Off,
        }
    }
}

fn basic_key(key: &str) -> bool {
    key.get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("basic "))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredGuard {
    pub mode: PanelGuard,
    pub cookie: Option<String>,
    pub api_key: Option<String>,
    pub from_env: bool,
}

pub async fn stored_guard(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    env: &RemnaConnection,
) -> ServiceResult<StoredGuard> {
    let chosen = db
        .settings()
        .get_as::<PanelGuard>(keys::REMNA_GUARD)
        .await?;
    let saved_cookie = read_secret(db, secrets, secret_keys::REMNA_COOKIE).await?;
    let saved_key = read_secret(db, secrets, secret_keys::REMNA_API_KEY).await?;
    let cookie_env = saved_cookie.is_none() && env.cookie.is_some();
    let key_env = saved_key.is_none() && env.api_key.is_some();
    let cookie = saved_cookie.or_else(|| env.cookie.clone());
    let api_key = saved_key.or_else(|| env.api_key.clone());
    let (cookie, api_key) = match chosen {
        None => (cookie, api_key),
        Some(PanelGuard::Off) => (None, None),
        Some(PanelGuard::Cookie) => (cookie, None),
        Some(PanelGuard::Tinyauth | PanelGuard::Portal) => (None, api_key),
    };
    let mode = chosen.unwrap_or_else(|| PanelGuard::of(cookie.as_deref(), api_key.as_deref()));
    let from_env = match mode {
        PanelGuard::Off => false,
        PanelGuard::Cookie => cookie_env && cookie.is_some(),
        PanelGuard::Tinyauth | PanelGuard::Portal => key_env && api_key.is_some(),
    };
    Ok(StoredGuard {
        mode,
        cookie,
        api_key,
        from_env,
    })
}

const COOKIE_FIELD: &str = "Кука";
const LOGIN_FIELD: &str = "Логин TinyAuth";
const PASSWORD_FIELD: &str = "Пароль TinyAuth";
const KEY_FIELD: &str = "Ключ портала";
const GUARD_TEXT_MAX: usize = 200;

pub fn tinyauth_key(login: &str, password: &str) -> ServiceResult<String> {
    let (login, password) = (login.trim(), password.trim());
    if login.is_empty()
        || login.contains(':')
        || login.chars().any(char::is_control)
        || login.chars().count() > GUARD_TEXT_MAX
    {
        return Err(field_error(
            LOGIN_FIELD,
            "логин без двоеточия и переносов строк, до 200 символов",
        ));
    }
    if password.is_empty()
        || password.chars().any(char::is_control)
        || password.chars().count() > GUARD_TEXT_MAX
    {
        return Err(field_error(
            PASSWORD_FIELD,
            "пароль без переносов строк, до 200 символов",
        ));
    }
    let pair = format!("{login}:{password}");
    Ok(format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(pair.as_bytes())
    ))
}

enum GuardSecret {
    Keep,
    Set(String),
    Clear,
}

impl GuardSecret {
    fn apply(self, current: Option<String>) -> Option<String> {
        match self {
            GuardSecret::Keep => current,
            GuardSecret::Set(value) => Some(value),
            GuardSecret::Clear => None,
        }
    }
}

struct GuardChange {
    cookie: GuardSecret,
    api_key: GuardSecret,
}

const TOKEN_NAME: &str = "API-токен";
const COOKIE_NAME: &str = "куку";

const fn key_name(mode: PanelGuard) -> &'static str {
    match mode {
        PanelGuard::Tinyauth => "логин и пароль TinyAuth",
        _ => "ключ портала",
    }
}

impl GuardChange {
    fn kept(&self, current: &StoredGuard) -> Vec<&'static str> {
        let mut kept = Vec::new();
        if matches!(self.cookie, GuardSecret::Keep) && current.cookie.is_some() {
            kept.push(COOKIE_NAME);
        }
        if matches!(self.api_key, GuardSecret::Keep) && current.api_key.is_some() {
            kept.push(key_name(current.mode));
        }
        kept
    }
}

fn joined<T: AsRef<str>>(names: &[T]) -> String {
    match names {
        [] => String::new(),
        [one] => one.as_ref().to_string(),
        [rest @ .., last] => {
            let rest: Vec<&str> = rest.iter().map(AsRef::as_ref).collect();
            format!("{} и {}", rest.join(", "), last.as_ref())
        }
    }
}

fn panel_moved(before: &str, after: &str) -> bool {
    let after = after.trim().trim_end_matches('/');
    !after.is_empty() && before.trim().trim_end_matches('/') != after
}

fn panel_secrets_again(kept: &[&str]) -> String {
    format!(
        "При смене адреса панели вставьте заново {}: панель не отправляет сохранённые ключи на новый адрес",
        joined(kept)
    )
}

fn address<'a>(field: &'a SettingField, config: &'a Map<String, Value>) -> &'a str {
    config
        .get(field.key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(field.default)
        .unwrap_or_default()
        .trim_end_matches('/')
}

pub fn secrets_again(
    schema: &[SettingField],
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    saved: &[&str],
    changes: &[SecretChange],
) -> ServiceResult<()> {
    let Some(moved) = schema
        .iter()
        .find(|f| f.kind == FieldKind::Url && address(f, before) != address(f, after))
    else {
        return Ok(());
    };
    let stale: Vec<String> = schema
        .iter()
        .filter(|f| f.is_secret() && saved.contains(&f.key))
        .filter(|f| {
            !changes.iter().any(|change| {
                matches!(change, SecretChange::Set(key, _) | SecretChange::Delete(key) if key.as_str() == f.key)
            })
        })
        .map(|f| format!("«{}»", f.label))
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    Err(ServiceError::rejected(
        SECRETS_AGAIN,
        format!(
            "При смене поля «{}» вставьте заново {}: панель не отправляет сохранённые ключи на новый адрес",
            moved.label,
            joined(&stale)
        ),
    ))
}

fn guard_change(update: &GuardUpdate, current: &StoredGuard) -> ServiceResult<GuardChange> {
    let same = current.mode == update.mode();
    Ok(match update {
        GuardUpdate::Off => GuardChange {
            cookie: GuardSecret::Clear,
            api_key: GuardSecret::Clear,
        },
        GuardUpdate::Cookie { cookie } => {
            let cookie = match optional_text(cookie.as_deref()) {
                Some(raw) => GuardSecret::Set(normalize_cookie(&raw).ok_or_else(|| {
                    field_error(
                        COOKIE_FIELD,
                        "не похоже на куку. Вставьте ссылку входа из установщика eGames или пару ИМЯ=ЗНАЧЕНИЕ",
                    )
                })?),
                None if same && current.cookie.is_some() => GuardSecret::Keep,
                None => return Err(field_error(COOKIE_FIELD, "вставьте куку")),
            };
            GuardChange {
                cookie,
                api_key: GuardSecret::Clear,
            }
        }
        GuardUpdate::Tinyauth { login, password } => {
            let api_key = match (
                optional_text(login.as_deref()),
                optional_text(password.as_deref()),
            ) {
                (Some(login), Some(password)) => GuardSecret::Set(tinyauth_key(&login, &password)?),
                (None, None) if same && current.api_key.is_some() => GuardSecret::Keep,
                (None, _) => return Err(field_error(LOGIN_FIELD, "укажите логин")),
                (Some(_), None) => return Err(field_error(PASSWORD_FIELD, "укажите пароль")),
            };
            GuardChange {
                cookie: GuardSecret::Clear,
                api_key,
            }
        }
        GuardUpdate::Portal { key } => {
            let api_key = match optional_text(key.as_deref()) {
                Some(raw) => GuardSecret::Set(normalize_api_key(&raw).ok_or_else(|| {
                    field_error(KEY_FIELD, "ключ без русских букв и переносов строк")
                })?),
                None if same && current.api_key.is_some() => GuardSecret::Keep,
                None => return Err(field_error(KEY_FIELD, "вставьте ключ")),
            };
            GuardChange {
                cookie: GuardSecret::Clear,
                api_key,
            }
        }
    })
}

async fn write_change(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    key: &str,
    change: GuardSecret,
    admin: AdminId,
) -> ServiceResult<()> {
    match change {
        GuardSecret::Keep => Ok(()),
        GuardSecret::Set(value) => write_secret(db, secrets, key, Some(&value), Some(admin)).await,
        GuardSecret::Clear => write_secret(db, secrets, key, None, None).await,
    }
}

const COOKIE_HINT_NONE: &str = "Так отвечает панель, поставленная скриптом eGames, когда в запросе нет куки. Выберите «Кука» в строке «Защита панели» и вставьте ссылку входа из установщика";
const COOKIE_HINT_WRONG: &str = "Кука не подошла. Сверьте её со ссылкой входа из установщика eGames или со строкой map $http_cookie в /opt/remnawave/nginx.conf";
const COOKIE_HINT_OTHER: &str = "Так отвечает защита eGames по куке, а не страница входа. Выберите «Кука» в строке «Защита панели» и вставьте ссылку входа из установщика";
const LOGIN_HINT_NONE: &str = "Панель закрыта страницей входа. Выберите в строке «Защита панели» «Логин и пароль TinyAuth» или «Ключ портала (Caddy)»";
const LOGIN_HINT_TINYAUTH: &str =
    "Логин и пароль TinyAuth не подошли. Установщик eGames показывает их в конце установки";
const LOGIN_HINT_PORTAL: &str =
    "Ключ портала не подошёл. Создайте новый на странице входа панели, в разделе API Keys";
const PROXY_HINT_NONE: &str = "Если панель ставили скриптом eGames со страницей входа, выберите в строке «Защита панели» «Логин и пароль TinyAuth» или «Ключ портала (Caddy)»";
const TOKEN_HINT: &str = "Проверьте, что токен не удалён в панели Remnawave, раздел «API-токены»";

fn check_problem(
    err: &RemnaError,
    guard: PanelGuard,
    url: &str,
    probe: Option<&RemnaResult<()>>,
) -> (String, Option<String>) {
    let login_hint = match guard {
        PanelGuard::Tinyauth => LOGIN_HINT_TINYAUTH,
        PanelGuard::Portal => LOGIN_HINT_PORTAL,
        PanelGuard::Off | PanelGuard::Cookie => LOGIN_HINT_NONE,
    };
    match err {
        RemnaError::Dropped => {
            let hint = match guard {
                PanelGuard::Off => COOKIE_HINT_NONE,
                PanelGuard::Cookie => COOKIE_HINT_WRONG,
                PanelGuard::Tinyauth | PanelGuard::Portal => COOKIE_HINT_OTHER,
            };
            (
                "Панель закрыла соединение, не ответив".into(),
                Some(hint.into()),
            )
        }
        RemnaError::Redirected(location) if https_upgrade(url, location) => (
            "Панель перенаправляет на https".into(),
            Some("Укажите адрес панели, начиная с https://".into()),
        ),
        RemnaError::Redirected(location) => {
            let error = match redirect_host(location) {
                Some(host) => format!("Панель отправляет на страницу входа {host}"),
                None => "Панель отправляет на страницу входа".into(),
            };
            (error, Some(login_hint.into()))
        }
        RemnaError::Unauthorized(status) => match probe {
            Some(Err(RemnaError::Unauthorized(_))) => {
                let hint = match guard {
                    PanelGuard::Off | PanelGuard::Cookie => PROXY_HINT_NONE,
                    PanelGuard::Tinyauth | PanelGuard::Portal => login_hint,
                };
                (
                    format!("Защита перед панелью не пропустила запрос (HTTP {status})"),
                    Some(hint.into()),
                )
            }
            _ => (err.to_string(), Some(TOKEN_HINT.into())),
        },
        _ => (err.to_string(), None),
    }
}

fn https_upgrade(url: &str, location: &str) -> bool {
    match (reqwest::Url::parse(url), reqwest::Url::parse(location)) {
        (Ok(from), Ok(to)) => {
            from.scheme() == "http" && to.scheme() == "https" && from.host_str() == to.host_str()
        }
        _ => false,
    }
}

fn redirect_host(location: &str) -> Option<String> {
    reqwest::Url::parse(location)
        .ok()?
        .host_str()
        .map(str::to_string)
}

pub fn clean_methods(
    provider: &dyn PaymentProvider,
    methods: &[MethodChoice],
    currency: Currency,
) -> ServiceResult<Vec<MethodChoice>> {
    let available = provider.methods(currency);
    let mut cleaned: Vec<MethodChoice> = Vec::new();
    for choice in methods {
        let code = choice.code.trim();
        if !available.iter().any(|m| m.code == code) {
            return Err(ServiceError::rejected(
                "settings.method_unknown",
                format!("У {} нет способа оплаты «{code}»", provider.title()),
            ));
        }
        if cleaned.iter().any(|c| c.code == code) {
            continue;
        }
        let title = choice
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        if title
            .as_deref()
            .is_some_and(|t| t.chars().count() > METHOD_TITLE_MAX)
        {
            return Err(ServiceError::rejected(
                "settings.method_title",
                format!("Название способа оплаты — не длиннее {METHOD_TITLE_MAX} символов"),
            ));
        }
        cleaned.push(MethodChoice {
            code: code.to_string(),
            title,
        });
    }
    Ok(cleaned)
}

pub fn clean_topup_bonus(
    provider: &dyn PaymentProvider,
    update: &TopupBonusUpdate,
    currency: Currency,
) -> ServiceResult<TopupBonus> {
    if update.enabled && provider.kind() != ProviderKind::External {
        return Err(ServiceError::rejected(
            "settings.topup_bonus_kind",
            "Этим способом баланс не пополняют, бонус для него не задаётся",
        ));
    }
    let percent = u8::try_from(update.percent)
        .ok()
        .filter(|percent| *percent <= TOPUP_BONUS_MAX && (*percent > 0 || !update.enabled))
        .ok_or_else(|| {
            ServiceError::rejected(
                "settings.topup_bonus_percent",
                format!("Процент бонуса — целое число от 1 до {TOPUP_BONUS_MAX}"),
            )
        })?;
    let limit = |minor: Option<i64>, code: &'static str, title: &str| match minor {
        None => Ok(None),
        Some(minor) if (1..=TOPUP_BONUS_LIMIT).contains(&minor) => {
            Ok(Some(Money::new(minor, currency)))
        }
        Some(_) => Err(ServiceError::rejected(
            code,
            format!(
                "«{title}» — сумма больше нуля и не больше {}, пустое поле — без ограничения",
                format::money(Money::new(TOPUP_BONUS_LIMIT, currency))
            ),
        )),
    };
    let min = limit(
        update.min_minor,
        "settings.topup_bonus_min",
        "Действует от суммы пополнения",
    )?;
    let max = limit(
        update.max_minor,
        "settings.topup_bonus_max",
        "Бонус не больше",
    )?;
    let available = provider.methods(currency);
    let mut methods: Vec<String> = Vec::new();
    for code in &update.methods {
        let code = code.trim();
        if !available.iter().any(|m| m.code == code) {
            return Err(ServiceError::rejected(
                "settings.topup_bonus_methods",
                format!("У {} нет способа оплаты «{code}»", provider.title()),
            ));
        }
        if !methods.iter().any(|m| m == code) {
            methods.push(code.to_string());
        }
    }
    if methods.len() == available.len() {
        methods.clear();
    }
    Ok(TopupBonus {
        enabled: update.enabled,
        percent,
        min,
        max,
        methods,
    })
}

fn bonus_limit(limit: Option<Money>) -> Value {
    match limit {
        Some(money) => serde_json::json!(money),
        None => Value::from("без ограничения"),
    }
}

fn bonus_audit(
    provider: &dyn PaymentProvider,
    bonus: &TopupBonus,
    currency: Currency,
    details: &mut Map<String, Value>,
) {
    let titles: Vec<&str> = provider
        .methods(currency)
        .iter()
        .filter(|method| bonus.methods.iter().any(|code| code == method.code))
        .map(|method| method.title)
        .collect();
    details.insert("topup_bonus_enabled".into(), bonus.enabled.into());
    details.insert("topup_bonus_percent".into(), bonus.percent.into());
    details.insert("topup_bonus_min".into(), bonus_limit(bonus.min));
    details.insert("topup_bonus_max".into(), bonus_limit(bonus.max));
    details.insert(
        "topup_bonus_methods".into(),
        if bonus.methods.is_empty() {
            Value::from("все")
        } else {
            serde_json::json!(titles)
        },
    );
}

pub fn clean_label(label: &ButtonLabel) -> ServiceResult<ButtonLabel> {
    let tidy = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    let title = tidy(&label.title);
    let hint = tidy(&label.hint);
    if title
        .as_deref()
        .is_some_and(|t| t.chars().count() > METHOD_TITLE_MAX)
    {
        return Err(ServiceError::rejected(
            "settings.method_title",
            format!("Название кнопки — не длиннее {METHOD_TITLE_MAX} символов"),
        ));
    }
    if hint
        .as_deref()
        .is_some_and(|t| t.chars().count() > LABEL_HINT_MAX)
    {
        return Err(ServiceError::rejected(
            "settings.label_hint",
            format!("Пояснение к кнопке — не длиннее {LABEL_HINT_MAX} символов"),
        ));
    }
    Ok(ButtonLabel { title, hint })
}

pub async fn registry_snapshot(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    registry: &ProviderRegistry,
    core: &CoreSettings,
    bot_token: Option<&str>,
) -> ServiceResult<RegistrySnapshot> {
    let mut states = HashMap::new();
    for provider in registry.providers() {
        let id = provider.id();
        let stored: ProviderSettings = db
            .settings()
            .get(&keys::provider(id))
            .await?
            .and_then(|v| parse(&keys::provider(id), v))
            .unwrap_or_default();
        let mut config = ProviderConfig::new(stored.config);
        for field in provider.settings_schema().iter().filter(|f| f.is_secret()) {
            if let Some(value) =
                read_secret(db, secrets, &secret_keys::provider(id, field.key)).await?
            {
                config.set(field.key, Value::String(value));
            }
        }
        if id == providers::STARS
            && let Some(token) = bot_token
        {
            config.set(STARS_TOKEN_FIELD, Value::String(token.to_string()));
        }
        states.insert(
            id.to_string(),
            ProviderState {
                enabled: stored
                    .enabled
                    .unwrap_or_else(|| provider.enabled_by_default()),
                config,
                channels: stored.channels,
                methods: stored.methods,
                label: stored.label,
                topup_bonus: stored.topup_bonus,
            },
        );
    }
    Ok(RegistrySnapshot {
        currency: core.currency,
        states,
        order: core.payment_order.for_currency(core.currency).to_vec(),
    })
}

pub async fn provider_checks(
    db: &mut impl Db,
    registry: &ProviderRegistry,
) -> ServiceResult<HashMap<String, CheckRecord>> {
    let mut checks = HashMap::new();
    for (key, value) in db.settings().with_prefix(keys::PROVIDER_CHECKS).await? {
        let Some(id) = key.strip_prefix(keys::PROVIDER_CHECKS) else {
            continue;
        };
        if registry.get(id).is_none() {
            continue;
        }
        if let Some(record) = parse::<CheckRecord>(&key, value) {
            checks.insert(id.to_string(), record);
        }
    }
    Ok(checks)
}

pub async fn module_secret_rows(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    modules: &sn_modules::ModuleRegistry,
) -> ServiceResult<Vec<ModuleStateRow>> {
    let mut merged = Vec::new();
    for row in db.modules().all().await? {
        let Ok(id) = ModuleId::parse(&row.module) else {
            continue;
        };
        let schema = modules.get(id).settings_schema();
        let mut config = row.config.as_object().cloned().unwrap_or_default();
        let mut found = false;
        for field in schema.iter().filter(|f| f.is_secret()) {
            if let Some(value) =
                read_secret(db, secrets, &secret_keys::module(id.as_str(), field.key)).await?
            {
                config.insert(field.key.to_string(), Value::String(value));
                found = true;
            }
        }
        if found {
            merged.push(ModuleStateRow {
                config: Value::Object(config),
                ..row
            });
        }
    }
    Ok(merged)
}

const TRIAL_ID_MAX: u32 = 999_999;

fn variant_error(index: usize, variant: &TrialVariant, reason: &str) -> ServiceError {
    let name = variant.name.trim();
    let who = if name.is_empty() {
        format!("вариант {}", index + 1)
    } else {
        format!("вариант «{name}»")
    };
    ServiceError::rejected("settings.invalid_field", format!("{who}: {reason}"))
}

pub(crate) fn checked_trial(
    trial: &TrialSettings,
    saved: &TrialSettings,
) -> ServiceResult<TrialSettings> {
    let count = trial.variants.len();
    if count == 0 || count > TRIAL_VARIANTS_MAX {
        return Err(field_error(
            "variants",
            &format!("от 1 до {TRIAL_VARIANTS_MAX} вариантов пробного периода"),
        ));
    }
    let mut next = trial
        .variants
        .iter()
        .chain(saved.variants.iter())
        .map(|v| v.id)
        .filter(|id| *id <= TRIAL_ID_MAX)
        .max()
        .unwrap_or(0);
    let mut ids: Vec<u32> = Vec::with_capacity(count);
    let mut names: Vec<String> = Vec::with_capacity(count);
    let mut variants: Vec<TrialVariant> = Vec::with_capacity(count);
    for (index, sent) in trial.variants.iter().enumerate() {
        let mut variant = sent.clone();
        variant.name = variant.name.trim().to_string();
        if variant.name.chars().count() > TRIAL_NAME_MAX {
            return Err(variant_error(
                index,
                &variant,
                &format!("название до {TRIAL_NAME_MAX} знаков"),
            ));
        }
        if count > 1 && variant.name.is_empty() {
            return Err(variant_error(
                index,
                &variant,
                "укажите название, по нему клиент выбирает вариант",
            ));
        }
        let folded = variant.name.to_lowercase();
        if !folded.is_empty() && names.contains(&folded) {
            return Err(variant_error(
                index,
                &variant,
                "такое название уже есть у другого варианта",
            ));
        }
        names.push(folded);
        if !(1..=90).contains(&variant.days) {
            return Err(variant_error(index, &variant, "срок от 1 до 90 дней"));
        }
        if !(1..=100).contains(&variant.devices) {
            return Err(variant_error(index, &variant, "от 1 до 100 устройств"));
        }
        variant.traffic.validate()?;
        if trial.enabled && variant.enabled && variant.squads.is_empty() {
            return Err(variant_error(
                index,
                &variant,
                "выберите хотя бы один сквад, иначе у пробной подписки не будет серверов",
            ));
        }
        let mut unique = variant.squads.clone();
        unique.sort();
        unique.dedup();
        if unique.len() != variant.squads.len() {
            return Err(variant_error(index, &variant, "сквад указан дважды"));
        }
        if variant.id > TRIAL_ID_MAX {
            return Err(variant_error(index, &variant, "неверный номер варианта"));
        }
        if variant.id == 0 {
            next += 1;
            variant.id = next;
        }
        if ids.contains(&variant.id) {
            return Err(variant_error(index, &variant, "номер варианта повторяется"));
        }
        ids.push(variant.id);
        variants.push(variant);
    }
    if trial.enabled && !variants.iter().any(|v| v.enabled) {
        return Err(field_error(
            "variants",
            "включите хотя бы один вариант или выключите пробный период",
        ));
    }
    Ok(TrialSettings {
        enabled: trial.enabled,
        variants,
    })
}

fn field_error(field: &str, reason: &str) -> ServiceError {
    ServiceError::rejected(
        "settings.invalid_field",
        format!("поле «{field}»: {reason}"),
    )
}

fn checked_value(field: &SettingField, value: &Value) -> ServiceResult<Value> {
    match field.kind {
        FieldKind::Flag => match value {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) if matches!(s.trim(), "true" | "1" | "yes") => Ok(Value::Bool(true)),
            Value::String(s) if matches!(s.trim(), "false" | "0" | "no" | "") => {
                Ok(Value::Bool(false))
            }
            _ => Err(field_error(field.key, "ожидается да или нет")),
        },
        FieldKind::Number => {
            let raw = match value {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.trim().to_string(),
                _ => return Err(field_error(field.key, "ожидается число")),
            };
            let digits = raw.strip_prefix('-').unwrap_or(&raw);
            let valid = !digits.is_empty()
                && digits.split('.').count() <= 2
                && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
                && !digits.starts_with('.')
                && !digits.ends_with('.');
            if valid {
                Ok(Value::String(raw))
            } else {
                Err(field_error(field.key, "ожидается число"))
            }
        }
        FieldKind::Url => {
            let raw = value
                .as_str()
                .map(str::trim)
                .ok_or_else(|| field_error(field.key, "ожидается адрес"))?;
            if raw.is_empty() {
                return Ok(Value::String(String::new()));
            }
            if !(raw.starts_with("https://") || raw.starts_with("http://"))
                || raw.contains(char::is_whitespace)
                || raw.len() > MAX_TEXT_FIELD
            {
                return Err(field_error(field.key, "адрес должен начинаться с https://"));
            }
            Ok(Value::String(raw.to_string()))
        }
        FieldKind::Select { options } => {
            let raw = value
                .as_str()
                .map(str::trim)
                .ok_or_else(|| field_error(field.key, "выберите значение из списка"))?;
            if options.contains(&raw) {
                Ok(Value::String(raw.to_string()))
            } else {
                Err(field_error(field.key, "выберите значение из списка"))
            }
        }
        FieldKind::List => match value {
            Value::Array(items) => Ok(Value::Array(items.clone())),
            _ => Err(field_error(field.key, "ожидается список")),
        },
        FieldKind::Text | FieldKind::Secret => {
            let raw = match value {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => return Err(field_error(field.key, "ожидается текст")),
            };
            let limit = if field.large {
                MAX_LARGE_FIELD
            } else {
                MAX_TEXT_FIELD
            };
            if raw.chars().count() > limit {
                return Err(field_error(field.key, "слишком длинное значение"));
            }
            Ok(Value::String(raw))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretChange {
    Set(String, String),
    Delete(String),
}

pub fn apply_fields(
    schema: &[SettingField],
    incoming: &Map<String, Value>,
    clear: &[String],
    stored: &mut Map<String, Value>,
) -> ServiceResult<Vec<SecretChange>> {
    let mut changes = Vec::new();
    for (key, value) in incoming {
        let field = schema
            .iter()
            .find(|f| f.key == key)
            .ok_or_else(|| field_error(key, "такого поля нет"))?;
        if field.is_secret() {
            match value {
                Value::Null => {}
                Value::String(s) if s.trim().is_empty() => {}
                other => {
                    if let Value::String(text) = checked_value(field, other)? {
                        changes.push(SecretChange::Set(field.key.to_string(), text));
                    }
                }
            }
            continue;
        }
        match value {
            Value::Null => {
                stored.remove(field.key);
            }
            other => {
                stored.insert(field.key.to_string(), checked_value(field, other)?);
            }
        }
    }
    for key in clear {
        let field = schema
            .iter()
            .find(|f| f.key == key)
            .ok_or_else(|| field_error(key, "такого поля нет"))?;
        if field.is_secret() {
            changes.push(SecretChange::Delete(field.key.to_string()));
        } else {
            stored.remove(field.key);
        }
    }
    Ok(changes)
}

pub fn public_config(schema: &[SettingField], config: &Map<String, Value>) -> Map<String, Value> {
    schema
        .iter()
        .map(|field| {
            let value = if field.is_secret() {
                let filled = config
                    .get(field.key)
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.trim().is_empty());
                serde_json::json!({ "filled": filled })
            } else {
                config.get(field.key).cloned().unwrap_or(Value::Null)
            };
            (field.key.to_string(), value)
        })
        .collect()
}

pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 8 {
        return "…".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

fn captcha_domains(config: &ModuleConfig, public_url: Option<&str>) -> Result<(), String> {
    let settings = AntibotSettings::lenient(config);
    if settings.captcha == captcha::Provider::None {
        return Ok(());
    }
    match public_url.and_then(captcha::clean_host) {
        Some(host) if !captcha::covered(&settings.captcha_domains, &host) => Err(format!(
            "Домены капчи: добавьте домен панели {host}, иначе капчу на нём не пройдёт никто"
        )),
        _ => Ok(()),
    }
}

pub fn webhook_env(public_url: Option<&str>, secret: &str) -> String {
    let url = match public_url {
        Some(url) => format!("{}{REMNA_WEBHOOK_PATH}", url.trim_end_matches('/')),
        None => format!("<SN_PUBLIC_URL>{REMNA_WEBHOOK_PATH}"),
    };
    format!("WEBHOOK_ENABLED=true\nWEBHOOK_URL={url}\nWEBHOOK_SECRET_HEADER={secret}")
}

fn optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoreSettingsView {
    pub settings: CoreSettings,
    pub currencies: [Currency; 2],
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CoreUpdate {
    #[serde(default)]
    pub currency: Option<Currency>,
    #[serde(default)]
    pub confirm_currency_change: bool,
    #[serde(default)]
    pub brand_name: Option<String>,
    #[serde(default)]
    pub support_url: Option<String>,
    #[serde(default)]
    pub support_mode: Option<SupportMode>,
    #[serde(default)]
    pub invoice_ttl_minutes: Option<u32>,
    #[serde(default)]
    pub remna_utc_offset_minutes: Option<i32>,
    #[serde(default)]
    pub topup: Option<TopUpSettings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BalanceHolders {
    pub clients: i64,
    pub total: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentsView {
    pub currency: Currency,
    pub order: Vec<String>,
    pub providers: Vec<ProviderView>,
    pub balances: BalanceHolders,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaymentsUpdate {
    pub order: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct TopupBonusUpdate {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub percent: i64,
    #[serde(default)]
    pub min_minor: Option<i64>,
    #[serde(default)]
    pub max_minor: Option<i64>,
    #[serde(default)]
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ProviderUpdate {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: Map<String, Value>,
    #[serde(default)]
    pub clear: Vec<String>,
    #[serde(default)]
    pub channels: Option<PaymentChannels>,
    #[serde(default)]
    pub methods: Option<Vec<MethodChoice>>,
    #[serde(default)]
    pub label: Option<ButtonLabel>,
    #[serde(default)]
    pub topup_bonus: Option<TopupBonusUpdate>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderDetails {
    pub provider: ProviderView,
    pub schema: Vec<SettingField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemnaSettingsView {
    pub url: Option<String>,
    pub url_from_env: bool,
    pub token_set: bool,
    pub token_from_env: bool,
    pub cookie_set: bool,
    pub api_key_set: bool,
    pub guard: PanelGuard,
    pub guard_from_env: bool,
    pub webhook_secret_set: bool,
    pub webhook_secret_hint: Option<String>,
    pub webhook_url: Option<String>,
    pub env: Option<String>,
    pub utc_offset_minutes: i32,
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum GuardUpdate {
    #[serde(rename = "none")]
    Off,
    Cookie {
        #[serde(default)]
        cookie: Option<String>,
    },
    Tinyauth {
        #[serde(default)]
        login: Option<String>,
        #[serde(default)]
        password: Option<String>,
    },
    Portal {
        #[serde(default)]
        key: Option<String>,
    },
}

impl GuardUpdate {
    pub const fn mode(&self) -> PanelGuard {
        match self {
            GuardUpdate::Off => PanelGuard::Off,
            GuardUpdate::Cookie { .. } => PanelGuard::Cookie,
            GuardUpdate::Tinyauth { .. } => PanelGuard::Tinyauth,
            GuardUpdate::Portal { .. } => PanelGuard::Portal,
        }
    }
}

impl std::fmt::Debug for GuardUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardUpdate")
            .field("mode", &self.mode())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RemnaUpdate {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub cookie: Option<String>,
    #[serde(default)]
    pub clear_cookie: bool,
    #[serde(default)]
    pub guard: Option<GuardUpdate>,
    #[serde(default)]
    pub webhook_secret: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RemnaCheckRequest {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub cookie: Option<String>,
    #[serde(default)]
    pub guard: Option<GuardUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemnaCheck {
    pub ok: bool,
    pub version: Option<String>,
    pub supported: bool,
    pub sub_public_domain: Option<String>,
    pub webhooks_enabled: Option<bool>,
    pub usage_records: Option<bool>,
    pub request_history: Option<bool>,
    pub error: Option<String>,
    pub hint: Option<String>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageConfig {
    pub uuid: String,
    pub name: String,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebhookSecret {
    pub secret: String,
    pub env: String,
}

pub const BOT_TAKEOVER_NOT_NEEDED: &str = "bot.takeover_not_needed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BotSettingsView {
    pub token_set: bool,
    pub token_from_env: bool,
    pub username: Option<String>,
    pub status: crate::BotState,
    pub updates: crate::bot_webhook::UpdatesView,
    pub flood: FloodView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FloodView {
    pub settings: sn_domain::settings::FloodSettings,
    pub stats: Option<crate::bot_flood::FloodSnapshot>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct BotUpdate {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub updates: Option<sn_domain::settings::UpdatesMode>,
    #[serde(default)]
    pub flood: Option<sn_domain::settings::FloodSettings>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleConfigView {
    pub module: ModuleId,
    pub schema: Vec<SettingField>,
    pub config: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ModuleConfigUpdate {
    #[serde(default)]
    pub config: Map<String, Value>,
    #[serde(default)]
    pub clear: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SubscriptionSettingsUpdate {
    #[serde(default)]
    pub hwid: Option<HwidSettings>,
    #[serde(default)]
    pub show_custom_remarks: Option<bool>,
    #[serde(default)]
    pub custom_remarks: Option<CustomRemarks>,
}

fn validate_remarks(remarks: &CustomRemarks) -> ServiceResult<()> {
    let groups: [(&str, &Vec<String>); 6] = [
        ("expiredUsers", &remarks.expired_users),
        ("limitedUsers", &remarks.limited_users),
        ("disabledUsers", &remarks.disabled_users),
        ("emptyHosts", &remarks.empty_hosts),
        ("HWIDMaxDevicesExceeded", &remarks.hwid_max_devices_exceeded),
        ("HWIDNotSupported", &remarks.hwid_not_supported),
    ];
    for (name, lines) in groups {
        if lines.iter().all(|l| l.trim().is_empty()) {
            return Err(field_error(name, "нужна хотя бы одна строка"));
        }
        if lines.iter().any(|l| l.chars().count() > MAX_REMARK_CHARS) {
            return Err(field_error(name, "строка длиннее 200 символов"));
        }
    }
    Ok(())
}

fn bot_username(raw: &str) -> ServiceResult<String> {
    let name = raw.trim().trim_start_matches('@');
    let valid = (5..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if valid {
        Ok(name.to_string())
    } else {
        Err(field_error(
            "username",
            "имя бота из латинских букв, цифр и _",
        ))
    }
}

fn bot_token(raw: &str) -> ServiceResult<String> {
    let token = raw.trim();
    let valid = token.split_once(':').is_some_and(|(id, secret)| {
        !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_digit())
            && secret.len() >= 20
            && secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    if valid {
        Ok(token.to_string())
    } else {
        Err(field_error(
            "token",
            "токен выдаёт @BotFather, вида 123456:ABC…",
        ))
    }
}

fn remna_url(raw: &str) -> ServiceResult<String> {
    let url = raw.trim().trim_end_matches('/');
    if (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() > "http://".len()
        && !url.contains(char::is_whitespace)
    {
        Ok(url.to_string())
    } else {
        Err(field_error(
            "url",
            "адрес панели должен начинаться с https:// или http:// и быть без пробелов",
        ))
    }
}

pub fn generate_webhook_secret() -> String {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

pub struct SettingsOps<'a> {
    services: &'a Services,
}

impl<'a> SettingsOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        SettingsOps { services }
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub fn core(&self) -> CoreSettingsView {
        CoreSettingsView {
            settings: self.services.settings().as_ref().clone(),
            currencies: Currency::ALL,
        }
    }

    async fn check_currency_switch(
        &self,
        db: &mut impl Db,
        from: Currency,
        to: Currency,
        confirmed: bool,
    ) -> ServiceResult<()> {
        let mut missing = Vec::new();
        for tariff in db.tariffs().list(false).await? {
            let days: Vec<String> = tariff
                .periods
                .iter()
                .filter(|p| p.active && p.price(to).is_none())
                .map(|p| format!("{} дн.", p.days))
                .collect();
            if !days.is_empty() {
                missing.push(format!("«{}» ({})", tariff.name, days.join(", ")));
            } else if tariff
                .devices
                .extra
                .as_ref()
                .is_some_and(|e| e.monthly.get(to).is_none())
            {
                missing.push(format!("«{}» (доп. устройство)", tariff.name));
            }
        }
        if !missing.is_empty() {
            return Err(ServiceError::rejected(
                "settings.currency_prices",
                format!(
                    "Сначала задайте цены в {}: {}. Форма тарифа принимает цены в текущей валюте, поэтому снимите эти тарифы с продажи, смените валюту, задайте цены и верните тарифы в продажу",
                    to.code(),
                    missing.join("; ")
                ),
            ));
        }
        let holders: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT client_id) FROM client_balances WHERE currency = $1 AND amount_minor > 0",
        )
        .bind(from.code())
        .fetch_one(db.conn())
        .await?;
        if holders > 0 && !confirmed {
            return Err(ServiceError::rejected(
                "settings.currency_balances",
                format!(
                    "У {holders} клиентов есть баланс в {}. В {} он не переносится и не будет виден. Подтвердите смену валюты",
                    from.code(),
                    to.code()
                ),
            ));
        }
        Ok(())
    }

    pub async fn update_core(
        &self,
        update: &CoreUpdate,
        admin: AdminId,
    ) -> ServiceResult<CoreSettingsView> {
        let current = self.services.settings();
        let mut tx = self.services.store().begin().await?;
        let mut changed = serde_json::Map::new();
        if let Some(currency) = update.currency
            && currency != current.currency
        {
            self.check_currency_switch(
                &mut tx,
                current.currency,
                currency,
                update.confirm_currency_change,
            )
            .await?;
            tx.settings()
                .set_as(keys::CURRENCY, &currency, Some(admin))
                .await?;
            changed.insert(
                "currency".into(),
                serde_json::json!([current.currency, currency]),
            );
        }
        if let Some(name) = &update.brand_name {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 64 {
                return Err(field_error("brand_name", "от 1 до 64 символов"));
            }
            tx.settings()
                .set_as(keys::BRAND_NAME, &name, Some(admin))
                .await?;
            changed.insert("brand_name".into(), Value::String(name.to_string()));
        }
        let next_url = match &update.support_url {
            Some(url) => optional_text(Some(url)),
            None => current.support_url.clone(),
        };
        let next_mode = update.support_mode.unwrap_or(current.support_mode);
        if next_mode.needs_link() && next_url.is_none() {
            return Err(field_error(
                "support_url",
                "укажите ссылку на чат или бота поддержки",
            ));
        }
        if update.support_mode.is_some() || update.support_url.is_some() {
            tx.settings()
                .set_as(keys::SUPPORT_MODE, &next_mode, Some(admin))
                .await?;
            if next_mode != current.support_mode {
                changed.insert(
                    "support_mode".into(),
                    Value::String(next_mode.as_str().into()),
                );
            }
        }
        if let Some(url) = &update.support_url {
            match optional_text(Some(url)) {
                None => {
                    tx.settings().delete(keys::SUPPORT_URL).await?;
                }
                Some(url) => {
                    let valid = (url.starts_with("https://") || url.starts_with("tg://"))
                        && !url.contains(char::is_whitespace)
                        && url.len() <= 500;
                    if !valid {
                        return Err(field_error(
                            "support_url",
                            "ссылка должна начинаться с https:// или tg://",
                        ));
                    }
                    tx.settings()
                        .set_as(keys::SUPPORT_URL, &url, Some(admin))
                        .await?;
                }
            }
            changed.insert("support_url".into(), Value::Bool(true));
        }
        if let Some(ttl) = update.invoice_ttl_minutes {
            if !(5..=24 * 60).contains(&ttl) {
                return Err(field_error("invoice_ttl_minutes", "от 5 минут до суток"));
            }
            tx.settings()
                .set_as(keys::INVOICE_TTL, &ttl, Some(admin))
                .await?;
            changed.insert("invoice_ttl_minutes".into(), Value::from(ttl));
        }
        if let Some(offset) = update.remna_utc_offset_minutes {
            if !(-14 * 60..=14 * 60).contains(&offset) {
                return Err(field_error(
                    "remna_utc_offset_minutes",
                    "смещение от UTC от −14 до +14 часов",
                ));
            }
            tx.settings()
                .set_as(keys::REMNA_UTC_OFFSET, &offset, Some(admin))
                .await?;
            changed.insert("remna_utc_offset_minutes".into(), Value::from(offset));
        }
        if let Some(topup) = update.topup {
            if topup.min_minor <= 0 || topup.max_minor < topup.min_minor {
                return Err(field_error(
                    "topup",
                    "минимум больше нуля и не больше максимума",
                ));
            }
            tx.settings()
                .set_as(keys::TOPUP, &topup, Some(admin))
                .await?;
            changed.insert("topup".into(), Value::Bool(true));
        }
        if changed.is_empty() {
            tx.commit().await?;
            return Ok(self.core());
        }
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.core",
                "settings",
                None,
                Value::Object(changed),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.core())
    }

    pub fn appearance(&self) -> Appearance {
        self.services.settings().appearance
    }

    pub async fn update_appearance(
        &self,
        update: &Appearance,
        admin: AdminId,
    ) -> ServiceResult<Appearance> {
        let current = self.appearance();
        if *update == current {
            return Ok(current);
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::APPEARANCE, update, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.appearance",
                "settings",
                None,
                serde_json::json!({ "from": current, "to": update }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.appearance())
    }

    pub fn connect(&self) -> ConnectSettings {
        self.services.settings().connect.clone()
    }

    pub async fn update_connect(
        &self,
        update: &ConnectSettings,
        admin: AdminId,
    ) -> ServiceResult<ConnectSettings> {
        let page_config = update
            .page_config
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| {
                uuid::Uuid::parse_str(v)
                    .map(|id| id.to_string())
                    .map_err(|_| {
                        field_error("page_config", "нужен идентификатор оформления страницы")
                    })
            })
            .transpose()?;
        let next = ConnectSettings {
            mode: update.mode,
            page_config,
        };
        let current = self.connect();
        if next == current {
            return Ok(current);
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::CONNECT, &next, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.connect",
                "settings",
                None,
                serde_json::json!({ "from": current, "to": next }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.connect())
    }

    pub async fn page_configs(&self) -> ServiceResult<Vec<PageConfig>> {
        let remna = self.services.remna()?;
        let configs = remna.subscription_page_configs().await?;
        let mut list: Vec<PageConfig> = configs
            .configs
            .into_iter()
            .map(|c| PageConfig {
                uuid: c.uuid.to_string(),
                name: c.name,
                position: c.view_position,
            })
            .collect();
        list.sort_by_key(|c| c.position);
        Ok(list)
    }

    pub fn trial(&self) -> TrialSettings {
        self.services.settings().trial.clone()
    }

    pub async fn update_trial(
        &self,
        trial: &TrialSettings,
        admin: AdminId,
    ) -> ServiceResult<TrialSettings> {
        let trial = &checked_trial(trial, &self.services.settings().trial)?;
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::TRIAL, trial, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.trial",
                "settings",
                None,
                serde_json::to_value(trial).map_err(|e| ServiceError::Internal(e.to_string()))?,
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.trial())
    }

    pub fn client_access(&self) -> ClientAccess {
        self.services.settings().client_access
    }

    pub async fn update_client_access(
        &self,
        value: &ClientAccess,
        admin: AdminId,
    ) -> ServiceResult<ClientAccess> {
        let current = self.client_access();
        if *value == current {
            return Ok(current);
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::CLIENT_ACCESS, value, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.client_access",
                "settings",
                None,
                serde_json::json!({ "from": current, "to": value }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.services.bot_runtime().wake();
        Ok(self.client_access())
    }

    pub fn languages(&self) -> Languages {
        self.services.settings().languages
    }

    pub async fn update_languages(
        &self,
        value: &Languages,
        admin: AdminId,
    ) -> ServiceResult<Languages> {
        if !value.allows(value.default) {
            return Err(ServiceError::rejected(
                "settings.languages",
                "Языком по умолчанию можно сделать только включённый язык",
            ));
        }
        let before = self.languages();
        if before == *value {
            return Ok(before);
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::LANGUAGES, value, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.languages",
                "settings",
                None,
                serde_json::json!(value),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.services.bot_runtime().wake();
        Ok(self.languages())
    }

    pub fn multi_subscriptions(&self) -> MultiSubscriptions {
        MultiSubscriptions {
            enabled: self.services.settings().multi_subscriptions,
        }
    }

    pub async fn update_multi_subscriptions(
        &self,
        value: &MultiSubscriptions,
        admin: AdminId,
    ) -> ServiceResult<MultiSubscriptions> {
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::MULTI_SUBSCRIPTIONS, value, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.multi_subscriptions",
                "settings",
                None,
                serde_json::json!({ "enabled": value.enabled }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.multi_subscriptions())
    }

    pub fn autorenew(&self) -> AutoRenewSettings {
        self.services.settings().autorenew
    }

    pub async fn update_autorenew(
        &self,
        settings: &AutoRenewSettings,
        admin: AdminId,
    ) -> ServiceResult<AutoRenewSettings> {
        if settings.charge_days_before > 7 {
            return Err(field_error(
                "charge_days_before",
                "списывать не раньше чем за 7 дней",
            ));
        }
        if settings.retry_days > 14 {
            return Err(field_error("retry_days", "повторять не дольше 14 дней"));
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::AUTORENEW, settings, Some(admin))
            .await?;
        tx.audit()
            .record(
                &Actor::Admin(admin).audit(
                    "settings.autorenew",
                    "settings",
                    None,
                    serde_json::to_value(settings)
                        .map_err(|e| ServiceError::Internal(e.to_string()))?,
                ),
            )
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.autorenew())
    }

    pub async fn payments(&self) -> ServiceResult<PaymentsView> {
        let settings = self.services.settings();
        let (clients, total) = {
            let mut conn = self.services.store().conn().await?;
            conn.ledger()
                .holders(Wallet::Main, settings.currency)
                .await?
        };
        Ok(PaymentsView {
            currency: settings.currency,
            order: settings
                .payment_order
                .for_currency(settings.currency)
                .to_vec(),
            providers: self.services.payments().views(),
            balances: BalanceHolders { clients, total },
        })
    }

    pub async fn update_payments(
        &self,
        update: &PaymentsUpdate,
        admin: AdminId,
    ) -> ServiceResult<PaymentsView> {
        let settings = self.services.settings();
        let registry = self.services.payments();
        let mut order: Vec<String> = Vec::new();
        for id in &update.order {
            let id = id.trim();
            let provider = registry.get(id).ok_or_else(|| {
                field_error("order", &format!("неизвестная платёжная система {id}"))
            })?;
            if provider.kind() == ProviderKind::Balance {
                continue;
            }
            if !provider.currencies().contains(&settings.currency) {
                return Err(field_error(
                    "order",
                    &format!(
                        "{} не работает с {}",
                        provider.title(),
                        settings.currency.code()
                    ),
                ));
            }
            if order.iter().any(|o| o == id) {
                return Err(field_error("order", &format!("{id} указан дважды")));
            }
            order.push(id.to_string());
        }
        let mut tx = self.services.store().begin().await?;
        let mut payment_order = tx
            .settings()
            .get(keys::PAYMENT_ORDER)
            .await?
            .and_then(|v| parse::<PaymentOrder>(keys::PAYMENT_ORDER, v))
            .unwrap_or_default();
        match settings.currency {
            Currency::Usd => payment_order.usd = order.clone(),
            Currency::Rub => payment_order.rub = order.clone(),
        }
        tx.settings()
            .set_as(keys::PAYMENT_ORDER, &payment_order, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.payment_order",
                "settings",
                None,
                serde_json::json!({ "currency": settings.currency, "order": order }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.payments().await
    }

    async fn stored_provider(&self, db: &mut impl Db, id: &str) -> ServiceResult<ProviderSettings> {
        Ok(db
            .settings()
            .get(&keys::provider(id))
            .await?
            .and_then(|v| parse(&keys::provider(id), v))
            .unwrap_or_default())
    }

    pub fn provider(&self, id: &str) -> ServiceResult<ProviderDetails> {
        let provider = self
            .services
            .payments()
            .get(id)
            .ok_or(ServiceError::NotFound("платёжная система"))?;
        let view = self
            .services
            .payments()
            .views()
            .into_iter()
            .find(|v| v.id == provider.id())
            .ok_or(ServiceError::NotFound("платёжная система"))?;
        Ok(ProviderDetails {
            provider: view,
            schema: provider.settings_schema(),
        })
    }

    pub async fn update_provider(
        &self,
        id: &str,
        update: &ProviderUpdate,
        admin: AdminId,
    ) -> ServiceResult<ProviderDetails> {
        let provider = self
            .services
            .payments()
            .get(id)
            .ok_or(ServiceError::NotFound("платёжная система"))?;
        let id = provider.id();
        let schema = provider.settings_schema();
        let mut tx = self.services.store().begin().await?;
        let mut stored = self.stored_provider(&mut tx, id).await?;
        let before = stored.config.clone();
        let changes = apply_fields(&schema, &update.config, &update.clear, &mut stored.config)?;
        if let Some(enabled) = update.enabled {
            stored.enabled = Some(enabled);
        }
        if let Some(channels) = update.channels {
            stored.channels = channels;
        }
        if let Some(methods) = &update.methods {
            stored.methods = clean_methods(
                provider.as_ref(),
                methods,
                self.services.settings().currency,
            )?;
        }
        if let Some(label) = &update.label {
            stored.label = clean_label(label)?;
        }
        if let Some(bonus) = &update.topup_bonus {
            stored.topup_bonus =
                clean_topup_bonus(provider.as_ref(), bonus, self.services.settings().currency)?;
        }
        let secrets = self.services.secrets();
        let mut merged = stored.config.clone();
        let mut saved = Vec::new();
        for field in schema.iter().filter(|f| f.is_secret()) {
            if let Some(value) =
                read_secret(&mut tx, secrets, &secret_keys::provider(id, field.key)).await?
            {
                saved.push(field.key);
                merged.insert(field.key.to_string(), Value::String(value));
            }
        }
        secrets_again(&schema, &before, &stored.config, &saved, &changes)?;
        for change in &changes {
            match change {
                SecretChange::Set(key, value) => {
                    merged.insert(key.clone(), Value::String(value.clone()));
                }
                SecretChange::Delete(key) => {
                    merged.remove(key);
                }
            }
        }
        let mut merged = ProviderConfig::new(merged);
        if id == providers::STARS
            && let Some(token) = self.services.bot_token()
        {
            merged.set(STARS_TOKEN_FIELD, Value::String(token));
        }
        let enabled = stored
            .enabled
            .unwrap_or_else(|| provider.enabled_by_default());
        let connection_touched =
            update.enabled == Some(true) || !update.config.is_empty() || !update.clear.is_empty();
        if enabled && connection_touched && provider.kind() != ProviderKind::Balance {
            provider
                .validate_settings(&merged)
                .map_err(|e| ServiceError::rejected("settings.provider_invalid", e.to_string()))?;
            if !provider.is_configured(&merged) {
                return Err(ServiceError::rejected(
                    "settings.provider_unavailable",
                    format!(
                        "{} пока нельзя включить: подключение не готово",
                        provider.title()
                    ),
                ));
            }
        }
        for change in &changes {
            match change {
                SecretChange::Set(key, value) => {
                    write_secret(
                        &mut tx,
                        secrets,
                        &secret_keys::provider(id, key),
                        Some(value),
                        Some(admin),
                    )
                    .await?;
                }
                SecretChange::Delete(key) => {
                    write_secret(
                        &mut tx,
                        secrets,
                        &secret_keys::provider(id, key),
                        None,
                        None,
                    )
                    .await?;
                }
            }
        }
        tx.settings()
            .set_as(&keys::provider(id), &stored, Some(admin))
            .await?;
        if !update.config.is_empty() || !update.clear.is_empty() {
            tx.settings().delete(&keys::provider_check(id)).await?;
        }
        let changed_secrets: Vec<&str> = changes
            .iter()
            .map(|c| match c {
                SecretChange::Set(key, _) | SecretChange::Delete(key) => key.as_str(),
            })
            .collect();
        let mut details = serde_json::json!({
            "provider": id,
            "enabled": stored.enabled,
            "channels": update.channels,
            "methods": update.methods,
            "label": update.label,
            "fields": update.config.keys().collect::<Vec<_>>(),
            "secrets": changed_secrets,
        });
        if update.topup_bonus.is_some()
            && let Some(details) = details.as_object_mut()
        {
            bonus_audit(
                provider.as_ref(),
                &stored.topup_bonus,
                self.services.settings().currency,
                details,
            );
        }
        tx.audit()
            .record(&Actor::Admin(admin).audit("settings.provider", "provider", None, details))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.provider(id)
    }

    pub async fn provider_catalog(&self, id: &str) -> ServiceResult<Value> {
        let registry = self.services.payments();
        let provider = registry
            .get(id)
            .filter(|p| p.kind() == ProviderKind::External)
            .ok_or(ServiceError::NotFound("платёжная система"))?;
        let id = provider.id();
        let config = registry.config(id);
        let secrets: Vec<&str> = provider
            .settings_schema()
            .iter()
            .filter(|f| f.is_secret())
            .map(|f| config.get(f.key))
            .collect::<Option<Vec<&str>>>()
            .ok_or_else(|| {
                ServiceError::rejected(
                    CHECK_UNCONFIGURED,
                    format!(
                        "Сначала сохраните ключ {}, потом загрузите список",
                        provider.title()
                    ),
                )
            })?;
        crate::limits::RateLimits::new(self.services)
            .hit(crate::limits::rules::PAYMENT_CATALOG, id)
            .await?;
        let scrub = |text: String| {
            secrets
                .iter()
                .filter(|s| s.len() >= 6)
                .fold(text, |text, secret| text.replace(secret, "•••"))
        };
        match tokio::time::timeout(PROVIDER_CHECK_TIMEOUT, provider.catalog(&config)).await {
            Err(_) => Err(ServiceError::rejected(
                "payments.catalog_timeout",
                format!(
                    "{} не ответила за {} секунд, попробуйте позже",
                    provider.title(),
                    PROVIDER_CHECK_TIMEOUT.as_secs()
                ),
            )),
            Ok(Err(err)) => Err(ServiceError::rejected(
                "payments.catalog_failed",
                scrub(format!("Список из {} не получен: {err}", provider.title())),
            )),
            Ok(Ok(None)) => Err(ServiceError::NotFound("список товаров")),
            Ok(Ok(Some(catalog))) => Ok(catalog),
        }
    }

    pub async fn check_provider(&self, id: &str, admin: AdminId) -> ServiceResult<CheckRecord> {
        let registry = self.services.payments();
        let provider = registry
            .get(id)
            .filter(|p| p.kind() != ProviderKind::Balance)
            .ok_or(ServiceError::NotFound("платёжная система"))?;
        let id = provider.id();
        let config = registry.config(id);
        if !provider.is_configured(&config) {
            return Err(ServiceError::rejected(
                CHECK_UNCONFIGURED,
                format!(
                    "{} ещё не подключена: заполните и сохраните ключи, потом проверьте",
                    provider.title()
                ),
            ));
        }
        let mut check = match provider.validate_settings(&config) {
            Err(err) => ConnectionCheck::failed(&err),
            Ok(()) => {
                crate::limits::RateLimits::new(self.services)
                    .hit(crate::limits::rules::PAYMENT_CHECK, id)
                    .await?;
                let context = CheckContext {
                    webhook_url: self
                        .services
                        .settings()
                        .public_url
                        .as_deref()
                        .map(|u| u.trim().trim_end_matches('/'))
                        .filter(|u| !u.is_empty())
                        .map(|base| format!("{base}/api/webhooks/pay/{id}")),
                };
                tokio::time::timeout(
                    PROVIDER_CHECK_TIMEOUT,
                    provider.check_connection(&config, &context),
                )
                .await
                .unwrap_or_else(|_| {
                    ConnectionCheck::new(
                        CheckStatus::Unreachable,
                        format!(
                            "Платёжная система не ответила за {} секунд, попробуйте позже",
                            PROVIDER_CHECK_TIMEOUT.as_secs()
                        ),
                    )
                })
            }
        };
        if provider.accepts_http_webhooks() {
            let since = self.services.now() - chrono::Duration::days(INBOX_TRACE_DAYS);
            let mut conn = self.services.store().conn().await?;
            let (verified_at, rejected_at) = conn.webhooks().trace(id, since).await?;
            if verified_at.is_some() || rejected_at.is_some() {
                check.inbox = Some(InboxTrace {
                    verified_at,
                    rejected_at,
                });
            }
        }
        let secrets: Vec<&str> = provider
            .settings_schema()
            .iter()
            .filter(|f| f.is_secret())
            .filter_map(|f| config.get(f.key))
            .chain(config.get(STARS_TOKEN_FIELD))
            .collect();
        check.scrub(&secrets);
        let record = CheckRecord {
            at: self.services.now(),
            check,
        };
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(&keys::provider_check(id), &record, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.provider_check",
                "provider",
                None,
                serde_json::json!({ "provider": id, "status": record.check.status }),
            ))
            .await?;
        tx.commit().await?;
        registry.record_check(id, Some(record.clone()));
        tracing::info!(provider = id, status = ?record.check.status, "проверено подключение платёжной системы");
        Ok(record)
    }

    pub async fn remnawave(&self) -> ServiceResult<RemnaSettingsView> {
        let env = self.services.env().remna.clone().unwrap_or_default();
        let mut conn = self.services.store().conn().await?;
        let stored_url = conn
            .settings()
            .get_as::<String>(keys::REMNA_URL)
            .await?
            .filter(|u| !u.trim().is_empty());
        let secrets = self.services.secrets();
        let token = read_secret(&mut conn, secrets, secret_keys::REMNA_TOKEN).await?;
        let guard = stored_guard(&mut conn, secrets, &env).await?;
        drop(conn);
        let webhook = self.services.remna_webhook_secret();
        let public_url = self.services.settings().public_url.clone();
        let url = stored_url.clone().or_else(|| optional_text(Some(&env.url)));
        Ok(RemnaSettingsView {
            url_from_env: stored_url.is_none() && url.is_some(),
            url,
            token_set: token.is_some() || !env.token.trim().is_empty(),
            token_from_env: token.is_none() && !env.token.trim().is_empty(),
            cookie_set: guard.cookie.is_some(),
            api_key_set: guard.api_key.is_some(),
            guard: guard.mode,
            guard_from_env: guard.from_env,
            webhook_secret_set: webhook.is_some(),
            webhook_secret_hint: webhook.as_deref().map(mask_secret),
            webhook_url: public_url
                .as_deref()
                .map(|u| format!("{}{REMNA_WEBHOOK_PATH}", u.trim_end_matches('/'))),
            env: webhook
                .as_deref()
                .map(|secret| webhook_env(public_url.as_deref(), &mask_secret(secret))),
            utc_offset_minutes: self.services.settings().remna_utc_offset_minutes,
        })
    }

    pub fn webhook_env_full(&self) -> Option<String> {
        let public_url = self.services.settings().public_url.clone();
        self.services
            .remna_webhook_secret()
            .as_deref()
            .map(|secret| webhook_env(public_url.as_deref(), secret))
    }

    pub async fn update_remnawave(
        &self,
        update: &RemnaUpdate,
        admin: AdminId,
    ) -> ServiceResult<RemnaSettingsView> {
        let secrets = self.services.secrets();
        let env = self.services.env().remna.clone().unwrap_or_default();
        let mut tx = self.services.store().begin().await?;
        let before = remna_connection(&mut tx, secrets, self.services.env()).await?;
        let current = stored_guard(&mut tx, secrets, &env).await?;
        let new_url = match &update.url {
            Some(url) => Some(match optional_text(Some(url)) {
                Some(url) => Some(remna_url(&url)?),
                None => None,
            }),
            None => None,
        };
        let token = optional_text(update.token.as_deref());
        let guard = match (&update.guard, optional_text(update.cookie.as_deref())) {
            (Some(guard), _) => Some(guard.clone()),
            (None, Some(cookie)) => Some(GuardUpdate::Cookie {
                cookie: Some(cookie),
            }),
            (None, None) => None,
        };
        let change = guard
            .as_ref()
            .map(|guard| guard_change(guard, &current))
            .transpose()?;
        let after_url = match &new_url {
            Some(Some(url)) => url.clone(),
            Some(None) => env.url.clone(),
            None => before.url.clone(),
        };
        if panel_moved(&before.url, &after_url) {
            let mut kept = Vec::new();
            if token.is_none() && !before.token.trim().is_empty() {
                kept.push(TOKEN_NAME);
            }
            match &change {
                Some(change) => kept.extend(change.kept(&current)),
                None => {
                    if !update.clear_cookie && current.cookie.is_some() {
                        kept.push(COOKIE_NAME);
                    }
                    if current.api_key.is_some() {
                        kept.push(key_name(current.mode));
                    }
                }
            }
            if !kept.is_empty() {
                return Err(ServiceError::rejected(
                    SECRETS_AGAIN,
                    panel_secrets_again(&kept),
                ));
            }
        }
        let mut changed = Vec::new();
        if let Some(url) = new_url {
            match url {
                Some(url) => {
                    tx.settings()
                        .set_as(keys::REMNA_URL, &url, Some(admin))
                        .await?;
                }
                None => {
                    tx.settings().delete(keys::REMNA_URL).await?;
                }
            }
            changed.push("url");
        }
        if let Some(token) = token {
            if token.contains(char::is_whitespace) {
                return Err(field_error("token", "токен без пробелов"));
            }
            write_secret(
                &mut tx,
                secrets,
                secret_keys::REMNA_TOKEN,
                Some(&token),
                Some(admin),
            )
            .await?;
            changed.push("token");
        }
        if let (Some(guard), Some(change)) = (guard, change) {
            write_change(
                &mut tx,
                secrets,
                secret_keys::REMNA_COOKIE,
                change.cookie,
                admin,
            )
            .await?;
            write_change(
                &mut tx,
                secrets,
                secret_keys::REMNA_API_KEY,
                change.api_key,
                admin,
            )
            .await?;
            tx.settings()
                .set_as(keys::REMNA_GUARD, &guard.mode(), Some(admin))
                .await?;
            changed.push("guard");
        } else if update.clear_cookie {
            write_secret(&mut tx, secrets, secret_keys::REMNA_COOKIE, None, None).await?;
            changed.push("cookie");
        }
        if let Some(secret) = optional_text(update.webhook_secret.as_deref()) {
            sn_remnawave::webhook::validate_secret(&secret)
                .map_err(|e| field_error("webhook_secret", &e.to_string()))?;
            write_secret(
                &mut tx,
                secrets,
                secret_keys::REMNA_WEBHOOK_SECRET,
                Some(&secret),
                Some(admin),
            )
            .await?;
            changed.push("webhook_secret");
        }
        if !changed.is_empty() {
            tx.audit()
                .record(&Actor::Admin(admin).audit(
                    "settings.remnawave",
                    "settings",
                    None,
                    serde_json::json!({ "fields": changed }),
                ))
                .await?;
        }
        tx.commit().await?;
        if !changed.is_empty() {
            self.saved().await?;
        }
        self.remnawave().await
    }

    pub async fn generate_remna_webhook_secret(
        &self,
        admin: AdminId,
    ) -> ServiceResult<WebhookSecret> {
        let secret = generate_webhook_secret();
        let mut tx = self.services.store().begin().await?;
        write_secret(
            &mut tx,
            self.services.secrets(),
            secret_keys::REMNA_WEBHOOK_SECRET,
            Some(&secret),
            Some(admin),
        )
        .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.remnawave_webhook_secret",
                "settings",
                None,
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        let env = webhook_env(self.services.settings().public_url.as_deref(), &secret);
        Ok(WebhookSecret { secret, env })
    }

    pub async fn check_remnawave(&self, request: &RemnaCheckRequest) -> ServiceResult<RemnaCheck> {
        let secrets = self.services.secrets();
        let env = self.services.env().remna.clone().unwrap_or_default();
        let mut conn = self.services.store().conn().await?;
        let mut target = remna_connection(&mut conn, secrets, self.services.env()).await?;
        let current = stored_guard(&mut conn, secrets, &env).await?;
        drop(conn);
        let custom = request.url.is_some()
            || request.token.is_some()
            || request.cookie.is_some()
            || request.guard.is_some();
        let saved_url = target.url.clone();
        let mut kept = Vec::new();
        if let Some(url) = optional_text(request.url.as_deref()) {
            target.url = remna_url(&url)?;
        }
        match optional_text(request.token.as_deref()) {
            Some(token) => target.token = token,
            None if !target.token.trim().is_empty() => kept.push(TOKEN_NAME),
            None => {}
        }
        match &request.guard {
            Some(guard) => {
                let change = guard_change(guard, &current)?;
                kept.extend(change.kept(&current));
                target.cookie = change.cookie.apply(current.cookie.clone());
                target.api_key = change.api_key.apply(current.api_key.clone());
            }
            None => {
                match optional_text(request.cookie.as_deref()) {
                    Some(cookie) => target.cookie = Some(cookie),
                    None if target.cookie.is_some() => kept.push(COOKIE_NAME),
                    None => {}
                }
                if target.api_key.is_some() {
                    kept.push(key_name(current.mode));
                }
            }
        }
        if panel_moved(&saved_url, &target.url) && !kept.is_empty() {
            return Ok(self.failed_check(panel_secrets_again(&kept), None));
        }
        let api: Arc<dyn RemnawaveApi> = if custom {
            match RemnawaveClient::new(target.client_config()) {
                Ok(client) => Arc::new(client),
                Err(err) => return Ok(self.failed_check(err.to_string(), None)),
            }
        } else {
            match self.services.remna() {
                Ok(api) => api,
                Err(err) => {
                    let broken = if target.url.is_empty() || target.token.is_empty() {
                        None
                    } else {
                        RemnawaveClient::new(target.client_config()).err()
                    };
                    let reason = broken.map_or_else(|| err.to_string(), |e| e.to_string());
                    return Ok(self.failed_check(reason, None));
                }
            }
        };
        let guard = PanelGuard::of(target.cookie.as_deref(), target.api_key.as_deref());
        let metadata = match api.metadata().await {
            Ok(metadata) => metadata,
            Err(err) => {
                let probe = match err {
                    RemnaError::Unauthorized(_) => Some(api.auth_status().await),
                    _ => None,
                };
                let (error, hint) = check_problem(&err, guard, &target.url, probe.as_ref());
                return Ok(self.failed_check(error, hint));
            }
        };
        let configuration = api.configuration().await;
        let supported = metadata.version == sn_remnawave::SUPPORTED_VERSION;
        Ok(match configuration {
            Ok(configuration) => RemnaCheck {
                ok: true,
                supported,
                version: Some(metadata.version),
                sub_public_domain: optional_text(Some(&configuration.misc.sub_public_domain)),
                webhooks_enabled: Some(configuration.notifications.webhook),
                usage_records: Some(!configuration.service.disable_user_usage_records),
                request_history: Some(!configuration.service.disable_srh_records),
                error: None,
                hint: None,
                checked_at: self.services.now(),
            },
            Err(err) => RemnaCheck {
                ok: false,
                supported,
                version: Some(metadata.version),
                error: Some(err.to_string()),
                ..self.failed_check(String::new(), None)
            },
        })
    }

    fn failed_check(&self, error: String, hint: Option<String>) -> RemnaCheck {
        RemnaCheck {
            ok: false,
            version: None,
            supported: false,
            sub_public_domain: None,
            webhooks_enabled: None,
            usage_records: None,
            request_history: None,
            error: Some(error),
            hint,
            checked_at: self.services.now(),
        }
    }

    pub async fn subscription_settings(&self) -> ServiceResult<SubscriptionSettings> {
        Ok(self.services.remna()?.subscription_settings().await?)
    }

    pub async fn update_subscription_settings(
        &self,
        update: &SubscriptionSettingsUpdate,
        admin: AdminId,
    ) -> ServiceResult<SubscriptionSettings> {
        let remna = self.services.remna()?;
        if let Some(hwid) = &update.hwid {
            if !(0..=100).contains(&hwid.fallback_device_limit) {
                return Err(field_error("fallbackDeviceLimit", "от 0 до 100 устройств"));
            }
            if hwid
                .max_devices_announce
                .as_deref()
                .is_some_and(|text| text.chars().count() > MAX_REMARK_CHARS)
            {
                return Err(field_error("maxDevicesAnnounce", "не длиннее 200 символов"));
            }
        }
        if let Some(remarks) = &update.custom_remarks {
            validate_remarks(remarks)?;
        }
        let current = remna.subscription_settings().await?;
        let updated = remna
            .update_subscription_settings(&UpdateSubscriptionSettingsRequest {
                uuid: current.uuid,
                is_show_custom_remarks: update.show_custom_remarks,
                custom_remarks: update.custom_remarks.clone(),
                hwid_settings: update.hwid.clone(),
            })
            .await?;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.remnawave_subscription",
                "settings",
                None,
                serde_json::json!({
                    "hwid": update.hwid.is_some(),
                    "remarks": update.custom_remarks.is_some(),
                    "show_remarks": update.show_custom_remarks,
                }),
            ))
            .await?;
        Ok(updated)
    }

    pub async fn bot(&self) -> ServiceResult<BotSettingsView> {
        let mut conn = self.services.store().conn().await?;
        let stored =
            read_secret(&mut conn, self.services.secrets(), secret_keys::BOT_TOKEN).await?;
        let username = conn
            .settings()
            .get_as::<String>(keys::BOT_USERNAME)
            .await?
            .filter(|u| !u.trim().is_empty());
        let env = self.services.env().bot_token.is_some();
        let stats = conn
            .bot_state()
            .get(crate::bot_flood::STATE_KEY)
            .await?
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::from_value(v).ok());
        drop(conn);
        Ok(BotSettingsView {
            token_set: stored.is_some() || env,
            token_from_env: stored.is_none() && env,
            username,
            status: self.services.bot_runtime().state(),
            updates: self.services.bot_webhook().view().await?,
            flood: FloodView {
                settings: self.services.settings().bot_flood,
                stats,
            },
        })
    }

    pub async fn restart_bot(
        &self,
        take_over: bool,
        admin: AdminId,
    ) -> ServiceResult<BotSettingsView> {
        let runtime = self.services.bot_runtime();
        if take_over {
            let state = runtime.state();
            let crate::BotState::Failed {
                problem: crate::BotProblem::Webhook,
                detail,
                ..
            } = state
            else {
                return Err(ServiceError::rejected(
                    BOT_TAKEOVER_NOT_NEEDED,
                    "бот не подключён к другому сайту, забирать нечего",
                ));
            };
            let mut conn = self.services.store().conn().await?;
            conn.audit()
                .record(&Actor::Admin(admin).audit(
                    "settings.bot_takeover",
                    "settings",
                    None,
                    serde_json::json!({ "from": detail }),
                ))
                .await?;
            drop(conn);
            runtime.request_takeover();
        } else {
            runtime.wake();
        }
        self.bot().await
    }

    pub async fn update_bot(
        &self,
        update: &BotUpdate,
        admin: AdminId,
    ) -> ServiceResult<BotSettingsView> {
        let mut tx = self.services.store().begin().await?;
        let mut changed = Vec::new();
        if let Some(token) = optional_text(update.token.as_deref()) {
            let token = bot_token(&token)?;
            write_secret(
                &mut tx,
                self.services.secrets(),
                secret_keys::BOT_TOKEN,
                Some(&token),
                Some(admin),
            )
            .await?;
            tx.settings()
                .delete(&keys::provider_check(providers::STARS))
                .await?;
            changed.push("token");
        }
        if let Some(username) = &update.username {
            match optional_text(Some(username)) {
                Some(username) => {
                    let username = bot_username(&username)?;
                    tx.settings()
                        .set_as(keys::BOT_USERNAME, &username, Some(admin))
                        .await?;
                }
                None => {
                    tx.settings().delete(keys::BOT_USERNAME).await?;
                }
            }
            changed.push("username");
        }
        if !changed.is_empty() {
            tx.audit()
                .record(&Actor::Admin(admin).audit(
                    "settings.bot",
                    "settings",
                    None,
                    serde_json::json!({ "fields": changed }),
                ))
                .await?;
        }
        let token_given = update
            .token
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty());
        let switched = match update.updates {
            Some(mode) => {
                self.switch_updates(&mut tx, mode, token_given, admin)
                    .await?
            }
            None => false,
        };
        let guarded = match &update.flood {
            Some(flood) => self.save_flood(&mut tx, flood, admin).await?,
            None => false,
        };
        tx.commit().await?;
        if !changed.is_empty() || switched || guarded {
            self.saved().await?;
        }
        self.bot().await
    }

    async fn save_flood(
        &self,
        tx: &mut impl Db,
        flood: &sn_domain::settings::FloodSettings,
        admin: AdminId,
    ) -> ServiceResult<bool> {
        use sn_domain::settings::FloodSettings;
        if let Some(field) = flood.invalid_field() {
            let (min, max) = match field {
                "start_interval_secs" => FloodSettings::START_INTERVAL,
                "messages_per_minute" => FloodSettings::MESSAGES,
                "block_after" => FloodSettings::BLOCK_AFTER,
                "block_minutes" => FloodSettings::BLOCK_MINUTES,
                _ => FloodSettings::REGISTRATIONS,
            };
            return Err(field_error(
                field,
                &format!("целое число от {min} до {max}"),
            ));
        }
        let current = self.services.settings().bot_flood;
        if current == *flood {
            return Ok(false);
        }
        let value =
            serde_json::to_value(flood).map_err(|e| ServiceError::Internal(e.to_string()))?;
        tx.settings()
            .set(keys::BOT_FLOOD, &value, Some(admin))
            .await?;
        let before =
            serde_json::to_value(current).map_err(|e| ServiceError::Internal(e.to_string()))?;
        let mut details = Map::new();
        if let (Value::Object(after), Value::Object(before)) = (&value, &before) {
            for (key, now) in after {
                let was = before.get(key).cloned().unwrap_or(Value::Null);
                if *now != was {
                    details.insert(key.clone(), now.clone());
                    details.insert(format!("{key}_before"), was);
                }
            }
        }
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                crate::bot_flood::SETTINGS_ACTION,
                "settings",
                None,
                Value::Object(details),
            ))
            .await?;
        Ok(true)
    }

    async fn switch_updates(
        &self,
        tx: &mut impl Db,
        mode: sn_domain::settings::UpdatesMode,
        token_given: bool,
        admin: AdminId,
    ) -> ServiceResult<bool> {
        use crate::bot_webhook::{MODE_ACTION, NEEDS_TOKEN, WebhookKeys, mode_word};
        use sn_domain::settings::{BotDelivery, UpdatesMode};
        let current = self.services.settings().bot_updates.clone();
        if current.mode == mode && current.fallback.is_none() {
            return Ok(false);
        }
        if mode == UpdatesMode::Webhook {
            self.services.bot_webhook().check_ready()?;
            if !token_given && self.services.bot_token().is_none() {
                return Err(ServiceError::rejected(
                    NEEDS_TOKEN,
                    "Сначала сохраните токен бота, потом включайте вебхук",
                ));
            }
            write_secret(
                tx,
                self.services.secrets(),
                secret_keys::BOT_WEBHOOK,
                Some(&WebhookKeys::generate().to_secret()),
                Some(admin),
            )
            .await?;
        }
        let value = serde_json::to_value(BotDelivery {
            mode,
            fallback: None,
        })
        .map_err(|e| ServiceError::Internal(e.to_string()))?;
        tx.settings()
            .set(keys::BOT_UPDATES, &value, Some(admin))
            .await?;
        if current.mode != mode {
            tx.audit()
                .record(&Actor::Admin(admin).audit(
                    MODE_ACTION,
                    "settings",
                    None,
                    serde_json::json!({
                        "from": mode_word(current.mode),
                        "to": mode_word(mode),
                    }),
                ))
                .await?;
        }
        Ok(true)
    }

    pub fn module_config(&self, id: ModuleId) -> ModuleConfigView {
        let modules = self.services.modules();
        let schema = modules.get(id).settings_schema();
        let config = public_config(&schema, &modules.config(id).0);
        ModuleConfigView {
            module: id,
            schema,
            config,
        }
    }

    pub async fn update_module_config(
        &self,
        id: ModuleId,
        update: &ModuleConfigUpdate,
        admin: AdminId,
    ) -> ServiceResult<ModuleConfigView> {
        let modules = self.services.modules();
        let module = modules.get(id);
        let schema = module.settings_schema();
        let secrets = self.services.secrets();
        let mut tx = self.services.store().begin().await?;
        let mut stored = tx
            .modules()
            .all()
            .await?
            .into_iter()
            .find(|row| row.module == id.as_str())
            .and_then(|row| row.config.as_object().cloned())
            .unwrap_or_default();
        let before = ModuleConfig(stored.clone());
        let changes = apply_fields(&schema, &update.config, &update.clear, &mut stored)?;
        for field in schema.iter().filter(|f| f.is_secret()) {
            stored.remove(field.key);
        }
        let mut merged = stored.clone();
        for field in schema.iter().filter(|f| f.is_secret()) {
            if let Some(value) = read_secret(
                &mut tx,
                secrets,
                &secret_keys::module(id.as_str(), field.key),
            )
            .await?
            {
                merged.insert(field.key.to_string(), Value::String(value));
            }
        }
        for change in &changes {
            match change {
                SecretChange::Set(key, value) => {
                    merged.insert(key.clone(), Value::String(value.clone()));
                }
                SecretChange::Delete(key) => {
                    merged.remove(key);
                }
            }
        }
        let merged = ModuleConfig(merged);
        module
            .validate_config(&merged)
            .map_err(|e| ServiceError::rejected("modules.config", e.to_string()))?;
        let new_secrets: Vec<&str> = changes
            .iter()
            .filter_map(|change| match change {
                SecretChange::Set(key, _) => Some(key.as_str()),
                SecretChange::Delete(_) => None,
            })
            .collect();
        module
            .validate_change(&before, &merged, &new_secrets)
            .map_err(|e| match e {
                sn_modules::ModuleError::SecretsAgain(message) => {
                    ServiceError::rejected(SECRETS_AGAIN, message)
                }
                other => ServiceError::rejected("modules.config", other.to_string()),
            })?;
        if id == ModuleId::Antibot {
            captcha_domains(&merged, self.services.settings().public_url.as_deref())
                .map_err(|reason| ServiceError::rejected("modules.config", reason))?;
        }
        for change in &changes {
            match change {
                SecretChange::Set(key, value) => {
                    write_secret(
                        &mut tx,
                        secrets,
                        &secret_keys::module(id.as_str(), key),
                        Some(value),
                        Some(admin),
                    )
                    .await?;
                }
                SecretChange::Delete(key) => {
                    write_secret(
                        &mut tx,
                        secrets,
                        &secret_keys::module(id.as_str(), key),
                        None,
                        None,
                    )
                    .await?;
                }
            }
        }
        tx.modules()
            .set_config(id.as_str(), &Value::Object(stored), Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "modules.config",
                "module",
                None,
                serde_json::json!({
                    "module": id,
                    "fields": update.config.keys().collect::<Vec<_>>(),
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.module_config(id))
    }

    pub async fn toggle_module(
        &self,
        id: ModuleId,
        enabled: bool,
        admin: AdminId,
    ) -> ServiceResult<ModuleView> {
        let modules = self.services.modules();
        let view = modules
            .set_enabled(self.services.store(), id, enabled, Some(admin))
            .await?;
        let mut conn = self.services.store().conn().await?;
        let rows = module_secret_rows(&mut conn, self.services.secrets(), modules).await?;
        modules.apply_rows(&rows);
        let view = modules
            .views()
            .into_iter()
            .find(|v| v.info.id == id)
            .unwrap_or(view);
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "modules.toggle",
                "module",
                None,
                serde_json::json!({ "module": id, "enabled": enabled }),
            ))
            .await?;
        drop(conn);
        self.services.notify_settings_changed().await?;
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Vec<SettingField> {
        vec![
            SettingField::secret("api_key", "Ключ", ""),
            SettingField::text("shop_id", "Магазин", ""),
            SettingField::flag("testnet", "Тест", ""),
            SettingField::number("vat", "НДС", ""),
            SettingField::select("mode", "Режим", "", &["a", "b"]),
        ]
    }

    #[test]
    fn секрет_только_на_запись_пустое_значение_не_меняет() {
        let mut stored = Map::new();
        stored.insert("shop_id".into(), Value::String("old".into()));
        let incoming: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "api_key": "", "shop_id": "  new  ", "testnet": "true", "vat": 20, "mode": "b"
        }))
        .unwrap();
        let changes = apply_fields(&schema(), &incoming, &[], &mut stored).unwrap();
        assert!(changes.is_empty());
        assert_eq!(stored["shop_id"], "new");
        assert_eq!(stored["testnet"], true);
        assert_eq!(stored["vat"], "20");
        let incoming: Map<String, Value> =
            serde_json::from_value(serde_json::json!({ "api_key": "s3cr3t" })).unwrap();
        let changes = apply_fields(&schema(), &incoming, &["shop_id".into()], &mut stored).unwrap();
        assert_eq!(
            changes,
            vec![SecretChange::Set("api_key".into(), "s3cr3t".into())]
        );
        assert!(!stored.contains_key("shop_id"));
        assert!(!stored.contains_key("api_key"));
        let changes =
            apply_fields(&schema(), &Map::new(), &["api_key".into()], &mut stored).unwrap();
        assert_eq!(changes, vec![SecretChange::Delete("api_key".into())]);
    }

    #[test]
    fn длинный_секрет_принимается_только_в_поле_для_длинных_значений() {
        let pem = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----",
            "A".repeat(4_000)
        );
        let incoming: Map<String, Value> =
            serde_json::from_value(serde_json::json!({ "api_key": pem.clone() })).unwrap();
        assert!(apply_fields(&schema(), &incoming, &[], &mut Map::new()).is_err());
        let large = vec![SettingField::secret("api_key", "Сертификат", "").large()];
        let changes = apply_fields(&large, &incoming, &[], &mut Map::new()).unwrap();
        assert_eq!(changes, vec![SecretChange::Set("api_key".into(), pem)]);
        let huge: Map<String, Value> = serde_json::from_value(
            serde_json::json!({ "api_key": "A".repeat(MAX_LARGE_FIELD + 1) }),
        )
        .unwrap();
        assert!(apply_fields(&large, &huge, &[], &mut Map::new()).is_err());
    }

    #[test]
    fn неизвестное_поле_и_неверный_тип_отклоняются() {
        let mut stored = Map::new();
        for body in [
            serde_json::json!({ "unknown": 1 }),
            serde_json::json!({ "vat": "двадцать" }),
            serde_json::json!({ "mode": "c" }),
            serde_json::json!({ "testnet": [1] }),
        ] {
            let incoming: Map<String, Value> = serde_json::from_value(body).unwrap();
            assert!(apply_fields(&schema(), &incoming, &[], &mut stored).is_err());
        }
    }

    #[test]
    fn список_хранится_целиком_и_сбрасывается_пустым_значением() {
        let schema = vec![SettingField::list("items", "Список", "")];
        let mut stored = Map::new();
        let items = serde_json::json!([{ "id": "a", "title": " Первый " }, { "id": "b" }]);
        let incoming: Map<String, Value> =
            serde_json::from_value(serde_json::json!({ "items": items.clone() })).unwrap();
        assert!(
            apply_fields(&schema, &incoming, &[], &mut stored)
                .unwrap()
                .is_empty()
        );
        assert_eq!(stored["items"], items);
        assert_eq!(public_config(&schema, &stored)["items"], items);
        for body in [
            serde_json::json!({ "items": "a,b" }),
            serde_json::json!({ "items": { "id": "a" } }),
        ] {
            let incoming: Map<String, Value> = serde_json::from_value(body).unwrap();
            assert!(apply_fields(&schema, &incoming, &[], &mut stored).is_err());
        }
        let reset: Map<String, Value> =
            serde_json::from_value(serde_json::json!({ "items": null })).unwrap();
        apply_fields(&schema, &reset, &[], &mut stored).unwrap();
        assert!(!stored.contains_key("items"));
    }

    #[test]
    fn публичный_вид_не_показывает_секреты() {
        let mut config = Map::new();
        config.insert("api_key".into(), Value::String("s3cr3t".into()));
        config.insert("shop_id".into(), Value::String("42".into()));
        let view = public_config(&schema(), &config);
        assert_eq!(view["api_key"], serde_json::json!({ "filled": true }));
        assert_eq!(view["shop_id"], "42");
        assert!(!serde_json::to_string(&view).unwrap().contains("s3cr3t"));
    }

    #[test]
    fn блок_env_для_вебхуков_remnawave() {
        let secret = generate_webhook_secret();
        assert_eq!(secret.len(), 64);
        assert!(secret.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(sn_remnawave::webhook::validate_secret(&secret).is_ok());
        assert_eq!(
            webhook_env(Some("https://bot.example.com/"), "abc"),
            "WEBHOOK_ENABLED=true\nWEBHOOK_URL=https://bot.example.com/api/remnawave/webhook\nWEBHOOK_SECRET_HEADER=abc"
        );
        assert_eq!(mask_secret(&"7f3c".repeat(16)), "7f3c…7f3c");
        assert_eq!(mask_secret("short"), "…");
    }

    fn variant(id: u32, name: &str, enabled: bool, squads: &[u128]) -> TrialVariant {
        TrialVariant {
            id,
            name: name.into(),
            enabled,
            days: 3,
            squads: squads
                .iter()
                .map(|n| sn_domain::ids::SquadId(uuid::Uuid::from_u128(*n)))
                .collect(),
            traffic: sn_domain::traffic::TrafficPolicy::UNLIMITED,
            devices: 1,
        }
    }

    fn trial(enabled: bool, variants: Vec<TrialVariant>) -> TrialSettings {
        TrialSettings { enabled, variants }
    }

    fn reason(result: ServiceResult<TrialSettings>) -> String {
        match result {
            Err(ServiceError::Rejected { message, .. }) => message,
            other => panic!("ожидалась ошибка проверки: {other:?}"),
        }
    }

    #[test]
    fn варианты_пробного_периода_проверяются_и_получают_номера() {
        let saved = trial(
            true,
            vec![variant(1, "", true, &[1]), variant(5, "Старый", true, &[2])],
        );
        let checked = checked_trial(
            &trial(
                true,
                vec![
                    variant(1, "  Европа ", true, &[1]),
                    variant(0, "Россия", false, &[]),
                    variant(0, "США", true, &[3]),
                ],
            ),
            &saved,
        )
        .unwrap();
        let ids: Vec<u32> = checked.variants.iter().map(|v| v.id).collect();
        assert_eq!(ids, [1, 6, 7]);
        assert_eq!(checked.variants[0].name, "Европа");
        let unnumbered: TrialSettings = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "variants": [
                { "name": "Европа", "squads": [uuid::Uuid::from_u128(1)] },
                { "name": "Россия", "squads": [uuid::Uuid::from_u128(2)] }
            ]
        }))
        .unwrap();
        let ids: Vec<u32> = checked_trial(&unnumbered, &saved)
            .unwrap()
            .variants
            .iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(ids, [6, 7]);
        let single = checked_trial(
            &trial(true, vec![variant(0, "", true, &[1])]),
            &TrialSettings::default(),
        )
        .unwrap();
        assert_eq!(single.variants[0].id, 2);
        assert_eq!(single.variants[0].name, "");
        let off = checked_trial(&trial(false, vec![variant(1, "", true, &[])]), &saved).unwrap();
        assert!(!off.enabled);
        assert!(
            reason(checked_trial(
                &trial(true, vec![variant(1, "", true, &[])]),
                &saved
            ))
            .contains("сквад")
        );
        assert!(
            reason(checked_trial(
                &trial(
                    true,
                    vec![variant(1, "Европа", true, &[1]), variant(2, "", true, &[2])]
                ),
                &saved,
            ))
            .contains("вариант 2: укажите название")
        );
        assert!(
            reason(checked_trial(
                &trial(
                    true,
                    vec![
                        variant(1, "Европа", true, &[1]),
                        variant(2, "европа", true, &[2])
                    ]
                ),
                &saved,
            ))
            .contains("уже есть")
        );
        assert!(
            reason(checked_trial(
                &trial(
                    true,
                    vec![variant(3, "А", true, &[1]), variant(3, "Б", true, &[2])]
                ),
                &saved,
            ))
            .contains("повторяется")
        );
        assert!(
            reason(checked_trial(
                &trial(
                    true,
                    vec![variant(1, "А", false, &[1]), variant(2, "Б", false, &[2])]
                ),
                &saved,
            ))
            .contains("включите хотя бы один вариант")
        );
        assert!(
            reason(checked_trial(
                &trial(true, vec![variant(1, "", true, &[1, 1])]),
                &saved
            ))
            .contains("дважды")
        );
        let mut long = variant(1, &"я".repeat(TRIAL_NAME_MAX + 1), true, &[1]);
        assert!(
            reason(checked_trial(&trial(true, vec![long.clone()]), &saved)).contains("название до")
        );
        long.name = "я".repeat(TRIAL_NAME_MAX);
        assert!(checked_trial(&trial(true, vec![long]), &saved).is_ok());
        let mut short = variant(1, "", true, &[1]);
        short.days = 0;
        assert!(
            reason(checked_trial(&trial(true, vec![short.clone()]), &saved)).contains("от 1 до 90")
        );
        short.days = 3;
        short.devices = 101;
        assert!(reason(checked_trial(&trial(true, vec![short]), &saved)).contains("устройств"));
        let many: Vec<TrialVariant> = (1..=TRIAL_VARIANTS_MAX as u32 + 1)
            .map(|n| variant(n, &format!("Вариант {n}"), true, &[u128::from(n)]))
            .collect();
        assert!(reason(checked_trial(&trial(true, many), &saved)).contains("вариантов"));
        assert!(reason(checked_trial(&trial(true, Vec::new()), &saved)).contains("вариантов"));
        assert!(
            reason(checked_trial(
                &trial(true, vec![variant(TRIAL_ID_MAX + 1, "", true, &[1])]),
                &saved
            ))
            .contains("номер")
        );
    }

    #[test]
    fn имя_и_токен_бота_проверяются() {
        assert_eq!(bot_username("@stealth_vpn_bot").unwrap(), "stealth_vpn_bot");
        assert!(bot_username("бот").is_err());
        assert!(bot_token("123456:AAEhBP0av28N6CLdRmP6gWBJr2cs2hZ1T9Q").is_ok());
        assert!(bot_token("not-a-token").is_err());
    }
}
