use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sn_domain::format;
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const API: &str = "https://api.yookassa.ru";
const TAX_SYSTEMS: RangeInclusive<i64> = 1..=6;
const VAT_CODES: RangeInclusive<i64> = 1..=12;
const MISS_TTL: Duration = Duration::from_secs(60);
const MISS_CAPACITY: usize = 4096;
const PAGE: &str = "page";
const SBER_LOAN: &str = "sber_loan";
const METHODS: [PayMethod; 7] = [
    PayMethod::new(PAGE, "Все способы", "Выбор способа на странице ЮKassa"),
    PayMethod::new("bank_card", "Карта", "Карты российских банков"),
    PayMethod::new("sbp", "СБП", "Через приложение банка"),
    PayMethod::new("sberbank", "SberPay", "Через СберБанк Онлайн"),
    PayMethod::new("tinkoff_bank", "T-Pay", "Через приложение Т-Банка"),
    PayMethod::new("yoo_money", "ЮMoney", "Кошелёк ЮMoney"),
    PayMethod::new(
        SBER_LOAN,
        "Кредит от Сбера",
        "Кредит или рассрочка от СберБанка",
    )
    .min_minor(300_000),
];

#[derive(Default)]
struct Misses {
    until: Mutex<HashMap<String, Instant>>,
}

impl Misses {
    fn entries(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        match self.until.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn known(&self, key: &str, now: Instant) -> bool {
        self.entries().get(key).is_some_and(|until| *until > now)
    }

    fn remember(&self, key: String, now: Instant, ttl: Duration) {
        let mut entries = self.entries();
        if entries.len() >= MISS_CAPACITY {
            entries.retain(|_, until| *until > now);
        }
        if entries.len() >= MISS_CAPACITY {
            entries.clear();
        }
        entries.insert(key, now + ttl);
    }
}

pub struct YooKassa {
    http: sn_egress::Http,
    api_base: String,
    misses: Misses,
    miss_ttl: Duration,
}

impl Default for YooKassa {
    fn default() -> Self {
        Self::new()
    }
}

impl YooKassa {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        YooKassa {
            http: http_client(sn_domain::payment::providers::YOOKASSA),
            api_base: api_base.into().trim_end_matches('/').to_string(),
            misses: Misses::default(),
            miss_ttl: MISS_TTL,
        }
    }

    async fn authentic(
        &self,
        config: &ProviderConfig,
        collection: &str,
        id: &str,
    ) -> PaymentResult<Json> {
        let id = util::safe_id(id)?;
        let key = format!(
            "{}|{collection}|{id}",
            config.get("shop_id").unwrap_or_default()
        );
        if self.misses.known(&key, Instant::now()) {
            return Err(PaymentError::BadSignature);
        }
        match self.find(config, collection, id).await? {
            Some(object) => Ok(object),
            None => {
                self.misses.remember(key, Instant::now(), self.miss_ttl);
                Err(PaymentError::BadSignature)
            }
        }
    }

    fn request(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
    ) -> PaymentResult<reqwest::RequestBuilder> {
        let shop = config.require("shop_id")?;
        let secret = config.require("secret_key")?;
        Ok(self
            .http
            .request(method, format!("{}{path}", self.api_base))
            .basic_auth(shop, Some(secret)))
    }

    async fn post(
        &self,
        config: &ProviderConfig,
        path: &str,
        body: &Json,
        idempotence_key: &str,
    ) -> PaymentResult<Json> {
        let reply = util::send(
            self.request(config, reqwest::Method::POST, path)?
                .header("Idempotence-Key", idempotence_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        let parsed = reply.json()?;
        if reply.is_success() {
            Ok(parsed)
        } else {
            Err(reply.failure(parsed.get("description").and_then(Json::as_str)))
        }
    }

    async fn find(
        &self,
        config: &ProviderConfig,
        collection: &str,
        id: &str,
    ) -> PaymentResult<Option<Json>> {
        let id = util::safe_id(id)?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/v3/{collection}/{id}"),
        )?)
        .await?;
        if reply.status == 404 {
            return Ok(None);
        }
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(parsed.get("description").and_then(Json::as_str)));
        }
        if parsed.get("id").and_then(Json::as_str) != Some(id) {
            return Err(util::bad("ЮKassa вернула другой объект"));
        }
        Ok(Some(parsed))
    }

    async fn refund_outcome(
        &self,
        config: &ProviderConfig,
        refund_id: &str,
        refund: &Json,
    ) -> PaymentResult<WebhookOutcome> {
        let event_key = format!("refund:{refund_id}");
        let declined = match refund.get("status").and_then(Json::as_str) {
            Some("succeeded") => false,
            Some("canceled") => true,
            _ => return Ok(util::skipped(event_key, None, None, WebhookReply::ok())),
        };
        let payment_id = refund
            .get("payment_id")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("в возврате нет payment_id"))?;
        let refunded = util::json_amount(
            refund.at(&["amount", "value"]),
            refund.at(&["amount", "currency"]),
        )?;
        if declined {
            return Ok(WebhookOutcome {
                event_key,
                payment: Some(PaymentRef::ProviderTxid(payment_id.to_string())),
                provider_txid: Some(payment_id.to_string()),
                status: ProviderStatus::RefundDeclined,
                amount: Some(refunded),
                saved_method: None,
                reply: WebhookReply::ok(),
                signal: None,
            });
        }
        let payment = self
            .find(config, "payments", payment_id)
            .await?
            .ok_or(PaymentError::BadSignature)?;
        let paid = util::json_amount(
            payment.at(&["amount", "value"]),
            payment.at(&["amount", "currency"]),
        )?;
        let total = match payment.at(&["refunded_amount", "value"]) {
            Some(_) => util::json_amount(
                payment.at(&["refunded_amount", "value"]),
                payment.at(&["refunded_amount", "currency"]),
            )?,
            None => refunded,
        };
        let status = if total.currency == paid.currency && total.minor >= paid.minor {
            ProviderStatus::Refunded
        } else {
            ProviderStatus::PartiallyRefunded { refunded }
        };
        Ok(WebhookOutcome {
            event_key,
            payment: util::public_ref(
                payment
                    .at(&["metadata", "payment_id"])
                    .and_then(Json::as_str),
            )
            .or_else(|| Some(PaymentRef::ProviderTxid(payment_id.to_string()))),
            provider_txid: Some(payment_id.to_string()),
            status,
            amount: Some(refunded),
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        })
    }

    async fn refund_receipt(
        &self,
        config: &ProviderConfig,
        payment_id: &str,
        request: &RefundRequest,
    ) -> PaymentResult<Option<Json>> {
        let Some(vat) = code(config, "vat_code", VAT_CODES)? else {
            return Ok(None);
        };
        let payment = self
            .find(config, "payments", payment_id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("платёж не найден в ЮKassa".into()))?;
        let paid = util::json_amount(
            payment.at(&["amount", "value"]),
            payment.at(&["amount", "currency"]),
        )?;
        if paid.currency == request.amount.currency && request.amount.minor >= paid.minor {
            return Ok(None);
        }
        let email = request
            .email
            .as_deref()
            .map(str::trim)
            .filter(|e| e.len() > 2 && e.contains('@') && !e.contains(char::is_whitespace))
            .ok_or_else(|| {
                PaymentError::Rejected("для чека частичного возврата нужна почта клиента".into())
            })?;
        receipt_for(
            config,
            vat,
            email,
            request.description.as_deref().unwrap_or_default(),
            request.amount,
        )
        .map(Some)
    }
}

fn money(amount: Amount) -> Json {
    Json::object([
        ("value", Json::from(amount.units())),
        ("currency", Json::from(amount.currency.code())),
    ])
}

fn code(
    config: &ProviderConfig,
    key: &str,
    range: RangeInclusive<i64>,
) -> PaymentResult<Option<i64>> {
    let present = match config.0.get(key) {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    };
    if !present {
        return Ok(None);
    }
    match config.integer(key) {
        Some(value) if range.contains(&value) => Ok(Some(value)),
        _ => Err(PaymentError::NotConfigured(format!(
            "{key}: допустимы значения от {} до {}",
            range.start(),
            range.end()
        ))),
    }
}

fn receipt_for(
    config: &ProviderConfig,
    vat: i64,
    email: &str,
    description: &str,
    amount: Amount,
) -> PaymentResult<Json> {
    let description = util::clip(description.trim(), 128);
    let mut receipt = Json::object([
        ("customer", Json::object([("email", Json::from(email))])),
        (
            "items",
            Json::Array(vec![Json::object([
                (
                    "description",
                    Json::from(if description.is_empty() {
                        "Оплата подписки".to_string()
                    } else {
                        description
                    }),
                ),
                ("quantity", Json::number("1")),
                ("amount", money(amount)),
                ("vat_code", Json::from(vat)),
                ("payment_subject", Json::from("service")),
                ("payment_mode", Json::from("full_payment")),
                ("measure", Json::from("piece")),
            ])]),
        ),
    ]);
    if let Some(tax) = code(config, "tax_system_code", TAX_SYSTEMS)? {
        receipt.push("tax_system_code", Json::from(tax));
    }
    Ok(receipt)
}

fn receipt(config: &ProviderConfig, request: &InvoiceRequest) -> PaymentResult<Option<Json>> {
    let Some(vat) = code(config, "vat_code", VAT_CODES)? else {
        return Ok(None);
    };
    let email = util::client_email(request).ok_or(PaymentError::EmailRequired)?;
    receipt_for(config, vat, email, &request.description, request.amount).map(Some)
}

fn chosen(code: Option<&str>) -> PaymentResult<Option<PayMethod>> {
    match code {
        None | Some(PAGE) => Ok(None),
        Some(code) => METHODS
            .iter()
            .find(|m| m.code == code)
            .copied()
            .map(Some)
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn saved_method(payment: &Json) -> Option<SavedMethod> {
    let method = payment.get("payment_method")?;
    if method.get("saved").and_then(Json::as_bool) != Some(true)
        || method.get("status").and_then(Json::as_str) != Some("active")
    {
        return None;
    }
    let token = method.get("id").and_then(Json::as_str)?.to_string();
    let title = method
        .get("title")
        .and_then(Json::as_str)
        .map(str::to_string)
        .or_else(|| {
            method
                .at(&["card", "last4"])
                .and_then(Json::as_str)
                .map(|last4| format!("Карта *{last4}"))
        });
    Some(SavedMethod { token, title })
}

fn payment_outcome(payment: &Json) -> PaymentResult<WebhookOutcome> {
    let id = payment
        .get("id")
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет id платежа ЮKassa"))?;
    let status_text = payment
        .get("status")
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет статуса платежа ЮKassa"))?;
    let status = match status_text {
        "succeeded" => ProviderStatus::Paid,
        "canceled" => ProviderStatus::Canceled,
        _ => ProviderStatus::Pending,
    };
    let amount = util::json_amount(
        payment.at(&["amount", "value"]),
        payment.at(&["amount", "currency"]),
    )?;
    Ok(WebhookOutcome {
        event_key: format!("payment.{status_text}:{id}"),
        payment: util::public_ref(
            payment
                .at(&["metadata", "payment_id"])
                .and_then(Json::as_str),
        )
        .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string()))),
        provider_txid: Some(id.to_string()),
        status,
        amount: Some(amount),
        saved_method: match status {
            ProviderStatus::Paid => saved_method(payment),
            _ => None,
        },
        reply: WebhookReply::ok(),
        signal: (status == ProviderStatus::Canceled
            && payment
                .at(&["cancellation_details", "reason"])
                .and_then(Json::as_str)
                == Some("permission_revoked"))
        .then_some(OutcomeSignal::MethodRevoked),
    })
}

#[async_trait]
impl PaymentProvider for YooKassa {
    fn api_url(&self, _config: &ProviderConfig) -> Option<String> {
        Some(self.api_base.clone())
    }

    fn needs_email(&self, config: &ProviderConfig, _method: Option<&str>) -> bool {
        matches!(code(config, "vat_code", VAT_CODES), Ok(Some(_)))
    }

    fn id(&self) -> &'static str {
        providers::YOOKASSA
    }

    fn title(&self) -> &'static str {
        "ЮKassa"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Карты российских банков и СБП",
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
            SettingField::text(
                "shop_id",
                "shopId",
                "Кабинет ЮKassa → Интеграция → Ключи API",
            ),
            SettingField::secret(
                "secret_key",
                "Секретный ключ",
                "Кабинет ЮKassa → Интеграция → Ключи API",
            ),
            SettingField::flag(
                "autopayments",
                "Автоплатежи",
                "Сохранять способ оплаты для автопродления. Включайте, только если ЮKassa подключила магазину автоплатежи, иначе оплата не пройдёт",
            ),
            SettingField::number(
                "tax_system_code",
                "Система налогообложения",
                "Только для сторонней онлайн-кассы, код от 1 до 6. С Чеками от ЮKassa оставьте пустым",
            ),
            SettingField::number(
                "vat_code",
                "Ставка НДС",
                "Код ставки для чеков 54-ФЗ; пусто — без чека",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if !config
            .get("shop_id")
            .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(PaymentError::NotConfigured(
                "shopId ЮKassa состоит из цифр".into(),
            ));
        }
        let tax = code(config, "tax_system_code", TAX_SYSTEMS)?;
        let vat = code(config, "vat_code", VAT_CODES)?;
        if tax.is_some() && vat.is_none() {
            return Err(PaymentError::NotConfigured(
                "для чеков укажите и ставку НДС".into(),
            ));
        }
        Ok(())
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn verifies_webhooks_by_api(&self) -> bool {
        true
    }

    fn supports_recurring(&self) -> bool {
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
        let method = chosen(request.method.as_deref())?;
        if let Some(min) = method.and_then(|m| m.min_minor)
            && request.amount.minor < min
        {
            return Err(PaymentError::Rejected(format!(
                "этот способ оплаты доступен от {}",
                format::money(Money::new(min, Currency::Rub))
            )));
        }
        let return_url = util::return_url(request)
            .ok_or_else(|| PaymentError::NotConfigured("не задан адрес панели".into()))?;
        let mut body = Json::object([
            ("amount", money(request.amount)),
            ("capture", Json::from(true)),
            (
                "confirmation",
                Json::object([
                    ("type", Json::from("redirect")),
                    ("return_url", Json::from(return_url)),
                ]),
            ),
            (
                "description",
                Json::from(util::clip(&request.description, 128)),
            ),
            (
                "metadata",
                Json::object([(
                    "payment_id",
                    Json::from(request.payment_public_id.to_string()),
                )]),
            ),
        ]);
        if let Some(receipt) = receipt(config, request)? {
            body.push("receipt", receipt);
        }
        if let Some(method) = method {
            body.push(
                "payment_method_data",
                Json::object([("type", Json::from(method.code))]),
            );
        }
        if config.flag("autopayments")
            && request.save_payment_method
            && method.is_none_or(|m| m.code != SBER_LOAN)
        {
            body.push("save_payment_method", Json::from(true));
        }
        let payment = self
            .post(
                config,
                "/v3/payments",
                &body,
                &request.payment_public_id.to_string(),
            )
            .await?;
        let id = payment
            .get("id")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("ЮKassa не вернула id платежа"))?;
        let pay_url = payment
            .at(&["confirmation", "confirmation_url"])
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("ЮKassa не вернула ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "id": id,
                "status": payment.get("status").and_then(Json::as_str),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        let event = body
            .get("event")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет event"))?;
        let object_id = body
            .at(&["object", "id"])
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет object.id"))?;
        if event.starts_with("payment.") {
            let payment = self.authentic(config, "payments", object_id).await?;
            return payment_outcome(&payment);
        }
        if !event.starts_with("refund.") {
            return Ok(util::skipped(
                format!("{event}:{object_id}"),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let refund = self.authentic(config, "refunds", object_id).await?;
        self.refund_outcome(config, object_id, &refund).await
    }

    async fn fetch_refund(
        &self,
        config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let refund = self
            .find(config, "refunds", provider_refund_id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("возврат не найден в ЮKassa".into()))?;
        self.refund_outcome(config, provider_refund_id, &refund)
            .await
            .map(Some)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        match self.find(config, "payments", external_id).await? {
            Some(payment) => payment_outcome(&payment).map(Some),
            None => Err(PaymentError::Rejected("платёж не найден в ЮKassa".into())),
        }
    }

    async fn charge_saved(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
        token: &str,
    ) -> PaymentResult<WebhookOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let mut body = Json::object([
            ("amount", money(request.amount)),
            ("capture", Json::from(true)),
            ("payment_method_id", Json::from(token)),
            (
                "description",
                Json::from(util::clip(&request.description, 128)),
            ),
            (
                "metadata",
                Json::object([(
                    "payment_id",
                    Json::from(request.payment_public_id.to_string()),
                )]),
            ),
        ]);
        if let Some(receipt) = receipt(config, request)? {
            body.push("receipt", receipt);
        }
        let payment = self
            .post(
                config,
                "/v3/payments",
                &body,
                &request.payment_public_id.to_string(),
            )
            .await?;
        payment_outcome(&payment)
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let payment_id = util::safe_id(&request.provider_txid)?;
        let mut body = Json::object([
            ("payment_id", Json::from(payment_id)),
            ("amount", money(request.amount)),
        ]);
        if let Some(reason) = request
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            body.push("description", Json::from(util::clip(reason, 250)));
        }
        if let Some(receipt) = self.refund_receipt(config, payment_id, request).await? {
            body.push("receipt", receipt);
        }
        let refund = self
            .post(config, "/v3/refunds", &body, &request.key.to_string())
            .await?;
        let id = refund
            .get("id")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("ЮKassa не вернула id возврата"))?;
        match refund.get("status").and_then(Json::as_str) {
            Some("succeeded") => Ok(RefundOutcome {
                provider_refund_id: Some(id.to_string()),
                completed: true,
            }),
            Some("canceled") => Err(PaymentError::Rejected(format!(
                "ЮKassa отклонила возврат: {}",
                refund
                    .at(&["cancellation_details", "reason"])
                    .and_then(Json::as_str)
                    .unwrap_or("без пояснения")
            ))),
            _ => Ok(RefundOutcome {
                provider_refund_id: Some(id.to_string()),
                completed: false,
            }),
        }
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        _context: &CheckContext,
    ) -> ConnectionCheck {
        let request = match self.request(config, reqwest::Method::GET, "/v3/me") {
            Ok(request) => request,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let reply = match util::send(request).await {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        if matches!(reply.status, 401 | 403) {
            return ConnectionCheck::new(
                CheckStatus::KeysRejected,
                "ЮKassa не приняла shopId и секретный ключ",
            )
            .detail("Оба значения есть в кабинете ЮKassa: Интеграция → Ключи API. Ключ тестового магазина подходит только к shopId тестового магазина");
        }
        let me = match reply.json() {
            Ok(me) if reply.is_success() => me,
            Ok(me) => {
                return ConnectionCheck::failed(
                    &reply.failure(me.get("description").and_then(Json::as_str)),
                );
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        if me.get("status").and_then(Json::as_str) == Some("disabled") {
            return ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                "Ключи приняты, но магазин в ЮKassa отключён: оплаты не пройдут",
            )
            .detail("Причину покажет кабинет ЮKassa или менеджер ЮKassa");
        }
        let fiscal = me
            .at(&["fiscalization", "enabled"])
            .or_else(|| me.get("fiscalization_enabled"))
            .and_then(Json::as_bool);
        let vat = code(config, "vat_code", VAT_CODES).ok().flatten();
        if fiscal == Some(true) && vat.is_none() {
            return ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                "Ключи приняты, но в магазине включены чеки, а ставка НДС в настройках не указана",
            )
            .detail("Без ставки НДС панель не передаёт чек, и ЮKassa отклонит такую оплату. Укажите код ставки НДС");
        }
        let mut check = ConnectionCheck::ok("shopId и секретный ключ приняты ЮKassa");
        if me.get("test").and_then(Json::as_bool) == Some(true) {
            check = check.detail(
                "Это тестовый магазин: деньги ненастоящие. Перед запуском вставьте shopId и ключ боевого магазина",
            );
        }
        if fiscal == Some(false) && vat.is_some() {
            check = check.detail(
                "В магазине не включены чеки, поэтому ставка НДС из настроек не используется",
            );
        }
        if let Some(enabled) = me.get("payment_methods").and_then(Json::as_array) {
            let enabled: Vec<&str> = enabled.iter().filter_map(Json::as_str).collect();
            let missing: Vec<&str> = METHODS
                .iter()
                .filter(|m| m.code != PAGE && !enabled.contains(&m.code))
                .map(|m| m.title)
                .collect();
            if !missing.is_empty() && !enabled.is_empty() {
                check = check.detail(format!(
                    "Магазину не подключены способы: {}. Если они отмечены в способах оплаты, клиент увидит ошибку",
                    missing.join(", ")
                ));
            }
        }
        check
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use sn_domain::ids::PaymentId;
    use sn_domain::payment::{AmountCheck, check_paid_amount};
    use wiremock::matchers::{basic_auth, body_partial_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID: &str = "2a8f5d1e-000f-5000-9000-1b4f0c5e7d21";

    fn settings(extra: &[(&str, serde_json::Value)]) -> ProviderConfig {
        let mut pairs = vec![
            ("shop_id", json!("504367")),
            ("secret_key", json!("live_secret")),
        ];
        pairs.extend_from_slice(extra);
        config(&pairs)
    }

    fn me(fiscal: bool, test: bool, status: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "account_id": "504367",
            "test": test,
            "fiscalization_enabled": fiscal,
            "fiscalization": {"enabled": fiscal, "provider": "avanpost"},
            "payment_methods": ["bank_card", "sbp", "yoo_money"],
            "itn": "7736207543",
            "status": status
        }))
    }

    #[tokio::test]
    async fn проверка_подключения_юkassa_по_сведениям_о_магазине() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v3/me"))
            .and(basic_auth("504367", "live_secret"))
            .respond_with(me(false, false, "enabled"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v3/me"))
            .and(basic_auth("504368", "test_secret"))
            .respond_with(me(true, true, "enabled"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v3/me"))
            .and(basic_auth("504369", "off_secret"))
            .respond_with(me(false, false, "disabled"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v3/me"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "type": "error",
                "id": "ab5a11cd-13cc-4e33-af8b-75a74e18dd09",
                "code": "invalid_credentials",
                "description": "Basic authentication failed"
            })))
            .with_priority(10)
            .mount(&server)
            .await;
        let provider = YooKassa::with_api_base(server.uri());
        let context = CheckContext::default();
        let ok = provider.check_connection(&settings(&[]), &context).await;
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
        assert_eq!(ok.details.len(), 1);
        assert!(ok.details[0].contains("SberPay"));
        assert!(ok.details[0].contains("T-Pay"));
        assert!(!ok.details[0].contains("СБП"));
        let unused_vat = provider
            .check_connection(&settings(&[("vat_code", json!(1))]), &context)
            .await;
        assert!(unused_vat.details[0].contains("ставка НДС"));
        let test_shop = config(&[
            ("shop_id", json!("504368")),
            ("secret_key", json!("test_secret")),
        ]);
        let no_vat = provider.check_connection(&test_shop, &context).await;
        assert_eq!(no_vat.status, CheckStatus::SettingsRejected);
        assert!(no_vat.summary.contains("ставка НДС"));
        let mut with_vat = test_shop.clone();
        with_vat.set("vat_code", json!(1));
        let test = provider.check_connection(&with_vat, &context).await;
        assert_eq!(test.status, CheckStatus::Ok);
        assert!(test.details[0].contains("тестовый магазин"));
        let disabled = provider
            .check_connection(
                &config(&[
                    ("shop_id", json!("504369")),
                    ("secret_key", json!("off_secret")),
                ]),
                &context,
            )
            .await;
        assert_eq!(disabled.status, CheckStatus::SettingsRejected);
        let rejected = provider
            .check_connection(&settings(&[("secret_key", json!("wrong"))]), &context)
            .await;
        assert_eq!(rejected.status, CheckStatus::KeysRejected);
        for request in server.received_requests().await.unwrap() {
            assert_eq!(request.method, wiremock::http::Method::GET);
            assert!(request.headers.get("idempotence-key").is_none());
        }
        let down = failing_api(502).await;
        let unreachable = YooKassa::with_api_base(down.uri())
            .check_connection(&settings(&[]), &context)
            .await;
        assert_eq!(unreachable.status, CheckStatus::Unreachable);
        let closed = YooKassa::with_api_base(closed_api())
            .check_connection(&settings(&[]), &context)
            .await;
        assert_eq!(closed.status, CheckStatus::Unreachable);
    }

    fn payment(status: &str, value: &str, currency: &str) -> serde_json::Value {
        json!({
            "id": ID,
            "status": status,
            "paid": status == "succeeded",
            "amount": {"value": value, "currency": currency},
            "metadata": {"payment_id": PAYMENT.to_string()},
            "payment_method": {
                "type": "bank_card", "id": ID, "saved": true, "status": "active",
                "title": "Bank card *4477", "card": {"last4": "4477"}
            }
        })
    }

    async fn sent_bodies(server: &MockServer, route: &str) -> Vec<serde_json::Value> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path() == route)
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    async fn accepting_payments() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": ID, "status": "pending",
                "confirmation": {"type": "redirect", "confirmation_url": "https://yoomoney.ru/checkout/x"}
            })))
            .mount(&server)
            .await;
        server
    }

    fn notification(event: &str, id: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "type": "notification",
            "event": event,
            "object": {"id": id, "status": "succeeded", "amount": {"value": "0.01", "currency": "RUB"}}
        }))
        .unwrap()
    }

    async fn api(payment_body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/v3/payments/{ID}")))
            .and(basic_auth("504367", "live_secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(payment_body))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn уведомление_подтверждается_только_запросом_в_юkassa() {
        let server = api(payment("succeeded", "499.00", "RUB")).await;
        let kassa = YooKassa::with_api_base(server.uri());
        let outcome = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payment.succeeded", ID)),
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(49_900)));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.provider_txid.as_deref(), Some(ID));
        assert_eq!(
            outcome.saved_method,
            Some(SavedMethod {
                token: ID.into(),
                title: Some("Bank card *4477".into())
            })
        );
        let forged = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(
                    &[],
                    notification("payment.succeeded", "2a8f5d1e-0000-5000-9000-000000000000"),
                ),
            )
            .await;
        assert_eq!(forged, Err(PaymentError::BadSignature));
        let traversal = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payment.succeeded", "../me")),
            )
            .await;
        assert!(matches!(traversal, Err(PaymentError::BadPayload(_))));
    }

    async fn lookups(server: &MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    #[tokio::test]
    async fn повтор_поддельного_идентификатора_не_вызывает_второй_запрос() {
        let server = api(payment("succeeded", "499.00", "RUB")).await;
        let kassa = YooKassa::with_api_base(server.uri());
        let forged = "2a8f5d1e-0000-5000-9000-000000000000";
        for event in ["payment.succeeded", "payment.succeeded", "payment.canceled"] {
            assert_eq!(
                kassa
                    .parse_webhook(&settings(&[]), &hook(&[], notification(event, forged)))
                    .await,
                Err(PaymentError::BadSignature)
            );
        }
        assert_eq!(lookups(&server).await, 1);
        assert_eq!(
            kassa
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("refund.succeeded", forged))
                )
                .await,
            Err(PaymentError::BadSignature)
        );
        assert_eq!(lookups(&server).await, 2);
        let other_shop = config(&[
            ("shop_id", json!("777000")),
            ("secret_key", json!("live_secret")),
        ]);
        assert_eq!(
            kassa
                .parse_webhook(
                    &other_shop,
                    &hook(&[], notification("payment.succeeded", forged))
                )
                .await,
            Err(PaymentError::BadSignature)
        );
        assert_eq!(lookups(&server).await, 3);
        assert!(matches!(
            kassa.fetch_status(&settings(&[]), forged).await,
            Err(PaymentError::Rejected(_))
        ));
        assert_eq!(lookups(&server).await, 4);
        for _ in 0..2 {
            let genuine = kassa
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("payment.succeeded", ID)),
                )
                .await
                .unwrap();
            assert_eq!(genuine.status, ProviderStatus::Paid);
        }
        assert_eq!(lookups(&server).await, 6);
    }

    #[tokio::test]
    async fn отрицательный_ответ_помнится_недолго() {
        let server = MockServer::start().await;
        let mut kassa = YooKassa::with_api_base(server.uri());
        kassa.miss_ttl = Duration::ZERO;
        for _ in 0..2 {
            assert_eq!(
                kassa
                    .parse_webhook(
                        &settings(&[]),
                        &hook(&[], notification("payment.succeeded", "forged-1"))
                    )
                    .await,
                Err(PaymentError::BadSignature)
            );
        }
        assert_eq!(lookups(&server).await, 2);
        assert!(YooKassa::new().verifies_webhooks_by_api());
    }

    #[test]
    fn память_отрицательных_ответов_ограничена() {
        let misses = Misses::default();
        let now = Instant::now();
        let ttl = Duration::from_secs(60);
        for n in 0..MISS_CAPACITY {
            misses.remember(format!("old-{n}"), now, ttl);
        }
        assert!(misses.known("old-0", now));
        assert!(!misses.known("old-0", now + ttl));
        assert!(!misses.known("absent", now));
        misses.remember("fresh".into(), now + ttl, ttl);
        assert_eq!(misses.entries().len(), 1);
        assert!(misses.known("fresh", now + ttl));
        for n in 0..MISS_CAPACITY {
            misses.remember(format!("flood-{n}"), now + ttl, ttl);
        }
        assert!(misses.entries().len() <= MISS_CAPACITY);
        assert!(misses.known(&format!("flood-{}", MISS_CAPACITY - 1), now + ttl));
    }

    #[tokio::test]
    async fn поддельный_succeeded_в_теле_не_засчитывается() {
        let server = api(payment("pending", "499.00", "RUB")).await;
        let outcome = YooKassa::with_api_base(server.uri())
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payment.succeeded", ID)),
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        assert_eq!(outcome.saved_method, None);
    }

    #[tokio::test]
    async fn статусы_юkassa() {
        for (status, expected) in [
            ("succeeded", ProviderStatus::Paid),
            ("canceled", ProviderStatus::Canceled),
            ("pending", ProviderStatus::Pending),
            ("waiting_for_capture", ProviderStatus::Pending),
        ] {
            let server = api(payment(status, "499.00", "RUB")).await;
            let outcome = YooKassa::with_api_base(server.uri())
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("payment.succeeded", ID)),
                )
                .await
                .unwrap();
            assert_eq!(outcome.status, expected, "{status}");
            assert_eq!(outcome.event_key, format!("payment.{status}:{ID}"));
        }
        let other = YooKassa::new()
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payout.succeeded", "po-1")),
            )
            .await
            .unwrap();
        assert_eq!(other.payment, None);
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_юkassa() {
        let expected = rub(49_900);
        for (value, currency, check) in [("498.99", "RUB", "under"), ("499.00", "USD", "currency")]
        {
            let server = api(payment("succeeded", value, currency)).await;
            let outcome = YooKassa::with_api_base(server.uri())
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("payment.succeeded", ID)),
                )
                .await
                .unwrap();
            let result = check_paid_amount(expected, outcome.amount);
            match check {
                "under" => assert!(matches!(result, AmountCheck::Underpaid { .. })),
                _ => assert!(matches!(result, AmountCheck::CurrencyMismatch { .. })),
            }
        }
        let server = api(payment("succeeded", "499.001", "RUB")).await;
        assert!(matches!(
            YooKassa::with_api_base(server.uri())
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("payment.succeeded", ID))
                )
                .await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn возврат_по_уведомлению() {
        let server = api(json!({
            "id": ID, "status": "succeeded",
            "amount": {"value": "499.00", "currency": "RUB"},
            "refunded_amount": {"value": "100.00", "currency": "RUB"},
            "metadata": {"payment_id": PAYMENT.to_string()}
        }))
        .await;
        Mock::given(method("GET"))
            .and(path("/v3/refunds/rf-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "rf-1", "payment_id": ID, "status": "succeeded",
                "amount": {"value": "100.00", "currency": "RUB"}
            })))
            .mount(&server)
            .await;
        let outcome = YooKassa::with_api_base(server.uri())
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("refund.succeeded", "rf-1")),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.status,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(10_000)
            }
        );
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.event_key, "refund:rf-1");
    }

    #[tokio::test]
    async fn отменённый_и_незавершённый_возврат_юkassa() {
        let server = MockServer::start().await;
        for (id, status) in [("rf-5", "canceled"), ("rf-6", "pending")] {
            Mock::given(method("GET"))
                .and(path(format!("/v3/refunds/{id}")))
                .and(basic_auth("504367", "live_secret"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "id": id, "payment_id": ID, "status": status,
                    "amount": {"value": "100.00", "currency": "RUB"},
                    "cancellation_details": {"party": "yoo_money", "reason": "insufficient_funds"}
                })))
                .mount(&server)
                .await;
        }
        let kassa = YooKassa::with_api_base(server.uri());
        let declined = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("refund.canceled", "rf-5")),
            )
            .await
            .unwrap();
        assert_eq!(declined.status, ProviderStatus::RefundDeclined);
        assert_eq!(declined.event_key, "refund:rf-5");
        assert_eq!(
            declined.payment,
            Some(PaymentRef::ProviderTxid(ID.to_string()))
        );
        assert_eq!(declined.amount, Some(rub(10_000)));
        let polled = kassa
            .fetch_refund(&settings(&[]), "rf-5")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(polled, declined);
        let waiting = kassa
            .fetch_refund(&settings(&[]), "rf-6")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.status, ProviderStatus::Pending);
        assert_eq!(waiting.payment, None);
        assert!(matches!(
            kassa.fetch_refund(&settings(&[]), "rf-404").await,
            Err(PaymentError::Rejected(_))
        ));
    }

    #[tokio::test]
    async fn счёт_с_ключом_идемпотентности_и_чеком() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .and(basic_auth("504367", "live_secret"))
            .and(header("Idempotence-Key", PAYMENT.to_string().as_str()))
            .and(body_partial_json(json!({
                "amount": {"value": "499.00", "currency": "RUB"},
                "capture": true,
                "confirmation": {"type": "redirect", "return_url": "https://panel.example.com/cabinet/payment-wait"},
                "metadata": {"payment_id": PAYMENT.to_string()},
                "save_payment_method": true,
                "receipt": {
                    "customer": {"email": "client@example.com"},
                    "tax_system_code": 2,
                    "items": [{
                        "quantity": 1, "vat_code": 7, "payment_subject": "service",
                        "payment_mode": "full_payment", "measure": "piece",
                        "amount": {"value": "499.00", "currency": "RUB"}
                    }]
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": ID, "status": "pending",
                "confirmation": {"type": "redirect", "confirmation_url": "https://yoomoney.ru/checkout/payments/v2/contract?orderId=2a8f"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let kassa = YooKassa::with_api_base(server.uri());
        let receipts = settings(&[
            ("autopayments", json!(true)),
            ("tax_system_code", json!(2)),
            ("vat_code", json!("7")),
        ]);
        assert!(kassa.needs_email(&receipts, None));
        assert!(kassa.needs_email(&receipts, Some("bank_card")));
        assert!(
            !kassa.needs_email(&settings(&[]), None),
            "без чеков почта не нужна"
        );
        assert!(!kassa.needs_email(&settings(&[("vat_code", json!(""))]), None));
        let mut request = invoice(rub(49_900), Some("client@example.com"));
        request.save_payment_method = true;
        let created = kassa.create_invoice(&receipts, &request).await.unwrap();
        assert_eq!(created.external_id.as_deref(), Some(ID));
        assert!(created.pay_url.starts_with("https://yoomoney.ru/checkout/"));
        assert_eq!(
            kassa
                .create_invoice(&receipts, &invoice(rub(49_900), None))
                .await,
            Err(PaymentError::EmailRequired)
        );
        assert!(matches!(
            kassa
                .create_invoice(&receipts, &invoice(usd(699), Some("c@example.com")))
                .await,
            Err(PaymentError::UnsupportedCurrency(_))
        ));
    }

    #[tokio::test]
    async fn без_ставки_ндс_чек_не_отправляется() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .and(header_exists("Idempotence-Key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": ID, "status": "pending",
                "confirmation": {"type": "redirect", "confirmation_url": "https://yoomoney.ru/checkout/x"}
            })))
            .mount(&server)
            .await;
        YooKassa::with_api_base(server.uri())
            .create_invoice(&settings(&[]), &invoice(rub(49_900), None))
            .await
            .unwrap();
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert!(body.get("receipt").is_none());
        assert!(body.get("save_payment_method").is_none());
        assert!(body.get("payment_method_data").is_none());
    }

    #[tokio::test]
    async fn с_чеками_от_юkassa_система_налогообложения_не_уходит() {
        let server = accepting_payments().await;
        YooKassa::with_api_base(server.uri())
            .create_invoice(
                &settings(&[("vat_code", json!(11))]),
                &invoice(rub(49_900), Some("client@example.com")),
            )
            .await
            .unwrap();
        let sent = sent_bodies(&server, "/v3/payments").await;
        assert_eq!(
            sent[0]["receipt"],
            json!({
                "customer": {"email": "client@example.com"},
                "items": [{
                    "description": "Стандарт · 3 месяца",
                    "quantity": 1,
                    "amount": {"value": "499.00", "currency": "RUB"},
                    "vat_code": 11,
                    "payment_subject": "service",
                    "payment_mode": "full_payment",
                    "measure": "piece"
                }]
            })
        );
    }

    #[tokio::test]
    async fn способ_сохраняется_только_с_автоплатежами() {
        let server = accepting_payments().await;
        let kassa = YooKassa::with_api_base(server.uri());
        let mut request = invoice(rub(499_000), None);
        let autopay = settings(&[("autopayments", json!(true))]);
        let cases: [(&ProviderConfig, bool, Option<&str>, bool); 7] = [
            (&settings(&[]), true, None, false),
            (
                &settings(&[("autopayments", json!(false))]),
                true,
                None,
                false,
            ),
            (&autopay, false, None, false),
            (&autopay, true, None, true),
            (&autopay, true, Some("page"), true),
            (&autopay, true, Some("sbp"), true),
            (&autopay, true, Some("sber_loan"), false),
        ];
        for (config, save, code, expected) in cases {
            request.save_payment_method = save;
            request.method = code.map(str::to_string);
            kassa.create_invoice(config, &request).await.unwrap();
            let sent = sent_bodies(&server, "/v3/payments").await;
            let body = sent.last().unwrap();
            assert_eq!(
                body.get("save_payment_method"),
                expected.then_some(&json!(true)),
                "{save} {code:?}"
            );
        }
    }

    #[tokio::test]
    async fn выбранный_способ_уходит_в_payment_method_data() {
        let server = accepting_payments().await;
        let kassa = YooKassa::with_api_base(server.uri());
        for code in [
            "bank_card",
            "sbp",
            "sberbank",
            "tinkoff_bank",
            "yoo_money",
            "sber_loan",
        ] {
            let mut request = invoice(rub(300_000), None);
            request.method = Some(code.into());
            kassa
                .create_invoice(&settings(&[]), &request)
                .await
                .unwrap();
            let sent = sent_bodies(&server, "/v3/payments").await;
            assert_eq!(
                sent.last().unwrap()["payment_method_data"],
                json!({"type": code}),
                "{code}"
            );
        }
        let mut page = invoice(rub(49_900), None);
        page.method = Some("page".into());
        kassa.create_invoice(&settings(&[]), &page).await.unwrap();
        let sent = sent_bodies(&server, "/v3/payments").await;
        assert!(sent.last().unwrap().get("payment_method_data").is_none());
        let before = sent.len();
        let mut unknown = invoice(rub(49_900), None);
        unknown.method = Some("alfa_pay".into());
        assert_eq!(
            kassa.create_invoice(&settings(&[]), &unknown).await,
            Err(PaymentError::Rejected(
                "этот способ оплаты недоступен".into()
            ))
        );
        let mut loan = invoice(rub(299_999), None);
        loan.method = Some("sber_loan".into());
        assert_eq!(
            kassa.create_invoice(&settings(&[]), &loan).await,
            Err(PaymentError::Rejected(
                "этот способ оплаты доступен от 3\u{a0}000\u{a0}₽".into()
            ))
        );
        assert_eq!(sent_bodies(&server, "/v3/payments").await.len(), before);
    }

    #[test]
    fn способы_оплаты_юkassa() {
        let kassa = YooKassa::new();
        let methods = kassa.methods(Currency::Rub);
        let codes: Vec<&str> = methods.iter().map(|m| m.code).collect();
        assert_eq!(
            codes,
            [
                "page",
                "bank_card",
                "sbp",
                "sberbank",
                "tinkoff_bank",
                "yoo_money",
                "sber_loan"
            ]
        );
        let titles: Vec<&str> = methods.iter().map(|m| m.title).collect();
        assert_eq!(
            titles,
            [
                "Все способы",
                "Карта",
                "СБП",
                "SberPay",
                "T-Pay",
                "ЮMoney",
                "Кредит от Сбера"
            ]
        );
        assert!(methods.iter().all(|m| !m.hint.is_empty()));
        assert_eq!(
            kassa.method(Currency::Rub, "sber_loan").unwrap().min_minor,
            Some(300_000)
        );
        assert!(
            methods
                .iter()
                .filter(|m| m.code != "sber_loan")
                .all(|m| m.min_minor.is_none())
        );
        assert!(kassa.methods(Currency::Usd).is_empty());
        assert!(kassa.method(Currency::Rub, "alfa_pay").is_none());
    }

    #[tokio::test]
    async fn отказ_403_показывает_текст_юkassa() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "type": "error", "id": "17a47682-8d47-4655-a414-a58bdbcd46f9",
                "code": "forbidden",
                "description": "You don't have the rights to make recurring payments"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v3/payments/{ID}")))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "type": "error", "code": "invalid_credentials",
                "description": "Basic auth failed"
            })))
            .mount(&server)
            .await;
        let kassa = YooKassa::with_api_base(server.uri());
        let mut request = invoice(rub(49_900), None);
        request.save_payment_method = true;
        assert_eq!(
            kassa
                .create_invoice(&settings(&[("autopayments", json!(true))]), &request)
                .await,
            Err(PaymentError::Rejected(
                "You don't have the rights to make recurring payments".into()
            ))
        );
        assert_eq!(
            kassa.fetch_status(&settings(&[]), ID).await,
            Err(PaymentError::Rejected(
                "платёжная система не приняла ключи доступа, проверьте настройки".into()
            ))
        );
    }

    #[tokio::test]
    async fn сохраняется_только_активный_способ() {
        for (saved, status, expected) in [
            (json!(true), json!("active"), true),
            (json!(true), json!("inactive"), false),
            (json!(true), json!("pending"), false),
            (json!(true), serde_json::Value::Null, false),
            (json!(false), json!("active"), false),
        ] {
            let mut body = payment("succeeded", "499.00", "RUB");
            body["payment_method"]["saved"] = saved.clone();
            body["payment_method"]["status"] = status.clone();
            let server = api(body).await;
            let outcome = YooKassa::with_api_base(server.uri())
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("payment.succeeded", ID)),
                )
                .await
                .unwrap();
            assert_eq!(outcome.status, ProviderStatus::Paid);
            assert_eq!(outcome.saved_method.is_some(), expected, "{saved} {status}");
        }
    }

    #[tokio::test]
    async fn ошибка_юkassa_с_описанием() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "type": "error", "code": "invalid_request",
                "description": "Receipt is missing or illegal"
            })))
            .mount(&server)
            .await;
        let result = YooKassa::with_api_base(server.uri())
            .create_invoice(&settings(&[]), &invoice(rub(49_900), None))
            .await;
        assert_eq!(
            result,
            Err(PaymentError::Rejected(
                "Receipt is missing or illegal".into()
            ))
        );
    }

    #[tokio::test]
    async fn автосписание_сохранённой_картой() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .and(body_partial_json(
                json!({"payment_method_id": "pm-1", "capture": true}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(payment(
                "succeeded",
                "499.00",
                "RUB",
            )))
            .mount(&server)
            .await;
        let outcome = YooKassa::with_api_base(server.uri())
            .charge_saved(&settings(&[]), &invoice(rub(49_900), None), "pm-1")
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert!(!outcome.method_revoked());
    }

    #[tokio::test]
    async fn отозванное_разрешение_на_автосписание_помечается_для_снятия_карты() {
        let server = MockServer::start().await;
        let mut revoked = payment("canceled", "499.00", "RUB");
        revoked["cancellation_details"] =
            json!({"party": "yoo_money", "reason": "permission_revoked"});
        let mut poor = payment("canceled", "499.00", "RUB");
        poor["cancellation_details"] =
            json!({"party": "yoo_money", "reason": "insufficient_funds"});
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .and(body_partial_json(
                json!({"payment_method_id": "pm-revoked"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(revoked.clone()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .and(body_partial_json(json!({"payment_method_id": "pm-poor"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(poor))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v3/payments/{ID}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(revoked))
            .mount(&server)
            .await;
        let kassa = YooKassa::with_api_base(server.uri());
        let charged = kassa
            .charge_saved(&settings(&[]), &invoice(rub(49_900), None), "pm-revoked")
            .await
            .unwrap();
        assert_eq!(charged.status, ProviderStatus::Canceled);
        assert!(charged.method_revoked());
        assert_eq!(charged.signal, Some(OutcomeSignal::MethodRevoked));
        let declined = kassa
            .charge_saved(&settings(&[]), &invoice(rub(49_900), None), "pm-poor")
            .await
            .unwrap();
        assert_eq!(declined.status, ProviderStatus::Canceled);
        assert!(!declined.method_revoked());
        let notified = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payment.canceled", ID)),
            )
            .await
            .unwrap();
        assert!(notified.method_revoked());
        let polled = kassa
            .fetch_status(&settings(&[]), ID)
            .await
            .unwrap()
            .unwrap();
        assert!(polled.method_revoked());
    }

    fn refund_request(minor: i64, key: u128, email: Option<&str>) -> RefundRequest {
        RefundRequest {
            payment_id: PaymentId(41),
            provider_txid: ID.into(),
            amount: rub(minor),
            reason: Some("Не подошёл тариф".into()),
            key: uuid::Uuid::from_u128(key),
            email: email.map(str::to_string),
            description: Some("STEALTHNET: Стандарт · 3 месяца".into()),
        }
    }

    async fn refunds_api(paid: &str) -> MockServer {
        let server = api(payment("succeeded", paid, "RUB")).await;
        Mock::given(method("POST"))
            .and(path("/v3/refunds"))
            .and(basic_auth("504367", "live_secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "rf-2", "status": "succeeded", "payment_id": ID,
                "amount": {"value": "100.00", "currency": "RUB"}
            })))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn возврат_через_юkassa() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/refunds"))
            .and(header(
                "Idempotence-Key",
                uuid::Uuid::from_u128(5).to_string().as_str(),
            ))
            .and(body_partial_json(json!({
                "payment_id": ID, "amount": {"value": "100.00", "currency": "RUB"},
                "description": "Не подошёл тариф"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "rf-2", "status": "succeeded", "payment_id": ID,
                "amount": {"value": "100.00", "currency": "RUB"}
            })))
            .mount(&server)
            .await;
        let outcome = YooKassa::with_api_base(server.uri())
            .refund(&settings(&[]), &refund_request(10_000, 5, None))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RefundOutcome {
                provider_refund_id: Some("rf-2".into()),
                completed: true
            }
        );
        assert_eq!(lookups(&server).await, 1);
        let sent = sent_bodies(&server, "/v3/refunds").await;
        assert!(sent[0].get("receipt").is_none());
    }

    #[tokio::test]
    async fn каждый_возврат_со_своим_ключом_идемпотентности() {
        let server = refunds_api("499.00").await;
        let kassa = YooKassa::with_api_base(server.uri());
        for key in [7, 8] {
            kassa
                .refund(&settings(&[]), &refund_request(10_000, key, None))
                .await
                .unwrap();
        }
        let keys: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/v3/refunds")
            .map(|r| {
                r.headers
                    .get("Idempotence-Key")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            keys,
            [
                uuid::Uuid::from_u128(7).to_string(),
                uuid::Uuid::from_u128(8).to_string()
            ]
        );
    }

    #[tokio::test]
    async fn частичный_возврат_с_чеком_полный_без_чека() {
        let receipts = settings(&[("vat_code", json!(11))]);
        let server = refunds_api("499.00").await;
        let kassa = YooKassa::with_api_base(server.uri());
        kassa
            .refund(
                &receipts,
                &refund_request(10_000, 9, Some(" client@example.com ")),
            )
            .await
            .unwrap();
        kassa
            .refund(
                &receipts,
                &refund_request(49_900, 10, Some("client@example.com")),
            )
            .await
            .unwrap();
        let sent = sent_bodies(&server, "/v3/refunds").await;
        assert_eq!(
            sent[0]["receipt"],
            json!({
                "customer": {"email": "client@example.com"},
                "items": [{
                    "description": "STEALTHNET: Стандарт · 3 месяца",
                    "quantity": 1,
                    "amount": {"value": "100.00", "currency": "RUB"},
                    "vat_code": 11,
                    "payment_subject": "service",
                    "payment_mode": "full_payment",
                    "measure": "piece"
                }]
            })
        );
        assert_eq!(
            sent[0]["amount"],
            json!({"value": "100.00", "currency": "RUB"})
        );
        assert!(sent[1].get("receipt").is_none());
        let third_party = settings(&[("vat_code", json!(7)), ("tax_system_code", json!(2))]);
        kassa
            .refund(
                &third_party,
                &refund_request(10_000, 11, Some("client@example.com")),
            )
            .await
            .unwrap();
        let sent = sent_bodies(&server, "/v3/refunds").await;
        assert_eq!(sent[2]["receipt"]["tax_system_code"], json!(2));
    }

    #[tokio::test]
    async fn частичный_возврат_с_чеком_требует_почту() {
        let server = refunds_api("499.00").await;
        let kassa = YooKassa::with_api_base(server.uri());
        for email in [None, Some(""), Some("нет почты")] {
            assert_eq!(
                kassa
                    .refund(
                        &settings(&[("vat_code", json!(11))]),
                        &refund_request(10_000, 12, email),
                    )
                    .await,
                Err(PaymentError::Rejected(
                    "для чека частичного возврата нужна почта клиента".into()
                )),
                "{email:?}"
            );
        }
        assert!(sent_bodies(&server, "/v3/refunds").await.is_empty());
    }

    #[test]
    fn настройки_юkassa() {
        let kassa = YooKassa::new();
        assert!(kassa.is_configured(&settings(&[])));
        assert!(kassa.is_configured(&settings(&[("vat_code", json!(7))])));
        assert!(!kassa.is_configured(&settings(&[("vat_code", json!(13))])));
        assert!(!kassa.is_configured(&settings(&[("tax_system_code", json!(1))])));
        assert!(!kassa.is_configured(&settings(&[("vat_code", json!("НДС"))])));
        assert!(!kassa.is_configured(&config(&[
            ("shop_id", json!("shop")),
            ("secret_key", json!("s"))
        ])));
        assert!(kassa.supports_recurring() && kassa.supports_refunds());
        assert!(kassa.supports_refund_polling());
        assert!(kassa.is_configured(&settings(&[("autopayments", json!(true))])));
        let schema = kassa.settings_schema();
        let autopayments = schema.iter().find(|f| f.key == "autopayments").unwrap();
        assert_eq!(autopayments.kind, sn_domain::settings::FieldKind::Flag);
        assert!(!autopayments.required && autopayments.default.is_none());
    }
}
