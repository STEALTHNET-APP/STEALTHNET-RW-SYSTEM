use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::payment::{AmountCheck, check_paid_amount};
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "cryptomus-payment-key-7f3c";
const MERCHANT: &str = "8b03432e-385b-4670-8d06-064591096795";
const UUID: &str = "62f88b36-a9d5-4fa6-aa26-e040c3dbf26d";
const DOC_BODY: &str = r#"{"type":"payment","uuid":"62f88b36-a9d5-4fa6-aa26-e040c3dbf26d","order_id":"97a75bf8eda5cca41ba9d2e104840fcd","amount":"3.00000000","payment_amount":"3.00000000","payment_amount_usd":"0.23","merchant_amount":"2.94000000","commission":"0.06000000","is_final":true,"status":"paid","from":"THgEWubVc8tPKXLJ4VZ5zbiiAK7AgqSeGH","wallet_address_uuid":null,"network":"tron","currency":"TRX","payer_currency":"TRX","payer_amount":"0.00234567","payer_amount_exchange_rate":"42650.00000000","transfer_id":"uuid-123-456","additional_data":null,"convert":{"to_currency":"USDT","commission":null,"rate":"0.07700000","amount":"0.22638000"},"convert_from":{"from_currency":"USDT","from_amount":"10.00","from_network":"tron"},"aml_verification_url":null,"txid":"6f0d9c8374db57cac0d806251473de754f361c83a03cd805f74aa9da3193486b"}"#;
const DOC_SIGN: &str = "f40beee5faaec89f20027db3bc42b3c7";
const SLASH_BODY: &str =
    r#"{"amount":"20","currency":"USDT","network":"tron","txid":"someTxidWith\/Slash"}"#;
const SLASH_SIGN: &str = "c6414a5008b20ac61616eaf75f23f859";
const UNICODE_BODY: &str = r#"{"type":"payment","uuid":"u-1","order_id":"x","amount":"1.00","currency":"USD","status":"paid","additional_data":"Тест\/1"}"#;
const UNICODE_SIGN: &str = "fd3b0d39816e535b102bf2d18c1f855a";

fn settings() -> ProviderConfig {
    config(&[("merchant_id", json!(MERCHANT)), ("api_key", json!(KEY))])
}

fn with_sign(body: &str, sign: &str) -> String {
    format!("{},\"sign\":\"{sign}\"}}", &body[..body.len() - 1])
}

fn signed(body: &str, key: &str) -> String {
    with_sign(body, &signature(body.as_bytes(), key))
}

fn data(status: &str, amount: &str, currency: &str) -> String {
    format!(
        r#"{{"type":"payment","uuid":"{UUID}","order_id":"{PAYMENT}","amount":"{amount}","payment_amount":"6.99000000","payment_amount_usd":"6.99","merchant_amount":"6.85","commission":"0.14","is_final":true,"status":"{status}","from":"THgEWubVc8tPKXLJ4VZ5zbiiAK7AgqSeGH","wallet_address_uuid":null,"network":"tron","currency":"{currency}","payer_currency":"USDT","additional_data":null,"txid":"6f0d9c83"}}"#
    )
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

async fn parse(body: String) -> PaymentResult<WebhookOutcome> {
    Cryptomus::new()
        .parse_webhook(&settings(), &hook(&[], body))
        .await
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

async fn last_body(server: &MockServer) -> serde_json::Value {
    let sent = server.received_requests().await.unwrap();
    serde_json::from_slice(&sent.last().unwrap().body).unwrap()
}

fn created(order: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "state": 0,
        "result": {
            "uuid": "26109ba0-b05b-4ee0-93d1-fd62c822ce95",
            "order_id": order,
            "amount": "6.99",
            "payment_amount": null,
            "payer_amount": null,
            "currency": "USD",
            "url": "https://pay.cryptomus.com/pay/26109ba0-b05b-4ee0-93d1-fd62c822ce95",
            "expired_at": 1_763_308_800,
            "payment_status": "check",
            "status": "check",
            "is_final": false
        }
    }))
}

#[test]
fn подпись_запроса_совпадает_с_эталоном() {
    assert_eq!(
        signature(br#"{"amount":"15","currency":"USD","order_id":"1"}"#, KEY),
        "4eda38134c01885998d9108622fef563"
    );
    assert_eq!(signature(b"", KEY), "caccc754b2fd0587a6bf1ac6938da285");
    assert_eq!(signature(b"", KEY), crate::md5::hex(KEY.as_bytes()));
}

#[test]
fn уведомление_из_документации_сходится_с_эталонной_подписью() {
    let body = with_sign(DOC_BODY, DOC_SIGN);
    assert!(merchant::verify(KEY, body.as_bytes()));
    assert!(!merchant::verify("other-key", body.as_bytes()));
    let upper = with_sign(DOC_BODY, &DOC_SIGN.to_ascii_uppercase());
    assert!(merchant::verify(KEY, upper.as_bytes()));
    let tampered = body.replace("\"amount\":\"3.00000000\"", "\"amount\":\"30.00000000\"");
    assert_ne!(tampered, body);
    assert!(!merchant::verify(KEY, tampered.as_bytes()));
    let short = with_sign(DOC_BODY, &DOC_SIGN[..31]);
    assert!(!merchant::verify(KEY, short.as_bytes()));
    assert!(!merchant::verify(KEY, DOC_BODY.as_bytes()));
}

#[tokio::test]
async fn подлинное_уведомление_в_чужой_валюте_не_засчитывается() {
    let body = with_sign(DOC_BODY, DOC_SIGN);
    assert!(matches!(
        parse(body).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[test]
fn косая_черта_и_кириллица_сверяются_как_у_php() {
    let escaped = with_sign(SLASH_BODY, SLASH_SIGN);
    assert!(merchant::verify(KEY, escaped.as_bytes()));
    let plain = escaped.replace("someTxidWith\\/Slash", "someTxidWith/Slash");
    assert_ne!(plain, escaped);
    assert!(merchant::verify(KEY, plain.as_bytes()));
    let raw = with_sign(UNICODE_BODY, UNICODE_SIGN);
    assert!(merchant::verify(KEY, raw.as_bytes()));
    let pretty = format!(
        "{{ \"type\": \"payment\", \"uuid\": \"u-1\", \"order_id\": \"x\", \"amount\": \"1.00\", \"currency\": \"USD\", \"status\": \"paid\", \"additional_data\": \"\\u0422\\u0435\\u0441\\u0442/1\", \"sign\": \"{UNICODE_SIGN}\" }}"
    );
    assert!(merchant::verify(KEY, pretty.as_bytes()));
    let other = pretty.replace("\\u0422", "\\u0423");
    assert!(!merchant::verify(KEY, other.as_bytes()));
}

#[tokio::test]
async fn уведомление_засчитывается_только_с_верной_подписью() {
    let body = signed(&data("paid", "6.99", "USD"), KEY);
    let outcome = parse(body.clone()).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(usd(699)));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(UUID));
    assert_eq!(outcome.event_key, format!("{UUID}:paid"));
    assert_eq!(outcome.reply, WebhookReply::ok());
    assert_eq!(
        parse(signed(&data("paid", "6.99", "USD"), "other-key")).await,
        Err(PaymentError::BadSignature)
    );
    assert_eq!(
        parse(data("paid", "6.99", "USD")).await,
        Err(PaymentError::BadSignature)
    );
    let other_order = body.replace(&PAYMENT.to_string(), "00000000-0000-4000-8000-000000000000");
    assert_eq!(parse(other_order).await, Err(PaymentError::BadSignature));
    assert!(matches!(
        parse("not json".into()).await,
        Err(PaymentError::BadPayload(_))
    ));
    let doubled = format!("{},\"status\":\"paid\"}}", &body[..body.len() - 1]);
    assert!(matches!(
        parse(doubled).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn повтор_того_же_уведомления_даёт_тот_же_ключ_события() {
    let body = signed(&data("paid", "6.99", "USD"), KEY);
    let first = parse(body.clone()).await.unwrap();
    let again = parse(body).await.unwrap();
    assert_eq!(first, again);
    let later = parse(signed(&data("cancel", "6.99", "USD"), KEY))
        .await
        .unwrap();
    assert_ne!(later.event_key, first.event_key);
}

#[tokio::test]
async fn статусы_cryptomus() {
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
        ("locked", ProviderStatus::Held),
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
        assert_eq!(outcome.event_key, format!("{UUID}:{status}"));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    }
    let wallet = signed(
        &data("paid", "6.99", "USD").replace("\"type\":\"payment\"", "\"type\":\"wallet\""),
        KEY,
    );
    let skipped = parse(wallet).await.unwrap();
    assert_eq!(skipped.payment, None);
    assert_eq!(skipped.status, ProviderStatus::Pending);
    assert_eq!(skipped.event_key, format!("wallet:{UUID}"));
    let foreign = signed(
        &data("paid", "6.99", "USD").replace(&PAYMENT.to_string(), "order-17"),
        KEY,
    );
    assert_eq!(
        parse(foreign).await.unwrap().payment,
        Some(PaymentRef::ProviderTxid(UUID.into()))
    );
}

#[tokio::test]
async fn недоплата_и_переплата_считаются_по_полученному_в_долларах() {
    let under = parse(with_usd("wrong_amount", "6.99", Some("5.4999")))
        .await
        .unwrap();
    assert_eq!(
        under.status,
        ProviderStatus::Underpaid { received: usd(549) }
    );
    assert_eq!(under.amount, Some(usd(549)));
    for paid in [None, Some("0.004"), Some("-1"), Some("много")] {
        let failed = parse(with_usd("wrong_amount", "6.99", paid)).await.unwrap();
        assert_eq!(failed.status, ProviderStatus::Failed, "{paid:?}");
        assert_eq!(failed.amount, None);
    }
    let over = parse(with_usd("paid_over", "6.99", Some("7.519")))
        .await
        .unwrap();
    assert_eq!(over.status, ProviderStatus::Overpaid { received: usd(751) });
    assert_eq!(over.amount, Some(usd(699)));
    for paid in [None, Some("6.99"), Some("6.5")] {
        let even = parse(with_usd("paid_over", "6.99", paid)).await.unwrap();
        assert_eq!(even.status, ProviderStatus::Paid, "{paid:?}");
    }
}

#[tokio::test]
async fn рублёвый_счёт_с_переплатой_засчитывается_целиком() {
    let over = parse(signed(&data("paid_over", "499.00", "RUB"), KEY))
        .await
        .unwrap();
    assert_eq!(over.status, ProviderStatus::Paid);
    assert_eq!(over.amount, Some(rub(49_900)));
    let under = parse(signed(&data("wrong_amount", "499.00", "RUB"), KEY))
        .await
        .unwrap();
    assert_eq!(
        under.status,
        ProviderStatus::Underpaid { received: usd(699) }
    );
    assert!(matches!(
        check_paid_amount(rub(49_900), under.amount),
        AmountCheck::CurrencyMismatch { .. }
    ));
}

#[tokio::test]
async fn сумма_и_валюта_сверяются_со_счётом() {
    let expected = usd(699);
    let less = parse(signed(&data("paid", "6.98", "USD"), KEY))
        .await
        .unwrap();
    assert!(matches!(
        check_paid_amount(expected, less.amount),
        AmountCheck::Underpaid { .. }
    ));
    let rub_paid = parse(signed(&data("paid", "6.99", "RUB"), KEY))
        .await
        .unwrap();
    assert!(matches!(
        check_paid_amount(expected, rub_paid.amount),
        AmountCheck::CurrencyMismatch { .. }
    ));
    let exact = parse(signed(&data("paid", "6.99000000", "USD"), KEY))
        .await
        .unwrap();
    assert_eq!(check_paid_amount(expected, exact.amount), AmountCheck::Ok);
    for (amount, currency) in [("6.99", "USDT"), ("6.999", "USD"), ("", "USD")] {
        assert!(
            matches!(
                parse(signed(&data("paid", amount, currency), KEY)).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{amount} {currency}"
        );
    }
}

#[tokio::test]
async fn блокировка_aml_держит_платёж_со_ссылкой_из_уведомления() {
    let locked = signed(
        &data("locked", "6.99", "USD").replace(
            "\"txid\":\"6f0d9c83\"",
            "\"aml_verification_url\":\"https:\\/\\/app.cryptomus.com\\/aml\\/7f3c\",\"txid\":\"6f0d9c83\"",
        ),
        KEY,
    );
    let outcome = parse(locked).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Held);
    let notice = outcome.notice().unwrap();
    assert_eq!(notice.kind, NoticeKind::AmlCheck);
    assert!(
        notice.text.contains("https://app.cryptomus.com/aml/7f3c"),
        "{}",
        notice.text
    );
    let plain = signed(
        &data("locked", "6.99", "USD").replace(
            "\"txid\":\"6f0d9c83\"",
            "\"aml_verification_url\":\"http:\\/\\/evil.example\\/x\",\"txid\":\"6f0d9c83\"",
        ),
        KEY,
    );
    let outcome = parse(plain).await.unwrap();
    let notice = outcome.notice().unwrap();
    assert!(!notice.text.contains("evil.example"), "{}", notice.text);
    assert!(
        notice.text.contains("кабинете Cryptomus"),
        "{}",
        notice.text
    );
    let paid = parse(signed(&data("paid", "6.99", "USD"), KEY))
        .await
        .unwrap();
    assert_eq!(paid.signal, None);
}

#[tokio::test]
async fn возврат_из_кабинета_приходит_админу_оповещением() {
    let refund = parse(signed(&data("refund_paid", "6.99", "USD"), KEY))
        .await
        .unwrap();
    assert_eq!(refund.status, ProviderStatus::Pending);
    assert_eq!(refund.amount, None);
    assert_eq!(refund.payment, Some(PaymentRef::PublicId(PAYMENT)));
    let notice = refund.notice().unwrap();
    assert_eq!(notice.kind, NoticeKind::RefundReported);
    assert!(notice.text.contains("Cryptomus"), "{}", notice.text);
    assert!(notice.text.contains("вне панели"), "{}", notice.text);
    let provider = Cryptomus::new();
    assert!(!provider.supports_refunds());
    assert_eq!(
        provider
            .refund(
                &settings(),
                &RefundRequest {
                    payment_id: sn_domain::ids::PaymentId(41),
                    provider_txid: UUID.into(),
                    amount: usd(699),
                    reason: None,
                    key: Uuid::from_u128(1),
                    email: None,
                    description: None,
                },
            )
            .await,
        Err(PaymentError::Unsupported("возврат через платёжную систему"))
    );
}

#[tokio::test]
async fn счёт_создаётся_с_подписью_тела() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .and(header("merchant", MERCHANT))
        .and(header("content-type", "application/json"))
        .and(SignedBody)
        .respond_with(created(&PAYMENT.to_string()))
        .expect(2)
        .mount(&server)
        .await;
    let provider = Cryptomus::with_api_base(server.uri());
    let invoice_usd = provider
        .create_invoice(&settings(), &invoice(usd(699), None))
        .await
        .unwrap();
    assert_eq!(
        invoice_usd.external_id.as_deref(),
        Some("26109ba0-b05b-4ee0-93d1-fd62c822ce95")
    );
    assert_eq!(
        invoice_usd.pay_url,
        "https://pay.cryptomus.com/pay/26109ba0-b05b-4ee0-93d1-fd62c822ce95"
    );
    assert_eq!(
        invoice_usd.expires_at.map(|at| at.timestamp()),
        Some(1_763_308_800)
    );
    let body = last_body(&server).await;
    assert_eq!(
        body,
        json!({
            "amount": "6.99",
            "currency": "USD",
            "order_id": PAYMENT.to_string(),
            "url_callback": "https://panel.example.com/api/webhooks/pay/test",
            "is_payment_multiple": true,
            "lifetime": 3600,
            "url_return": "https://panel.example.com/cabinet/payment-wait",
            "url_success": "https://panel.example.com/cabinet/payment-wait"
        })
    );
    provider
        .create_invoice(&settings(), &invoice(rub(49_900), None))
        .await
        .unwrap();
    let body = last_body(&server).await;
    assert_eq!(body["amount"], "499.00");
    assert_eq!(body["currency"], "RUB");
    assert!(matches!(
        provider
            .create_invoice(
                &settings(),
                &invoice(Amount::new(350, PayCurrency::Xtr), None)
            )
            .await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
}

#[tokio::test]
async fn срок_счёта_в_пределах_cryptomus() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .respond_with(created(&PAYMENT.to_string()))
        .mount(&server)
        .await;
    let provider = Cryptomus::with_api_base(server.uri());
    for (minutes, lifetime) in [(1, 300), (60, 3600), (24 * 60, 43_200)] {
        let request = InvoiceRequest {
            expires_in: chrono::Duration::minutes(minutes),
            return_url: None,
            ..invoice(usd(699), None)
        };
        provider
            .create_invoice(&settings(), &request)
            .await
            .unwrap();
        let body = last_body(&server).await;
        assert_eq!(body["lifetime"], lifetime, "{minutes}");
        assert_eq!(body["url_return"], "https://panel.example.com");
    }
}

#[tokio::test]
async fn счёт_чужого_заказа_не_принимается() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .respond_with(created("order-1"))
        .mount(&server)
        .await;
    let result = Cryptomus::with_api_base(server.uri())
        .create_invoice(&settings(), &invoice(usd(699), None))
        .await;
    assert!(matches!(result, Err(PaymentError::BadPayload(_))));
}

#[tokio::test]
async fn выбранная_монета_уходит_в_список_currencies() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment/services"))
        .and(body_string(""))
        .and(header("sign", signature(b"", KEY).as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "state": 0,
            "result": [
                {"network": "TRON", "currency": "USDT", "is_available": true},
                {"network": "GRAM", "currency": "USDT", "is_available": true},
                {"network": "GRAM", "currency": "GRAM", "is_available": true}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .and(SignedBody)
        .respond_with(created(&PAYMENT.to_string()))
        .mount(&server)
        .await;
    let provider = Cryptomus::with_api_base(server.uri());
    for (code, currency, network) in [
        ("usdt_trc20", "USDT", "tron"),
        ("usdt_ton", "USDT", "GRAM"),
        ("usdt_bep20", "USDT", "bsc"),
        ("btc", "BTC", "btc"),
        ("ton", "GRAM", "GRAM"),
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
async fn коды_ton_берутся_из_списка_монет_мерчанта() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment/services"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "state": 0,
            "result": [
                {"network": "TON", "currency": "TON", "is_available": true},
                {"network": "TON", "currency": "USDT", "is_available": true}
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .respond_with(created(&PAYMENT.to_string()))
        .mount(&server)
        .await;
    let provider = Cryptomus::with_api_base(server.uri());
    for (code, currency, network) in [("ton", "TON", "TON"), ("usdt_ton", "USDT", "TON")] {
        let request = InvoiceRequest {
            method: Some(code.into()),
            ..invoice(rub(49_900), None)
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
        assert_eq!(body["currency"], "RUB");
    }
}

#[tokio::test]
async fn без_списка_монет_ton_уходит_с_кодами_из_справочника() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment/services"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .respond_with(created(&PAYMENT.to_string()))
        .mount(&server)
        .await;
    let provider = Cryptomus::with_api_base(server.uri());
    for _ in 0..2 {
        let request = InvoiceRequest {
            method: Some("ton".into()),
            ..invoice(usd(699), None)
        };
        provider
            .create_invoice(&settings(), &request)
            .await
            .unwrap();
        assert_eq!(
            last_body(&server).await["currencies"],
            json!([{"currency": "GRAM", "network": "GRAM"}])
        );
    }
}

#[tokio::test]
async fn ошибка_cryptomus_не_становится_счётом() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .and(header("merchant", MERCHANT))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "state": 1,
            "message": "Minimum amount 0.5 USDT"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "state": 1,
            "errors": {
                "amount": ["validation.required"],
                "url_callback": ["The url callback format is invalid."]
            }
        })))
        .with_priority(10)
        .mount(&server)
        .await;
    let provider = Cryptomus::with_api_base(server.uri());
    assert_eq!(
        provider
            .create_invoice(&settings(), &invoice(usd(10), None))
            .await,
        Err(PaymentError::Rejected("Minimum amount 0.5 USDT".into()))
    );
    let other = config(&[
        ("merchant_id", json!("00000000-0000-4000-8000-000000000000")),
        ("api_key", json!(KEY)),
    ]);
    assert_eq!(
        provider
            .create_invoice(&other, &invoice(usd(10), None))
            .await,
        Err(PaymentError::Rejected(
            "validation.required; The url callback format is invalid".into()
        ))
    );
    let accepted_without_result = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": 0})))
        .mount(&accepted_without_result)
        .await;
    assert!(
        Cryptomus::with_api_base(accepted_without_result.uri())
            .create_invoice(&settings(), &invoice(usd(699), None))
            .await
            .is_err()
    );
    let down = failing_api(502).await;
    assert_eq!(
        Cryptomus::with_api_base(down.uri())
            .create_invoice(&settings(), &invoice(usd(699), None))
            .await,
        Err(PaymentError::Http { status: 502 })
    );
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
                "status": "paid",
                "is_final": true
            }
        })))
        .mount(&server)
        .await;
    let provider = Cryptomus::with_api_base(server.uri());
    let outcome = provider
        .fetch_status(&settings(), "26109ba0")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(usd(699)));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent[0].body, br#"{"uuid":"26109ba0"}"#);
    assert!(matches!(
        provider.fetch_status(&settings(), "../x").await,
        Err(PaymentError::BadPayload(_))
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn опрос_не_принимает_ответ_о_другом_счёте() {
    for answered in [json!("ffff0000"), json!(null)] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/info"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": answered, "order_id": PAYMENT.to_string(), "amount": "6.99",
                    "currency": "USD", "payment_status": "paid", "status": "paid", "is_final": true
                }
            })))
            .mount(&server)
            .await;
        assert!(
            matches!(
                Cryptomus::with_api_base(server.uri())
                    .fetch_status(&settings(), "26109ba0")
                    .await,
                Err(PaymentError::BadPayload(_))
            ),
            "{answered}"
        );
    }
}

#[tokio::test]
async fn проверка_подключения_по_списку_монет() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment/services"))
        .and(header("merchant", MERCHANT))
        .and(header("sign", crate::md5::hex(KEY.as_bytes()).as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "state": 0,
            "result": [
                {"network": "TRON", "currency": "USDT", "is_available": true,
                 "limit": {"min_amount": "1.00000000", "max_amount": "100000.00000000"},
                 "commission": {"fee_amount": "0.00", "percent": "2.00"}},
                {"network": "BSC", "currency": "USDT", "is_available": true},
                {"network": "GRAM", "currency": "USDT", "is_available": true},
                {"network": "GRAM", "currency": "GRAM", "is_available": true},
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
            ResponseTemplate::new(401).set_body_json(json!({"state": 1, "message": "Wrong key"})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    let provider = Cryptomus::with_api_base(server.uri());
    let context = CheckContext::default();
    let ok = provider.check_connection(&settings(), &context).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    assert!(ok.summary.contains("Cryptomus"));
    assert_eq!(ok.details.len(), 2, "{:?}", ok.details);
    assert_eq!(ok.details[0], "Сейчас Cryptomus не принимает: Bitcoin");
    assert!(ok.details[1].contains("Litecoin"), "{}", ok.details[1]);
    assert!(!ok.details[1].contains("TON"), "{}", ok.details[1]);
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
    assert_eq!(unknown.summary, "Cryptomus не знает мерчанта с таким ID");
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
    assert!(wrong.details[0].contains("ключ выплат не подходит"));
    for request in server.received_requests().await.unwrap() {
        assert!(request.body.is_empty());
    }
    let empty = provider
        .check_connection(&ProviderConfig::default(), &context)
        .await;
    assert_eq!(empty.status, CheckStatus::SettingsRejected);
    let down = failing_api(500).await;
    let unreachable = Cryptomus::with_api_base(down.uri())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(unreachable.status, CheckStatus::Unreachable);
    let closed = Cryptomus::with_api_base(closed_api())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(closed.status, CheckStatus::Unreachable);
}

#[test]
fn способы_оплаты_cryptomus() {
    let cryptomus = Cryptomus::new();
    for currency in [Currency::Usd, Currency::Rub] {
        let codes: Vec<&str> = cryptomus.methods(currency).iter().map(|m| m.code).collect();
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
            ],
            "{currency}"
        );
    }
    for method in cryptomus.methods(Currency::Usd) {
        assert!(!method.title.is_empty() && !method.hint.is_empty());
        assert!(
            crate::english::text(method.hint).is_some(),
            "{}",
            method.hint
        );
    }
    assert_eq!(
        cryptomus.method(Currency::Rub, "btc").map(|m| m.title),
        Some("Bitcoin")
    );
    assert_eq!(cryptomus.currencies(), &[Currency::Usd, Currency::Rub]);
    let label = cryptomus.client_label(Currency::Usd, &settings());
    assert_eq!(label.title, "Криптовалюта");
}

#[test]
fn настройки_cryptomus_проверяются() {
    let cryptomus = Cryptomus::new();
    assert!(cryptomus.is_configured(&settings()));
    assert!(!cryptomus.is_configured(&ProviderConfig::default()));
    let bad = config(&[("merchant_id", json!("shop-1")), ("api_key", json!(KEY))]);
    assert!(matches!(
        cryptomus.validate_settings(&bad),
        Err(PaymentError::NotConfigured(_))
    ));
    assert_eq!(cryptomus.base(&settings()), "https://api.cryptomus.com");
    let mut custom = settings();
    custom.set("api_url", json!("https://api.cryptomus.example/"));
    assert_eq!(cryptomus.base(&custom), "https://api.cryptomus.example");
    assert!(cryptomus.is_configured(&custom));
    custom.set("api_url", json!("http://api.cryptomus.example"));
    assert!(!cryptomus.is_configured(&custom));
    assert_eq!(cryptomus.base(&custom), "https://api.cryptomus.com");
    assert!(cryptomus.accepts_http_webhooks());
    assert!(
        cryptomus
            .settings_schema()
            .iter()
            .any(|field| field.key == "api_key" && field.is_secret())
    );
}
