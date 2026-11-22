use crate::client::Languages;
use crate::ids::SquadId;
use crate::image::{FAVICON_RULES, ImageKind, ImageRules, LOGO_RULES};
use crate::landing::LandingConfig;
use crate::money::{Currency, Money};
use crate::payment::providers;
use crate::pricing::TopUpLimits;
use crate::traffic::TrafficPolicy;
use chrono::{DateTime, FixedOffset, Offset, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Secret,
    Number,
    Flag,
    Url,
    Select { options: &'static [&'static str] },
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FieldCase {
    pub when: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<&'static str>,
}

impl FieldCase {
    pub const fn new(when: &'static str) -> Self {
        FieldCase {
            when,
            hint: None,
            placeholder: None,
        }
    }

    pub const fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    pub const fn placeholder(mut self, placeholder: &'static str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FieldDepends {
    pub on: &'static str,
    pub cases: &'static [FieldCase],
    pub reset: bool,
}

impl FieldDepends {
    pub fn case(&self, value: &str) -> Option<&'static FieldCase> {
        self.cases.iter().find(|c| c.when == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SettingField {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    pub default: Option<&'static str>,
    pub large: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<&'static [&'static str]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends: Option<FieldDepends>,
}

impl SettingField {
    const fn base(
        key: &'static str,
        label: &'static str,
        hint: &'static str,
        kind: FieldKind,
        required: bool,
    ) -> Self {
        SettingField {
            key,
            label,
            hint,
            kind,
            required,
            default: None,
            large: false,
            labels: None,
            depends: None,
        }
    }

    pub const fn text(key: &'static str, label: &'static str, hint: &'static str) -> Self {
        Self::base(key, label, hint, FieldKind::Text, true)
    }

    pub const fn secret(key: &'static str, label: &'static str, hint: &'static str) -> Self {
        Self::base(key, label, hint, FieldKind::Secret, true)
    }

    pub const fn number(key: &'static str, label: &'static str, hint: &'static str) -> Self {
        Self::base(key, label, hint, FieldKind::Number, false)
    }

    pub const fn flag(key: &'static str, label: &'static str, hint: &'static str) -> Self {
        Self::base(key, label, hint, FieldKind::Flag, false)
    }

    pub const fn url(key: &'static str, label: &'static str, hint: &'static str) -> Self {
        Self::base(key, label, hint, FieldKind::Url, true)
    }

    pub const fn select(
        key: &'static str,
        label: &'static str,
        hint: &'static str,
        options: &'static [&'static str],
    ) -> Self {
        Self::base(key, label, hint, FieldKind::Select { options }, true)
    }

    pub const fn list(key: &'static str, label: &'static str, hint: &'static str) -> Self {
        Self::base(key, label, hint, FieldKind::List, false)
    }

    pub const fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    pub const fn with_default(mut self, value: &'static str) -> Self {
        self.default = Some(value);
        self
    }

    pub const fn large(mut self) -> Self {
        self.large = true;
        self
    }

    pub const fn labels(mut self, labels: &'static [&'static str]) -> Self {
        self.labels = Some(labels);
        self
    }

    pub const fn depends(
        mut self,
        on: &'static str,
        cases: &'static [FieldCase],
        reset: bool,
    ) -> Self {
        self.depends = Some(FieldDepends { on, cases, reset });
        self
    }

    pub const fn is_secret(&self) -> bool {
        matches!(self.kind, FieldKind::Secret)
    }
}

pub fn missing_required(
    schema: &[SettingField],
    config: &serde_json::Map<String, serde_json::Value>,
) -> Vec<&'static str> {
    schema
        .iter()
        .filter(|f| f.required && f.default.is_none())
        .filter(|f| match config.get(f.key) {
            None | Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::String(s)) => s.trim().is_empty(),
            Some(_) => false,
        })
        .map(|f| f.key)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoRenewSettings {
    pub charge_days_before: u32,
    pub retry_days: u32,
    pub enable_for_new: bool,
    pub use_saved_card: bool,
}

impl Default for AutoRenewSettings {
    fn default() -> Self {
        AutoRenewSettings {
            charge_days_before: 1,
            retry_days: 3,
            enable_for_new: true,
            use_saved_card: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrialPlan {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub days: u32,
    pub devices: u32,
    pub traffic: TrafficPolicy,
    pub squads: Vec<SquadId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrialSettings {
    pub enabled: bool,
    pub days: u32,
    pub squads: Vec<SquadId>,
    pub traffic: TrafficPolicy,
    pub devices: u32,
    #[serde(default)]
    pub plans: Vec<TrialPlan>,
}

impl TrialSettings {
    pub fn active_plans(&self) -> Vec<TrialPlan> {
        if !self.enabled {
            return Vec::new();
        }
        if !self.plans.is_empty() {
            return self
                .plans
                .iter()
                .filter(|plan| plan.enabled && !plan.squads.is_empty())
                .cloned()
                .collect();
        }
        vec![TrialPlan {
            id: "default".into(),
            name: "Пробный период".into(),
            enabled: true,
            days: self.days,
            devices: self.devices,
            traffic: self.traffic,
            squads: self.squads.clone(),
        }]
    }
}

impl Default for TrialSettings {
    fn default() -> Self {
        TrialSettings {
            enabled: false,
            days: 3,
            squads: Vec::new(),
            traffic: TrafficPolicy::UNLIMITED,
            devices: 1,
            plans: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpSettings {
    pub min_minor: i64,
    pub max_minor: i64,
}

impl TopUpSettings {
    pub const fn default_for(currency: Currency) -> Self {
        match currency {
            Currency::Usd => TopUpSettings {
                min_minor: 100,
                max_minor: 100_000,
            },
            Currency::Rub => TopUpSettings {
                min_minor: 10_000,
                max_minor: 10_000_000,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaymentOrder {
    pub usd: Vec<String>,
    pub rub: Vec<String>,
}

impl Default for PaymentOrder {
    fn default() -> Self {
        PaymentOrder {
            usd: [
                providers::CRYPTOPAY,
                providers::HELEKET,
                providers::LAVA,
                providers::STARS,
            ]
            .map(String::from)
            .to_vec(),
            rub: [
                providers::PLATEGA,
                providers::YOOKASSA,
                providers::YOOMONEY,
                providers::STARS,
                providers::CLOUDPAYMENTS,
                providers::OVERPAY,
                providers::CRYPTOPAY,
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

impl PaymentOrder {
    pub fn for_currency(&self, currency: Currency) -> &[String] {
        match currency {
            Currency::Usd => &self.usd,
            Currency::Rub => &self.rub,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminDesign {
    #[default]
    Side,
    Top,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminTheme {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientDesign {
    #[default]
    Accent,
    Mono,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientTheme {
    Dark,
    Light,
    #[default]
    Client,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default)]
    pub admin: AdminDesign,
    #[serde(default)]
    pub admin_theme: AdminTheme,
    #[serde(default)]
    pub client: ClientDesign,
    #[serde(default)]
    pub client_theme: ClientTheme,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectMode {
    #[default]
    Ours,
    Remnawave,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectSettings {
    #[serde(default)]
    pub mode: ConnectMode,
    #[serde(default)]
    pub page_config: Option<String>,
}

pub const SITE_TITLE_MAX: usize = 70;
pub const SITE_DESCRIPTION_MAX: usize = 300;

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteSettings {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "yes")]
    pub indexing: bool,
}

impl Default for SiteSettings {
    fn default() -> Self {
        SiteSettings {
            title: None,
            description: None,
            indexing: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteImage {
    Logo,
    Favicon,
}

impl SiteImage {
    pub fn name(self) -> &'static str {
        match self {
            SiteImage::Logo => "logo",
            SiteImage::Favicon => "favicon",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SiteImage::Logo => keys::LOGO,
            SiteImage::Favicon => keys::FAVICON,
        }
    }

    pub fn rules(self) -> &'static ImageRules {
        match self {
            SiteImage::Logo => &LOGO_RULES,
            SiteImage::Favicon => &FAVICON_RULES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub tag: String,
    pub kind: ImageKind,
    pub width: u32,
    pub height: u32,
    pub size: u32,
}

impl ImageMeta {
    pub fn path(&self, image: SiteImage) -> String {
        format!(
            "/api/public/{}/{}.{}",
            image.name(),
            self.tag,
            self.kind.extension()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreSettings {
    pub currency: Currency,
    pub brand_name: String,
    pub public_url: Option<String>,
    pub support_url: Option<String>,
    #[serde(default)]
    pub support_mode: SupportMode,
    pub remna_utc_offset_minutes: i32,
    pub autorenew: AutoRenewSettings,
    pub trial: TrialSettings,
    pub topup: TopUpSettings,
    pub payment_order: PaymentOrder,
    pub invoice_ttl_minutes: u32,
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub connect: ConnectSettings,
    #[serde(default)]
    pub connect_apps: Option<crate::connect::ConnectApps>,
    #[serde(default)]
    pub site: SiteSettings,
    #[serde(default)]
    pub logo: Option<ImageMeta>,
    #[serde(default)]
    pub favicon: Option<ImageMeta>,
    #[serde(default)]
    pub landing: LandingConfig,
    #[serde(default)]
    pub multi_subscriptions: bool,
    #[serde(default)]
    pub client_access: ClientAccess,
    #[serde(default)]
    pub languages: Languages,
    #[serde(default)]
    pub bot_updates: BotDelivery,
    #[serde(default)]
    pub bot_flood: FloodSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloodSettings {
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default = "FloodSettings::start_interval")]
    pub start_interval_secs: u32,
    #[serde(default = "FloodSettings::messages")]
    pub messages_per_minute: u32,
    #[serde(default = "FloodSettings::block_after")]
    pub block_after: u32,
    #[serde(default = "FloodSettings::block_minutes")]
    pub block_minutes: u32,
    #[serde(default = "FloodSettings::registrations")]
    pub registrations_per_minute: u32,
}

impl FloodSettings {
    pub const START_INTERVAL: (u32, u32) = (1, 60);
    pub const MESSAGES: (u32, u32) = (5, 600);
    pub const BLOCK_AFTER: (u32, u32) = (0, 1_000);
    pub const BLOCK_MINUTES: (u32, u32) = (1, 1_440);
    pub const REGISTRATIONS: (u32, u32) = (0, 100_000);

    const fn start_interval() -> u32 {
        3
    }

    const fn messages() -> u32 {
        30
    }

    const fn block_after() -> u32 {
        20
    }

    const fn block_minutes() -> u32 {
        10
    }

    const fn registrations() -> u32 {
        300
    }

    pub fn invalid_field(&self) -> Option<&'static str> {
        let within = |value: u32, (min, max): (u32, u32)| (min..=max).contains(&value);
        if !within(self.start_interval_secs, Self::START_INTERVAL) {
            return Some("start_interval_secs");
        }
        if !within(self.messages_per_minute, Self::MESSAGES) {
            return Some("messages_per_minute");
        }
        if !within(self.block_after, Self::BLOCK_AFTER) {
            return Some("block_after");
        }
        if !within(self.block_minutes, Self::BLOCK_MINUTES) {
            return Some("block_minutes");
        }
        if !within(self.registrations_per_minute, Self::REGISTRATIONS) {
            return Some("registrations_per_minute");
        }
        None
    }
}

impl Default for FloodSettings {
    fn default() -> Self {
        FloodSettings {
            enabled: true,
            start_interval_secs: Self::start_interval(),
            messages_per_minute: Self::messages(),
            block_after: Self::block_after(),
            block_minutes: Self::block_minutes(),
            registrations_per_minute: Self::registrations(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatesMode {
    #[default]
    Polling,
    Webhook,
}

impl UpdatesMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            UpdatesMode::Polling => "polling",
            UpdatesMode::Webhook => "webhook",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    Rejected,
    Undelivered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookFallback {
    pub at: DateTime<Utc>,
    pub reason: FallbackReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotDelivery {
    #[serde(default)]
    pub mode: UpdatesMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<WebhookFallback>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportMode {
    #[default]
    Tickets,
    Link,
    Both,
}

impl SupportMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            SupportMode::Tickets => "tickets",
            SupportMode::Link => "link",
            SupportMode::Both => "both",
        }
    }

    pub const fn needs_link(self) -> bool {
        matches!(self, SupportMode::Link | SupportMode::Both)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiSubscriptions {
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientAccess {
    #[serde(default = "enabled")]
    pub miniapp: bool,
    #[serde(default = "enabled")]
    pub site_login: bool,
    #[serde(default = "enabled")]
    pub self_devices: bool,
    #[serde(default = "enabled")]
    pub self_reissue: bool,
}

const fn enabled() -> bool {
    true
}

impl Default for ClientAccess {
    fn default() -> Self {
        ClientAccess {
            miniapp: true,
            site_login: true,
            self_devices: true,
            self_reissue: true,
        }
    }
}

impl Default for CoreSettings {
    fn default() -> Self {
        CoreSettings {
            currency: Currency::Usd,
            brand_name: "STEALTHNET".into(),
            public_url: None,
            support_url: None,
            support_mode: SupportMode::Tickets,
            remna_utc_offset_minutes: 0,
            autorenew: AutoRenewSettings::default(),
            trial: TrialSettings::default(),
            topup: TopUpSettings::default_for(Currency::Usd),
            payment_order: PaymentOrder::default(),
            invoice_ttl_minutes: 60,
            appearance: Appearance::default(),
            connect: ConnectSettings::default(),
            connect_apps: None,
            site: SiteSettings::default(),
            logo: None,
            favicon: None,
            landing: LandingConfig::standard(),
            multi_subscriptions: false,
            client_access: ClientAccess::default(),
            languages: Languages::default(),
            bot_updates: BotDelivery::default(),
            bot_flood: FloodSettings::default(),
        }
    }
}

impl CoreSettings {
    pub fn image(&self, image: SiteImage) -> Option<&ImageMeta> {
        match image {
            SiteImage::Logo => self.logo.as_ref(),
            SiteImage::Favicon => self.favicon.as_ref(),
        }
    }

    pub fn logo_url(&self) -> Option<String> {
        self.logo.as_ref().map(|logo| logo.path(SiteImage::Logo))
    }

    pub fn support_link(&self) -> Option<&str> {
        if !self.support_mode.needs_link() {
            return None;
        }
        self.support_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
    }

    pub fn support_tickets(&self) -> bool {
        self.support_mode != SupportMode::Link || self.support_link().is_none()
    }

    pub fn miniapp_url(&self) -> Option<String> {
        if !self.client_access.miniapp {
            return None;
        }
        let base = self.public_url.as_deref()?.trim().trim_end_matches('/');
        base.starts_with("https://").then(|| format!("{base}/app"))
    }
}

impl CoreSettings {
    pub fn remna_offset(&self) -> FixedOffset {
        let seconds = self.remna_utc_offset_minutes.clamp(-1439, 1439) * 60;
        FixedOffset::east_opt(seconds).unwrap_or_else(|| Utc.fix())
    }

    pub const fn topup_limits(&self) -> TopUpLimits {
        TopUpLimits {
            min: Money::new(self.topup.min_minor, self.currency),
            max: Money::new(self.topup.max_minor, self.currency),
        }
    }
}

pub const BOT_PROXY: &str = "bot.proxy";
pub const PAYMENTS_PROXY: &str = "payments.proxy";

pub const PROXY_SCHEMES: [&str; 4] = ["http", "https", "socks5", "socks5h"];

pub fn check_proxy(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("адрес прокси пуст".into());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("в адресе прокси не должно быть пробелов".into());
    }
    let url = url::Url::parse(trimmed).map_err(|_| {
        "нужен адрес вида socks5://127.0.0.1:1080 или http://user:pass@host:port".to_string()
    })?;
    if !PROXY_SCHEMES.contains(&url.scheme()) {
        return Err(format!(
            "схема {} не поддерживается: нужны http, https, socks5 или socks5h",
            url.scheme()
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("в адресе прокси нет сервера".into());
    }
    if !url.path().is_empty() && url.path() != "/" {
        return Err("в адресе прокси не должно быть пути".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("в адресе прокси не должно быть параметров после ? и #".into());
    }
    Ok(trimmed.to_string())
}

pub fn mask_proxy(raw: &str) -> String {
    match url::Url::parse(raw.trim()) {
        Ok(mut url) => {
            if url.password().is_some() {
                let _ = url.set_password(Some("***"));
            }
            url.to_string()
        }
        Err(_) => raw.trim().to_string(),
    }
}

pub mod keys {
    pub const BOT_PROXY: &str = super::BOT_PROXY;
    pub const PAYMENTS_PROXY: &str = super::PAYMENTS_PROXY;
    pub const CURRENCY: &str = "core.currency";
    pub const BRAND_NAME: &str = "core.brand_name";
    pub const PUBLIC_URL: &str = "core.public_url";
    pub const SUPPORT_URL: &str = "core.support_url";
    pub const SUPPORT_MODE: &str = "core.support_mode";
    pub const REMNA_URL: &str = "remnawave.url";
    pub const REMNA_GUARD: &str = "remnawave.guard";
    pub const REMNA_UTC_OFFSET: &str = "remnawave.utc_offset_minutes";
    pub const AUTORENEW: &str = "billing.autorenew";
    pub const TRIAL: &str = "billing.trial";
    pub const TOPUP: &str = "billing.topup";
    pub const PAYMENT_ORDER: &str = "billing.payment_order";
    pub const INVOICE_TTL: &str = "billing.invoice_ttl_minutes";
    pub const MULTI_SUBSCRIPTIONS: &str = "billing.multi_subscriptions";
    pub const CLIENT_ACCESS: &str = "core.client_access";
    pub const APPEARANCE: &str = "core.appearance";
    pub const CONNECT: &str = "core.connect";
    pub const CONNECT_APPS: &str = "core.connect_apps";
    pub const BOT_USERNAME: &str = "bot.username";
    pub const SITE: &str = "core.site";
    pub const LOGO: &str = "core.logo";
    pub const FAVICON: &str = "core.favicon";
    pub const LANDING: &str = "core.landing";
    pub const LANGUAGES: &str = "core.languages";
    pub const BOT_UPDATES: &str = "bot.updates";
    pub const BOT_FLOOD: &str = "bot.flood";

    pub fn provider(id: &str) -> String {
        format!("payments.{id}")
    }

    pub const PROVIDER_CHECKS: &str = "payments.check.";

    pub fn provider_check(id: &str) -> String {
        format!("{PROVIDER_CHECKS}{id}")
    }

    pub fn module(id: &str) -> String {
        format!("modules.{id}")
    }
}

pub mod secret_keys {
    pub const REMNA_TOKEN: &str = "remnawave.token";
    pub const REMNA_COOKIE: &str = "remnawave.cookie";
    pub const REMNA_API_KEY: &str = "remnawave.x_api_key";
    pub const REMNA_WEBHOOK_SECRET: &str = "remnawave.webhook_secret";
    pub const BOT_TOKEN: &str = "bot.token";
    pub const BOT_WEBHOOK: &str = "bot.webhook_secret";

    pub fn provider(id: &str, field: &str) -> String {
        format!("payments.{id}.{field}")
    }

    pub fn module(id: &str, field: &str) -> String {
        format!("modules.{id}.{field}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn адреса_прокси_проверяются_и_пароль_скрывается() {
        for good in [
            "http://127.0.0.1:8080",
            "https://proxy.example.com:443",
            "socks5://127.0.0.1:1080",
            "socks5h://user:pass@proxy.example.com:1080",
            " http://user:pass@proxy.example.com:3128 ",
        ] {
            assert!(check_proxy(good).is_ok(), "{good}");
        }
        for bad in [
            "ftp://127.0.0.1:21",
            "socks5://",
            "http://proxy.example.com:8080/path",
            "http://proxy.example.com:8080?x=1",
            "http://proxy.example.com:8080#x",
            "http://proxy.example.com:8080 with space",
            "proxy.example.com:8080",
        ] {
            assert!(check_proxy(bad).is_err(), "{bad}");
        }
        assert_eq!(
            mask_proxy("socks5://user:secret@127.0.0.1:1080"),
            "socks5://user:***@127.0.0.1:1080"
        );
        assert_eq!(
            mask_proxy("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080/"
        );
        assert_eq!(mask_proxy("not a url secret"), "not a url secret");
    }

    #[test]
    fn ссылка_поддержки_видна_только_в_режимах_со_ссылкой() {
        let with = |mode, url: Option<&str>| CoreSettings {
            support_mode: mode,
            support_url: url.map(str::to_string),
            ..CoreSettings::default()
        };
        let chat = Some("https://t.me/help_chat");
        let tickets = with(SupportMode::Tickets, chat);
        assert_eq!(
            (tickets.support_link(), tickets.support_tickets()),
            (None, true)
        );
        let link = with(SupportMode::Link, chat);
        assert_eq!((link.support_link(), link.support_tickets()), (chat, false));
        let both = with(SupportMode::Both, chat);
        assert_eq!((both.support_link(), both.support_tickets()), (chat, true));
        let empty = with(SupportMode::Link, None);
        assert_eq!(
            (empty.support_link(), empty.support_tickets()),
            (None, true)
        );
    }

    #[test]
    fn обязательные_поля_ищутся_по_схеме() {
        let schema = [
            SettingField::secret("api_token", "Токен", ""),
            SettingField::flag("testnet", "Тестовая сеть", ""),
            SettingField::text("shop", "Магазин", "").with_default("main"),
        ];
        let mut config = serde_json::Map::new();
        assert_eq!(missing_required(&schema, &config), ["api_token"]);
        config.insert("api_token".into(), serde_json::json!("  "));
        assert_eq!(missing_required(&schema, &config), ["api_token"]);
        config.insert("api_token".into(), serde_json::json!("abc"));
        assert!(missing_required(&schema, &config).is_empty());
    }

    #[test]
    fn пробники_поддерживают_несколько_локаций() {
        let uuid = |n: u128| crate::ids::SquadId(uuid::Uuid::from_u128(n));
        let plan = |id: &str, enabled: bool, days: u32, squad: u128| TrialPlan {
            id: id.into(),
            name: format!("Пробник {id}"),
            enabled,
            days,
            devices: 1,
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![uuid(squad)],
        };
        let legacy = TrialSettings {
            enabled: true,
            days: 5,
            squads: vec![uuid(9)],
            ..TrialSettings::default()
        };
        let fallback = legacy.active_plans();
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].id, "default");
        assert_eq!(fallback[0].days, 5);
        let multi = TrialSettings {
            enabled: true,
            plans: vec![
                plan("ru", true, 3, 1),
                plan("eu", false, 1, 2),
                plan("us", true, 7, 3),
            ],
            ..TrialSettings::default()
        };
        let active: Vec<String> = multi.active_plans().into_iter().map(|p| p.id).collect();
        assert_eq!(active, ["ru", "us"]);
        let none = TrialSettings {
            enabled: true,
            plans: vec![plan("ru", false, 3, 1)],
            ..TrialSettings::default()
        };
        assert!(none.active_plans().is_empty());
        let off = TrialSettings {
            enabled: false,
            plans: vec![plan("ru", true, 3, 1)],
            ..TrialSettings::default()
        };
        assert!(off.active_plans().is_empty());
        let old: TrialSettings = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "days": 2,
            "squads": [],
            "devices": 1,
        }))
        .unwrap();
        assert!(old.plans.is_empty());
    }

    #[test]
    fn автопродление_по_умолчанию_как_в_настройках() {
        let s = AutoRenewSettings::default();
        assert_eq!(
            (
                s.charge_days_before,
                s.retry_days,
                s.enable_for_new,
                s.use_saved_card
            ),
            (1, 3, true, true)
        );
        let order = PaymentOrder::default();
        assert_eq!(order.for_currency(Currency::Usd)[0], "cryptopay");
        assert_eq!(order.for_currency(Currency::Rub)[0], "platega");
    }

    #[test]
    fn crypto_pay_есть_в_порядке_способов_для_рублей() {
        let order = PaymentOrder::default();
        assert!(order.rub.iter().any(|id| id == providers::CRYPTOPAY));
        assert_eq!(
            order
                .rub
                .iter()
                .filter(|id| *id == providers::CRYPTOPAY)
                .count(),
            1
        );
        assert_eq!(order.usd[0], providers::CRYPTOPAY);
    }
}
