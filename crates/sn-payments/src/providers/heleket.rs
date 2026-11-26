use super::merchant;
use crate::api_address::{API_URL, ApiAddress};
use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use chrono::DateTime;
use sn_domain::money::{Currency, PayCurrency};
use sn_domain::payment::{NoticeKind, providers};
use sn_domain::settings::SettingField;
use uuid::Uuid;

pub use merchant::signature;

const ADDRESS: ApiAddress = ApiAddress::new("https://api.heleket.hk");
const BRAND: &str = "Heleket";

const PAGE: PayMethod = PayMethod::new(
    "page",
    "Любая монета",
    "Монету и сеть выбирают на странице Heleket",
);

struct Coin {
    method: PayMethod,
    currency: &'static str,
    network: &'static str,
}

const COINS: [Coin; 6] = [
    Coin {
        method: PayMethod::new("usdt_trc20", "USDT TRC-20", "USDT в сети Tron"),
        currency: "USDT",
        network: "tron",
    },
    Coin {
        method: PayMethod::new("usdt_ton", "USDT TON", "USDT в сети TON"),
        currency: "USDT",
        network: "ton",
    },
    Coin {
        method: PayMethod::new("usdt_bep20", "USDT BEP-20", "USDT в сети BNB Smart Chain"),
        currency: "USDT",
        network: "bsc",
    },
    Coin {
        method: PayMethod::new("btc", "Bitcoin", "BTC в сети Bitcoin"),
        currency: "BTC",
        network: "btc",
    },
    Coin {
        method: PayMethod::new("ton", "TON", "Gram, бывший Toncoin, в сети TON"),
        currency: "GRAM",
        network: "ton",
    },
    Coin {
        method: PayMethod::new("ltc", "Litecoin", "LTC в сети Litecoin"),
        currency: "LTC",
        network: "ltc",
    },
];

pub struct Heleket {
    http: sn_egress::Http,
    api_base: Option<String>,
}

impl Default for Heleket {
    fn default() -> Self {
        Self::new()
    }
}

impl Heleket {
    pub fn new() -> Self {
        Heleket {
            http: http_client(sn_domain::payment::providers::HELEKET),
            api_base: None,
        }
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Heleket {
            http: http_client(sn_domain::payment::providers::HELEKET),
            api_base: Some(api_base.into()),
        }
    }

    fn base(&self, config: &ProviderConfig) -> String {
        match &self.api_base {
            Some(base) => base.trim_end_matches('/').to_string(),
            None => ADDRESS.resolve(config),
        }
    }

    async fn call(&self, config: &ProviderConfig, path: &str, body: &Json) -> PaymentResult<Json> {
        merchant::call(
            &self.http,
            format!("{}{path}", self.base(config)),
            config,
            body,
        )
        .await
    }

    async fn read(&self, config: &ProviderConfig, info: &Json) -> PaymentResult<WebhookOutcome> {
        let outcome = surplus(info, merchant::outcome(info, REFUND_NOTE)?);
        Ok(self.explained(config, outcome).await)
    }

    async fn explained(&self, config: &ProviderConfig, outcome: WebhookOutcome) -> WebhookOutcome {
        if outcome.status != ProviderStatus::Held {
            return outcome;
        }
        let links = match outcome.provider_txid.as_deref() {
            Some(uuid) => self
                .call(
                    config,
                    "/v1/payment/aml-links",
                    &Json::object([("uuid", Json::from(uuid))]),
                )
                .await
                .map_err(|err| {
                    tracing::warn!(invoice = uuid, error = %err, "ссылка на анкету AML от Heleket не получена");
                })
                .ok(),
            None => None,
        };
        outcome.with_notice(NoticeKind::AmlCheck, aml_note(links.as_ref()))
    }

    async fn connection(&self, config: &ProviderConfig) -> ConnectionCheck {
        let services = match merchant::services(
            &self.http,
            format!("{}/v1/payment/services", self.base(config)),
            config,
            BRAND,
        )
        .await
        {
            Ok(services) => services,
            Err(check) => return check,
        };
        let closed: Vec<&str> = COINS
            .iter()
            .filter(|coin| {
                services.iter().any(|service| {
                    merchant::serves(service, &[coin.currency], &[coin.network])
                        && merchant::service_closed(service)
                })
            })
            .map(|coin| coin.method.title)
            .collect();
        let check = ConnectionCheck::ok("ID мерчанта и платёжный ключ приняты Heleket");
        if closed.is_empty() {
            check
        } else {
            check.detail(format!(
                "Сейчас Heleket не принимает: {}",
                closed.join(", ")
            ))
        }
    }
}

fn coin(code: &str) -> PaymentResult<Option<&'static Coin>> {
    if code == PAGE.code {
        return Ok(None);
    }
    COINS
        .iter()
        .find(|coin| coin.method.code == code)
        .map(Some)
        .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into()))
}

const REFUND_NOTE: &str = "Heleket сообщил о возврате денег по этому платежу, но сумму возврата \
                           в уведомлении не прислал. Сверьте возврат в кабинете Heleket и отметьте \
                           его в панели как сделанный вне панели";

fn surplus(info: &Json, outcome: WebhookOutcome) -> WebhookOutcome {
    let status = info
        .get("status")
        .or_else(|| info.get("payment_status"))
        .and_then(Json::as_str);
    let foreign = outcome
        .amount
        .is_some_and(|amount| amount.currency != PayCurrency::Usd);
    if status != Some("paid_over") || outcome.status != ProviderStatus::Paid || !foreign {
        return outcome;
    }
    let head = "Heleket сообщил о переплате по счёту в рублях: заказ оплачен";
    let tail = "Сумму излишка в рублях Heleket не присылает, поэтому на баланс клиента он сам \
                не зачислился: сверьте платёж в кабинете Heleket и зачислите излишек вручную \
                или верните его клиенту";
    let note = match merchant::received_usd(info).filter(|paid| paid.minor > 0) {
        Some(paid) => format!(
            "{head}, по данным Heleket всего получено {}. {tail}",
            util::amount_text(paid)
        ),
        None => format!("{head}. {tail}"),
    };
    outcome.with_notice(NoticeKind::ManualCredit, note)
}

fn aml_note(links: Option<&Json>) -> String {
    let head = "Heleket задержал оплату для проверки AML: деньги заблокированы, пока клиент \
                не заполнит анкету";
    let items: &[Json] = links.and_then(Json::as_array).unwrap_or_default();
    let open = items
        .iter()
        .filter(|item| {
            !matches!(
                item.get("status").and_then(Json::as_str),
                Some("completed" | "expired")
            )
        })
        .chain(items.iter())
        .find_map(|item| {
            let link = item
                .get("link")
                .and_then(Json::as_str)
                .filter(|l| util::is_https(l))?;
            Some((link, item.get("expired_at").and_then(Json::as_str)))
        });
    match open {
        Some((link, Some(until))) => format!(
            "{head}. Ссылка на анкету для клиента: {link} (действует до {})",
            util::clip(until, 19).replace('T', " ")
        ),
        Some((link, None)) => format!("{head}. Ссылка на анкету для клиента: {link}"),
        None => {
            format!("{head}. Ссылку на анкету получить не удалось: возьмите её в кабинете Heleket")
        }
    }
}

fn issued_as_asked(result: &Json, request: &InvoiceRequest) -> PaymentResult<()> {
    if let Some(order) = result.get("order_id").and_then(Json::as_str)
        && order != request.payment_public_id.to_string()
    {
        return Err(util::bad("Heleket вернул счёт другого заказа"));
    }
    let amount = result.get("amount").filter(|a| a.scalar_text().is_some());
    let currency = result.get("currency").filter(|c| c.as_str().is_some());
    if amount.is_some()
        && currency.is_some()
        && util::json_amount(amount, currency)? != request.amount
    {
        return Err(util::bad(
            "Heleket выставил счёт не на ту сумму или не в той валюте",
        ));
    }
    Ok(())
}

#[async_trait]
impl PaymentProvider for Heleket {
    fn api_url(&self, config: &ProviderConfig) -> Option<String> {
        Some(self.base(config))
    }

    fn own_api_url(&self, config: &ProviderConfig) -> Option<String> {
        ADDRESS.own(config)
    }

    fn id(&self) -> &'static str {
        providers::HELEKET
    }

    fn title(&self) -> &'static str {
        BRAND
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Usd, Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Криптовалюта",
            hint: "USDT и другие монеты",
        }
    }

    fn methods(&self, _currency: Currency) -> Vec<PayMethod> {
        std::iter::once(PAGE)
            .chain(COINS.iter().map(|coin| coin.method))
            .collect()
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                "merchant_id",
                "ID мерчанта",
                "Личный кабинет Heleket → Настройки",
            ),
            SettingField::secret(
                "api_key",
                "Платёжный API-ключ",
                "Личный кабинет Heleket → API",
            )
            .depends(API_URL, &[], true),
            ADDRESS.field(
                "Меняйте, только если Heleket объявит новый адрес. Нужно только имя сервера с https://, без пути. Пустое поле — стандартный адрес",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        ADDRESS.validate(config, BRAND)?;
        match config.get("merchant_id").map(Uuid::parse_str) {
            Some(Ok(_)) => Ok(()),
            _ => Err(PaymentError::NotConfigured(
                "ID мерчанта Heleket — это UUID из личного кабинета".into(),
            )),
        }
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        let currency = merchant::invoice_currency(request.amount.currency)?;
        let coin = match request.method.as_deref() {
            Some(code) => coin(code)?,
            None => None,
        };
        let mut body = Json::object([
            ("amount", Json::from(request.amount.units())),
            ("currency", Json::from(currency)),
            (
                "order_id",
                Json::from(request.payment_public_id.to_string()),
            ),
            ("url_callback", Json::from(request.webhook_url.clone())),
            ("is_payment_multiple", Json::from(true)),
            (
                "lifetime",
                Json::from(request.expires_in.num_seconds().clamp(300, 43_200)),
            ),
        ]);
        if let Some(url) = util::return_url(request) {
            body.push("url_return", Json::from(url.clone()));
            body.push("url_success", Json::from(url));
        }
        if let Some(coin) = coin {
            body.push(
                "currencies",
                Json::Array(vec![Json::object([
                    ("currency", Json::from(coin.currency)),
                    ("network", Json::from(coin.network)),
                ])]),
            );
        }
        let result = self.call(config, "/v1/payment", &body).await?;
        let uuid = result
            .get("uuid")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| util::bad("Heleket не вернул uuid счёта"))?;
        let pay_url = result
            .get("url")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Heleket не вернул ссылку на оплату"))?;
        issued_as_asked(&result, request)?;
        let expires_at = result
            .get("expired_at")
            .and_then(Json::scalar_text)
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|secs| DateTime::from_timestamp(secs, 0));
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(uuid.to_string()),
            expires_at,
            payload: serde_json::json!({ "uuid": uuid }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let key = config.require("api_key")?;
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        if !merchant::verify(key, &request.body) {
            return Err(PaymentError::BadSignature);
        }
        match body.get("type").and_then(Json::as_str) {
            Some("payment") | None => self.read(config, &body).await,
            Some(other) => Ok(merchant::skipped_type(&body, other)),
        }
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let uuid = util::safe_id(external_id)?;
        let result = self
            .call(
                config,
                "/v1/payment/info",
                &Json::object([("uuid", Json::from(uuid))]),
            )
            .await?;
        let result = merchant::requested(result, uuid, BRAND)?;
        self.read(config, &result).await.map(Some)
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        _context: &CheckContext,
    ) -> ConnectionCheck {
        let check = self.connection(config).await;
        match ADDRESS.own(config).filter(|_| self.api_base.is_none()) {
            Some(own) => check.detail(format!(
                "Запрос ушёл на свой адрес API {own}, а не на стандартный {}",
                ADDRESS.standard
            )),
            None => check,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::md5;
    use crate::test_support::*;
    use serde_json::json;
    use sn_domain::access::Permission;
    use sn_domain::money::Amount;
    use sn_domain::payment::{AmountCheck, check_paid_amount};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Match, Mock, MockServer, Request, Respond, ResponseTemplate};

    const KEY: &str = "heleket-payment-key";
    const MERCHANT: &str = "8b03432e-385b-4670-8d06-064591096795";

    fn settings() -> ProviderConfig {
        config(&[("merchant_id", json!(MERCHANT)), ("api_key", json!(KEY))])
    }

    fn data(status: &str, amount: &str, currency: &str) -> String {
        format!(
            r#"{{"type":"payment","uuid":"62f88b36-a9d5-4fa6-aa26-e040c3dbf26d","order_id":"{PAYMENT}","amount":"{amount}","payment_amount":"6.99000000","payment_amount_usd":"6.99","merchant_amount":"6.85","is_final":true,"status":"{status}","from":null,"network":"tron","currency":"{currency}","payer_currency":"USDT","additional_data":null,"url":"https:\/\/pay.heleket.com\/pay\/62f88b36","txid":"6f0d9c83"}}"#
        )
    }

    fn signed(data: &str, key: &str) -> String {
        let sign = signature(data.as_bytes(), key);
        format!("{},\"sign\":\"{sign}\"}}", &data[..data.len() - 1])
    }

    #[tokio::test]
    async fn проверка_подключения_heleket_списком_способов() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/services"))
            .and(header("merchant", MERCHANT))
            .and(header("sign", md5::hex(KEY.as_bytes()).as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": [
                    {"network": "TRON", "currency": "USDT", "is_available": true,
                     "limit": {"min_amount": "1.00000000", "max_amount": "100000.00000000"},
                     "commission": {"fee_amount": "0.00", "percent": "2.00"}},
                    {"network": "BTC", "currency": "BTC", "is_available": false},
                    {"network": "DOGE", "currency": "DOGE", "is_available": false}
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/services"))
            .and(header("merchant", "00000000-0000-4000-8000-000000000000"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"message": "Merchant unknown."})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/services"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(json!({"state": 1, "message": "Wrong key"})),
            )
            .with_priority(10)
            .mount(&server)
            .await;
        let provider = Heleket::with_api_base(server.uri());
        let context = CheckContext::default();
        let ok = provider.check_connection(&settings(), &context).await;
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
        assert_eq!(ok.details, ["Сейчас Heleket не принимает: Bitcoin"]);
        let unknown = provider
            .check_connection(
                &config(&[
                    ("merchant_id", json!("00000000-0000-4000-8000-000000000000")),
                    ("api_key", json!(KEY)),
                ]),
                &context,
            )
            .await;
        assert_eq!(unknown.status, CheckStatus::KeysRejected);
        assert!(unknown.summary.contains("мерчанта"));
        let wrong = provider
            .check_connection(
                &config(&[
                    ("merchant_id", json!(MERCHANT)),
                    ("api_key", json!("other-key")),
                ]),
                &context,
            )
            .await;
        assert_eq!(wrong.status, CheckStatus::KeysRejected);
        assert!(wrong.details[0].contains("«Wrong key»"));
        for request in server.received_requests().await.unwrap() {
            assert!(request.body.is_empty());
        }
        let down = failing_api(500).await;
        let unreachable = Heleket::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await;
        assert_eq!(unreachable.status, CheckStatus::Unreachable);
        let closed = Heleket::with_api_base(closed_api())
            .check_connection(&settings(), &context)
            .await;
        assert_eq!(closed.status, CheckStatus::Unreachable);
    }

    async fn parse(body: String) -> PaymentResult<WebhookOutcome> {
        Heleket::new()
            .parse_webhook(&settings(), &hook(&[], body))
            .await
    }

    #[tokio::test]
    async fn подпись_проверяется_по_исходным_байтам() {
        let body = signed(&data("paid", "6.99", "USD"), KEY);
        let outcome = parse(body.clone()).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(usd(699)));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(
            outcome.provider_txid.as_deref(),
            Some("62f88b36-a9d5-4fa6-aa26-e040c3dbf26d")
        );
        let wrong = signed(&data("paid", "6.99", "USD"), "other-key");
        assert_eq!(parse(wrong).await, Err(PaymentError::BadSignature));
        let missing = data("paid", "6.99", "USD");
        assert_eq!(parse(missing).await, Err(PaymentError::BadSignature));
        let tampered = body.replace("\"6.99\",\"payment_amount\"", "\"0.01\",\"payment_amount\"");
        assert_ne!(tampered, body);
        assert_eq!(parse(tampered).await, Err(PaymentError::BadSignature));
        assert!(matches!(
            parse("not json".into()).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn подпись_сходится_и_для_тела_с_пробелами_и_экранированным_юникодом() {
        let canonical = r#"{"type":"payment","uuid":"u-1","order_id":"x","amount":"1.00","currency":"USD","status":"paid","additional_data":"Тест\/1"}"#;
        let sign = signature(canonical.as_bytes(), KEY);
        let pretty = format!(
            "{{ \"type\": \"payment\", \"uuid\": \"u-1\", \"order_id\": \"x\", \"amount\": \"1.00\", \"currency\": \"USD\", \"status\": \"paid\", \"additional_data\": \"\\u0422\\u0435\\u0441\\u0442/1\", \"sign\": \"{sign}\" }}"
        );
        let outcome = parse(pretty).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::ProviderTxid("u-1".into()))
        );
    }

    #[tokio::test]
    async fn статусы_heleket() {
        for (status, expected) in [
            ("paid", ProviderStatus::Paid),
            ("paid_over", ProviderStatus::Paid),
            (
                "wrong_amount",
                ProviderStatus::Underpaid { received: usd(699) },
            ),
            ("fail", ProviderStatus::Failed),
            ("system_fail", ProviderStatus::Failed),
            ("cancel", ProviderStatus::Canceled),
            ("refund_paid", ProviderStatus::Pending),
            ("process", ProviderStatus::Pending),
            ("check", ProviderStatus::Pending),
            ("confirm_check", ProviderStatus::Pending),
            ("wrong_amount_waiting", ProviderStatus::Pending),
            ("refund_process", ProviderStatus::Pending),
            ("refund_fail", ProviderStatus::Pending),
        ] {
            let outcome = parse(signed(&data(status, "6.99", "USD"), KEY))
                .await
                .unwrap();
            assert_eq!(outcome.status, expected, "{status}");
            assert_eq!(
                outcome.event_key,
                format!("62f88b36-a9d5-4fa6-aa26-e040c3dbf26d:{status}")
            );
        }
        let not_final = signed(
            &data("paid", "6.99", "USD").replace("\"is_final\":true", "\"is_final\":false"),
            KEY,
        );
        assert_eq!(parse(not_final).await.unwrap().status, ProviderStatus::Paid);
        let over_not_final = signed(
            &data("paid_over", "7.10", "USD").replace("\"is_final\":true", "\"is_final\":false"),
            KEY,
        );
        let over = parse(over_not_final).await.unwrap();
        assert_eq!(over.status, ProviderStatus::Paid);
        assert_eq!(over.amount, Some(usd(710)));
        let wallet = signed(
            &data("paid", "6.99", "USD").replace("\"type\":\"payment\"", "\"type\":\"wallet\""),
            KEY,
        );
        let skipped = parse(wallet).await.unwrap();
        assert_eq!(skipped.payment, None);
        assert_eq!(skipped.status, ProviderStatus::Pending);
    }

    fn with_usd(status: &str, amount: &str, paid_usd: Option<&str>) -> String {
        let body = data(status, amount, "USD");
        let body = match paid_usd {
            Some(paid) => body.replace(
                "\"payment_amount_usd\":\"6.99\"",
                &format!("\"payment_amount_usd\":\"{paid}\""),
            ),
            None => body.replace("\"payment_amount_usd\":\"6.99\",", ""),
        };
        signed(&body, KEY)
    }

    #[tokio::test]
    async fn итоговая_недоплата_несёт_полученное_в_долларах_вниз_до_центов() {
        let under = parse(with_usd("wrong_amount", "6.99", Some("5.4999")))
            .await
            .unwrap();
        assert_eq!(
            under.status,
            ProviderStatus::Underpaid { received: usd(549) }
        );
        assert_eq!(under.amount, Some(usd(549)));
        assert_eq!(under.payment, Some(PaymentRef::PublicId(PAYMENT)));
        let whole = parse(with_usd("wrong_amount", "6.99", Some("3")))
            .await
            .unwrap();
        assert_eq!(
            whole.status,
            ProviderStatus::Underpaid { received: usd(300) }
        );
        for paid in [None, Some("0.004"), Some("-1"), Some("много")] {
            let failed = parse(with_usd("wrong_amount", "6.99", paid)).await.unwrap();
            assert_eq!(failed.status, ProviderStatus::Failed, "{paid:?}");
            assert_eq!(failed.amount, None);
        }
        let waiting = parse(with_usd("wrong_amount_waiting", "6.99", Some("5.00")))
            .await
            .unwrap();
        assert_eq!(waiting.status, ProviderStatus::Pending);
    }

    #[tokio::test]
    async fn излишек_paid_over_считается_по_полученному_в_долларах() {
        let over = parse(with_usd("paid_over", "6.99", Some("7.519")))
            .await
            .unwrap();
        assert_eq!(over.status, ProviderStatus::Overpaid { received: usd(751) });
        assert_eq!(over.amount, Some(usd(699)));
        for paid in [None, Some("6.99"), Some("6.5")] {
            let even = parse(with_usd("paid_over", "6.99", paid)).await.unwrap();
            assert_eq!(even.status, ProviderStatus::Paid, "{paid:?}");
            assert_eq!(even.amount, Some(usd(699)));
        }
    }

    #[tokio::test]
    async fn возврат_неизвестной_суммы_уходит_админу_а_не_в_учёт() {
        let refund = parse(with_usd("refund_paid", "6.99", Some("6.99")))
            .await
            .unwrap();
        assert_eq!(refund.status, ProviderStatus::Pending);
        assert_eq!(refund.amount, None);
        assert_eq!(refund.payment, Some(PaymentRef::PublicId(PAYMENT)));
        let notice = refund.notice().unwrap();
        assert_eq!(notice.kind, NoticeKind::RefundReported);
        assert!(notice.text.contains("вне панели"), "{}", notice.text);
        let paid = parse(with_usd("paid", "6.99", None)).await.unwrap();
        assert_eq!(paid.signal, None);
    }

    #[tokio::test]
    async fn блокировка_aml_держит_платёж_и_приносит_ссылку_на_анкету() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/aml-links"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": [
                    {"link": "https://aml.heleket.hk/old", "expired_at": "2026-09-15T12:00:00.000000Z", "status": "expired"},
                    {"link": "https://aml.heleket.hk/form/7f3c", "expired_at": "2026-11-25T12:00:00.000000Z", "status": "init"}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider = Heleket::with_api_base(server.uri());
        let locked = provider
            .parse_webhook(
                &settings(),
                &hook(&[], with_usd("locked", "6.99", Some("6.99"))),
            )
            .await
            .unwrap();
        assert_eq!(locked.status, ProviderStatus::Held);
        assert_eq!(locked.payment, Some(PaymentRef::PublicId(PAYMENT)));
        let notice = locked.notice().unwrap();
        assert_eq!(notice.kind, NoticeKind::AmlCheck);
        assert!(
            notice.text.contains("https://aml.heleket.hk/form/7f3c")
                && notice.text.contains("2026-11-25 12:00:00")
                && !notice.text.contains("/old"),
            "{}",
            notice.text
        );
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(
            body,
            json!({"uuid": "62f88b36-a9d5-4fa6-aa26-e040c3dbf26d"})
        );
    }

    #[tokio::test]
    async fn без_ссылки_на_анкету_блокировка_всё_равно_держит_платёж() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/aml-links"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let locked = Heleket::with_api_base(server.uri())
            .parse_webhook(
                &settings(),
                &hook(&[], with_usd("locked", "6.99", Some("6.99"))),
            )
            .await
            .unwrap();
        assert_eq!(locked.status, ProviderStatus::Held);
        let notice = locked.notice().unwrap();
        assert_eq!(notice.kind, NoticeKind::AmlCheck);
        assert!(notice.text.contains("кабинете Heleket"), "{}", notice.text);
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_не_проходят_сверку() {
        let expected = usd(699);
        let under = parse(signed(&data("paid", "6.98", "USD"), KEY))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, under.amount),
            AmountCheck::Underpaid { .. }
        ));
        let rub = parse(signed(&data("paid", "6.99", "RUB"), KEY))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, rub.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        assert!(matches!(
            parse(signed(&data("paid", "6.99", "USDT"), KEY)).await,
            Err(PaymentError::BadPayload(_))
        ));
        let eight = parse(signed(&data("paid", "6.99000000", "USD"), KEY))
            .await
            .unwrap();
        assert_eq!(eight.amount, Some(usd(699)));
        assert!(matches!(
            parse(signed(&data("paid", "6.999", "USD"), KEY)).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    async fn last_body(server: &MockServer) -> serde_json::Value {
        let sent = server.received_requests().await.unwrap();
        serde_json::from_slice(&sent.last().unwrap().body).unwrap()
    }

    struct SignedBody;

    impl Match for SignedBody {
        fn matches(&self, request: &Request) -> bool {
            request
                .headers
                .get("sign")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|sign| sign == signature(&request.body, KEY))
        }
    }

    #[tokio::test]
    async fn счёт_создаётся_с_подписью_тела() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .and(header("merchant", MERCHANT))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "26109ba0-b05b-4ee0-93d1-fd62c822ce95",
                    "order_id": PAYMENT.to_string(),
                    "amount": "6.99",
                    "currency": "USD",
                    "url": "https://pay.heleket.com/pay/26109ba0",
                    "expired_at": 1_763_308_800,
                    "status": "check",
                    "is_final": false
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider = Heleket::with_api_base(server.uri());
        let created = provider
            .create_invoice(&settings(), &invoice(usd(699), None))
            .await
            .unwrap();
        assert_eq!(
            created.external_id.as_deref(),
            Some("26109ba0-b05b-4ee0-93d1-fd62c822ce95")
        );
        assert_eq!(created.pay_url, "https://pay.heleket.com/pay/26109ba0");
        assert_eq!(
            created.expires_at.map(|at| at.timestamp()),
            Some(1_763_308_800)
        );
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(body["amount"], "6.99");
        assert_eq!(body["currency"], "USD");
        assert_eq!(body["order_id"], PAYMENT.to_string());
        assert_eq!(body["lifetime"], 3600);
        assert_eq!(body["is_payment_multiple"], true);
        assert!(matches!(
            provider
                .create_invoice(
                    &settings(),
                    &invoice(Amount::new(350, PayCurrency::Xtr), None)
                )
                .await,
            Err(PaymentError::UnsupportedCurrency(_))
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[test]
    fn способы_оплаты_heleket() {
        let heleket = Heleket::new();
        let methods = heleket.methods(Currency::Usd);
        let codes: Vec<&str> = methods.iter().map(|m| m.code).collect();
        assert_eq!(
            codes,
            [
                "page",
                "usdt_trc20",
                "usdt_ton",
                "usdt_bep20",
                "btc",
                "ton",
                "ltc"
            ]
        );
        for method in &methods {
            assert!(
                method.code.len() <= 20
                    && method
                        .code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{}",
                method.code
            );
            assert!(!method.title.is_empty() && !method.hint.is_empty());
        }
        assert_eq!(
            heleket.method(Currency::Usd, "btc").map(|m| m.title),
            Some("Bitcoin")
        );
        assert_eq!(heleket.methods(Currency::Rub), methods);
        assert_eq!(
            heleket.method(Currency::Rub, "usdt_trc20").map(|m| m.title),
            Some("USDT TRC-20")
        );
        assert!(methods.iter().all(|m| m.min_minor.is_none()));
        assert_eq!(heleket.currencies(), &[Currency::Usd, Currency::Rub]);
    }

    #[tokio::test]
    async fn выбранная_монета_уходит_в_список_currencies() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .and(header("merchant", MERCHANT))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {"uuid": "5d1a7c2e", "url": "https://pay.heleket.hk/pay/5d1a7c2e"}
            })))
            .mount(&server)
            .await;
        let provider = Heleket::with_api_base(server.uri());
        for (code, currency, network) in [
            ("usdt_trc20", "USDT", "tron"),
            ("usdt_ton", "USDT", "ton"),
            ("usdt_bep20", "USDT", "bsc"),
            ("btc", "BTC", "btc"),
            ("ton", "GRAM", "ton"),
            ("ltc", "LTC", "ltc"),
        ] {
            let request = InvoiceRequest {
                method: Some(code.into()),
                ..invoice(usd(699), None)
            };
            provider
                .create_invoice(&settings(), &request)
                .await
                .unwrap();
            let body = last_body(&server).await;
            assert_eq!(
                body["currencies"],
                json!([{"currency": currency, "network": network}]),
                "{code}"
            );
            assert_eq!(body["currency"], "USD");
            assert_eq!(body["amount"], "6.99");
            assert!(body.get("to_currency").is_none());
        }
        for code in [None, Some("page")] {
            let request = InvoiceRequest {
                method: code.map(str::to_string),
                ..invoice(usd(699), None)
            };
            provider
                .create_invoice(&settings(), &request)
                .await
                .unwrap();
            assert!(last_body(&server).await.get("currencies").is_none());
        }
        let sent = server.received_requests().await.unwrap().len();
        let unknown = InvoiceRequest {
            method: Some("doge".into()),
            ..invoice(usd(699), None)
        };
        assert_eq!(
            provider.create_invoice(&settings(), &unknown).await,
            Err(PaymentError::Rejected(
                "этот способ оплаты недоступен".into()
            ))
        );
        assert_eq!(server.received_requests().await.unwrap().len(), sent);
    }

    #[tokio::test]
    async fn ошибка_heleket_не_становится_счётом() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "state": 1,
                "message": "The amount field must be at least 1"
            })))
            .mount(&server)
            .await;
        let result = Heleket::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(usd(50), None))
            .await;
        assert_eq!(
            result,
            Err(PaymentError::Rejected(
                "The amount field must be at least 1".into()
            ))
        );
    }

    #[tokio::test]
    async fn ошибки_полей_heleket_видны_оператору() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "state": 1,
                "errors": {
                    "amount": ["The amount field must be at least 1."],
                    "url_callback": ["The url callback format is invalid."]
                }
            })))
            .mount(&server)
            .await;
        let result = Heleket::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(usd(50), None))
            .await;
        assert_eq!(
            result,
            Err(PaymentError::Rejected(
                "The amount field must be at least 1; The url callback format is invalid".into()
            ))
        );
    }

    #[test]
    fn старые_настройки_без_адреса_идут_на_стандартный_адрес() {
        let heleket = Heleket::new();
        let old = settings();
        assert!(old.get("api_url").is_none());
        assert!(heleket.is_configured(&old));
        assert_eq!(heleket.base(&old), "https://api.heleket.hk");
        assert_eq!(
            heleket.api_url(&old).as_deref(),
            Some("https://api.heleket.hk")
        );
        assert_eq!(heleket.own_api_url(&old), None);
        let mut blank = settings();
        blank.set("api_url", json!(""));
        assert!(heleket.is_configured(&blank));
        assert_eq!(heleket.base(&blank), "https://api.heleket.hk");
    }

    #[test]
    fn адрес_api_меняется_только_на_https_без_пути() {
        let heleket = Heleket::new();
        let mut custom = settings();
        custom.set("api_url", json!("https://API.heleket.example/"));
        assert_eq!(heleket.base(&custom), "https://api.heleket.example");
        assert_eq!(
            heleket.api_url(&custom).as_deref(),
            Some("https://api.heleket.example")
        );
        assert_eq!(
            heleket.own_api_url(&custom).as_deref(),
            Some("https://api.heleket.example")
        );
        assert!(heleket.is_configured(&custom));
        for wrong in [
            "http://api.heleket.example",
            "https://api.heleket.example/v1",
            "https://user:secret@api.heleket.example",
            "https://api.heleket.example?merchant=1",
        ] {
            custom.set("api_url", json!(wrong));
            assert!(
                matches!(
                    heleket.validate_settings(&custom),
                    Err(PaymentError::NotConfigured(text)) if text.contains("без пути, логина и пароля")
                ),
                "{wrong}"
            );
            assert!(!heleket.is_configured(&custom), "{wrong}");
            assert_eq!(heleket.base(&custom), "https://api.heleket.hk", "{wrong}");
            assert_eq!(heleket.own_api_url(&custom), None, "{wrong}");
        }
        custom.set("api_url", json!("https://api.heleket.hk/"));
        assert_eq!(heleket.own_api_url(&custom), None);
        let schema = heleket.settings_schema();
        let url = schema.iter().find(|f| f.key == "api_url").unwrap();
        assert_eq!(url.need, Some(Permission::Money));
        assert!(url.origin && !url.required);
        assert_eq!(url.default, Some("https://api.heleket.hk"));
        let key = schema.iter().find(|f| f.key == "api_key").unwrap();
        assert!(key.depends.is_some_and(|d| d.on == "api_url" && d.reset));
        assert!(
            schema
                .iter()
                .filter(|f| f.key != "api_url")
                .all(|f| f.need.is_none() && !f.origin)
        );
    }

    struct Issued {
        amount: Option<&'static str>,
        currency: Option<&'static str>,
        order: Option<&'static str>,
    }

    const ECHO: Issued = Issued {
        amount: None,
        currency: None,
        order: None,
    };

    impl Respond for Issued {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "a3f1c2d4-1b2c-4d5e-8f90-1a2b3c4d5e6f",
                    "order_id": self.order.map_or_else(|| body["order_id"].clone(), |o| json!(o)),
                    "amount": self.amount.map_or_else(|| body["amount"].clone(), |a| json!(a)),
                    "currency": self.currency.map_or_else(|| body["currency"].clone(), |c| json!(c)),
                    "payer_currency": null,
                    "url": "https://pay.heleket.hk/pay/a3f1c2d4",
                    "expired_at": 1_763_308_800,
                    "payment_status": "check",
                    "status": "check",
                    "is_final": false
                }
            }))
        }
    }

    async fn issuing(answer: Issued) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .and(header("merchant", MERCHANT))
            .and(SignedBody)
            .respond_with(answer)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn счёт_в_рублях_выставляется_в_рублях() {
        let server = issuing(ECHO).await;
        let provider = Heleket::with_api_base(server.uri());
        let created = provider
            .create_invoice(&settings(), &invoice(rub(49_900), None))
            .await
            .unwrap();
        assert_eq!(
            created.external_id.as_deref(),
            Some("a3f1c2d4-1b2c-4d5e-8f90-1a2b3c4d5e6f")
        );
        assert_eq!(created.pay_url, "https://pay.heleket.hk/pay/a3f1c2d4");
        let body = last_body(&server).await;
        assert_eq!(body["amount"], "499.00");
        assert_eq!(body["currency"], "RUB");
        assert_eq!(body["order_id"], PAYMENT.to_string());
        assert!(body.get("to_currency").is_none());
        assert!(body.get("currencies").is_none());
        let request = InvoiceRequest {
            method: Some("usdt_trc20".into()),
            ..invoice(rub(150_050), None)
        };
        provider
            .create_invoice(&settings(), &request)
            .await
            .unwrap();
        let body = last_body(&server).await;
        assert_eq!(body["amount"], "1500.50");
        assert_eq!(body["currency"], "RUB");
        assert_eq!(
            body["currencies"],
            json!([{"currency": "USDT", "network": "tron"}])
        );
        provider
            .create_invoice(&settings(), &invoice(usd(699), None))
            .await
            .unwrap();
        let body = last_body(&server).await;
        assert_eq!(body["amount"], "6.99");
        assert_eq!(body["currency"], "USD");
    }

    #[tokio::test]
    async fn счёт_не_на_ту_сумму_валюту_или_заказ_не_принимается() {
        for answer in [
            Issued {
                amount: Some("6.99"),
                currency: Some("USD"),
                ..ECHO
            },
            Issued {
                currency: Some("USD"),
                ..ECHO
            },
            Issued {
                amount: Some("500.00"),
                ..ECHO
            },
            Issued {
                order: Some("9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d"),
                ..ECHO
            },
        ] {
            let server = issuing(answer).await;
            let result = Heleket::with_api_base(server.uri())
                .create_invoice(&settings(), &invoice(rub(49_900), None))
                .await;
            assert!(
                matches!(result, Err(PaymentError::BadPayload(_))),
                "{result:?}"
            );
        }
        let longer = issuing(Issued {
            amount: Some("499.00000000"),
            ..ECHO
        })
        .await;
        assert!(
            Heleket::with_api_base(longer.uri())
                .create_invoice(&settings(), &invoice(rub(49_900), None))
                .await
                .is_ok()
        );
        let blank = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "a3f1c2d4", "order_id": null, "amount": null, "currency": "RUB",
                    "url": "https://pay.heleket.hk/pay/a3f1c2d4"
                }
            })))
            .mount(&blank)
            .await;
        assert!(
            Heleket::with_api_base(blank.uri())
                .create_invoice(&settings(), &invoice(rub(49_900), None))
                .await
                .is_ok()
        );
    }

    fn rub_event(status: &str, paid_usd: Option<&str>) -> String {
        let body = data(status, "499.00", "RUB");
        let body = match paid_usd {
            Some(paid) => body.replace(
                "\"payment_amount_usd\":\"6.99\"",
                &format!("\"payment_amount_usd\":\"{paid}\""),
            ),
            None => body.replace("\"payment_amount_usd\":\"6.99\",", ""),
        };
        signed(&body, KEY)
    }

    #[tokio::test]
    async fn рублёвый_счёт_засчитывается_в_рублях() {
        let paid = parse(rub_event("paid", Some("6.31"))).await.unwrap();
        assert_eq!(paid.status, ProviderStatus::Paid);
        assert_eq!(paid.amount, Some(rub(49_900)));
        assert_eq!(paid.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(paid.signal, None);
        assert_eq!(check_paid_amount(rub(49_900), paid.amount), AmountCheck::Ok);
        assert!(matches!(
            check_paid_amount(rub(50_000), paid.amount),
            AmountCheck::Underpaid { .. }
        ));
        assert!(matches!(
            check_paid_amount(usd(49_900), paid.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        let eight = parse(signed(&data("paid", "499.00000000", "RUB"), KEY))
            .await
            .unwrap();
        assert_eq!(eight.amount, Some(rub(49_900)));
    }

    #[tokio::test]
    async fn излишек_по_рублёвому_счёту_ждёт_ручного_решения() {
        let over = parse(rub_event("paid_over", Some("7.519"))).await.unwrap();
        assert_eq!(over.status, ProviderStatus::Paid);
        assert_eq!(over.amount, Some(rub(49_900)));
        assert_eq!(
            over.event_key,
            "62f88b36-a9d5-4fa6-aa26-e040c3dbf26d:paid_over"
        );
        let notice = over.notice().unwrap();
        assert_eq!(notice.kind, NoticeKind::ManualCredit);
        assert!(
            notice.text.contains("$7.51") && notice.text.contains("вручную"),
            "{}",
            notice.text
        );
        let unknown = parse(rub_event("paid_over", None)).await.unwrap();
        assert_eq!(unknown.status, ProviderStatus::Paid);
        let notice = unknown.notice().unwrap();
        assert_eq!(notice.kind, NoticeKind::ManualCredit);
        assert!(!notice.text.contains('$'), "{}", notice.text);
        let usd_over = parse(with_usd("paid_over", "6.99", Some("7.519")))
            .await
            .unwrap();
        assert_eq!(
            usd_over.status,
            ProviderStatus::Overpaid { received: usd(751) }
        );
        assert_eq!(usd_over.signal, None);
        for status in ["paid", "check", "cancel"] {
            assert_eq!(
                parse(rub_event(status, Some("7.519")))
                    .await
                    .unwrap()
                    .signal,
                None,
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn недоплата_по_рублёвому_счёту_несёт_полученное_в_долларах() {
        let under = parse(rub_event("wrong_amount", Some("3.2049")))
            .await
            .unwrap();
        assert_eq!(
            under.status,
            ProviderStatus::Underpaid { received: usd(320) }
        );
        assert_eq!(under.amount, Some(usd(320)));
        assert_eq!(under.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(under.signal, None);
        assert!(matches!(
            check_paid_amount(rub(49_900), under.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        let waiting = parse(rub_event("wrong_amount_waiting", Some("3.20")))
            .await
            .unwrap();
        assert_eq!(waiting.status, ProviderStatus::Pending);
        let nothing = parse(rub_event("wrong_amount", None)).await.unwrap();
        assert_eq!(nothing.status, ProviderStatus::Failed);
        assert_eq!(nothing.amount, None);
    }

    #[tokio::test]
    async fn сверка_рублёвого_счёта_по_uuid() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/info"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "26109ba0", "order_id": PAYMENT.to_string(), "amount": "499.00",
                    "currency": "RUB", "payer_currency": "USDT", "payment_amount": "6.70000000",
                    "payment_amount_usd": "6.70", "payment_status": "paid_over", "is_final": true
                }
            })))
            .mount(&server)
            .await;
        let outcome = Heleket::with_api_base(server.uri())
            .fetch_status(&settings(), "26109ba0")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(49_900)));
        assert_eq!(
            check_paid_amount(rub(49_900), outcome.amount),
            AmountCheck::Ok
        );
        let notice = outcome.notice().unwrap();
        assert_eq!(notice.kind, NoticeKind::ManualCredit);
        assert!(notice.text.contains("$6.70"), "{}", notice.text);
        let dollars = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/info"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "26109ba0", "order_id": PAYMENT.to_string(), "amount": "499.00",
                    "currency": "USD", "payment_status": "paid", "is_final": true
                }
            })))
            .mount(&dollars)
            .await;
        let foreign = Heleket::with_api_base(dollars.uri())
            .fetch_status(&settings(), "26109ba0")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            check_paid_amount(rub(49_900), foreign.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
    }

    fn trusting() -> reqwest::ClientBuilder {
        http_builder()
            .add_root_certificate(reqwest::Certificate::from_pem(TEST_CA.as_bytes()).unwrap())
    }

    static TRUSTING: sn_egress::Profile = sn_egress::Profile {
        name: "payments",
        build: trusting,
    };

    fn relayed() -> Heleket {
        Heleket {
            http: sn_egress::Http::new(
                &TRUSTING,
                sn_egress::Route::Payments,
                Some(providers::HELEKET),
            ),
            api_base: None,
        }
    }

    #[tokio::test]
    async fn свой_адрес_api_получает_все_запросы_heleket() {
        let api = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .and(header("merchant", MERCHANT))
            .and(SignedBody)
            .respond_with(ECHO)
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/info"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "26109ba0", "order_id": PAYMENT.to_string(), "amount": "499.00",
                    "currency": "RUB", "payment_status": "paid", "is_final": true
                }
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/services"))
            .and(header("merchant", MERCHANT))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": [{"network": "TRON", "currency": "USDT", "is_available": true}]
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/aml-links"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": [{"link": "https://aml.heleket.hk/form/9c1d", "status": "init"}]
            })))
            .mount(&api)
            .await;
        let front = https_front(&api.uri()).await;
        let mut custom = settings();
        custom.set("api_url", json!(format!("{front}/")));
        let heleket = relayed();
        assert!(heleket.is_configured(&custom));
        assert_eq!(heleket.api_url(&custom).as_deref(), Some(front.as_str()));
        assert_eq!(
            heleket.own_api_url(&custom).as_deref(),
            Some(front.as_str())
        );
        let created = heleket
            .create_invoice(&custom, &invoice(rub(49_900), None))
            .await
            .unwrap();
        assert_eq!(created.pay_url, "https://pay.heleket.hk/pay/a3f1c2d4");
        let polled = heleket
            .fetch_status(&custom, "26109ba0")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(polled.amount, Some(rub(49_900)));
        let check = heleket
            .check_connection(&custom, &CheckContext::default())
            .await;
        assert_eq!(check.status, CheckStatus::Ok, "{}", check.summary);
        assert!(
            check
                .details
                .iter()
                .any(|line| line.contains(&front) && line.contains("https://api.heleket.hk")),
            "{:?}",
            check.details
        );
        let held = heleket
            .parse_webhook(
                &custom,
                &hook(&[], with_usd("locked", "6.99", Some("6.99"))),
            )
            .await
            .unwrap();
        assert!(
            held.notice()
                .unwrap()
                .text
                .contains("https://aml.heleket.hk/form/9c1d")
        );
        let paths: Vec<String> = api
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.url.path().to_string())
            .collect();
        assert_eq!(
            paths,
            [
                "/v1/payment",
                "/v1/payment/info",
                "/v1/payment/services",
                "/v1/payment/aml-links"
            ]
        );
        let standard = Heleket::with_api_base(api.uri())
            .check_connection(&custom, &CheckContext::default())
            .await;
        assert!(standard.details.is_empty(), "{:?}", standard.details);
    }

    #[tokio::test]
    async fn статус_счёта_запрашивается_по_uuid() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/info"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "26109ba0",
                    "order_id": PAYMENT.to_string(),
                    "amount": "6.99",
                    "currency": "USD",
                    "payment_status": "paid",
                    "is_final": true
                }
            })))
            .mount(&server)
            .await;
        let outcome = Heleket::with_api_base(server.uri())
            .fetch_status(&settings(), "26109ba0")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(usd(699)));
        let sent = server.received_requests().await.unwrap();
        assert_eq!(sent[0].body, br#"{"uuid":"26109ba0"}"#);
        let other = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/info"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "ffff0000", "order_id": PAYMENT.to_string(), "amount": "6.99",
                    "currency": "USD", "payment_status": "paid", "is_final": true
                }
            })))
            .mount(&other)
            .await;
        assert!(matches!(
            Heleket::with_api_base(other.uri())
                .fetch_status(&settings(), "26109ba0")
                .await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[test]
    fn настройки_heleket_проверяются() {
        let heleket = Heleket::new();
        assert!(heleket.is_configured(&settings()));
        assert!(!heleket.is_configured(&ProviderConfig::default()));
        let bad = config(&[("merchant_id", json!("shop-1")), ("api_key", json!(KEY))]);
        assert!(matches!(
            heleket.validate_settings(&bad),
            Err(PaymentError::NotConfigured(_))
        ));
    }
}
