use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sn_domain::money::{Currency, PayCurrency};
use sn_domain::payment::{NoticeKind, providers};
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://rollypay.io";
const API_KEY: &str = "api_key";
const TERMINAL_ID: &str = "terminal_id";
const SIGNING_SECRET: &str = "signing_secret";
const TEST_MODE: &str = "test_mode";
const TERMINAL_REQUIRED: &str =
    "RollyPay просит ID кассы: заполните «ID кассы (Terminal ID)» в настройках RollyPay";
const TERMINAL_WHERE: &str = "ID кассы — UUID вида d290f1ee-6c54-4b01-90e6-d701748f0851. Ищите его в кабинете RollyPay в настройках кассы, там же, где API-ключ; если не нашли, спросите у поддержки RollyPay";
const WINDOW_SECONDS: i64 = 15 * 60;
const DRIFT_WARN_SECONDS: i64 = 5 * 60;
const KEY_MAX: usize = 512;
const HOOK_LABEL: &str = "callback_url";
const PLACEHOLDER_HOST: &str = "example.com";
const NONCE_REUSED: &str = "RollyPay отклонила запрос как повторный (X-Nonce), повторите попытку";
const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков");
const INTL_CARD: PayMethod =
    PayMethod::new("intl_card", "Иностранная карта", "Карты зарубежных банков");
const CRYPTO: PayMethod = PayMethod::new("crypto", "Криптовалюта", "Перевод с криптокошелька");
const PAGE: PayMethod = PayMethod::new(
    "page",
    "Все способы",
    "Способ выбирается на странице оплаты",
);
const METHODS: [PayMethod; 5] = [SBP, CARD, INTL_CARD, CRYPTO, PAGE];

pub fn signature(secret: &str, sent: &str, body: &[u8]) -> String {
    signer(secret, sent, body)
        .map(|mac| hex::encode(mac.finalize().into_bytes()))
        .unwrap_or_default()
}

fn signer(secret: &str, sent: &str, body: &[u8]) -> Option<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(sent.as_bytes());
    mac.update(b".");
    mac.update(body);
    Some(mac)
}

fn verify(secret: &str, sent: &str, body: &[u8], header: &str) -> bool {
    let Ok(expected) = hex::decode(header.trim()) else {
        return false;
    };
    signer(secret, sent, body).is_some_and(|mac| mac.verify_slice(&expected).is_ok())
}

fn sent_at(raw: &str) -> Option<DateTime<Utc>> {
    if raw.is_empty() || raw.len() > 16 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: i64 = raw.parse().ok()?;
    if value >= 100_000_000_000 {
        DateTime::from_timestamp_millis(value)
    } else {
        DateTime::from_timestamp(value, 0)
    }
}

fn minutes(seconds: i64) -> i64 {
    (seconds.abs() + 59) / 60
}

fn fresh(raw: &str, now: DateTime<Utc>) -> PaymentResult<()> {
    let sent =
        sent_at(raw).ok_or_else(|| util::bad("время отправки в X-Timestamp не разобрано"))?;
    let drift = now.signed_duration_since(sent).num_seconds();
    if drift.abs() <= WINDOW_SECONDS {
        return Ok(());
    }
    let window = WINDOW_SECONDS / 60;
    Err(util::bad(if drift > 0 {
        format!(
            "отправлено {} мин назад, а принимаются уведомления не старше {window} мин: \
             это повтор старого уведомления или часы сервера панели спешат",
            minutes(drift)
        )
    } else {
        format!(
            "время отправки на {} мин впереди часов сервера панели: часы сервера отстают",
            minutes(drift)
        )
    }))
}

fn flagged(value: Option<&Json>) -> bool {
    match value {
        Some(Json::Bool(flag)) => *flag,
        Some(other) => other
            .scalar_text()
            .is_some_and(|t| t.trim() == "1" || t.trim().eq_ignore_ascii_case("true")),
        None => false,
    }
}

fn sandbox(object: &Json, header: Option<&str>) -> bool {
    flagged(object.get("test"))
        || object
            .get("environment")
            .and_then(Json::as_str)
            .is_some_and(|e| e.trim().eq_ignore_ascii_case("sandbox"))
        || header.is_some_and(|h| h.trim().eq_ignore_ascii_case("true"))
}

fn text<'a>(object: &'a Json, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn payment_outcome(object: &Json, test: bool, test_mode: bool) -> PaymentResult<WebhookOutcome> {
    let id = util::safe_id(text(object, "payment_id").ok_or_else(|| util::bad("нет payment_id"))?)?;
    let status = text(object, "status")
        .ok_or_else(|| util::bad("нет status"))?
        .to_ascii_lowercase();
    let status = util::clip(&status, 32);
    if test && !test_mode {
        return Ok(util::skipped(
            format!("{id}:{status}:test"),
            None,
            None,
            WebhookReply::ok(),
        ));
    }
    let currency = object
        .get("currency")
        .or_else(|| object.get("payment_currency"));
    let amount = util::json_amount(object.get("amount"), currency)?;
    let payment = util::public_ref(text(object, "order_id"))
        .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string())));
    let (event_key, state) = match status.as_str() {
        "paid" => (format!("{id}:paid"), ProviderStatus::Paid),
        "canceled" | "expired" => (format!("{id}:{status}"), ProviderStatus::Canceled),
        "refunded" => (format!("refund:{id}"), ProviderStatus::Refunded),
        "chargeback" => (format!("chargeback:{id}"), ProviderStatus::Refunded),
        _ => (format!("{id}:{status}"), ProviderStatus::Pending),
    };
    let outcome = WebhookOutcome {
        event_key,
        payment,
        provider_txid: Some(id.to_string()),
        status: state,
        amount: Some(amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    };
    Ok(if status == "chargeback" {
        outcome.with_notice(
            NoticeKind::Chargeback,
            format!(
                "RollyPay сообщила о чарджбэке по платежу {id} на {}: банк покупателя отозвал оплату. \
                 Платёж отмечен возвращённым, подписка не изменена — решите, отключать ли её",
                util::amount_text(amount)
            ),
        )
    } else {
        outcome
    })
}

fn wire_method(chosen: Option<&str>) -> PaymentResult<Option<&'static str>> {
    match chosen {
        None => Ok(None),
        Some(code) if code == PAGE.code => Ok(None),
        Some(code) => METHODS
            .iter()
            .find(|m| m.code == code && m.code != PAGE.code)
            .map(|m| Some(m.code))
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn error_text(body: &[u8]) -> Option<String> {
    let parsed = Json::parse(body)?;
    parsed
        .get("error")
        .or_else(|| parsed.get("message"))
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

fn terminal_required(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("terminal_id") && lower.contains("required")
}

fn plain(message: &str) -> String {
    if terminal_required(message) {
        return TERMINAL_REQUIRED.into();
    }
    match message.to_ascii_lowercase().as_str() {
        "terminal not found" => "RollyPay не нашла кассу этого API-ключа".into(),
        "payment cap exceeded" => "превышен лимит платежей кассы RollyPay".into(),
        "amount must be positive" => "сумма платежа должна быть больше нуля".into(),
        _ => util::clip(message, 200),
    }
}

fn nonce_reused(reply: &util::Reply) -> bool {
    matches!(reply.status, 401 | 409)
        && error_text(&reply.body).is_some_and(|t| t.to_ascii_lowercase().contains("nonce"))
}

fn failure(reply: &util::Reply) -> PaymentError {
    if nonce_reused(reply) {
        return PaymentError::Rejected(NONCE_REUSED.into());
    }
    let message = error_text(&reply.body).map(|t| plain(&t));
    reply.failure(message.as_deref())
}

fn key_ok(value: &str) -> bool {
    value.len() <= KEY_MAX && value.bytes().all(|b| b.is_ascii_graphic())
}

fn terminal_ok(value: &str) -> bool {
    value.len() == 36 && Uuid::parse_str(value).is_ok()
}

fn terminal_of(config: &ProviderConfig) -> Option<&str> {
    config.get(TERMINAL_ID).filter(|t| terminal_ok(t))
}

fn scoped(path: &str, config: &ProviderConfig) -> String {
    match terminal_of(config) {
        Some(terminal) => format!("{path}?terminal_id={terminal}"),
        None => path.to_string(),
    }
}

fn terminal_problem(reply: &util::Reply, configured: Option<&str>) -> Option<ConnectionCheck> {
    if reply.is_success() || nonce_reused(reply) {
        return None;
    }
    let message = error_text(&reply.body);
    let said = message
        .as_deref()
        .map(|m| format!("«{}»", util::clip(m, 120)))
        .unwrap_or_else(|| format!("HTTP {}", reply.status));
    if message.as_deref().is_some_and(terminal_required) {
        let check = match configured {
            None => ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                "RollyPay просит ID кассы: заполните поле «ID кассы (Terminal ID)»",
            ),
            Some(terminal) => ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                format!("RollyPay не приняла ID кассы {terminal}: ответ {said}"),
            ),
        };
        return Some(check.detail(TERMINAL_WHERE));
    }
    let terminal = configured?;
    let unknown = message
        .as_deref()
        .is_some_and(|m| m.to_ascii_lowercase().contains("terminal"));
    (matches!(reply.status, 403 | 404) || unknown).then(|| {
        ConnectionCheck::new(
            CheckStatus::SettingsRejected,
            format!("ID кассы {terminal} не подходит к API-ключу: RollyPay ответила {said}"),
        )
        .detail(
            "ID кассы и API-ключ должны быть от одной кассы. Проверьте ID или оставьте поле пустым, если RollyPay определяет кассу по ключу",
        )
    })
}

fn placeholder(address: &str) -> bool {
    reqwest::Url::parse(address)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == PLACEHOLDER_HOST)
}

fn terminal_hook(terminal: &Json, expected: &str) -> HookCheck {
    let hook = |state, address: Option<String>, note: Option<String>| HookCheck {
        label: HOOK_LABEL.to_string(),
        state,
        address,
        note,
    };
    match text(terminal, "callback_url") {
        None => hook(HookState::Off, None, None),
        Some(address) if same_hook(expected, address) => hook(HookState::Same, None, None),
        Some(address) => hook(
            HookState::Other,
            Some(hook_address(address)),
            placeholder(address).then(|| {
                "Это заглушка новой кассы: уведомления до панели не дойдут, оплаты засчитаются только опросом и с задержкой".to_string()
            }),
        ),
    }
}

struct Answer {
    reply: util::Reply,
    date: Option<DateTime<Utc>>,
}

pub struct RollyPay {
    http: sn_egress::Http,
    api_base: String,
}

impl Default for RollyPay {
    fn default() -> Self {
        Self::new()
    }
}

impl RollyPay {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        RollyPay {
            http: http_client(sn_domain::payment::providers::ROLLYPAY),
            api_base: api_base.into().trim_end_matches('/').to_string(),
        }
    }

    async fn send(
        &self,
        key: &str,
        method: &reqwest::Method,
        path: &str,
        body: Option<&Json>,
    ) -> PaymentResult<Answer> {
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.api_base))
            .header("X-API-Key", key)
            .header("X-Nonce", Uuid::new_v4().to_string())
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes());
        }
        let response = request.send().await.map_err(PaymentError::transport)?;
        let status = response.status().as_u16();
        let date = response
            .headers()
            .get(reqwest::header::DATE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| DateTime::parse_from_rfc2822(v.trim()).ok())
            .map(|d| d.with_timezone(&Utc));
        let body = response.bytes().await.map_err(PaymentError::transport)?;
        Ok(Answer {
            reply: util::Reply { status, body },
            date,
        })
    }

    async fn call(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
        body: Option<&Json>,
    ) -> PaymentResult<Answer> {
        let key = config.require(API_KEY)?;
        let answer = self.send(key, &method, path, body).await?;
        if nonce_reused(&answer.reply) {
            return self.send(key, &method, path, body).await;
        }
        Ok(answer)
    }

    async fn payment(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Json> {
        let id = util::safe_id(id.trim())?;
        let reply = self
            .call(
                config,
                reqwest::Method::GET,
                &scoped(&format!("/api/v1/payments/{id}"), config),
                None,
            )
            .await?
            .reply;
        if reply.status == 404 {
            return Err(PaymentError::Rejected("платёж не найден в RollyPay".into()));
        }
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        if text(&parsed, "payment_id") != Some(id) {
            return Err(util::bad("RollyPay вернула другой платёж"));
        }
        Ok(parsed)
    }

    async fn terminal(&self, config: &ProviderConfig, terminal: &str) -> Option<Json> {
        let id = util::safe_id(terminal).ok()?;
        let reply = self
            .call(
                config,
                reqwest::Method::GET,
                &format!("/api/v1/terminals/{id}"),
                None,
            )
            .await
            .ok()?
            .reply;
        let parsed = reply.is_success().then(|| reply.json().ok()).flatten()?;
        parsed.get("callback_url").is_some().then_some(parsed)
    }
}

#[async_trait]
impl PaymentProvider for RollyPay {
    fn api_url(&self, _config: &ProviderConfig) -> Option<String> {
        Some(self.api_base.clone())
    }

    fn id(&self) -> &'static str {
        providers::ROLLYPAY
    }

    fn title(&self) -> &'static str {
        "RollyPay"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: SBP.title,
            hint: SBP.hint,
        }
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        if currency == Currency::Rub {
            METHODS.to_vec()
        } else {
            Vec::new()
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret(
                API_KEY,
                "API-ключ",
                "Выдаётся при создании кассы RollyPay, начинается с rpk_",
            ),
            SettingField::text(
                TERMINAL_ID,
                "ID кассы (Terminal ID)",
                "UUID кассы вида d290f1ee-6c54-4b01-90e6-d701748f0851: кабинет RollyPay, настройки кассы, там же, где API-ключ, или поддержка RollyPay. Пустое поле — касса берётся по API-ключу",
            )
            .optional(),
            SettingField::secret(
                SIGNING_SECRET,
                "Ключ подписи уведомлений",
                "signing_secret кассы, выдаётся вместе с API-ключом. По нему панель проверяет, что уведомление прислала RollyPay",
            ),
            SettingField::flag(
                TEST_MODE,
                "Тестовый режим",
                "Счета создаются тестовыми: деньги не списываются, оплату имитирует кнопка на странице RollyPay. Тестовые оплаты засчитываются только в этом режиме",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        for (key, title) in [(API_KEY, "API-ключ"), (SIGNING_SECRET, "Ключ подписи")]
        {
            if !config.get(key).is_some_and(key_ok) {
                return Err(PaymentError::NotConfigured(format!(
                    "{title} RollyPay вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов"
                )));
            }
        }
        if config.get(TERMINAL_ID).is_some_and(|t| !terminal_ok(t)) {
            return Err(PaymentError::NotConfigured(
                "ID кассы RollyPay — UUID вида d290f1ee-6c54-4b01-90e6-d701748f0851, без пробелов и скобок"
                    .into(),
            ));
        }
        Ok(())
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let chosen = wire_method(request.method.as_deref())?;
        let order = request.payment_public_id.to_string();
        let test = config.flag(TEST_MODE);
        let mut body = Json::object([
            ("amount", Json::from(request.amount.units())),
            ("payment_currency", Json::from("RUB")),
            ("order_id", Json::from(order.clone())),
            (
                "description",
                Json::from(util::clip(&request.description, 255)),
            ),
            (
                "customer_id",
                Json::from(request.client.public_id.to_string()),
            ),
        ]);
        if let Some(terminal) = terminal_of(config) {
            body.push("terminal_id", Json::from(terminal));
        }
        if let Some(code) = chosen {
            body.push("payment_method", Json::from(code));
        }
        if let Some(url) = util::return_url(request) {
            body.push("success_redirect_url", Json::from(url.clone()));
            body.push("fail_redirect_url", Json::from(url));
        }
        if test {
            body.push("test", Json::from(true));
        }
        let reply = self
            .call(
                config,
                reqwest::Method::POST,
                "/api/v1/payments",
                Some(&body),
            )
            .await?
            .reply;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let id = util::safe_id(
            text(&parsed, "payment_id")
                .ok_or_else(|| util::bad("RollyPay не вернула номер платежа"))?,
        )?;
        if text(&parsed, "order_id").is_some_and(|o| o != order) {
            return Err(util::bad("RollyPay вернула платёж другого заказа"));
        }
        let pay_url = text(&parsed, "pay_url")
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("RollyPay не вернула ссылку на оплату"))?;
        if parsed.get("amount").is_some() {
            let currency = parsed
                .get("payment_currency")
                .cloned()
                .unwrap_or_else(|| Json::from("RUB"));
            let billed = util::json_amount(parsed.get("amount"), Some(&currency))?;
            if billed != request.amount {
                return Err(util::bad(format!(
                    "RollyPay выставила счёт на {} вместо {}",
                    util::amount_text(billed),
                    util::amount_text(request.amount)
                )));
            }
        }
        let expires_at = text(&parsed, "expires_at")
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc));
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at,
            payload: serde_json::json!({
                "payment_id": id,
                "status": text(&parsed, "status"),
                "test": sandbox(&parsed, None),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let secret = config.require(SIGNING_SECRET)?;
        if request
            .route_suffix
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(util::bad(
                "у RollyPay один адрес уведомлений, без дополнительной части",
            ));
        }
        let (Some(sent), Some(signed)) =
            (request.header("x-timestamp"), request.header("x-signature"))
        else {
            return Err(PaymentError::BadSignature);
        };
        let sent = sent.trim();
        if !verify(secret, sent, &request.body, signed) {
            return Err(PaymentError::BadSignature);
        }
        fresh(sent, Utc::now())?;
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        if let Some(event) = text(&body, "event_type").filter(|e| e.starts_with("payout.")) {
            return Ok(util::skipped(
                util::clip(event, 64),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let test = sandbox(&body, request.header("x-test-mode"));
        payment_outcome(&body, test, config.flag(TEST_MODE))
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let object = self.payment(config, external_id).await?;
        let test = sandbox(&object, None);
        payment_outcome(&object, test, config.flag(TEST_MODE)).map(Some)
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let configured = terminal_of(config);
        let answer = match self
            .call(
                config,
                reqwest::Method::GET,
                &scoped("/api/v1/balance", config),
                None,
            )
            .await
        {
            Ok(answer) => answer,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let reply = &answer.reply;
        if let Some(problem) = terminal_problem(reply, configured) {
            return problem;
        }
        if matches!(reply.status, 401 | 403) && !nonce_reused(reply) {
            return ConnectionCheck::new(CheckStatus::KeysRejected, "RollyPay не приняла API-ключ")
                .detail(
                    "API-ключ и ключ подписи выдаются при создании кассы RollyPay: в кабинете RollyPay или через её поддержку",
                );
        }
        if !reply.is_success() {
            return ConnectionCheck::failed(&failure(reply));
        }
        let balance = match reply.json() {
            Ok(parsed) if parsed.get("available_usdt").is_some() => parsed,
            Ok(_) => {
                return ConnectionCheck::failed(&util::bad("в ответе RollyPay нет баланса кассы"));
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let found = text(&balance, "terminal_id").map(|t| util::clip(t, 64));
        if let (Some(configured), Some(found)) = (configured, found.as_deref())
            && !found.eq_ignore_ascii_case(configured)
        {
            return ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                format!(
                    "ID кассы в настройках ({configured}) не совпадает с кассой API-ключа ({found})"
                ),
            )
            .detail("Укажите ID кассы, к которой выдан этот API-ключ, или оставьте поле пустым");
        }
        let mut check = ConnectionCheck::ok("API-ключ принят RollyPay");
        match (configured, found.as_deref()) {
            (Some(terminal), _) => {
                check = check.detail(format!("ID кассы {terminal} подходит к API-ключу"));
            }
            (None, Some(found)) => {
                check = check.detail(format!(
                    "Ключ относится к кассе {found}. Впишите этот ID в поле «ID кассы (Terminal ID)»: RollyPay может требовать его в запросах"
                ));
            }
            (None, None) => {}
        }
        let terminal = configured.map(str::to_string).or(found);
        if config.flag(TEST_MODE) {
            check = check.detail(
                "Включён тестовый режим: счета создаются тестовыми, деньги не списываются. Перед приёмом настоящих оплат выключите его",
            );
        }
        match context.webhook_url.as_deref() {
            None => {
                check = check.detail(
                    "Адрес панели не задан в основных настройках: RollyPay некуда присылать уведомления",
                );
            }
            Some(expected) => {
                let found = match terminal.as_deref() {
                    Some(terminal) => self.terminal(config, terminal).await,
                    None => None,
                };
                match found {
                    Some(found) => check.hooks.push(terminal_hook(&found, expected)),
                    None => {
                        check = check.detail(
                            "Адрес уведомлений из кассы RollyPay прочитать не удалось. Проверьте поле callback_url в настройках кассы: у новой кассы там стоит заглушка example.com",
                        );
                    }
                }
            }
        }
        if let Some(date) = answer.date {
            let drift = Utc::now().signed_duration_since(date).num_seconds();
            if drift.abs() > DRIFT_WARN_SECONDS {
                check = check.detail(format!(
                    "Часы сервера панели расходятся с RollyPay на {} мин. Уведомления старше {} минут по часам панели отклоняются — сверьте время на сервере",
                    minutes(drift),
                    WINDOW_SECONDS / 60
                ));
            }
        }
        check.detail(
            "Ключ подписи этим запросом не проверить: он проверяется на уведомлениях. Чтобы проверить его, оплатите тестовый счёт в тестовом режиме",
        )
    }
}

#[cfg(test)]
mod tests;
