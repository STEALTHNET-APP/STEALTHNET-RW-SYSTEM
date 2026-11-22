use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use uuid::Uuid;

const API: &str = "https://api-pay.overpay.io";
const CERTIFICATE: &str = "certificate";
const PAGE: &str = "page";
const CARD: &str = "card";
const SBP: &str = "sbp";
const METHODS: [PayMethod; 3] = [
    PayMethod::new(PAGE, "Все способы", "Выбор способа на странице Overpay"),
    PayMethod::new(CARD, "Карта", "Банковская карта"),
    PayMethod::new(SBP, "СБП", "Через приложение банка"),
];
const LIFETIME_MINUTES: (i64, i64) = (5, 144_000);
const KEY_LABELS: [&str; 3] = ["PRIVATE KEY", "RSA PRIVATE KEY", "EC PRIVATE KEY"];
const ATTEMPTS_CAPACITY: usize = 1024;
const IN_PROGRESS: [&str; 2] = ["refund_in_progress", "processing"];

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct Pem {
    certificates: Vec<String>,
    key_label: String,
    key: String,
}

impl Pem {
    fn parse(text: &str) -> PaymentResult<Pem> {
        let mut certificates = Vec::new();
        let mut key = None;
        for (label, body) in sections(text)? {
            if label.contains("ENCRYPTED") || body.contains("ENCRYPTED") {
                return Err(PaymentError::NotConfigured(
                    "ключ сертификата Overpay зашифрован паролем: выгрузите его из .p12 с параметром -nodes"
                        .into(),
                ));
            }
            if !body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
                || body.is_empty()
            {
                return Err(broken_pem());
            }
            if label == "CERTIFICATE" {
                certificates.push(body);
            } else if KEY_LABELS.contains(&label.as_str()) {
                if key.is_some() {
                    return Err(PaymentError::NotConfigured(
                        "в поле сертификата Overpay больше одного закрытого ключа".into(),
                    ));
                }
                key = Some((label, body));
            }
        }
        let Some((key_label, key)) = key else {
            return Err(PaymentError::NotConfigured(
                "в поле сертификата Overpay нет закрытого ключа (PRIVATE KEY)".into(),
            ));
        };
        if certificates.is_empty() {
            return Err(PaymentError::NotConfigured(
                "в поле сертификата Overpay нет сертификата (CERTIFICATE)".into(),
            ));
        }
        Ok(Pem {
            certificates,
            key_label,
            key,
        })
    }

    fn ordered(&self, leaf: usize) -> String {
        let mut text = String::new();
        if let Some(first) = self.certificates.get(leaf) {
            text.push_str(&block("CERTIFICATE", first));
        }
        for (index, certificate) in self.certificates.iter().enumerate() {
            if index != leaf {
                text.push_str(&block("CERTIFICATE", certificate));
            }
        }
        text.push_str(&block(&self.key_label, &self.key));
        text
    }
}

fn broken_pem() -> PaymentError {
    PaymentError::NotConfigured(
        "сертификат Overpay не разобран: вставьте весь текст файла .pem вместе со строками BEGIN и END"
            .into(),
    )
}

fn sections(text: &str) -> PaymentResult<Vec<(String, String)>> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN ") {
        let after = &rest[start + "-----BEGIN ".len()..];
        let label_end = after.find("-----").ok_or_else(broken_pem)?;
        let label = after[..label_end].trim().to_string();
        let body_and_tail = &after[label_end + "-----".len()..];
        let end = format!("-----END {label}-----");
        let body_end = body_and_tail.find(&end).ok_or_else(broken_pem)?;
        let body: String = body_and_tail[..body_end]
            .replace("\\n", "")
            .replace("\\r", "")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        found.push((label, body));
        rest = &body_and_tail[body_end + end.len()..];
    }
    if found.is_empty() {
        return Err(broken_pem());
    }
    Ok(found)
}

fn block(label: &str, body: &str) -> String {
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(line));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

pub fn hook_token(config: &ProviderConfig) -> PaymentResult<String> {
    let pem = Pem::parse(config.require(CERTIFICATE)?)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(pem.key.as_bytes())
        .map_err(|_| PaymentError::NotConfigured("ключ сертификата Overpay пуст".into()))?;
    mac.update(b"stealthnet:overpay:webhook:");
    mac.update(config.require("project_id")?.as_bytes());
    Ok(hex::encode(&mac.finalize().into_bytes()[..16]))
}

fn order_id(raw: &str) -> PaymentResult<&str> {
    let id = raw.trim();
    if !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(util::bad("номер заказа Overpay в недопустимом формате"))
    }
}

fn refund_ref(raw: &str) -> PaymentResult<(&str, i64)> {
    let (id, target) = raw
        .split_once(':')
        .ok_or_else(|| util::bad("номер возврата Overpay в недопустимом формате"))?;
    let target = target
        .parse::<i64>()
        .ok()
        .filter(|t| *t > 0)
        .ok_or_else(|| util::bad("номер возврата Overpay в недопустимом формате"))?;
    Ok((order_id(id)?, target))
}

fn wire_method(code: Option<&str>) -> PaymentResult<Option<&'static str>> {
    match code {
        None | Some(PAGE) => Ok(None),
        Some(CARD) => Ok(Some("card")),
        Some(SBP) => Ok(Some("fps")),
        Some(_) => Err(PaymentError::Rejected(
            "этот способ оплаты недоступен".into(),
        )),
    }
}

fn lifetime(ttl: chrono::Duration) -> i64 {
    ttl.num_minutes()
        .clamp(LIFETIME_MINUTES.0, LIFETIME_MINUTES.1)
}

fn failure_text(body: &Json) -> Option<&str> {
    ["message", "error", "detail", "description"]
        .iter()
        .find_map(|key| match body.get(key) {
            Some(Json::String(text)) => Some(text.as_str()),
            Some(object @ Json::Object(_)) => object.get("message").and_then(Json::as_str),
            _ => None,
        })
}

fn failure(reply: &util::Reply) -> PaymentError {
    if String::from_utf8_lossy(&reply.body)
        .to_ascii_lowercase()
        .contains("ssl certificate")
    {
        return PaymentError::Rejected(
            "Overpay не принял сертификат клиента, проверьте его в настройках".into(),
        );
    }
    let parsed = Json::parse(&reply.body);
    reply.failure(parsed.as_ref().and_then(failure_text))
}

fn single(body: &Json) -> PaymentResult<Option<&Json>> {
    let orders = body
        .get("orders")
        .and_then(Json::as_array)
        .ok_or_else(|| util::bad("в ответе Overpay нет списка orders"))?;
    match orders {
        [] => Ok(None),
        [order] => Ok(Some(order)),
        _ => Err(util::bad("Overpay вернул несколько заказов вместо одного")),
    }
}

struct Operation {
    kind: String,
    status: String,
    amount: i64,
    created: String,
    message: Option<String>,
}

struct Order {
    id: String,
    status: String,
    currency: PayCurrency,
    amount: Option<i64>,
    charged: Option<i64>,
    refunded: i64,
    merchant: Option<String>,
    failure: Option<String>,
    operations: Vec<Operation>,
}

impl Order {
    fn parse(item: &Json) -> PaymentResult<Order> {
        let id = item
            .get("id")
            .and_then(Json::scalar_text)
            .ok_or_else(|| util::bad("в заказе Overpay нет id"))?;
        let status = item
            .get("status")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("в заказе Overpay нет статуса"))?;
        let currency = util::pay_currency(
            item.get("currency")
                .and_then(Json::as_str)
                .ok_or_else(|| util::bad("в заказе Overpay нет валюты"))?,
        )?;
        let sum = |key: &str| {
            item.get(key)
                .and_then(Json::scalar_text)
                .map(|raw| util::minor(raw, currency))
                .transpose()
        };
        let mut operations: Vec<Operation> = item
            .get("operations")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|op| operation(op, currency))
            .collect();
        operations.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(Order {
            id: order_id(id)?.to_string(),
            status: status.to_ascii_lowercase(),
            currency,
            amount: sum("amount")?,
            charged: sum("amount_charged")?,
            refunded: sum("amount_refunded")?.unwrap_or(0),
            merchant: item
                .get("merchant_order_id")
                .and_then(Json::scalar_text)
                .map(str::to_string),
            failure: item
                .get("failure_message")
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            operations,
        })
    }

    fn money(&self, minor: i64) -> Amount {
        Amount::new(minor, self.currency)
    }

    fn paid(&self) -> i64 {
        self.charged.or(self.amount).unwrap_or(0)
    }

    fn refunded_total(&self) -> i64 {
        if self.status == "refunded" && self.refunded == 0 {
            self.paid()
        } else {
            self.refunded
        }
    }

    fn latest(&self) -> Option<&Operation> {
        self.operations.last()
    }

    fn refund_failed(&self) -> bool {
        !IN_PROGRESS.contains(&self.status.as_str())
            && self.latest().is_some_and(|op| {
                op.kind == "refund" && matches!(op.status.as_str(), "failure" | "error")
            })
    }

    fn last_refund(&self) -> Option<i64> {
        self.operations
            .iter()
            .rev()
            .find(|op| op.kind == "refund" && op.status == "success")
            .map(|op| op.amount)
    }

    fn reason(&self) -> Option<&str> {
        self.latest()
            .and_then(|op| op.message.as_deref())
            .or(self.failure.as_deref())
    }

    fn reference(&self) -> Option<PaymentRef> {
        util::public_ref(self.merchant.as_deref())
            .or_else(|| Some(PaymentRef::ProviderTxid(self.id.clone())))
    }

    fn outcome(
        &self,
        event_key: String,
        status: ProviderStatus,
        amount: Option<i64>,
    ) -> WebhookOutcome {
        WebhookOutcome {
            event_key,
            payment: self.reference(),
            provider_txid: Some(self.id.clone()),
            status,
            amount: amount.map(|minor| self.money(minor)),
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        }
    }

    fn waiting(&self, event_key: String) -> WebhookOutcome {
        util::skipped(
            event_key,
            self.reference(),
            Some(self.id.clone()),
            WebhookReply::ok(),
        )
    }

    fn state(&self) -> PaymentResult<WebhookOutcome> {
        let key = format!("order.{}:{}", self.status, self.id);
        match self.status.as_str() {
            "charged" => {
                let charged = self
                    .charged
                    .ok_or_else(|| util::bad("Overpay не сообщил списанную сумму"))?;
                Ok(self.outcome(key, ProviderStatus::Paid, Some(charged)))
            }
            "declined" | "rejected" | "error" => {
                Ok(self.outcome(key, ProviderStatus::Failed, self.amount))
            }
            "reversed" => Ok(self.outcome(key, ProviderStatus::Canceled, self.amount)),
            "refunded" => Ok(self.refund(self.refunded_total())),
            "chargeback" => Ok(self.chargeback()),
            _ => Ok(self.waiting(key)),
        }
    }

    fn notified(&self) -> PaymentResult<WebhookOutcome> {
        if self.status == "charged" && self.refunded > 0 {
            return Ok(self.refund(self.refunded));
        }
        self.state()
    }

    fn refund(&self, mark: i64) -> WebhookOutcome {
        let total = self.refunded_total();
        let delta = self
            .last_refund()
            .filter(|amount| *amount > 0 && *amount <= total)
            .unwrap_or(total);
        let status = if total >= self.paid() {
            ProviderStatus::Refunded
        } else {
            ProviderStatus::PartiallyRefunded {
                refunded: self.money(delta),
            }
        };
        self.outcome(format!("refund:{}:{mark}", self.id), status, Some(delta))
    }

    fn chargeback(&self) -> WebhookOutcome {
        let paid = self.paid();
        let disputed: i64 = self
            .operations
            .iter()
            .filter(|op| op.kind == "chargeback" && op.status == "success")
            .map(|op| op.amount)
            .sum();
        let disputed = if disputed > 0 && disputed < paid {
            disputed
        } else {
            paid
        };
        let status = if disputed >= paid {
            ProviderStatus::Refunded
        } else {
            ProviderStatus::PartiallyRefunded {
                refunded: self.money(disputed),
            }
        };
        self.outcome(format!("chargeback:{}", self.id), status, Some(disputed))
    }

    fn refund_state(&self, target: i64) -> WebhookOutcome {
        let key = format!("refund:{}:{target}", self.id);
        if self.refunded_total() >= target {
            self.refund(target)
        } else if self.refund_failed() {
            self.outcome(key, ProviderStatus::RefundDeclined, None)
        } else {
            self.waiting(key)
        }
    }
}

fn operation(op: &Json, currency: PayCurrency) -> Option<Operation> {
    let currency = match op.get("currency").and_then(Json::as_str) {
        Some(code) => util::pay_currency(code).ok()?,
        None => currency,
    };
    Some(Operation {
        kind: op.get("type").and_then(Json::as_str)?.to_ascii_lowercase(),
        status: op
            .get("status")
            .and_then(Json::as_str)?
            .to_ascii_lowercase(),
        amount: util::minor(op.get("amount").and_then(Json::scalar_text)?, currency).ok()?,
        created: op
            .get("created")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
        message: op
            .get("iso_message")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string),
    })
}

fn notified_id(request: &WebhookRequest) -> PaymentResult<String> {
    let body = request.body.trim_ascii();
    let from_body = if body.starts_with(b"{") {
        Json::parse(body)
            .ok_or_else(|| util::bad("тело не JSON"))?
            .get("id")
            .and_then(Json::scalar_text)
            .map(str::to_string)
    } else if body.is_empty() {
        None
    } else {
        util::field(&util::form_pairs(body)?, "id").map(str::to_string)
    };
    let id = match from_body {
        Some(id) => id,
        None => {
            let pairs = util::form_pairs(request.query.as_deref().unwrap_or_default().as_bytes())?;
            util::field(&pairs, "id")
                .map(str::to_string)
                .ok_or_else(|| util::bad("в уведомлении нет id заказа"))?
        }
    };
    order_id(&id).map(str::to_string)
}

fn refund_outcome(id: &str, target: i64, completed: bool) -> RefundOutcome {
    RefundOutcome {
        provider_refund_id: Some(format!("{id}:{target}")),
        completed,
    }
}

#[derive(Default)]
struct Attempts {
    targets: Mutex<HashMap<Uuid, (String, i64)>>,
}

impl Attempts {
    fn target(&self, key: Uuid, order: &str) -> Option<i64> {
        locked(&self.targets)
            .get(&key)
            .filter(|(id, _)| id == order)
            .map(|(_, target)| *target)
    }

    fn remember(&self, key: Uuid, order: &str, target: i64) {
        let mut targets = locked(&self.targets);
        if targets.len() >= ATTEMPTS_CAPACITY {
            targets.clear();
        }
        targets.insert(key, (order.to_string(), target));
    }
}

struct Built {
    digest: [u8; 32],
    client: PaymentResult<reqwest::Client>,
}

pub struct Overpay {
    api_base: Option<String>,
    root: Option<reqwest::Certificate>,
    built: Mutex<Option<Built>>,
    attempts: Attempts,
}

impl Default for Overpay {
    fn default() -> Self {
        Self::new()
    }
}

impl Overpay {
    pub fn new() -> Self {
        Overpay {
            api_base: None,
            root: None,
            built: Mutex::new(None),
            attempts: Attempts::default(),
        }
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Overpay {
            api_base: Some(api_base.into().trim_end_matches('/').to_string()),
            ..Self::new()
        }
    }

    #[cfg(test)]
    fn trusting(api_base: impl Into<String>, root: reqwest::Certificate) -> Self {
        Overpay {
            root: Some(root),
            ..Self::with_api_base(api_base)
        }
    }

    fn api(&self, config: &ProviderConfig) -> String {
        match &self.api_base {
            Some(base) => base.clone(),
            None => config
                .get("api_url")
                .unwrap_or(API)
                .trim_end_matches('/')
                .to_string(),
        }
    }

    fn builder(&self) -> reqwest::ClientBuilder {
        let builder = apply_http_proxy(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION"))),
        );
        match &self.root {
            Some(root) => builder.add_root_certificate(root.clone()),
            None => builder,
        }
    }

    fn build(&self, raw: &str) -> PaymentResult<reqwest::Client> {
        let pem = Pem::parse(raw)?;
        let mut parsed = false;
        for leaf in 0..pem.certificates.len() {
            let Ok(identity) = reqwest::Identity::from_pem(pem.ordered(leaf).as_bytes()) else {
                continue;
            };
            parsed = true;
            if let Ok(client) = self.builder().identity(identity).build() {
                return Ok(client);
            }
        }
        Err(PaymentError::NotConfigured(if parsed {
            "сертификат Overpay не подходит к закрытому ключу или повреждён".into()
        } else {
            "сертификат Overpay не разобран: проверьте, что вставлен весь файл .pem".into()
        }))
    }

    fn client(&self, config: &ProviderConfig) -> PaymentResult<reqwest::Client> {
        let raw = config.require(CERTIFICATE)?;
        let digest: [u8; 32] = Sha256::digest(raw.as_bytes()).into();
        let mut built = locked(&self.built);
        if let Some(cached) = built.as_ref().filter(|b| b.digest == digest) {
            return cached.client.clone();
        }
        let client = self.build(raw);
        *built = Some(Built {
            digest,
            client: client.clone(),
        });
        client
    }

    async fn call(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
        body: Option<&Json>,
    ) -> PaymentResult<util::Reply> {
        let login = config.require("login")?;
        let password = config.require("password")?;
        let mut request = self
            .client(config)?
            .request(method, format!("{}{path}", self.api(config)))
            .basic_auth(login, Some(password))
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes());
        }
        util::send(request).await
    }

    async fn order(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Option<Order>> {
        let reply = self
            .call(
                config,
                reqwest::Method::GET,
                &format!("/api/orders/{id}"),
                None,
            )
            .await?;
        if reply.status == 404 {
            return Ok(None);
        }
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let body = reply.json()?;
        let Some(item) = single(&body)? else {
            return Ok(None);
        };
        let order = Order::parse(item)?;
        if order.id != id {
            return Err(util::bad("Overpay вернул другой заказ"));
        }
        Ok(Some(order))
    }

    async fn known_order(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Order> {
        self.order(config, id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("заказ не найден в Overpay".into()))
    }
}

#[async_trait]
impl PaymentProvider for Overpay {
    fn id(&self) -> &'static str {
        providers::OVERPAY
    }

    fn title(&self) -> &'static str {
        "Overpay"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Карты и СБП",
        }
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        if self.currencies().contains(&currency) {
            METHODS.to_vec()
        } else {
            Vec::new()
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::url(
                "api_url",
                "Адрес API",
                "Меняйте, только если Overpay выдал другой адрес",
            )
            .with_default(API),
            SettingField::text(
                "project_id",
                "ID проекта",
                "Номер проекта в кабинете Overpay, только цифры",
            ),
            SettingField::text("login", "Логин", "Логин API от Overpay"),
            SettingField::secret("password", "Пароль", "Пароль API от Overpay"),
            SettingField::secret(
                CERTIFICATE,
                "Сертификат клиента",
                "Сертификат и закрытый ключ одним текстом в формате PEM. Файл .p12 от Overpay переводится командой openssl pkcs12 -in cert.p12 -out cert.pem -nodes, она спросит пароль от .p12. Если openssl ответит ошибкой unsupported, добавьте к команде -legacy. Вставьте сюда весь текст из cert.pem",
            )
            .large(),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if config
            .get("api_url")
            .is_some_and(|url| !util::is_https(url))
        {
            return Err(PaymentError::NotConfigured(
                "адрес API Overpay должен начинаться с https://".into(),
            ));
        }
        if !config
            .get("project_id")
            .is_some_and(|id| id.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(PaymentError::NotConfigured(
                "ID проекта Overpay состоит из цифр".into(),
            ));
        }
        self.client(config).map(|_| ())
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn verifies_webhooks_by_api(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    fn supports_refund_polling(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let method = wire_method(request.method.as_deref())?;
        let hook = request.webhook_url.trim().trim_end_matches('/');
        if hook.is_empty() {
            return Err(PaymentError::NotConfigured(
                "не задан внешний адрес панели".into(),
            ));
        }
        let mut body = Json::object([
            ("amount", Json::from(request.amount.units())),
            ("currency", Json::from(request.amount.currency.code())),
            ("projectId", Json::from(config.require("project_id")?)),
            (
                "merchantTransactionId",
                Json::from(request.payment_public_id.to_string()),
            ),
            ("livetimeMinutes", Json::from(lifetime(request.expires_in))),
            (
                "description",
                Json::from(util::clip(request.description.trim(), 255)),
            ),
            (
                "options",
                Json::object([
                    (
                        "webhookUrl",
                        Json::from(format!("{hook}/{}", hook_token(config)?)),
                    ),
                    ("autoCharge", Json::from(true)),
                ]),
            ),
        ]);
        if let Some(url) = util::return_url(request) {
            body.push("returnUrl", Json::from(url));
        }
        if let Some(email) = util::client_email(request) {
            body.push("client", Json::object([("email", Json::from(email))]));
        }
        if let Some(method) = method {
            body.push("paymentMethods", Json::Array(vec![Json::from(method)]));
        }
        let reply = self
            .call(
                config,
                reqwest::Method::POST,
                "/api/orders/preflight",
                Some(&body),
            )
            .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let created = reply.json()?;
        let id = created
            .get("id")
            .and_then(Json::scalar_text)
            .and_then(|id| order_id(id).ok())
            .ok_or_else(|| util::bad("Overpay не вернул номер заказа"))?;
        let pay_url = created
            .get("resultUrl")
            .and_then(Json::as_str)
            .filter(|url| util::is_https(url))
            .ok_or_else(|| util::bad("Overpay не вернул ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({ "id": id }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let expected = hook_token(config)?;
        if !request
            .route_suffix
            .as_deref()
            .is_some_and(|token| util::secret_eq(&expected, token))
        {
            return Err(PaymentError::BadSignature);
        }
        let id = notified_id(request)?;
        let order = self
            .order(config, &id)
            .await?
            .ok_or(PaymentError::BadSignature)?;
        if order.status == "chargeback" {
            tracing::warn!(order = %order.id, "Overpay сообщил о чарджбэке по заказу");
        }
        order.notified()
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let order = self.known_order(config, order_id(external_id)?).await?;
        order.state().map(Some)
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let id = order_id(&request.provider_txid)?;
        let before = self.known_order(config, id).await?;
        let target = match self.attempts.target(request.key, id) {
            Some(target) if before.refunded_total() >= target => {
                return Ok(refund_outcome(id, target, true));
            }
            Some(target) if IN_PROGRESS.contains(&before.status.as_str()) => {
                return Ok(refund_outcome(id, target, false));
            }
            Some(target) => target,
            None => {
                let left = before.paid() - before.refunded_total();
                if request.amount.minor > left {
                    return Err(PaymentError::Rejected(format!(
                        "в Overpay по этому заказу можно вернуть не больше {} ₽",
                        before.money(left.max(0)).units()
                    )));
                }
                let target = before.refunded_total() + request.amount.minor;
                self.attempts.remember(request.key, id, target);
                target
            }
        };
        let body = Json::object([("amount", Json::from(request.amount.units()))]);
        let reply = self
            .call(
                config,
                reqwest::Method::PUT,
                &format!("/api/orders/{id}/refund"),
                Some(&body),
            )
            .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let Some(after) = single(&parsed)?.map(Order::parse).transpose()? else {
            return Ok(refund_outcome(id, target, false));
        };
        if after.id != id {
            return Err(util::bad("Overpay вернул другой заказ"));
        }
        if after.refunded_total() >= target {
            return Ok(refund_outcome(id, target, true));
        }
        if after.refund_failed() {
            return Err(PaymentError::Rejected(match after.reason() {
                Some(reason) => format!("Overpay отклонил возврат: {}", util::clip(reason, 200)),
                None => "Overpay отклонил возврат".into(),
            }));
        }
        Ok(refund_outcome(id, target, false))
    }

    async fn fetch_refund(
        &self,
        config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let (id, target) = refund_ref(provider_refund_id)?;
        let order = self.known_order(config, id).await?;
        Ok(Some(order.refund_state(target)))
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        _context: &CheckContext,
    ) -> ConnectionCheck {
        let project = match config.require("project_id") {
            Ok(project) => project,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let path = format!(
            "/api/orders?projectId={}&merchant_order_id={}",
            util::rfc3986(project),
            Uuid::nil()
        );
        let reply = match self.call(config, reqwest::Method::GET, &path, None).await {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        if reply.is_success() {
            return match reply.json() {
                Ok(body) if body.get("orders").and_then(Json::as_array).is_some() => {
                    ConnectionCheck::ok("Сертификат, логин и пароль приняты Overpay").detail(
                        "Номер проекта этой проверкой не подтверждается: сверьте его с кабинетом Overpay",
                    )
                }
                Ok(_) => ConnectionCheck::failed(&util::bad("в ответе Overpay нет списка заказов")),
                Err(err) => ConnectionCheck::failed(&err),
            };
        }
        if String::from_utf8_lossy(&reply.body)
            .to_ascii_lowercase()
            .contains("ssl certificate")
        {
            return ConnectionCheck::new(
                CheckStatus::KeysRejected,
                "Overpay не принял сертификат клиента",
            )
            .detail("Нужен сертификат с закрытым ключом, который Overpay выдал для этого проекта. Проверьте, что вставлен весь файл и срок сертификата не истёк");
        }
        if matches!(reply.status, 401 | 403) {
            return ConnectionCheck::new(
                CheckStatus::KeysRejected,
                "Overpay принял сертификат, но не принял логин и пароль API",
            );
        }
        ConnectionCheck::failed(&failure(&reply))
    }
}

#[cfg(test)]
pub(crate) mod tests;
