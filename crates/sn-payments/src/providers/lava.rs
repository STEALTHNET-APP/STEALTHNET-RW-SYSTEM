use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::{NoticeKind, providers};
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://gate.lava.top";
const KEY_HEADER: &str = "x-api-key";
const WEBHOOK_KEY_MIN: usize = 16;
const WEBHOOK_KEY_MAX: usize = 80;
const SALE_PAGES: u32 = 5;
const PRODUCT_PAGES: u32 = 10;
const PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, Copy)]
struct Route {
    pay: PayMethod,
    provider: Option<&'static str>,
    method: Option<&'static str>,
}

const RUB_ROUTES: [Route; 2] = [
    Route {
        pay: PayMethod::new("card", "Карта", "Карты российских банков"),
        provider: Some("SMART_GLOCAL"),
        method: Some("CARD"),
    },
    Route {
        pay: PayMethod::new("sbp", "СБП", "Через приложение банка"),
        provider: Some("PAY2ME"),
        method: Some("SBP"),
    },
];

const USD_ROUTES: [Route; 2] = [
    Route {
        pay: PayMethod::new("card", "Карта", "Visa и Mastercard любых стран"),
        provider: None,
        method: None,
    },
    Route {
        pay: PayMethod::new("paypal", "PayPal", "Со счёта PayPal"),
        provider: Some("PAYPAL"),
        method: None,
    },
];

fn routes(currency: Currency) -> &'static [Route] {
    match currency {
        Currency::Rub => &RUB_ROUTES,
        Currency::Usd => &USD_ROUTES,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sale {
    id: String,
    amount: Amount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Share {
    Full,
    Partial,
    ByAmount,
}

pub struct Lava {
    http: sn_egress::Http,
    api_base: String,
}

impl Default for Lava {
    fn default() -> Self {
        Self::new()
    }
}

impl Lava {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Lava {
            http: http_client(sn_domain::payment::providers::LAVA),
            api_base: api_base.into().trim_end_matches('/').to_string(),
        }
    }

    fn request(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
    ) -> PaymentResult<reqwest::RequestBuilder> {
        Ok(self
            .http
            .request(method, format!("{}{path}", self.api_base))
            .header("X-Api-Key", config.require("api_key")?)
            .header(reqwest::header::ACCEPT, "application/json"))
    }

    async fn get(
        &self,
        config: &ProviderConfig,
        path: &str,
        query: &[(&str, String)],
    ) -> PaymentResult<Json> {
        let reply = util::send(
            self.request(config, reqwest::Method::GET, path)?
                .query(query),
        )
        .await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        Ok(parsed)
    }

    async fn offer_product(
        &self,
        config: &ProviderConfig,
        offer: Uuid,
        visibility: &str,
    ) -> PaymentResult<Option<String>> {
        let mut before: Option<String> = None;
        for _ in 0..PRODUCT_PAGES {
            let mut query = vec![("feedVisibility", visibility.to_string())];
            if let Some(before) = &before {
                query.push(("beforeCreatedAt", before.clone()));
            }
            let page = self.get(config, "/api/v2/products", &query).await?;
            let items = page
                .get("items")
                .and_then(Json::as_array)
                .ok_or_else(|| util::bad("Lava.top не вернула список продуктов"))?;
            for item in items {
                let data = item.get("data").unwrap_or(item);
                let has_offer = data
                    .get("offers")
                    .and_then(Json::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|o| o.get("id").and_then(Json::as_str))
                    .any(|id| Uuid::parse_str(id.trim()).ok() == Some(offer));
                if has_offer {
                    return Ok(Some(
                        data.get("title")
                            .and_then(Json::as_str)
                            .map(|t| util::clip(t.trim(), 80))
                            .unwrap_or_default(),
                    ));
                }
            }
            before = page
                .get("nextPage")
                .and_then(Json::as_str)
                .and_then(next_before);
            if before.is_none() || items.is_empty() {
                break;
            }
        }
        Ok(None)
    }

    async fn invoice(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Json> {
        let id = util::safe_id(id)?;
        let invoice = self
            .get(config, &format!("/api/v1/invoices/{id}"), &[])
            .await?;
        if invoice
            .get("id")
            .and_then(Json::as_str)
            .is_some_and(|got| !got.eq_ignore_ascii_case(id))
        {
            return Err(util::bad("Lava.top вернула другой счёт"));
        }
        Ok(invoice)
    }

    async fn checked_payment(
        &self,
        config: &ProviderConfig,
        body: &Json,
        event: &str,
        contract: &str,
    ) -> PaymentResult<WebhookOutcome> {
        let invoice = self.invoice(config, contract).await?;
        let (_, status) = invoice_status(&invoice)?;
        if event == "payment.success" && status == ProviderStatus::Pending {
            return Err(PaymentError::Transport(
                "Lava.top ещё не отметила счёт оплаченным, ждём повторного уведомления".into(),
            ));
        }
        let amount = match status {
            ProviderStatus::Paid => Some(match invoice_total(&invoice) {
                Some(total) => total_amount(total)?,
                None => util::json_amount(body.get("amount"), body.get("currency"))?,
            }),
            _ => None,
        };
        Ok(WebhookOutcome {
            event_key: format!("{event}:{contract}"),
            payment: Some(PaymentRef::ProviderTxid(contract.to_string())),
            provider_txid: Some(contract.to_string()),
            status,
            amount,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        })
    }

    async fn sales(
        &self,
        config: &ProviderConfig,
        email: &str,
        currency: Currency,
        product: Option<&str>,
    ) -> PaymentResult<Option<Vec<Sale>>> {
        let mut found = Vec::new();
        let mut seen = 0usize;
        for page in 1..=SALE_PAGES {
            let reply = self
                .get(
                    config,
                    "/api/v2/invoices",
                    &[
                        ("buyerEmail", email.to_string()),
                        ("currencies", currency.code().to_string()),
                        ("invoiceStatuses", "COMPLETED".to_string()),
                        ("page", page.to_string()),
                    ],
                )
                .await?;
            let items = reply
                .get("items")
                .and_then(Json::as_array)
                .ok_or_else(|| util::bad("Lava.top не вернула список оплат"))?;
            seen += items.len();
            found.extend(
                items
                    .iter()
                    .filter_map(|item| sale(item, email, currency, product)),
            );
            let complete = match reply
                .get("total")
                .and_then(Json::scalar_text)
                .and_then(|t| t.parse::<usize>().ok())
            {
                Some(total) => seen >= total,
                None => items.len() < PAGE_SIZE,
            };
            if complete || items.is_empty() {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    async fn reversal(
        &self,
        config: &ProviderConfig,
        body: &Json,
        event: &str,
    ) -> PaymentResult<WebhookOutcome> {
        let event_id = body
            .get("event_id")
            .and_then(Json::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let data = body.get("data");
        let (prefix, id_field, share) = match event {
            "refund.success" => (
                "refund",
                "refund_id",
                match data
                    .and_then(|d| d.get("refund_type"))
                    .and_then(Json::as_str)
                {
                    Some("full") => Share::Full,
                    Some("partial") => Share::Partial,
                    _ => Share::ByAmount,
                },
            ),
            "chargeback.initiated" => ("chargeback", "chargeback_id", Share::ByAmount),
            _ => return Ok(ignored(format!("{event}:{event_id}"))),
        };
        let Some(data) = data else {
            return Ok(ignored(format!("{event}:{event_id}")));
        };
        let id = data
            .get(id_field)
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .unwrap_or(event_id);
        if id.is_empty() {
            return Ok(ignored(format!("{event}:")));
        }
        let event_key = format!("{prefix}:{id}");
        let amount = util::json_amount(data.get("amount"), data.get("currency"))
            .ok()
            .filter(|amount| amount.minor > 0);
        let email = data
            .get("customer_email")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|e| e.contains('@'));
        let unmatched = |why: &str| {
            let (kind, what) = match prefix {
                "chargeback" => (NoticeKind::Chargeback, "чарджбэке"),
                _ => (NoticeKind::UnmatchedRefund, "возврате"),
            };
            let sum = amount.map_or_else(|| "без суммы".to_string(), util::amount_text);
            let buyer = email.map_or_else(String::new, |e| format!(", покупатель {e}"));
            let mut outcome = ignored(event_key.clone());
            outcome.amount = amount;
            outcome.with_notice(
                kind,
                format!(
                    "Lava.top сообщила о {what} {sum} (№ {id}{buyer}), но оплату для него найти \
                     не удалось: {why}. Найдите платёж в кабинете Lava.top и отметьте возврат \
                     в панели вручную"
                ),
            )
        };
        let Some(amount) = amount else {
            return Ok(unmatched("в уведомлении нет суммы"));
        };
        let (Some(currency), Some(email)) = (amount.currency.as_currency(), email) else {
            return Ok(unmatched("в уведомлении нет почты покупателя"));
        };
        let product = data
            .at(&["product", "product_name"])
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let sales = self
            .sales(config, email, currency, product)
            .await?
            .unwrap_or_default();
        let Some(sale) = pick(&sales, amount, share) else {
            tracing::warn!(
                event = %event_key,
                found = sales.len(),
                "возврат Lava.top не привязан к оплате: нет единственной подходящей оплаты"
            );
            return Ok(unmatched(&match sales.len() {
                0 => "оплат этого покупателя на такую сумму нет".to_string(),
                n => format!("подходят сразу несколько оплат ({n})"),
            }));
        };
        let status = match share {
            Share::Full => ProviderStatus::Refunded,
            Share::ByAmount if sale.amount == amount => ProviderStatus::Refunded,
            Share::Partial | Share::ByAmount => {
                ProviderStatus::PartiallyRefunded { refunded: amount }
            }
        };
        let outcome = WebhookOutcome {
            event_key,
            payment: Some(PaymentRef::ProviderTxid(sale.id.clone())),
            provider_txid: Some(sale.id.clone()),
            status,
            amount: Some(amount),
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        };
        Ok(match prefix {
            "chargeback" => outcome.with_notice(
                NoticeKind::Chargeback,
                format!(
                    "Lava.top сообщила о чарджбэке {} (№ {id}): банк покупателя вернул ему деньги. \
                     Платёж отмечен возвращённым, подписка не изменена — решите, отключать ли её",
                    util::amount_text(amount)
                ),
            ),
            _ => outcome,
        })
    }
}

fn next_before(next: &str) -> Option<String> {
    reqwest::Url::parse(next)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == "beforeCreatedAt")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty())
}

fn message(body: &Json) -> Option<&str> {
    body.get("error")
        .or_else(|| body.get("message"))
        .and_then(Json::as_str)
}

fn invoice_total(info: &Json) -> Option<&Json> {
    info.get("amountTotal").or_else(|| info.get("receipt"))
}

fn total_amount(total: &Json) -> PaymentResult<Amount> {
    util::json_amount(total.get("amount"), total.get("currency"))
}

fn whole_units(total: &Json) -> bool {
    total
        .get("amount")
        .and_then(Json::scalar_text)
        .is_some_and(|raw| !raw.contains('.'))
}

fn same_price(charged: Amount, requested: Amount, whole: bool) -> bool {
    let unit = 10_i64.pow(requested.currency.decimals());
    charged == requested
        || (whole
            && charged.currency == requested.currency
            && (charged.minor - requested.minor).abs() < unit)
}

fn invoice_status(invoice: &Json) -> PaymentResult<(&str, ProviderStatus)> {
    let text = invoice
        .get("status")
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет статуса счёта"))?;
    let status = match text.to_ascii_uppercase().as_str() {
        "COMPLETED" => ProviderStatus::Paid,
        "FAILED" => ProviderStatus::Failed,
        _ => ProviderStatus::Pending,
    };
    Ok((text, status))
}

fn basic_password(header: &str) -> Option<String> {
    let (scheme, encoded) = header.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let text = String::from_utf8(STANDARD.decode(encoded.trim()).ok()?).ok()?;
    text.split_once(':')
        .map(|(_, password)| password.trim().to_string())
}

fn authorize(config: &ProviderConfig, request: &WebhookRequest) -> PaymentResult<()> {
    let expected = config
        .get("webhook_key")
        .ok_or_else(|| PaymentError::NotConfigured("не задан ключ вебхука".into()))?;
    let by_key = request
        .header(KEY_HEADER)
        .is_some_and(|key| util::secret_eq(expected, key.trim()));
    let by_login = request
        .header("authorization")
        .and_then(basic_password)
        .is_some_and(|password| util::secret_eq(expected, &password));
    if by_key | by_login {
        Ok(())
    } else {
        Err(PaymentError::BadSignature)
    }
}

fn ignored(event_key: String) -> WebhookOutcome {
    util::skipped(event_key, None, None, WebhookReply::ok())
}

fn offer_id(config: &ProviderConfig) -> Option<Uuid> {
    config
        .get("offer_id")
        .and_then(|offer| Uuid::parse_str(offer).ok())
}

fn sale(item: &Json, email: &str, currency: Currency, product: Option<&str>) -> Option<Sale> {
    if item
        .get("status")
        .and_then(Json::as_str)
        .is_some_and(|s| !s.eq_ignore_ascii_case("COMPLETED"))
    {
        return None;
    }
    let buyer = item.at(&["buyer", "email"]).and_then(Json::as_str)?;
    if buyer.trim().to_lowercase() != email.to_lowercase() {
        return None;
    }
    if let (Some(wanted), Some(name)) = (
        product,
        item.at(&["product", "name"]).and_then(Json::as_str),
    ) && wanted != name.trim()
    {
        return None;
    }
    let amount = total_amount(item.get("receipt")?).ok()?;
    if amount.currency != PayCurrency::from(currency) || amount.minor <= 0 {
        return None;
    }
    let id = item
        .get("id")
        .and_then(Json::as_str)
        .filter(|id| util::safe_id(id).is_ok())?;
    Some(Sale {
        id: id.to_string(),
        amount,
    })
}

fn single<'a>(sales: &[&'a Sale]) -> Option<&'a Sale> {
    match sales {
        [only] => Some(*only),
        _ => None,
    }
}

fn pick(sales: &[Sale], amount: Amount, share: Share) -> Option<&Sale> {
    let exact: Vec<&Sale> = sales.iter().filter(|s| s.amount == amount).collect();
    let larger: Vec<&Sale> = sales
        .iter()
        .filter(|s| s.amount.minor > amount.minor)
        .collect();
    match share {
        Share::Partial => single(&larger),
        Share::Full | Share::ByAmount if exact.is_empty() => single(&larger),
        Share::Full | Share::ByAmount => single(&exact),
    }
}

#[async_trait]
impl PaymentProvider for Lava {
    fn api_url(&self, _config: &ProviderConfig) -> Option<String> {
        Some(self.api_base.clone())
    }

    fn id(&self) -> &'static str {
        providers::LAVA
    }

    fn title(&self) -> &'static str {
        "Lava.top"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Usd, Currency::Rub]
    }

    fn client_label(&self, currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: match currency {
                Currency::Rub => "Карты российских банков",
                Currency::Usd => "Visa и Mastercard любых стран",
            },
        }
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        routes(currency).iter().map(|route| route.pay).collect()
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret(
                "api_key",
                "API-ключ",
                "Кабинет Lava.top → Интеграции → API → Настроить API key",
            ),
            SettingField::text(
                "offer_id",
                "ID оффера",
                "Оффер продукта с включённой «Цена по запросу API»",
            ),
            SettingField::secret(
                "webhook_key",
                "Ключ вебхука",
                "Придумайте ключ от 16 до 80 латинских букв и цифр. В Lava.top: Интеграции → API → Добавить Webhook. URL — «Адрес для уведомлений» ниже. Тип события: «Успешная оплата», «Неуспешная оплата», «Успешный возврат», «Получен запрос на чарджбэк». Аутентификация: «API key вашего сервиса» с этим ключом или Basic: логин любой, пароль — этот ключ",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        config.get("api_key").is_some() && offer_id(config).is_some()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if offer_id(config).is_none() {
            return Err(PaymentError::NotConfigured(
                "ID оффера Lava.top — это UUID из кабинета".into(),
            ));
        }
        if config.get("webhook_key").is_some_and(|key| {
            key.len() > WEBHOOK_KEY_MAX || !key.bytes().all(|b| b.is_ascii_graphic())
        }) {
            return Err(PaymentError::NotConfigured(
                "ключ вебхука: до 80 латинских букв, цифр и знаков без пробелов, иначе Lava.top его не примет".into(),
            ));
        }
        if config
            .get("webhook_key")
            .is_some_and(|key| key.len() < WEBHOOK_KEY_MIN)
        {
            return Err(PaymentError::NotConfigured(
                "ключ вебхука слишком короткий: придумайте ключ хотя бы из 16 латинских букв и цифр".into(),
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

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        let (currency, language) = match request.amount.currency {
            PayCurrency::Usd => (Currency::Usd, "EN"),
            PayCurrency::Rub => (Currency::Rub, "RU"),
            other => return Err(PaymentError::UnsupportedCurrency(other.code().into())),
        };
        let route = match request.method.as_deref() {
            Some(code) => Some(
                routes(currency)
                    .iter()
                    .find(|route| route.pay.code == code)
                    .ok_or_else(|| {
                        PaymentError::Rejected("этот способ оплаты недоступен".into())
                    })?,
            ),
            None => None,
        };
        let offer = config.require("offer_id")?;
        let email = util::client_email(request).ok_or(PaymentError::EmailRequired)?;
        let mut body = Json::object([
            ("email", Json::from(email)),
            ("offerId", Json::from(offer)),
            ("currency", Json::from(currency.code())),
            ("amount", Json::number(request.amount.units())),
            ("buyerLanguage", Json::from(language)),
        ]);
        if let Some(provider) = route.and_then(|r| r.provider) {
            body.push("paymentProvider", Json::from(provider));
        }
        if let Some(method) = route.and_then(|r| r.method) {
            body.push("paymentMethod", Json::from(method));
        }
        if let Some(url) = util::return_url(request).filter(|u| util::is_https(u) && u.len() <= 512)
        {
            body.push("successful_return_url", Json::from(url.clone()));
            body.push("failure_return_url", Json::from(url.clone()));
            body.push("cancel_return_url", Json::from(url));
        }
        let reply = util::send(
            self.request(config, reqwest::Method::POST, "/api/v3/invoice")?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        let id = parsed
            .get("id")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| util::bad("Lava.top не вернула id счёта"))?;
        let pay_url = parsed
            .get("paymentUrl")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Lava.top не вернула ссылку на оплату"))?;
        if let Some(total) = invoice_total(&parsed)
            && let Ok(charged) = total_amount(total)
            && !same_price(charged, request.amount, whole_units(total))
        {
            return Err(PaymentError::Rejected(format!(
                "Lava.top выставила счёт на {charged}, а не на {}: включите у продукта «Цена по запросу API»",
                request.amount
            )));
        }
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "id": id,
                "status": parsed.get("status").and_then(Json::as_str),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        authorize(config, request)?;
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        if let Some(event) = body.get("event_type").and_then(Json::as_str) {
            return self.reversal(config, &body, event).await;
        }
        let Some(event) = body.get("eventType").and_then(Json::as_str) else {
            return Ok(ignored("unknown".into()));
        };
        let contract = body
            .get("contractId")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|c| !c.is_empty());
        match (event, contract) {
            ("payment.success" | "payment.failed", Some(contract)) => {
                self.checked_payment(config, &body, event, contract).await
            }
            (_, contract) => Ok(ignored(format!("{event}:{}", contract.unwrap_or("-")))),
        }
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let id = util::safe_id(external_id)?;
        let invoice = self.invoice(config, id).await?;
        let (status_text, status) = invoice_status(&invoice)?;
        let amount = match status {
            ProviderStatus::Paid => Some(
                invoice_total(&invoice)
                    .ok_or_else(|| util::bad("нет суммы счёта"))
                    .and_then(total_amount)?,
            ),
            _ => None,
        };
        Ok(Some(WebhookOutcome {
            event_key: format!("invoice.{status_text}:{id}"),
            payment: Some(PaymentRef::ProviderTxid(id.to_string())),
            provider_txid: Some(id.to_string()),
            status,
            amount,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        }))
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        _context: &CheckContext,
    ) -> ConnectionCheck {
        let Some(offer) = offer_id(config) else {
            return ConnectionCheck::failed(&PaymentError::NotConfigured(
                "ID оффера Lava.top — это UUID из кабинета".into(),
            ));
        };
        let refused = || {
            ConnectionCheck::new(CheckStatus::KeysRejected, "Lava.top не приняла API-ключ")
                .detail("Ключ создаётся в кабинете Lava.top: Интеграции → API → Настроить API key")
        };
        match self.offer_product(config, offer, "ONLY_HIDDEN").await {
            Ok(Some(title)) => {
                return ConnectionCheck::ok(if title.is_empty() {
                    "API-ключ принят, оффер найден в Lava.top".to_string()
                } else {
                    format!("API-ключ принят, оффер найден: продукт «{title}»")
                });
            }
            Ok(None) => {}
            Err(err) if util::keys_refused(&err) => return refused(),
            Err(err) => return ConnectionCheck::failed(&err),
        }
        match self.offer_product(config, offer, "ONLY_VISIBLE").await {
            Ok(Some(title)) => ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                if title.is_empty() {
                    "API-ключ принят, но у продукта с этим оффером не включена «Цена по запросу API»".to_string()
                } else {
                    format!("API-ключ принят, но у продукта «{title}» не включена «Цена по запросу API»")
                },
            )
            .detail("Продукты с ценой по запросу API Lava.top прячет со страницы автора, а этот продукт виден. Без неё Lava не примет сумму из панели"),
            Ok(None) => ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                "API-ключ принят, но оффера с таким ID нет среди продуктов Lava.top",
            )
            .detail("Скопируйте ID оффера из продукта с включённой «Ценой по запросу API»"),
            Err(err) if util::keys_refused(&err) => refused(),
            Err(err) => ConnectionCheck::failed(&err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::{Value, json};
    use sn_domain::payment::{AmountCheck, check_paid_amount};
    use wiremock::matchers::{body_json, body_partial_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const OFFER: &str = "836b9fc5-7ae9-4a27-9642-592bc44072b7";
    const CONTRACT: &str = "7ea82675-4ded-4133-95a7-a6efbaf165cc";
    const SECOND: &str = "c5a0cacc-3453-44b0-9532-aa492f1ba191";
    const KEY: &str = "lava-webhook-key";
    const EMAIL: &str = "client@example.com";

    fn settings() -> ProviderConfig {
        config(&[
            ("api_key", json!("lava-api-key")),
            ("offer_id", json!(OFFER)),
            ("webhook_key", json!(KEY)),
        ])
    }

    fn product(title: &str, offer: &str) -> Value {
        json!({"type": "PRODUCT", "data": {
            "id": "4e8f8b1c-6b0e-4a59-9d4f-3c1f2a7b9e10", "title": title, "type": "DIGITAL_PRODUCT",
            "offers": [{"id": offer, "name": "Подписка", "prices": [{"amount": 499.0, "currency": "RUB", "periodicity": "ONE_TIME"}]}]
        }})
    }

    async fn products(server: &MockServer, visibility: &str, before: Option<&str>, body: Value) {
        let mock = Mock::given(method("GET"))
            .and(path("/api/v2/products"))
            .and(header("x-api-key", "lava-api-key"))
            .and(query_param("feedVisibility", visibility));
        let mock = match before {
            Some(before) => mock.and(query_param("beforeCreatedAt", before)),
            None => mock.and(wiremock::matchers::query_param_is_missing(
                "beforeCreatedAt",
            )),
        };
        mock.respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn проверка_подключения_lava_ищет_оффер_среди_продуктов() {
        let server = MockServer::start().await;
        let stamp = "2026-11-01T05:43:18.922Z";
        products(
            &server,
            "ONLY_HIDDEN",
            None,
            json!({
                "items": [product("Другой продукт", "11111111-1111-4111-8111-111111111111")],
                "nextPage": format!("https://gate.lava.top/api/v2/products?beforeCreatedAt={stamp}&feedVisibility=ONLY_HIDDEN")
            }),
        )
        .await;
        products(
            &server,
            "ONLY_HIDDEN",
            Some(stamp),
            json!({"items": [product("VPN на месяц", OFFER)], "nextPage": null}),
        )
        .await;
        products(
            &server,
            "ONLY_VISIBLE",
            None,
            json!({"items": [product("Открытый продукт", SECOND)], "nextPage": null}),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/products"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": "Invalid API key", "details": {}, "timestamp": "2026-11-18T12:14:11.981087055+02:00"
            })))
            .with_priority(10)
            .mount(&server)
            .await;
        let provider = Lava::with_api_base(server.uri());
        let context = CheckContext::default();
        let ok = provider.check_connection(&settings(), &context).await;
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
        assert!(ok.summary.contains("«VPN на месяц»"));
        let mut visible = settings();
        visible.set("offer_id", json!(SECOND));
        let price = provider.check_connection(&visible, &context).await;
        assert_eq!(price.status, CheckStatus::SettingsRejected);
        assert!(price.summary.contains("Цена по запросу API"));
        let mut missing = settings();
        missing.set("offer_id", json!("22222222-2222-4222-8222-222222222222"));
        let unknown = provider.check_connection(&missing, &context).await;
        assert_eq!(unknown.status, CheckStatus::SettingsRejected);
        assert!(unknown.summary.contains("нет среди продуктов"));
        let mut wrong = settings();
        wrong.set("api_key", json!("other-key"));
        let rejected = provider.check_connection(&wrong, &context).await;
        assert_eq!(rejected.status, CheckStatus::KeysRejected);
        let down = failing_api(502).await;
        let unreachable = Lava::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await;
        assert_eq!(unreachable.status, CheckStatus::Unreachable);
        let closed = Lava::with_api_base(closed_api())
            .check_connection(&settings(), &context)
            .await;
        assert_eq!(closed.status, CheckStatus::Unreachable);
    }

    fn event(kind: &str, amount: Value, currency: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "eventType": kind,
            "product": {"id": "p-1", "title": "STEALTHNET"},
            "buyer": {"email": EMAIL},
            "contractId": CONTRACT,
            "amount": amount,
            "currency": currency,
            "timestamp": "2026-11-16T14:52:00Z",
            "status": "completed"
        }))
        .unwrap()
    }

    fn refund(kind: &str, id: &str, amount: Value, currency: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "event_id": format!("e-{id}"),
            "event_type": "refund.success",
            "created_at": "2026-11-17T09:38:27.33277Z",
            "data": {
                "refund_id": id,
                "refund_type": kind,
                "initiator": "creator",
                "amount": amount,
                "currency": currency,
                "product": {
                    "product_name": "STEALTHNET",
                    "product_id": "d31384b8-e412-4be5-a2ec-297ae6666c8f",
                    "product_type": "DIGITAL_PRODUCT",
                    "tier_id": OFFER
                },
                "customer_email": "Client@Example.com",
                "balance_impact": {"debited_amount": amount, "currency": currency},
                "subscription_cancelled": false
            }
        }))
        .unwrap()
    }

    fn chargeback(amount: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "event_id": "e-cb",
            "event_type": "chargeback.initiated",
            "created_at": "2026-11-17T09:38:27.33277Z",
            "data": {
                "chargeback_id": "cb-1",
                "dispute_date": "2026-11-17",
                "reason_category": "fraud",
                "amount": amount,
                "currency": "USD",
                "product": {"product_name": "STEALTHNET", "tier_id": OFFER},
                "customer_email": "Client@Example.com",
                "balance_impact": {"debited_amount": amount, "provider_fee": 0.5, "currency": "USD"},
                "subscription_cancelled": false
            }
        }))
        .unwrap()
    }

    fn sold(id: &str, email: &str, amount: Value, product: &str) -> Value {
        json!({
            "id": id,
            "type": "INVOICE",
            "datetime": "2026-11-10T10:00:00Z",
            "status": "COMPLETED",
            "receipt": {"amount": amount, "currency": "USD", "fee": 0.35},
            "buyer": {"email": email, "cardMask": "**** **** **** 1234"},
            "product": {"name": product, "offer": "Цена по запросу API"}
        })
    }

    fn keyed(body: Vec<u8>) -> WebhookRequest {
        hook(&[("X-Api-Key", KEY)], body)
    }

    async fn with_invoice(status: &str, receipt: Value) -> (MockServer, Lava) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/invoices/{CONTRACT}")))
            .and(header("X-Api-Key", "lava-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": CONTRACT,
                "type": "ONE_TIME",
                "status": status,
                "receipt": receipt
            })))
            .mount(&server)
            .await;
        let lava = Lava::with_api_base(server.uri());
        (server, lava)
    }

    async fn with_sales(items: Vec<Value>) -> (MockServer, Lava) {
        let server = MockServer::start().await;
        let total = items.len();
        Mock::given(method("GET"))
            .and(path("/api/v2/invoices"))
            .and(header("X-Api-Key", "lava-api-key"))
            .and(query_param("buyerEmail", "Client@Example.com"))
            .and(query_param("currencies", "USD"))
            .and(query_param("invoiceStatuses", "COMPLETED"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": items,
                "page": 1,
                "size": 20,
                "total": total
            })))
            .mount(&server)
            .await;
        let lava = Lava::with_api_base(server.uri());
        (server, lava)
    }

    async fn created(answer: Value, request: &InvoiceRequest) -> (PaymentResult<Invoice>, Value) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/invoice"))
            .respond_with(ResponseTemplate::new(201).set_body_json(answer))
            .mount(&server)
            .await;
        let result = Lava::with_api_base(server.uri())
            .create_invoice(&settings(), request)
            .await;
        let sent = server.received_requests().await.unwrap();
        let body = sent
            .first()
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .unwrap_or(Value::Null);
        (result, body)
    }

    fn answer(amount: Value, currency: &str) -> Value {
        json!({
            "id": CONTRACT,
            "status": "in-progress",
            "amountTotal": {"currency": currency, "amount": amount},
            "paymentUrl": "https://app.lava.top/payment/7ea82675"
        })
    }

    fn with_method(amount: Amount, code: &str) -> InvoiceRequest {
        InvoiceRequest {
            method: Some(code.to_string()),
            ..invoice(amount, Some(EMAIL))
        }
    }

    #[test]
    fn способы_оплаты_lava_по_валютам() {
        let lava = Lava::new();
        let list = |currency| {
            lava.methods(currency)
                .into_iter()
                .map(|m| (m.code, m.title))
                .collect::<Vec<_>>()
        };
        assert_eq!(list(Currency::Rub), [("card", "Карта"), ("sbp", "СБП")]);
        assert_eq!(
            list(Currency::Usd),
            [("card", "Карта"), ("paypal", "PayPal")]
        );
        assert!(lava.method(Currency::Usd, "sbp").is_none());
        assert!(lava.method(Currency::Rub, "paypal").is_none());
        for currency in [Currency::Rub, Currency::Usd] {
            for m in lava.methods(currency) {
                assert!(m.code.len() <= 20, "{}", m.code);
                assert!(
                    m.code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                    "{}",
                    m.code
                );
                assert!(!m.hint.is_empty(), "{}", m.code);
            }
        }
    }

    #[tokio::test]
    async fn выбранный_способ_lava_уходит_в_счёт() {
        for (amount, code, provider, kind, language) in [
            (
                rub(49_900),
                "card",
                Some("SMART_GLOCAL"),
                Some("CARD"),
                "RU",
            ),
            (rub(49_900), "sbp", Some("PAY2ME"), Some("SBP"), "RU"),
            (usd(699), "card", None, None, "EN"),
            (usd(699), "paypal", Some("PAYPAL"), None, "EN"),
        ] {
            let currency = amount.currency.code();
            let (result, sent) = created(
                answer(json!(amount.units()), currency),
                &with_method(amount, code),
            )
            .await;
            assert!(result.is_ok(), "{code} {currency}: {result:?}");
            assert_eq!(sent["currency"], currency, "{code}");
            assert_eq!(sent["buyerLanguage"], language, "{code}");
            assert_eq!(
                sent.get("paymentProvider").and_then(Value::as_str),
                provider,
                "{code} {currency}"
            );
            assert_eq!(
                sent.get("paymentMethod").and_then(Value::as_str),
                kind,
                "{code} {currency}"
            );
        }
    }

    #[tokio::test]
    async fn чужой_способ_lava_отклоняется_без_запроса() {
        for (amount, code) in [
            (usd(699), "sbp"),
            (rub(49_900), "paypal"),
            (rub(49_900), "crypto"),
        ] {
            let (result, sent) =
                created(answer(json!(6.99), "USD"), &with_method(amount, code)).await;
            assert_eq!(
                result,
                Err(PaymentError::Rejected(
                    "этот способ оплаты недоступен".into()
                )),
                "{code}"
            );
            assert_eq!(sent, Value::Null, "{code}");
        }
    }

    #[tokio::test]
    async fn счёт_lava_на_сумму_тарифа() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/invoice"))
            .and(header("X-Api-Key", "lava-api-key"))
            .and(body_json(json!({
                "email": EMAIL,
                "offerId": OFFER,
                "currency": "USD",
                "amount": 6.99,
                "buyerLanguage": "EN",
                "successful_return_url": "https://panel.example.com/cabinet/payment-wait",
                "failure_return_url": "https://panel.example.com/cabinet/payment-wait",
                "cancel_return_url": "https://panel.example.com/cabinet/payment-wait"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(answer(json!(6.99), "USD")))
            .expect(1)
            .mount(&server)
            .await;
        let lava = Lava::with_api_base(server.uri());
        let created = lava
            .create_invoice(&settings(), &invoice(usd(699), Some(EMAIL)))
            .await
            .unwrap();
        assert_eq!(created.external_id.as_deref(), Some(CONTRACT));
        assert_eq!(created.pay_url, "https://app.lava.top/payment/7ea82675");
        let sent = server.received_requests().await.unwrap();
        assert!(
            String::from_utf8_lossy(&sent[0].body).contains("\"amount\":6.99"),
            "сумма уходит числом без плавающей точки"
        );
        assert_eq!(
            lava.create_invoice(&settings(), &invoice(usd(699), None))
                .await,
            Err(PaymentError::EmailRequired)
        );
    }

    #[tokio::test]
    async fn счёт_lava_в_рублях() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/invoice"))
            .and(body_partial_json(
                json!({"currency": "RUB", "offerId": OFFER, "buyerLanguage": "RU"}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(answer(json!(499), "RUB")))
            .expect(1)
            .mount(&server)
            .await;
        let lava = Lava::with_api_base(server.uri());
        let created = lava
            .create_invoice(&settings(), &invoice(rub(49_900), Some("c@example.com")))
            .await
            .unwrap();
        assert_eq!(created.external_id.as_deref(), Some(CONTRACT));
        let sent = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert!(
            String::from_utf8_lossy(&sent[0].body).contains("\"amount\":499"),
            "{body}"
        );
        assert!(body.get("paymentProvider").is_none(), "{body}");
        assert!(body.get("paymentMethod").is_none(), "{body}");
        assert!(lava.currencies().contains(&Currency::Rub));
        assert_eq!(
            lava.client_label(Currency::Rub, &ProviderConfig::default())
                .hint,
            "Карты российских банков"
        );
    }

    #[tokio::test]
    async fn сумма_счёта_lava_числом_строкой_и_целым() {
        for (amount, accepted) in [
            (json!(6.99), true),
            (json!("6.99"), true),
            (json!("6.990"), true),
            (json!(7), true),
            (json!(6), true),
            (json!("7"), true),
            (json!(10), false),
            (json!(5), false),
            (json!(7.5), false),
            (json!("6.98"), false),
        ] {
            let (result, _) = created(
                answer(amount.clone(), "USD"),
                &invoice(usd(699), Some(EMAIL)),
            )
            .await;
            assert_eq!(result.is_ok(), accepted, "{amount}: {result:?}");
            if !accepted {
                assert!(matches!(result, Err(PaymentError::Rejected(_))), "{amount}");
            }
        }
        let (rubles, _) = created(
            answer(json!(499), "RUB"),
            &invoice(rub(49_950), Some(EMAIL)),
        )
        .await;
        assert!(rubles.is_ok(), "{rubles:?}");
        let (dollars, _) =
            created(answer(json!(6.99), "RUB"), &invoice(usd(699), Some(EMAIL))).await;
        assert!(
            matches!(dollars, Err(PaymentError::Rejected(_))),
            "{dollars:?}"
        );
    }

    #[tokio::test]
    async fn цена_оффера_не_совпала_с_тарифом() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/invoice"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": CONTRACT,
                "amountTotal": {"currency": "USD", "amount": 10.0},
                "paymentUrl": "https://app.lava.top/payment/7ea82675"
            })))
            .mount(&server)
            .await;
        let result = Lava::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(usd(699), Some(EMAIL)))
            .await;
        assert!(matches!(result, Err(PaymentError::Rejected(_))));
    }

    #[tokio::test]
    async fn ключ_вебхука_сверяется() {
        let (_server, lava) =
            with_invoice("COMPLETED", json!({"amount": 6.99, "currency": "USD"})).await;
        let body = event("payment.success", json!(6.99), "USD");
        let outcome = lava
            .parse_webhook(&settings(), &keyed(body.clone()))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(usd(699)));
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::ProviderTxid(CONTRACT.into()))
        );
        assert_eq!(outcome.event_key, format!("payment.success:{CONTRACT}"));
        for headers in [
            vec![("X-Api-Key", "lava-api-key")],
            vec![("X-Api-Key", "lava-webhook-ke")],
            vec![],
        ] {
            assert_eq!(
                lava.parse_webhook(&settings(), &hook(&headers, body.clone()))
                    .await,
                Err(PaymentError::BadSignature),
                "{headers:?}"
            );
        }
    }

    #[tokio::test]
    async fn вебхук_lava_по_логину_и_паролю() {
        let server = MockServer::start().await;
        let lava = Lava::with_api_base(server.uri());
        let body = event("subscription.cancelled", json!(6.99), "USD");
        let basic = |pair: &str| format!("Basic {}", STANDARD.encode(pair));
        let signed = |value: &str| hook(&[("Authorization", value)], body.clone());
        for pair in [
            "lava:lava-webhook-key",
            ":lava-webhook-key",
            "shop:lava-webhook-key",
        ] {
            let outcome = lava
                .parse_webhook(&settings(), &signed(&basic(pair)))
                .await
                .unwrap();
            assert_eq!(outcome.payment, None, "{pair}");
        }
        for value in [
            basic("lava:wrong"),
            basic("lava-webhook-key:"),
            basic("lava-webhook-key"),
            format!("Bearer {KEY}"),
            "Basic не-base64".to_string(),
        ] {
            assert_eq!(
                lava.parse_webhook(&settings(), &signed(&value)).await,
                Err(PaymentError::BadSignature),
                "{value}"
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn без_ключа_вебхука_api_ключ_не_подходит() {
        let settings = config(&[
            ("api_key", json!("lava-api-key")),
            ("offer_id", json!(OFFER)),
        ]);
        let result = Lava::new()
            .parse_webhook(
                &settings,
                &hook(
                    &[("x-api-key", "lava-api-key")],
                    event("payment.failed", json!(6.99), "USD"),
                ),
            )
            .await;
        assert!(
            matches!(result, Err(PaymentError::NotConfigured(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn незнакомые_события_lava_получают_200() {
        let server = MockServer::start().await;
        let lava = Lava::with_api_base(server.uri());
        let mut bodies = vec![
            event("subscription.recurring.payment.success", json!(6.99), "USD"),
            event("subscription.cancelled", json!(6.99), "USD"),
            serde_json::to_vec(&json!({"eventType": "payment.success", "amount": 6.99})).unwrap(),
            serde_json::to_vec(&json!({"eventType": "payout.created"})).unwrap(),
            serde_json::to_vec(&json!({"hello": "world"})).unwrap(),
            serde_json::to_vec(&json!([1, 2, 3])).unwrap(),
            serde_json::to_vec(
                &json!({"event_id": "e-9", "event_type": "payout.done", "data": {}}),
            )
            .unwrap(),
            serde_json::to_vec(&json!({"event_id": "e-8", "event_type": "refund.success"}))
                .unwrap(),
        ];
        bodies.push(refund("full", "r-eur", json!(6.99), "EUR"));
        bodies.push(refund("full", "r-zero", json!(0), "USD"));
        let mut no_email: Value =
            serde_json::from_slice(&refund("full", "r-x", json!(6.99), "USD")).unwrap();
        no_email["data"]["customer_email"] = Value::Null;
        bodies.push(serde_json::to_vec(&no_email).unwrap());
        for body in bodies {
            let outcome = lava
                .parse_webhook(&settings(), &keyed(body.clone()))
                .await
                .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&body)));
            assert_eq!(outcome.payment, None, "{}", String::from_utf8_lossy(&body));
            assert_eq!(outcome.status, ProviderStatus::Pending);
            assert_eq!(outcome.reply.status(), 200);
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(matches!(
            lava.parse_webhook(&settings(), &keyed(b"not json".to_vec()))
                .await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn оплата_lava_перепроверяется_через_api() {
        let (_server, lava) =
            with_invoice("IN_PROGRESS", json!({"amount": 6.99, "currency": "USD"})).await;
        let early = lava
            .parse_webhook(
                &settings(),
                &keyed(event("payment.success", json!(6.99), "USD")),
            )
            .await;
        assert!(
            matches!(&early, Err(e @ PaymentError::Transport(_)) if e.is_retryable()),
            "{early:?}"
        );
        let waiting = lava
            .parse_webhook(
                &settings(),
                &keyed(event("payment.failed", json!(6.99), "USD")),
            )
            .await
            .unwrap();
        assert_eq!(waiting.status, ProviderStatus::Pending);
        let (_server, lava) =
            with_invoice("FAILED", json!({"amount": 6.99, "currency": "USD"})).await;
        for kind in ["payment.success", "payment.failed"] {
            let outcome = lava
                .parse_webhook(&settings(), &keyed(event(kind, json!(6.99), "USD")))
                .await
                .unwrap();
            assert_eq!(outcome.status, ProviderStatus::Failed, "{kind}");
            assert_eq!(outcome.amount, None);
            assert_eq!(
                outcome.payment,
                Some(PaymentRef::ProviderTxid(CONTRACT.into()))
            );
        }
        let (_server, lava) =
            with_invoice("COMPLETED", json!({"amount": 6.99, "currency": "USD"})).await;
        let late = lava
            .parse_webhook(
                &settings(),
                &keyed(event("payment.failed", json!(6.99), "USD")),
            )
            .await
            .unwrap();
        assert_eq!(late.status, ProviderStatus::Paid);
        assert_eq!(late.amount, Some(usd(699)));
    }

    #[tokio::test]
    async fn чужой_счёт_в_ответе_api_lava_не_засчитывается() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/invoices/{CONTRACT}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": SECOND,
                "status": "COMPLETED",
                "receipt": {"amount": 6.99, "currency": "USD"}
            })))
            .mount(&server)
            .await;
        let result = Lava::with_api_base(server.uri())
            .parse_webhook(
                &settings(),
                &keyed(event("payment.success", json!(6.99), "USD")),
            )
            .await;
        assert!(
            matches!(result, Err(PaymentError::BadPayload(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_lava() {
        let expected = usd(699);
        let paid = |receipt: Value| async move {
            let (_server, lava) = with_invoice("COMPLETED", receipt).await;
            lava.parse_webhook(
                &settings(),
                &keyed(event("payment.success", json!(6.99), "USD")),
            )
            .await
        };
        let under = paid(json!({"amount": 6.98, "currency": "USD"}))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, under.amount),
            AmountCheck::Underpaid { .. }
        ));
        let rub = paid(json!({"amount": 6.99, "currency": "RUB"}))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, rub.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        for receipt in [
            json!({"amount": 6.999, "currency": "USD"}),
            json!({"amount": "abc", "currency": "USD"}),
            json!({"amount": 6.99, "currency": "EUR"}),
        ] {
            assert!(
                matches!(
                    paid(receipt.clone()).await,
                    Err(PaymentError::BadPayload(_))
                ),
                "{receipt}"
            );
        }
    }

    #[tokio::test]
    async fn полный_возврат_lava_находит_оплату_по_почте_и_сумме() {
        let (_server, lava) = with_sales(vec![
            sold(CONTRACT, "client@example.com", json!(6.99), "STEALTHNET"),
            sold(
                SECOND,
                "other.client@example.com",
                json!(6.99),
                "STEALTHNET",
            ),
            sold(
                "0d8a7c44-1b9e-4c0f-9a51-3e2f6d7b8c90",
                EMAIL,
                json!(6.99),
                "Курс",
            ),
            sold(
                "6f1e2d3c-4b5a-4978-8a6b-5c4d3e2f1a0b",
                EMAIL,
                json!(2.99),
                "STEALTHNET",
            ),
        ])
        .await;
        let outcome = lava
            .parse_webhook(
                &settings(),
                &keyed(refund("full", "r-1", json!(6.99), "USD")),
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Refunded);
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::ProviderTxid(CONTRACT.into()))
        );
        assert_eq!(outcome.provider_txid.as_deref(), Some(CONTRACT));
        assert_eq!(outcome.amount, Some(usd(699)));
        assert_eq!(outcome.event_key, "refund:r-1");
    }

    #[tokio::test]
    async fn частичный_возврат_lava() {
        let (_server, lava) = with_sales(vec![
            sold(CONTRACT, EMAIL, json!(6.99), "STEALTHNET"),
            sold(SECOND, EMAIL, json!(1.99), "STEALTHNET"),
        ])
        .await;
        let outcome = lava
            .parse_webhook(
                &settings(),
                &keyed(refund("partial", "r-2", json!("2.50"), "USD")),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.status,
            ProviderStatus::PartiallyRefunded { refunded: usd(250) }
        );
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::ProviderTxid(CONTRACT.into()))
        );
        assert_eq!(outcome.event_key, "refund:r-2");
    }

    #[tokio::test]
    async fn возврат_lava_без_однозначной_оплаты_пропускается() {
        for (items, body) in [
            (
                vec![
                    sold(CONTRACT, EMAIL, json!(6.99), "STEALTHNET"),
                    sold(SECOND, EMAIL, json!(6.99), "STEALTHNET"),
                ],
                refund("full", "r-3", json!(6.99), "USD"),
            ),
            (
                vec![
                    sold(CONTRACT, EMAIL, json!(6.99), "STEALTHNET"),
                    sold(SECOND, EMAIL, json!(9.99), "STEALTHNET"),
                ],
                refund("partial", "r-4", json!(1), "USD"),
            ),
            (vec![], refund("full", "r-5", json!(6.99), "USD")),
            (
                vec![sold(CONTRACT, EMAIL, json!(1.99), "STEALTHNET")],
                refund("full", "r-6", json!(6.99), "USD"),
            ),
        ] {
            let several = items.len() > 1;
            let (_server, lava) = with_sales(items).await;
            let outcome = lava.parse_webhook(&settings(), &keyed(body)).await.unwrap();
            assert_eq!(outcome.payment, None, "{}", outcome.event_key);
            assert_eq!(outcome.status, ProviderStatus::Pending);
            let notice = outcome.notice().unwrap();
            assert_eq!(notice.kind, NoticeKind::UnmatchedRefund);
            assert!(
                notice.text.to_lowercase().contains(EMAIL)
                    && notice.text.contains(&outcome.event_key[7..]),
                "{}",
                notice.text
            );
            assert!(outcome.amount.is_some_and(|a| a.minor > 0));
            if several && outcome.event_key == "refund:r-3" {
                assert!(
                    notice.text.contains("несколько оплат (2)"),
                    "{}",
                    notice.text
                );
            }
        }
    }

    #[tokio::test]
    async fn неполные_уведомления_о_возврате_lava_тоже_уходят_админу() {
        let (server, lava) = with_sales(vec![]).await;
        let mut no_email: Value =
            serde_json::from_slice(&refund("full", "r-x", json!(6.99), "USD")).unwrap();
        no_email["data"]["customer_email"] = Value::Null;
        for (body, why) in [
            (serde_json::to_vec(&no_email).unwrap(), "нет почты"),
            (refund("full", "r-zero", json!(0), "USD"), "нет суммы"),
            (refund("full", "r-eur", json!(6.99), "EUR"), "нет суммы"),
        ] {
            let outcome = lava.parse_webhook(&settings(), &keyed(body)).await.unwrap();
            assert_eq!(outcome.payment, None);
            let notice = outcome.notice().unwrap();
            assert_eq!(notice.kind, NoticeKind::UnmatchedRefund);
            assert!(notice.text.contains(why), "{}", notice.text);
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn чарджбэк_lava_считается_возвратом() {
        let (_server, lava) =
            with_sales(vec![sold(CONTRACT, EMAIL, json!(6.99), "STEALTHNET")]).await;
        let full = lava
            .parse_webhook(&settings(), &keyed(chargeback(json!(6.99))))
            .await
            .unwrap();
        assert_eq!(full.status, ProviderStatus::Refunded);
        assert_eq!(full.event_key, "chargeback:cb-1");
        assert_eq!(
            full.payment,
            Some(PaymentRef::ProviderTxid(CONTRACT.into()))
        );
        let notice = full.notice().unwrap();
        assert_eq!(notice.kind, NoticeKind::Chargeback);
        assert!(
            notice.text.contains("подписка не изменена"),
            "{}",
            notice.text
        );
        let rest = lava
            .parse_webhook(&settings(), &keyed(chargeback(json!(4.49))))
            .await
            .unwrap();
        assert_eq!(
            rest.status,
            ProviderStatus::PartiallyRefunded { refunded: usd(449) }
        );
        let mut untyped: Value =
            serde_json::from_slice(&refund("full", "r-7", json!(6.99), "USD")).unwrap();
        untyped["data"]["refund_type"] = Value::Null;
        let outcome = lava
            .parse_webhook(&settings(), &keyed(serde_json::to_vec(&untyped).unwrap()))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Refunded);
        assert_eq!(outcome.signal, None);
        let (_other, empty) = with_sales(vec![]).await;
        let lost = empty
            .parse_webhook(&settings(), &keyed(chargeback(json!(6.99))))
            .await
            .unwrap();
        assert_eq!(lost.payment, None);
        assert_eq!(lost.amount, Some(usd(699)));
        let notice = lost.notice().unwrap();
        assert_eq!(notice.kind, NoticeKind::Chargeback);
        assert!(notice.text.contains("чарджбэке"), "{}", notice.text);
    }

    #[tokio::test]
    async fn оплаты_lava_на_нескольких_страницах() {
        let server = MockServer::start().await;
        let page = |n: &str, items: Vec<Value>| {
            Mock::given(method("GET"))
                .and(path("/api/v2/invoices"))
                .and(query_param("page", n))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "items": items, "page": 1, "size": 1, "total": 2
                })))
        };
        page(
            "1",
            vec![sold(
                SECOND,
                "a.client@example.com",
                json!(6.99),
                "STEALTHNET",
            )],
        )
        .expect(1)
        .mount(&server)
        .await;
        page("2", vec![sold(CONTRACT, EMAIL, json!(6.99), "STEALTHNET")])
            .expect(1)
            .mount(&server)
            .await;
        let outcome = Lava::with_api_base(server.uri())
            .parse_webhook(
                &settings(),
                &keyed(refund("full", "r-8", json!(6.99), "USD")),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::ProviderTxid(CONTRACT.into()))
        );
    }

    #[tokio::test]
    async fn статус_счёта_lava() {
        let (_server, lava) = with_invoice(
            "COMPLETED",
            json!({"amount": 6.99, "currency": "USD", "fee": 0.35}),
        )
        .await;
        let outcome = lava
            .fetch_status(&settings(), CONTRACT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(usd(699)));
        assert_eq!(outcome.event_key, format!("invoice.COMPLETED:{CONTRACT}"));
        assert!(matches!(
            lava.fetch_status(&settings(), "../x").await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[test]
    fn настройки_lava() {
        let lava = Lava::new();
        assert!(lava.is_configured(&settings()));
        assert!(lava.validate_settings(&settings()).is_ok());
        let old = config(&[("api_key", json!("k")), ("offer_id", json!(OFFER))]);
        assert!(
            lava.is_configured(&old),
            "старые настройки без ключа вебхука продолжают выставлять счета"
        );
        assert!(lava.validate_settings(&old).is_err());
        assert!(!lava.is_configured(&config(&[("api_key", json!("k"))])));
        assert!(!lava.is_configured(&config(&[
            ("api_key", json!("k")),
            ("offer_id", json!("offer-1"))
        ])));
        for key in [
            "k".repeat(81),
            "ключ-вебхука".to_string(),
            "two words".to_string(),
            "k".to_string(),
            "k".repeat(15),
        ] {
            let mut bad = settings();
            bad.set("webhook_key", json!(key));
            assert!(lava.validate_settings(&bad).is_err(), "{key}");
            assert!(lava.is_configured(&bad), "{key}");
        }
        for length in [16, 80] {
            let mut good = settings();
            good.set("webhook_key", json!("k".repeat(length)));
            assert!(lava.validate_settings(&good).is_ok(), "{length}");
        }
        let schema = lava.settings_schema();
        let field = schema.iter().find(|f| f.key == "webhook_key").unwrap();
        assert!(field.required && field.is_secret());
        for needle in [
            "Добавить Webhook",
            "Успешный возврат",
            "чарджбэк",
            "Basic",
            "от 16 до 80",
        ] {
            assert!(field.hint.contains(needle), "{needle}");
        }
    }
}
