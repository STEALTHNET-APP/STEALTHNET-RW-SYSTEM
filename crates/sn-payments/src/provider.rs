use crate::error::{PaymentError, PaymentResult};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::ids::{ClientId, PaymentId};
use sn_domain::money::{Amount, Currency, Money};
use sn_domain::payment::NoticeKind;
use sn_domain::plan::Cycle;
use sn_domain::settings::{SettingField, missing_required};
use std::net::IpAddr;
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderConfig(pub serde_json::Map<String, serde_json::Value>);

impl ProviderConfig {
    pub fn new(map: serde_json::Map<String, serde_json::Value>) -> Self {
        ProviderConfig(map)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn require(&self, key: &'static str) -> PaymentResult<&str> {
        self.get(key)
            .ok_or_else(|| PaymentError::NotConfigured(format!("не задано поле {key}")))
    }

    pub fn flag(&self, key: &str) -> bool {
        match self.0.get(key) {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => matches!(s.trim(), "true" | "1" | "yes"),
            _ => false,
        }
    }

    pub fn set(&mut self, key: &str, value: serde_json::Value) {
        self.0.insert(key.to_string(), value);
    }

    pub fn integer(&self, key: &str) -> Option<i64> {
        match self.0.get(key) {
            Some(serde_json::Value::Number(n)) => n.as_i64(),
            Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn public_view(
        &self,
        schema: &[SettingField],
    ) -> serde_json::Map<String, serde_json::Value> {
        schema
            .iter()
            .map(|field| {
                let value = if field.is_secret() {
                    serde_json::json!({ "filled": self.get(field.key).is_some() })
                } else {
                    self.0
                        .get(field.key)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                };
                (field.key.to_string(), value)
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    External,
    Balance,
    Stars,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClientLabel {
    pub title: &'static str,
    pub hint: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PayMethod {
    pub code: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
    pub min_minor: Option<i64>,
}

impl PayMethod {
    pub const fn new(code: &'static str, title: &'static str, hint: &'static str) -> Self {
        PayMethod {
            code,
            title,
            hint,
            min_minor: None,
        }
    }

    pub const fn min_minor(mut self, minor: i64) -> Self {
        self.min_minor = Some(minor);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderIssue {
    pub at: DateTime<Utc>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRef {
    pub client_id: ClientId,
    pub public_id: Uuid,
    pub telegram_id: Option<i64>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssuedInvoice<'a> {
    pub external_id: Option<&'a str>,
    pub payload: &'a serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceRequest {
    pub payment_id: PaymentId,
    pub payment_public_id: Uuid,
    pub order_public_id: Uuid,
    pub amount: Amount,
    pub description: String,
    pub client: ClientRef,
    pub return_url: Option<String>,
    pub webhook_url: String,
    pub expires_in: Duration,
    pub save_payment_method: bool,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub plan: Option<PlanBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invoice {
    pub pay_url: String,
    pub external_id: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRequest {
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub remote_ip: Option<IpAddr>,
    pub route_suffix: Option<String>,
    pub query: Option<String>,
}

impl WebhookRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BuyerPurpose {
    TopUp,
    Plan { plan: String, series: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buyer {
    pub telegram_id: i64,
    pub txid: String,
    pub purpose: BuyerPurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanBinding {
    pub tariff_id: i64,
    pub period_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewalRef {
    pub series: String,
    pub txid: String,
    #[serde(default)]
    pub charged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PaymentRef {
    Payment(PaymentId),
    PublicId(Uuid),
    ProviderTxid(String),
    Buyer(Buyer),
    Renewal(RenewalRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderStatus {
    Pending,
    Paid,
    Failed,
    Canceled,
    Refunded,
    PartiallyRefunded { refunded: Amount },
    RefundDeclined,
    Underpaid { received: Amount },
    Overpaid { received: Amount },
    Held,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderNotice {
    pub kind: NoticeKind,
    pub text: String,
}

impl ProviderNotice {
    pub fn new(kind: NoticeKind, text: impl Into<String>) -> Self {
        ProviderNotice {
            kind,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenewalEvent {
    Failed {
        series: String,
        reason: Option<String>,
    },
    Cancelled {
        contract: String,
        at: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutcomeSignal {
    Notice { notice: ProviderNotice },
    MethodRevoked,
    Precheck { account: Option<String> },
    Renewal { event: RenewalEvent },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrecheckVerdict {
    Accept,
    UnknownInvoice,
    WrongAccount,
    WrongAmount,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMethod {
    pub token: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebhookReply {
    Empty {
        status: u16,
    },
    Text {
        status: u16,
        body: String,
    },
    Json {
        status: u16,
        body: serde_json::Value,
    },
}

impl WebhookReply {
    pub const fn ok() -> Self {
        WebhookReply::Empty { status: 200 }
    }

    pub const fn status(&self) -> u16 {
        match self {
            WebhookReply::Empty { status }
            | WebhookReply::Text { status, .. }
            | WebhookReply::Json { status, .. } => *status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookOutcome {
    pub event_key: String,
    pub payment: Option<PaymentRef>,
    pub provider_txid: Option<String>,
    pub status: ProviderStatus,
    pub amount: Option<Amount>,
    pub saved_method: Option<SavedMethod>,
    pub reply: WebhookReply,
    #[serde(default)]
    pub signal: Option<OutcomeSignal>,
}

impl WebhookOutcome {
    pub fn notice(&self) -> Option<&ProviderNotice> {
        match &self.signal {
            Some(OutcomeSignal::Notice { notice }) => Some(notice),
            _ => None,
        }
    }

    pub fn method_revoked(&self) -> bool {
        matches!(self.signal, Some(OutcomeSignal::MethodRevoked))
    }

    pub fn renewal_event(&self) -> Option<&RenewalEvent> {
        match &self.signal {
            Some(OutcomeSignal::Renewal { event }) => Some(event),
            _ => None,
        }
    }

    pub fn with_notice(mut self, kind: NoticeKind, text: impl Into<String>) -> Self {
        self.signal = Some(OutcomeSignal::Notice {
            notice: ProviderNotice::new(kind, text),
        });
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenewalPlan {
    pub binding: PlanBinding,
    pub amount: Money,
    pub offer: String,
    pub product: Option<String>,
    pub periodicity: String,
    pub name: String,
    pub every: &'static str,
    pub min_days: u32,
    pub max_days: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenewalCancel {
    Cancelled,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenewalStatus {
    Active,
    Cancelled,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewalCharge {
    pub id: String,
    pub at: Option<DateTime<Utc>>,
    pub paid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewalState {
    pub status: RenewalStatus,
    pub paid_until: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub charges: Vec<RenewalCharge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundRequest {
    pub payment_id: PaymentId,
    pub provider_txid: String,
    pub amount: Amount,
    pub reason: Option<String>,
    pub key: Uuid,
    pub email: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundOutcome {
    pub provider_refund_id: Option<String>,
    pub completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    KeysRejected,
    SettingsRejected,
    Unreachable,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookState {
    Same,
    Other,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCheck {
    pub label: String,
    pub state: HookState,
    pub address: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxTrace {
    pub verified_at: Option<DateTime<Utc>>,
    pub rejected_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionCheck {
    pub status: CheckStatus,
    pub summary: String,
    #[serde(default)]
    pub details: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<HookCheck>,
    #[serde(default)]
    pub inbox: Option<InboxTrace>,
}

pub const NO_CHECK_REQUEST: &str = "Проверить без оплаты нельзя: у платёжной системы нет запроса, \
     который проверяет ключи и ничего не создаёт. Проверьте тестовым платежом на небольшую сумму";

impl ConnectionCheck {
    pub fn new(status: CheckStatus, summary: impl Into<String>) -> Self {
        ConnectionCheck {
            status,
            summary: summary.into(),
            details: Vec::new(),
            hooks: Vec::new(),
            inbox: None,
        }
    }

    pub fn ok(summary: impl Into<String>) -> Self {
        Self::new(CheckStatus::Ok, summary)
    }

    pub fn manual(summary: impl Into<String>) -> Self {
        Self::new(CheckStatus::Manual, summary)
    }

    pub fn detail(mut self, text: impl Into<String>) -> Self {
        self.details.push(text.into());
        self
    }

    pub fn failed(err: &PaymentError) -> Self {
        let (status, summary) = match err {
            PaymentError::NotConfigured(reason) => (
                CheckStatus::SettingsRejected,
                format!("Настройки не готовы: {reason}"),
            ),
            PaymentError::Rejected(reason) => (
                CheckStatus::KeysRejected,
                format!("Платёжная система не приняла запрос: {reason}"),
            ),
            PaymentError::BadSignature => (
                CheckStatus::KeysRejected,
                "Платёжная система не приняла подпись запроса: проверьте ключ".to_string(),
            ),
            PaymentError::Http {
                status: status @ (401 | 403),
            } => (
                CheckStatus::KeysRejected,
                format!("Платёжная система не приняла ключи (HTTP {status})"),
            ),
            PaymentError::Http {
                status: status @ (429 | 500..=599),
            } => (
                CheckStatus::Unreachable,
                format!("Платёжная система временно не отвечает (HTTP {status}), попробуйте позже"),
            ),
            PaymentError::Http { status } => (
                CheckStatus::KeysRejected,
                format!("Платёжная система отказала в запросе (HTTP {status})"),
            ),
            PaymentError::Transport(reason) => (
                CheckStatus::Unreachable,
                format!(
                    "Не удалось связаться с платёжной системой: {reason}. Проверьте, что сервер панели выходит в интернет, и повторите позже"
                ),
            ),
            PaymentError::BadPayload(reason) => (
                CheckStatus::Unreachable,
                format!("Ответ платёжной системы не разобран: {reason}"),
            ),
            other => (CheckStatus::SettingsRejected, other.to_string()),
        };
        Self::new(status, summary)
    }

    pub fn scrub(&mut self, secrets: &[&str]) {
        let clean = |text: &mut String| {
            for secret in secrets.iter().map(|s| s.trim()).filter(|s| s.len() >= 6) {
                if text.contains(secret) {
                    *text = text.replace(secret, "•••");
                }
            }
        };
        clean(&mut self.summary);
        self.details.iter_mut().for_each(clean);
        for hook in &mut self.hooks {
            clean(&mut hook.label);
            if let Some(address) = hook.address.as_mut() {
                clean(address);
            }
            if let Some(note) = hook.note.as_mut() {
                clean(note);
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckContext {
    pub webhook_url: Option<String>,
}

pub fn hook_address(address: &str) -> String {
    let address = address.trim();
    let without_query = address.split(['?', '#']).next().unwrap_or(address);
    without_query.trim_end_matches('/').to_string()
}

pub fn same_hook(expected: &str, actual: &str) -> bool {
    let normalize = |raw: &str| -> Option<String> {
        let url = reqwest::Url::parse(raw.trim()).ok()?;
        let host = url.host_str()?.to_ascii_lowercase();
        let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
        let path = url.path().trim_end_matches('/').to_string();
        let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
        Some(format!("{}://{host}{port}{path}{query}", url.scheme()))
    };
    match (normalize(expected), normalize(actual)) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => false,
    }
}

#[async_trait]
pub trait PaymentProvider: Send + Sync + 'static {
    fn id(&self) -> &'static str;

    fn title(&self) -> &'static str;

    fn kind(&self) -> ProviderKind {
        ProviderKind::External
    }

    fn currencies(&self) -> &'static [Currency];

    fn client_label(&self, currency: Currency, config: &ProviderConfig) -> ClientLabel;

    fn methods(&self, _currency: Currency) -> Vec<PayMethod> {
        Vec::new()
    }

    fn method(&self, currency: Currency, code: &str) -> Option<PayMethod> {
        self.methods(currency).into_iter().find(|m| m.code == code)
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        Vec::new()
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        missing_required(&self.settings_schema(), &config.0).is_empty()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        let missing = missing_required(&self.settings_schema(), &config.0);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(PaymentError::NotConfigured(format!(
                "не заполнены поля: {}",
                missing.join(", ")
            )))
        }
    }

    fn enabled_by_default(&self) -> bool {
        false
    }

    fn accepts_http_webhooks(&self) -> bool {
        false
    }

    fn verifies_webhooks_by_api(&self) -> bool {
        false
    }

    async fn confirm_repeat(
        &self,
        _config: &ProviderConfig,
        _outcome: &WebhookOutcome,
    ) -> PaymentResult<bool> {
        Ok(true)
    }

    fn polls_status(&self) -> bool {
        false
    }

    fn issue(&self) -> Option<ProviderIssue> {
        None
    }

    fn supports_recurring(&self) -> bool {
        false
    }

    fn supports_refunds(&self) -> bool {
        false
    }

    fn supports_refund_polling(&self) -> bool {
        false
    }

    fn polls_by_payment_id(&self) -> bool {
        false
    }

    fn status_by_order(&self) -> bool {
        false
    }

    fn precheck_reply(&self, _verdict: PrecheckVerdict) -> WebhookReply {
        WebhookReply::ok()
    }

    fn needs_telegram(&self) -> bool {
        false
    }

    fn fixed_amounts(&self, _config: &ProviderConfig, _currency: Currency) -> Option<Vec<Money>> {
        None
    }

    fn plan_binding(&self, _config: &ProviderConfig, _plan: &str) -> Option<PlanBinding> {
        None
    }

    fn renewals_enabled(&self, _config: &ProviderConfig) -> bool {
        false
    }

    fn renewal_plans(&self, _config: &ProviderConfig) -> Vec<RenewalPlan> {
        Vec::new()
    }

    fn renewal_cycle(&self, _periodicity: &str) -> Option<Cycle> {
        None
    }

    async fn cancel_renewal(
        &self,
        _config: &ProviderConfig,
        _series: &str,
        _email: &str,
    ) -> PaymentResult<RenewalCancel> {
        Err(PaymentError::Unsupported("отмена автопродления"))
    }

    async fn fetch_renewal(
        &self,
        _config: &ProviderConfig,
        _series: &str,
    ) -> PaymentResult<RenewalState> {
        Err(PaymentError::Unsupported("сверка автопродления"))
    }

    async fn confirm_renewal(
        &self,
        _config: &ProviderConfig,
        _series: &str,
        _txid: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        Ok(None)
    }

    async fn catalog(&self, _config: &ProviderConfig) -> PaymentResult<Option<serde_json::Value>> {
        Ok(None)
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice>;

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome>;

    async fn fetch_status(
        &self,
        _config: &ProviderConfig,
        _external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        Ok(None)
    }

    async fn cancel_invoice(
        &self,
        _config: &ProviderConfig,
        _invoice: &IssuedInvoice<'_>,
    ) -> PaymentResult<()> {
        Err(PaymentError::Unsupported("отмена счёта"))
    }

    async fn charge_saved(
        &self,
        _config: &ProviderConfig,
        _request: &InvoiceRequest,
        _token: &str,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::Unsupported("автосписание"))
    }

    async fn refund(
        &self,
        _config: &ProviderConfig,
        _request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        Err(PaymentError::Unsupported("возврат через платёжную систему"))
    }

    async fn fetch_refund(
        &self,
        _config: &ProviderConfig,
        _provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        Ok(None)
    }

    async fn check_connection(
        &self,
        _config: &ProviderConfig,
        _context: &CheckContext,
    ) -> ConnectionCheck {
        ConnectionCheck::manual(NO_CHECK_REQUEST)
    }

    fn api_url(&self, _config: &ProviderConfig) -> Option<String> {
        None
    }

    fn own_api_url(&self, _config: &ProviderConfig) -> Option<String> {
        None
    }
}

pub fn http_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")))
}

pub static PAYMENTS_HTTP: sn_egress::Profile = sn_egress::Profile {
    name: "payments",
    build: http_builder,
};

pub fn http_client(provider: &'static str) -> sn_egress::Http {
    sn_egress::Http::new(&PAYMENTS_HTTP, sn_egress::Route::Payments, Some(provider))
}

pub fn telegram_client() -> sn_egress::Http {
    sn_egress::Http::new(&PAYMENTS_HTTP, sn_egress::Route::Telegram, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ошибки_запроса_становятся_понятным_итогом_проверки() {
        let status = |err: PaymentError| ConnectionCheck::failed(&err).status;
        assert_eq!(
            status(PaymentError::Transport("operation timed out".into())),
            CheckStatus::Unreachable
        );
        assert_eq!(
            status(PaymentError::Http { status: 502 }),
            CheckStatus::Unreachable
        );
        assert_eq!(
            status(PaymentError::Http { status: 429 }),
            CheckStatus::Unreachable
        );
        assert_eq!(
            status(PaymentError::Http { status: 401 }),
            CheckStatus::KeysRejected
        );
        assert_eq!(
            status(PaymentError::Rejected("нет доступа".into())),
            CheckStatus::KeysRejected
        );
        assert_eq!(
            status(PaymentError::NotConfigured("не задано поле api_key".into())),
            CheckStatus::SettingsRejected
        );
        assert_eq!(
            status(PaymentError::BadPayload("тело не JSON".into())),
            CheckStatus::Unreachable
        );
    }

    #[test]
    fn ключи_вычищаются_из_итога_проверки() {
        let mut check = ConnectionCheck::new(
            CheckStatus::KeysRejected,
            "Платёжная система не приняла запрос: key sk_live_12345 is revoked",
        )
        .detail("Ответ: sk_live_12345");
        check.hooks.push(HookCheck {
            label: "Pay".into(),
            state: HookState::Other,
            address: Some("https://old.example.com/hook/sk_live_12345".into()),
            note: Some("s".into()),
        });
        check.scrub(&["sk_live_12345", "s", "  "]);
        let text = serde_json::to_string(&check).unwrap();
        assert!(!text.contains("sk_live_12345"), "{text}");
        assert!(check.summary.ends_with("key ••• is revoked"));
        assert_eq!(check.hooks[0].note.as_deref(), Some("s"));
    }

    #[test]
    fn адреса_уведомлений_сравниваются_без_мелочей() {
        let ours = "https://panel.example.com/api/webhooks/pay/cloudpayments/pay";
        assert!(same_hook(ours, ours));
        assert!(same_hook(
            ours,
            " HTTPS://Panel.Example.com/api/webhooks/pay/cloudpayments/pay/ "
        ));
        assert!(!same_hook(
            ours,
            "http://panel.example.com/api/webhooks/pay/cloudpayments/pay"
        ));
        assert!(!same_hook(
            ours,
            "https://panel.example.com/api/webhooks/pay/cloudpayments/check"
        ));
        assert!(!same_hook(ours, &format!("{ours}?x=1")));
        assert!(!same_hook(ours, "не адрес"));
        assert_eq!(
            hook_address("https://old.example.com/cp/pay/?key=secret#frag"),
            "https://old.example.com/cp/pay"
        );
    }
}
