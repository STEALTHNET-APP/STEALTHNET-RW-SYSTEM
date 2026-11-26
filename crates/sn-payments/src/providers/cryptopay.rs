use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sn_domain::money::{Amount, Currency, PayCurrency, parse_units};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;

const MAINNET: &str = "https://pay.crypt.bot";
const TESTNET: &str = "https://testnet-pay.crypt.bot";
const SIGNATURE_HEADER: &str = "crypto-pay-api-signature";
const ASSETS: [&str; 8] = ["USDT", "TON", "BTC", "ETH", "LTC", "BNB", "TRX", "USDC"];
const TESTNET_ASSET: &str = "JET";

pub struct CryptoPay {
    http: sn_egress::Http,
    api_base: Option<String>,
}

impl Default for CryptoPay {
    fn default() -> Self {
        Self::new()
    }
}

impl CryptoPay {
    pub fn new() -> Self {
        CryptoPay {
            http: http_client(sn_domain::payment::providers::CRYPTOPAY),
            api_base: None,
        }
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        CryptoPay {
            http: http_client(sn_domain::payment::providers::CRYPTOPAY),
            api_base: Some(api_base.into()),
        }
    }

    fn base(&self, config: &ProviderConfig) -> String {
        match &self.api_base {
            Some(base) => base.trim_end_matches('/').to_string(),
            None if config.flag("testnet") => TESTNET.to_string(),
            None => MAINNET.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    result: Option<T>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct CreatedInvoice {
    invoice_id: i64,
    #[serde(default)]
    bot_invoice_url: Option<String>,
    #[serde(default)]
    mini_app_invoice_url: Option<String>,
    #[serde(default)]
    web_app_invoice_url: Option<String>,
    #[serde(default)]
    expiration_date: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct Update {
    update_type: String,
    payload: UpdateInvoice,
}

#[derive(Deserialize)]
struct UpdateInvoice {
    invoice_id: i64,
    status: String,
    #[serde(default)]
    currency_type: Option<String>,
    #[serde(default)]
    fiat: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    payload: Option<String>,
}

pub fn signature(token: &str, body: &[u8]) -> String {
    let key = Sha256::digest(token.as_bytes());
    match Hmac::<Sha256>::new_from_slice(&key) {
        Ok(mut mac) => {
            mac.update(body);
            hex::encode(mac.finalize().into_bytes())
        }
        Err(_) => String::new(),
    }
}

fn verify(token: &str, body: &[u8], header: Option<&str>) -> bool {
    let Some(expected) = header.and_then(|h| hex::decode(h.trim()).ok()) else {
        return false;
    };
    let key = Sha256::digest(token.as_bytes());
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&key) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

fn accepted_assets(config: &ProviderConfig) -> PaymentResult<Option<String>> {
    let Some(raw) = config.get("accepted_assets") else {
        return Ok(None);
    };
    let testnet = config.flag("testnet");
    let mut assets: Vec<String> = Vec::new();
    for part in raw
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|part| !part.is_empty())
    {
        let code = part.to_ascii_uppercase();
        if code == TESTNET_ASSET && !testnet {
            return Err(PaymentError::NotConfigured(
                "монета JET есть только в тестовой сети Crypto Pay".into(),
            ));
        }
        if code != TESTNET_ASSET && !ASSETS.contains(&code.as_str()) {
            return Err(PaymentError::NotConfigured(format!(
                "Crypto Pay не принимает монету «{}», можно: {}",
                util::clip(part, 16),
                ASSETS.join(", ")
            )));
        }
        if !assets.contains(&code) {
            assets.push(code);
        }
    }
    Ok((!assets.is_empty()).then(|| assets.join(",")))
}

fn paid_button_url(request: &InvoiceRequest) -> Option<&str> {
    request
        .return_url
        .as_deref()
        .map(str::trim)
        .filter(|url| util::is_https(url))
}

#[async_trait]
impl PaymentProvider for CryptoPay {
    fn api_url(&self, config: &ProviderConfig) -> Option<String> {
        Some(self.base(config))
    }

    fn id(&self) -> &'static str {
        providers::CRYPTOPAY
    }

    fn title(&self) -> &'static str {
        "Crypto Pay"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Usd, Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Криптовалюта",
            hint: "USDT, TON, BTC",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret(
                "api_token",
                "API-токен",
                "Создаётся в @CryptoBot: Crypto Pay → Create App. Для тестовой сети — в @CryptoTestnetBot",
            ),
            SettingField::flag(
                "testnet",
                "Тестовая сеть",
                "Только для проверки, деньги не настоящие. Нужен токен из @CryptoTestnetBot",
            ),
            SettingField::text(
                "accepted_assets",
                "Какие монеты принимать",
                "Через запятую, например USDT,TON. Можно: USDT, TON, BTC, ETH, LTC, BNB, TRX, USDC. Пусто — все монеты",
            )
            .optional(),
        ]
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        accepted_assets(config).map(|_| ())
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
        let token = config.require("api_token")?;
        if !matches!(request.amount.currency, PayCurrency::Usd | PayCurrency::Rub) {
            return Err(PaymentError::UnsupportedCurrency(
                request.amount.currency.code().into(),
            ));
        }
        if request.method.is_some() {
            return Err(PaymentError::Rejected(
                "этот способ оплаты недоступен".into(),
            ));
        }
        let description: String = request.description.chars().take(1024).collect();
        let mut body = serde_json::json!({
            "currency_type": "fiat",
            "fiat": request.amount.currency.code(),
            "amount": request.amount.units(),
            "description": description,
            "payload": request.payment_public_id.to_string(),
            "expires_in": request.expires_in.num_seconds().clamp(60, 2_678_400),
            "allow_comments": false,
            "allow_anonymous": false,
        });
        if let Some(assets) = accepted_assets(config)? {
            body["accepted_assets"] = serde_json::Value::from(assets);
        }
        if let Some(url) = paid_button_url(request) {
            body["paid_btn_name"] = serde_json::Value::from("callback");
            body["paid_btn_url"] = serde_json::Value::from(url);
        }
        let response = self
            .http
            .post(format!("{}/api/createInvoice", self.base(config)))
            .header("Crypto-Pay-API-Token", token)
            .json(&body)
            .send()
            .await
            .map_err(PaymentError::transport)?;
        let status = response.status();
        let parsed: ApiResponse<CreatedInvoice> =
            response.json().await.map_err(|_| PaymentError::Http {
                status: status.as_u16(),
            })?;
        match (parsed.ok, parsed.result) {
            (true, Some(invoice)) => {
                let pay_url = invoice
                    .bot_invoice_url
                    .clone()
                    .or_else(|| invoice.web_app_invoice_url.clone())
                    .ok_or_else(|| {
                        PaymentError::BadPayload("в ответе нет ссылки на оплату".into())
                    })?;
                Ok(Invoice {
                    pay_url,
                    external_id: Some(invoice.invoice_id.to_string()),
                    expires_at: invoice.expiration_date,
                    payload: serde_json::json!({
                        "invoice_id": invoice.invoice_id,
                        "mini_app_invoice_url": invoice.mini_app_invoice_url,
                        "web_app_invoice_url": invoice.web_app_invoice_url,
                    }),
                })
            }
            _ => Err(PaymentError::Rejected(
                parsed
                    .error
                    .map(|e| e.name)
                    .unwrap_or_else(|| format!("HTTP {status}")),
            )),
        }
    }

    async fn cancel_invoice(
        &self,
        config: &ProviderConfig,
        invoice: &IssuedInvoice<'_>,
    ) -> PaymentResult<()> {
        let token = config.require("api_token")?;
        let invoice_id: i64 = invoice
            .external_id
            .and_then(|id| id.trim().parse().ok())
            .ok_or_else(|| PaymentError::BadPayload("нет номера счёта Crypto Pay".into()))?;
        let response = self
            .http
            .post(format!("{}/api/deleteInvoice", self.base(config)))
            .header("Crypto-Pay-API-Token", token)
            .json(&serde_json::json!({ "invoice_id": invoice_id }))
            .send()
            .await
            .map_err(PaymentError::transport)?;
        let status = response.status();
        let parsed: ApiResponse<bool> = response.json().await.map_err(|_| PaymentError::Http {
            status: status.as_u16(),
        })?;
        match (parsed.ok, parsed.result) {
            (true, Some(true)) => Ok(()),
            _ => Err(PaymentError::Rejected(
                parsed
                    .error
                    .map(|e| e.name)
                    .unwrap_or_else(|| format!("HTTP {status}")),
            )),
        }
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let token = config.require("api_token")?;
        if !verify(token, &request.body, request.header(SIGNATURE_HEADER)) {
            return Err(PaymentError::BadSignature);
        }
        let update: Update = serde_json::from_slice(&request.body)
            .map_err(|e| PaymentError::BadPayload(e.to_string()))?;
        let invoice = update.payload;
        let event_key = format!("{}:{}", update.update_type, invoice.invoice_id);
        if update.update_type != "invoice_paid" {
            return Ok(WebhookOutcome {
                event_key,
                payment: None,
                provider_txid: Some(invoice.invoice_id.to_string()),
                status: ProviderStatus::Pending,
                amount: None,
                saved_method: None,
                reply: WebhookReply::ok(),
                signal: None,
            });
        }
        let payment = util::public_ref(invoice.payload.as_deref())
            .or_else(|| Some(PaymentRef::ProviderTxid(invoice.invoice_id.to_string())));
        let amount = match (
            invoice.currency_type.as_deref(),
            invoice.fiat.as_deref(),
            invoice.amount.as_deref(),
        ) {
            (Some("fiat"), Some(fiat), Some(amount)) => {
                let currency = PayCurrency::parse(fiat)
                    .map_err(|e| PaymentError::BadPayload(e.to_string()))?;
                let minor = parse_units(amount, currency)
                    .ok_or_else(|| PaymentError::BadPayload(format!("сумма {amount}")))?;
                Some(Amount::new(minor, currency))
            }
            _ => None,
        };
        Ok(WebhookOutcome {
            event_key,
            payment,
            provider_txid: Some(invoice.invoice_id.to_string()),
            status: if invoice.status == "paid" {
                ProviderStatus::Paid
            } else {
                ProviderStatus::Pending
            },
            amount,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        })
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        _context: &CheckContext,
    ) -> ConnectionCheck {
        let testnet = config.flag("testnet");
        let token = match config.require("api_token") {
            Ok(token) => token,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let reply = match util::send(
            self.http
                .get(format!("{}/api/getMe", self.base(config)))
                .header("Crypto-Pay-API-Token", token),
        )
        .await
        {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let parsed = match reply.json() {
            Ok(parsed) => parsed,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        if parsed.get("ok").and_then(Json::as_bool) == Some(true) {
            let name = parsed
                .at(&["result", "name"])
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty());
            let check = ConnectionCheck::ok(match name {
                Some(name) => format!(
                    "Токен принят: приложение «{}» в Crypto Pay",
                    util::clip(name, 64)
                ),
                None => "Токен принят Crypto Pay".to_string(),
            });
            return if testnet {
                check.detail(
                    "Включена тестовая сеть: оплаты ненастоящие. Перед запуском выключите её и вставьте токен из @CryptoBot",
                )
            } else {
                check
            };
        }
        let error = parsed.at(&["error", "name"]).and_then(Json::as_str);
        if reply.status == 401 || error == Some("UNAUTHORIZED") {
            return ConnectionCheck::new(CheckStatus::KeysRejected, "Crypto Pay не принял токен")
                .detail(if testnet {
                    "Включена тестовая сеть: нужен токен приложения из @CryptoTestnetBot. Токен из @CryptoBot подходит только без тестовой сети"
                } else {
                    "Нужен токен приложения из @CryptoBot: Crypto Pay → My Apps → API Token. Токен из @CryptoTestnetBot подходит только с включённой тестовой сетью"
                });
        }
        ConnectionCheck::failed(&reply.failure(error))
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let token = config.require("api_token")?;
        let id = external_id.trim();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(util::bad("номер счёта Crypto Pay состоит из цифр"));
        }
        let reply = util::send(
            self.http
                .post(format!("{}/api/getInvoices", self.base(config)))
                .header("Crypto-Pay-API-Token", token)
                .json(&serde_json::json!({ "invoice_ids": id })),
        )
        .await?;
        let parsed = reply.json()?;
        if parsed.get("ok").and_then(Json::as_bool) != Some(true) {
            return Err(reply.failure(parsed.at(&["error", "name"]).and_then(Json::as_str)));
        }
        let items: &[Json] = match parsed.get("result") {
            Some(Json::Array(items)) => items,
            Some(result) => result
                .get("items")
                .and_then(Json::as_array)
                .unwrap_or_default(),
            None => &[],
        };
        let Some(invoice) = items
            .iter()
            .find(|i| i.get("invoice_id").and_then(Json::scalar_text) == Some(id))
        else {
            return Ok(None);
        };
        let status_text = invoice
            .get("status")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет статуса счёта"))?;
        let status = match status_text {
            "paid" => ProviderStatus::Paid,
            "expired" => ProviderStatus::Canceled,
            _ => ProviderStatus::Pending,
        };
        let amount = match invoice.get("currency_type").and_then(Json::as_str) {
            Some("fiat") => Some(util::json_amount(
                invoice.get("amount"),
                invoice.get("fiat"),
            )?),
            _ => None,
        };
        Ok(Some(WebhookOutcome {
            event_key: format!("invoice_{status_text}:{id}"),
            payment: util::public_ref(invoice.get("payload").and_then(Json::as_str))
                .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string()))),
            provider_txid: Some(id.to_string()),
            status,
            amount,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use sn_domain::ids::{ClientId, PaymentId};
    use uuid::Uuid;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config() -> ProviderConfig {
        let mut config = ProviderConfig::default();
        config.set("api_token", serde_json::json!("1234:AAA"));
        config
    }

    fn with_assets(assets: &str, testnet: bool) -> ProviderConfig {
        let mut config = config();
        config.set("accepted_assets", serde_json::json!(assets));
        config.set("testnet", serde_json::json!(testnet));
        config
    }

    fn usd_request() -> InvoiceRequest {
        InvoiceRequest {
            plan: None,
            payment_id: PaymentId(1),
            payment_public_id: Uuid::from_u128(1),
            order_public_id: Uuid::from_u128(2),
            amount: Amount::new(699, PayCurrency::Usd),
            description: "Стандарт · 90 дней".into(),
            client: ClientRef {
                client_id: ClientId(1),
                public_id: Uuid::from_u128(3),
                telegram_id: None,
                email: None,
            },
            return_url: None,
            webhook_url: "https://panel.example.com/api/webhooks/pay/cryptopay".into(),
            expires_in: chrono::Duration::hours(1),
            save_payment_method: false,
            method: None,
        }
    }

    async fn invoice_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/createInvoice"))
            .and(header("Crypto-Pay-API-Token", "1234:AAA"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {"invoice_id": 77, "bot_invoice_url": "https://t.me/CryptoBot?start=IV77"}
            })))
            .mount(&server)
            .await;
        server
    }

    async fn sent_bodies(server: &MockServer) -> Vec<serde_json::Value> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    fn hook(body: &str, signature: Option<String>) -> WebhookRequest {
        WebhookRequest {
            headers: signature
                .map(|s| vec![(SIGNATURE_HEADER.to_string(), s)])
                .unwrap_or_default(),
            body: Bytes::from(body.to_string()),
            remote_ip: None,
            route_suffix: None,
            query: None,
        }
    }

    fn paid_body(payment: Uuid) -> String {
        serde_json::json!({
            "update_id": 1, "update_type": "invoice_paid", "request_date": "2026-11-16T14:52:00.000Z",
            "payload": {
                "invoice_id": 528890, "status": "paid", "currency_type": "fiat", "fiat": "USD",
                "amount": "6.99", "payload": payment.to_string()
            }
        })
        .to_string()
    }

    #[tokio::test]
    async fn подпись_проверяется_по_сырому_телу() {
        let provider = CryptoPay::new();
        let payment = Uuid::from_u128(5);
        let body = paid_body(payment);
        let ok = provider
            .parse_webhook(
                &config(),
                &hook(&body, Some(signature("1234:AAA", body.as_bytes()))),
            )
            .await
            .unwrap();
        assert_eq!(ok.status, ProviderStatus::Paid);
        assert_eq!(ok.payment, Some(PaymentRef::PublicId(payment)));
        assert_eq!(ok.amount, Some(Amount::new(699, PayCurrency::Usd)));
        assert_eq!(ok.event_key, "invoice_paid:528890");
        let wrong = provider
            .parse_webhook(
                &config(),
                &hook(&body, Some(signature("other", body.as_bytes()))),
            )
            .await;
        assert_eq!(wrong, Err(PaymentError::BadSignature));
        let missing = provider.parse_webhook(&config(), &hook(&body, None)).await;
        assert_eq!(missing, Err(PaymentError::BadSignature));
    }

    #[tokio::test]
    async fn оплата_с_чужим_payload_ищется_по_номеру_счёта() {
        let provider = CryptoPay::new();
        for payload in [serde_json::json!("order-17"), serde_json::Value::Null] {
            let body = serde_json::json!({
                "update_id": 2, "update_type": "invoice_paid", "request_date": "2026-11-18T10:00:00.000Z",
                "payload": {
                    "invoice_id": 528891, "status": "paid", "currency_type": "fiat", "fiat": "RUB",
                    "amount": "499.00", "payload": payload
                }
            })
            .to_string();
            let outcome = provider
                .parse_webhook(
                    &config(),
                    &hook(&body, Some(signature("1234:AAA", body.as_bytes()))),
                )
                .await
                .unwrap();
            assert_eq!(outcome.status, ProviderStatus::Paid);
            assert_eq!(
                outcome.payment,
                Some(PaymentRef::ProviderTxid("528891".into()))
            );
            assert_eq!(outcome.amount, Some(Amount::new(49_900, PayCurrency::Rub)));
        }
    }

    #[tokio::test]
    async fn счёт_создаётся_в_долларах() {
        let server = invoice_server().await;
        let provider = CryptoPay::with_api_base(server.uri());
        let request = usd_request();
        let invoice = provider.create_invoice(&config(), &request).await.unwrap();
        assert_eq!(invoice.external_id.as_deref(), Some("77"));
        assert!(invoice.pay_url.starts_with("https://t.me/"));
        let body = &sent_bodies(&server).await[0];
        assert_eq!(body["currency_type"], "fiat");
        assert_eq!(body["fiat"], "USD");
        assert_eq!(body["amount"], "6.99");
        assert_eq!(body["payload"], Uuid::from_u128(1).to_string());
        for absent in ["accepted_assets", "paid_btn_name", "paid_btn_url"] {
            assert!(body.get(absent).is_none(), "{absent}");
        }
        let stars = InvoiceRequest {
            amount: Amount::new(350, PayCurrency::Xtr),
            ..request.clone()
        };
        assert!(matches!(
            provider.create_invoice(&config(), &stars).await,
            Err(PaymentError::UnsupportedCurrency(_))
        ));
        let chosen = InvoiceRequest {
            method: Some("usdt".into()),
            ..request
        };
        assert_eq!(
            provider.create_invoice(&config(), &chosen).await,
            Err(PaymentError::Rejected(
                "этот способ оплаты недоступен".into()
            ))
        );
        assert_eq!(sent_bodies(&server).await.len(), 1);
    }

    #[tokio::test]
    async fn счёт_в_рублях_с_выбранными_монетами_и_кнопкой_возврата() {
        let server = invoice_server().await;
        let provider = CryptoPay::with_api_base(server.uri());
        assert!(provider.currencies().contains(&Currency::Rub));
        assert!(provider.methods(Currency::Rub).is_empty());
        let request = InvoiceRequest {
            amount: Amount::new(49_900, PayCurrency::Rub),
            return_url: Some("https://panel.example.com/cabinet/payment-wait".into()),
            ..usd_request()
        };
        provider
            .create_invoice(&with_assets("usdt, ton,USDT", false), &request)
            .await
            .unwrap();
        let body = &sent_bodies(&server).await[0];
        assert_eq!(body["fiat"], "RUB");
        assert_eq!(body["amount"], "499.00");
        assert_eq!(body["accepted_assets"], "USDT,TON");
        assert_eq!(body["paid_btn_name"], "callback");
        assert_eq!(
            body["paid_btn_url"],
            "https://panel.example.com/cabinet/payment-wait"
        );
        let invalid = with_assets("USDT,DOGE", false);
        assert!(matches!(
            provider.create_invoice(&invalid, &request).await,
            Err(PaymentError::NotConfigured(_))
        ));
        assert_eq!(sent_bodies(&server).await.len(), 1);
    }

    #[test]
    fn монеты_сверяются_со_списком_crypto_pay() {
        let provider = CryptoPay::new();
        assert_eq!(accepted_assets(&config()), Ok(None));
        assert_eq!(accepted_assets(&with_assets(" , ", false)), Ok(None));
        assert_eq!(
            accepted_assets(&with_assets(" usdt; TON  btc,usdt ", false)),
            Ok(Some("USDT,TON,BTC".into()))
        );
        assert!(
            provider
                .validate_settings(&with_assets("USDT,TON", false))
                .is_ok()
        );
        assert!(matches!(
            provider.validate_settings(&with_assets("USDT,DOGE", false)),
            Err(PaymentError::NotConfigured(message)) if message.contains("DOGE")
        ));
        assert!(
            provider
                .validate_settings(&with_assets("jet", false))
                .is_err()
        );
        assert_eq!(
            accepted_assets(&with_assets("jet,usdt", true)),
            Ok(Some("JET,USDT".into()))
        );
        assert!(
            provider
                .validate_settings(&ProviderConfig::default())
                .is_err()
        );
        assert!(provider.is_configured(&config()));
    }

    #[tokio::test]
    async fn проверка_подключения_crypto_pay() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/getMe"))
            .and(header("Crypto-Pay-API-Token", "1234:AAA"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {"app_id": 1, "name": "VPN Shop", "payment_processing_bot_username": "CryptoBot"}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/getMe"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "ok": false, "error": {"code": 401, "name": "UNAUTHORIZED"}
            })))
            .mount(&server)
            .await;
        let provider = CryptoPay::with_api_base(server.uri());
        let context = CheckContext::default();
        let ok = provider.check_connection(&config(), &context).await;
        assert_eq!(ok.status, CheckStatus::Ok);
        assert!(ok.summary.contains("«VPN Shop»"), "{}", ok.summary);
        assert!(ok.details.is_empty());
        let testnet = provider
            .check_connection(&with_assets("USDT", true), &context)
            .await;
        assert_eq!(testnet.status, CheckStatus::Ok);
        assert!(testnet.details[0].contains("тестовая сеть"));
        let mut wrong = config();
        wrong.set("api_token", serde_json::json!("9999:BBB"));
        let rejected = provider.check_connection(&wrong, &context).await;
        assert_eq!(rejected.status, CheckStatus::KeysRejected);
        assert!(rejected.details[0].contains("@CryptoBot"));
        wrong.set("testnet", serde_json::json!(true));
        let rejected = provider.check_connection(&wrong, &context).await;
        assert!(rejected.details[0].contains("@CryptoTestnetBot"));
        for request in server.received_requests().await.unwrap() {
            assert!(request.body.is_empty());
        }
        let down = crate::test_support::failing_api(502).await;
        let unreachable = CryptoPay::with_api_base(down.uri())
            .check_connection(&config(), &context)
            .await;
        assert_eq!(unreachable.status, CheckStatus::Unreachable);
        let closed = CryptoPay::with_api_base(crate::test_support::closed_api())
            .check_connection(&config(), &context)
            .await;
        assert_eq!(closed.status, CheckStatus::Unreachable);
        assert!(!closed.summary.contains("1234:AAA"));
    }

    #[tokio::test]
    async fn статус_счёта_crypto_pay_по_номеру() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/getInvoices"))
            .and(header("Crypto-Pay-API-Token", "1234:AAA"))
            .and(body_json(serde_json::json!({"invoice_ids": "528890"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {"items": [{
                    "invoice_id": 528890, "status": "paid", "currency_type": "fiat",
                    "fiat": "USD", "amount": "6.99", "payload": Uuid::from_u128(5).to_string()
                }]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/getInvoices"))
            .and(body_json(serde_json::json!({"invoice_ids": "77"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": [{"invoice_id": 77, "status": "expired", "currency_type": "fiat", "fiat": "USD", "amount": "6.99"}]
            })))
            .mount(&server)
            .await;
        let provider = CryptoPay::with_api_base(server.uri());
        let paid = provider
            .fetch_status(&config(), "528890")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(paid.status, ProviderStatus::Paid);
        assert_eq!(paid.amount, Some(Amount::new(699, PayCurrency::Usd)));
        assert_eq!(paid.payment, Some(PaymentRef::PublicId(Uuid::from_u128(5))));
        let expired = provider
            .fetch_status(&config(), "77")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expired.status, ProviderStatus::Canceled);
        assert!(matches!(
            provider.fetch_status(&config(), "../x").await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn заменённый_счёт_удаляется_в_crypto_pay() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/deleteInvoice"))
            .and(header("Crypto-Pay-API-Token", "1234:AAA"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "ok": true, "result": true })),
            )
            .mount(&server)
            .await;
        let provider = CryptoPay::with_api_base(server.uri());
        let payload = serde_json::json!({});
        provider
            .cancel_invoice(
                &config(),
                &IssuedInvoice {
                    external_id: Some("77"),
                    payload: &payload,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            sent_bodies(&server).await,
            [serde_json::json!({ "invoice_id": 77 })]
        );
        assert!(
            provider
                .cancel_invoice(
                    &config(),
                    &IssuedInvoice {
                        external_id: None,
                        payload: &payload,
                    },
                )
                .await
                .is_err()
        );
    }
}
