use super::*;
use crate::test_support::*;
use chrono::Duration;
use serde_json::json;
use sn_domain::ids::PaymentId;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use wiremock::matchers::{
    any, body_json, body_partial_json, header, header_exists, method, path, query_param,
    query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "rpk_live_abc123def456";
const SECRET: &str = "rp_signing_secret_7Qm2xK";
const PAY: &str = "pay_a1b2c3d4-e5f6-7890-abcd-ef1234567890";
const VECTOR_TIME: &str = "1771848000";
const PAID_BODY: &str = r#"{"event_type":"payment.paid","payment_id":"pay_a1b2c3d4-e5f6-7890-abcd-ef1234567890","order_id":"order_12345","status":"paid","amount":"1500.00","currency":"RUB","test":false}"#;
const PAID_SIGNATURE: &str = "539d4d78e84ad54e423d7032f57fceec87933f3e909a6c68fb9eedc14023fcfc";
const REFUND_BODY: &str = "{\n  \"event_type\": \"payment.refunded\",\n  \"payment_id\": \"pay_a1b2c3d4-e5f6-7890-abcd-ef1234567890\",\n  \"order_id\": \"order_12345\",\n  \"status\": \"refunded\",\n  \"amount\": \"1500.00\",\n  \"currency\": \"RUB\",\n  \"test\": false,\n  \"metadata\": {\"note\": \"Возврат\"}\n}";
const REFUND_SIGNATURE: &str = "6048fbbaa86df2966ca20ba95d9dd12d8bc5996d08afd7831bb7e8c3d6a52884";
const RETURN: &str = "https://panel.example.com/cabinet/payment-wait";

fn settings() -> ProviderConfig {
    config(&[("api_key", json!(KEY)), ("signing_secret", json!(SECRET))])
}

fn testing() -> ProviderConfig {
    let mut settings = settings();
    settings.set("test_mode", json!(true));
    settings
}

fn now() -> String {
    Utc::now().timestamp().to_string()
}

fn event(kind: &str, status: &str, amount: &str, currency: &str, test: bool) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "event_type": kind, "payment_id": PAY, "order_id": PAYMENT.to_string(),
        "status": status, "amount": amount, "currency": currency, "test": test
    }))
    .unwrap()
}

fn signed(body: impl Into<Vec<u8>>, sent: &str) -> WebhookRequest {
    let body = body.into();
    let sign = signature(SECRET, sent, &body);
    hook(
        &[("X-Timestamp", sent), ("X-Signature", sign.as_str())],
        body,
    )
}

async fn parse(config: &ProviderConfig, request: &WebhookRequest) -> PaymentResult<WebhookOutcome> {
    RollyPay::new().parse_webhook(config, request).await
}

async fn paid(status: &str) -> WebhookOutcome {
    parse(
        &settings(),
        &signed(
            event(
                &format!("payment.{status}"),
                status,
                "1500.00",
                "RUB",
                false,
            ),
            &now(),
        ),
    )
    .await
    .unwrap()
}

fn nonces(requests: &[wiremock::Request]) -> Vec<Uuid> {
    requests
        .iter()
        .map(|r| {
            let raw = r.headers.get("x-nonce").unwrap().to_str().unwrap();
            Uuid::parse_str(raw).unwrap()
        })
        .collect()
}

fn created(order: &str, amount: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "payment_id": PAY, "order_id": order, "status": "created",
        "token": "tok_9f8e7d6c5b4a3210",
        "pay_url": "https://pay.rollypay.io/pay/tok_9f8e7d6c5b4a3210",
        "amount": amount, "payment_currency": "RUB", "qr_content": "",
        "h2h_enabled": false, "qr_activated": false,
        "created_at": "2026-11-18T12:00:00Z", "expires_at": "2026-11-18T12:30:00Z"
    }))
}

fn balance() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "terminal_id": "d290f1ee-6c54-4b01-90e6-d701748f0851",
        "available_usdt": "1250.75", "hold_usdt": "50.00"
    }))
}

fn problem(status: u16, text: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({ "error": text }))
}

async fn create(
    server: &MockServer,
    config: &ProviderConfig,
    method: Option<&str>,
) -> PaymentResult<Invoice> {
    let mut request = invoice(rub(150_000), Some("client@example.com"));
    request.method = method.map(str::to_string);
    RollyPay::with_api_base(server.uri())
        .create_invoice(config, &request)
        .await
}

#[test]
fn подпись_rollypay_по_контрольным_векторам() {
    assert_eq!(
        signature(SECRET, VECTOR_TIME, PAID_BODY.as_bytes()),
        PAID_SIGNATURE
    );
    assert_eq!(
        signature(SECRET, VECTOR_TIME, REFUND_BODY.as_bytes()),
        REFUND_SIGNATURE
    );
    assert!(verify(
        SECRET,
        VECTOR_TIME,
        PAID_BODY.as_bytes(),
        PAID_SIGNATURE
    ));
    assert!(verify(
        SECRET,
        VECTOR_TIME,
        PAID_BODY.as_bytes(),
        &format!(" {} ", PAID_SIGNATURE.to_ascii_uppercase())
    ));
    let changed = PAID_BODY.replace("1500.00", "1500.01");
    assert!(!verify(
        SECRET,
        VECTOR_TIME,
        changed.as_bytes(),
        PAID_SIGNATURE
    ));
    assert!(!verify(
        SECRET,
        "1771848001",
        PAID_BODY.as_bytes(),
        PAID_SIGNATURE
    ));
    assert!(!verify(
        "other",
        VECTOR_TIME,
        PAID_BODY.as_bytes(),
        PAID_SIGNATURE
    ));
    for bad in ["", "zz", &PAID_SIGNATURE[..62], "sha256=539d"] {
        assert!(
            !verify(SECRET, VECTOR_TIME, PAID_BODY.as_bytes(), bad),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn подпись_считается_по_сырому_телу_а_не_по_разобранному() {
    let sent = now();
    let pretty = signed(REFUND_BODY, &sent);
    let outcome = parse(&settings(), &pretty).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Refunded);
    let compact =
        serde_json::to_vec(&serde_json::from_str::<serde_json::Value>(REFUND_BODY).unwrap())
            .unwrap();
    let mut reserialized = pretty.clone();
    reserialized.body = compact.into();
    assert_eq!(
        parse(&settings(), &reserialized).await,
        Err(PaymentError::BadSignature)
    );
}

#[tokio::test]
async fn уведомление_с_верной_подписью_засчитывает_оплату() {
    let outcome = paid("paid").await;
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(PAY));
    assert_eq!(outcome.event_key, format!("{PAY}:paid"));
    assert_eq!(outcome.reply, WebhookReply::ok());
    assert!(outcome.signal.is_none());
    let foreign = parse(&settings(), &signed(PAID_BODY, &now()))
        .await
        .unwrap();
    assert_eq!(foreign.payment, Some(PaymentRef::ProviderTxid(PAY.into())));
}

#[tokio::test]
async fn без_подписи_или_с_чужой_подписью_уведомление_отклоняется() {
    let sent = now();
    let body = event("payment.paid", "paid", "1500.00", "RUB", false);
    let forged = signature("guess", &sent, &body);
    let older = signature(SECRET, "1771848000", &body);
    for headers in [
        vec![],
        vec![("X-Timestamp", sent.as_str())],
        vec![("X-Signature", forged.as_str())],
        vec![
            ("X-Timestamp", sent.as_str()),
            ("X-Signature", forged.as_str()),
        ],
        vec![
            ("X-Timestamp", sent.as_str()),
            ("X-Signature", older.as_str()),
        ],
        vec![("X-Timestamp", sent.as_str()), ("X-Signature", "")],
    ] {
        assert_eq!(
            parse(&settings(), &hook(&headers, body.clone())).await,
            Err(PaymentError::BadSignature),
            "{headers:?}"
        );
    }
    let mut wrong_route = signed(body, &sent);
    wrong_route.route_suffix = Some("pay".into());
    assert!(matches!(
        parse(&settings(), &wrong_route).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[test]
fn окно_времени_уведомления() {
    let base = 1_771_848_000_i64;
    let at = DateTime::from_timestamp(base, 0).unwrap();
    for offset in [0, -60, 60, -14 * 60, 14 * 60, -15 * 60, 15 * 60] {
        assert_eq!(fresh(&(base + offset).to_string(), at), Ok(()), "{offset}");
    }
    assert_eq!(fresh(&(base * 1000 + 999).to_string(), at), Ok(()));
    let Err(PaymentError::BadPayload(stale)) = fresh(&(base - 16 * 60).to_string(), at) else {
        panic!("старое уведомление принято");
    };
    assert!(stale.contains("16 мин назад"), "{stale}");
    assert!(stale.contains("не старше 15 мин"), "{stale}");
    let Err(PaymentError::BadPayload(ahead)) = fresh(&(base + 20 * 60).to_string(), at) else {
        panic!("уведомление из будущего принято");
    };
    assert!(ahead.contains("20 мин впереди"), "{ahead}");
    for bad in [
        "",
        "abc",
        "-5",
        "1771848000.5",
        " 1771848000",
        "99999999999999999",
    ] {
        assert!(
            matches!(fresh(bad, at), Err(PaymentError::BadPayload(_))),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn старое_подлинное_уведомление_не_принимается_повторно() {
    let replayed = hook(
        &[
            ("X-Timestamp", VECTOR_TIME),
            ("X-Signature", PAID_SIGNATURE),
        ],
        PAID_BODY,
    );
    let Err(PaymentError::BadPayload(reason)) = parse(&settings(), &replayed).await else {
        panic!("повтор старого уведомления принят");
    };
    assert!(reason.contains("повтор старого уведомления"), "{reason}");
    let late = (Utc::now() - Duration::minutes(20)).timestamp().to_string();
    assert!(matches!(
        parse(
            &settings(),
            &signed(
                event("payment.paid", "paid", "1500.00", "RUB", false),
                &late
            )
        )
        .await,
        Err(PaymentError::BadPayload(_))
    ));
    let soon = (Utc::now() + Duration::minutes(3)).timestamp().to_string();
    let millis = Utc::now().timestamp_millis().to_string();
    for sent in [soon, millis] {
        let outcome = parse(
            &settings(),
            &signed(
                event("payment.paid", "paid", "1500.00", "RUB", false),
                &sent,
            ),
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid, "{sent}");
    }
}

#[tokio::test]
async fn повтор_того_же_уведомления_даёт_то_же_событие() {
    let request = signed(
        event("payment.paid", "paid", "1500.00", "RUB", false),
        &now(),
    );
    let first = parse(&settings(), &request).await.unwrap();
    let again = parse(&settings(), &request).await.unwrap();
    assert_eq!(first, again);
    let resent = signed(
        event("payment.paid", "paid", "1500.00", "RUB", false),
        &(Utc::now().timestamp() + 30).to_string(),
    );
    assert_eq!(parse(&settings(), &resent).await.unwrap(), first);
}

#[tokio::test]
async fn статусы_rollypay() {
    for (status, expected, key) in [
        ("created", ProviderStatus::Pending, format!("{PAY}:created")),
        (
            "processing",
            ProviderStatus::Pending,
            format!("{PAY}:processing"),
        ),
        ("paid", ProviderStatus::Paid, format!("{PAY}:paid")),
        (
            "canceled",
            ProviderStatus::Canceled,
            format!("{PAY}:canceled"),
        ),
        (
            "expired",
            ProviderStatus::Canceled,
            format!("{PAY}:expired"),
        ),
        (
            "refunded",
            ProviderStatus::Refunded,
            format!("refund:{PAY}"),
        ),
        (
            "chargeback",
            ProviderStatus::Refunded,
            format!("chargeback:{PAY}"),
        ),
        ("on_hold", ProviderStatus::Pending, format!("{PAY}:on_hold")),
    ] {
        let outcome = paid(status).await;
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.event_key, key, "{status}");
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::PublicId(PAYMENT)),
            "{status}"
        );
        assert_eq!(outcome.amount, Some(rub(150_000)), "{status}");
        assert_eq!(
            outcome.notice().map(|n| n.kind),
            (status == "chargeback").then_some(NoticeKind::Chargeback),
            "{status}"
        );
    }
    let chargeback = paid("chargeback").await;
    let notice = chargeback.notice().unwrap();
    assert!(notice.text.contains(PAY), "{}", notice.text);
    assert!(
        notice.text.contains("подписка не изменена"),
        "{}",
        notice.text
    );
    let upper = parse(
        &settings(),
        &signed(
            event("payment.paid", "PAID", "1500.00", "RUB", false),
            &now(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(upper.status, ProviderStatus::Paid);
}

#[tokio::test]
async fn возврат_приходит_двумя_событиями_с_одним_ключом() {
    let sent = now();
    let request = signed(
        event(
            "refund_request.completed",
            "refunded",
            "1500.00",
            "RUB",
            false,
        ),
        &sent,
    );
    let first = parse(&settings(), &request).await.unwrap();
    let second = parse(
        &settings(),
        &signed(
            event("payment.refunded", "refunded", "1500.00", "RUB", false),
            &sent,
        ),
    )
    .await
    .unwrap();
    assert_eq!(first.status, ProviderStatus::Refunded);
    assert_eq!(first.event_key, second.event_key);
    assert_eq!(first.amount, Some(rub(150_000)));
    assert_eq!(first.provider_txid.as_deref(), Some(PAY));
    let provider = RollyPay::new();
    assert!(!provider.supports_refunds());
    assert!(!provider.supports_refund_polling());
    let refund = RefundRequest {
        payment_id: PaymentId(41),
        provider_txid: PAY.into(),
        amount: rub(150_000),
        reason: None,
        key: Uuid::from_u128(1),
        email: None,
        description: None,
    };
    assert!(matches!(
        provider.refund(&settings(), &refund).await,
        Err(PaymentError::Unsupported(_))
    ));
}

#[tokio::test]
async fn вывод_и_неизвестные_события_пропускаются() {
    let payout = serde_json::to_vec(&json!({
        "event_type": "payout.completed", "payout_id": "po_1", "amount": "100.00", "currency": "USDT"
    }))
    .unwrap();
    let skipped = parse(&settings(), &signed(payout, &now())).await.unwrap();
    assert_eq!(skipped.payment, None);
    assert_eq!(skipped.status, ProviderStatus::Pending);
    for broken in [
        json!({"event_type": "payment.paid", "status": "paid", "amount": "1.00", "currency": "RUB"}),
        json!({"event_type": "payment.paid", "payment_id": PAY, "amount": "1.00", "currency": "RUB"}),
        json!({"event_type": "payment.paid", "payment_id": "../x", "status": "paid", "amount": "1.00", "currency": "RUB"}),
    ] {
        let body = serde_json::to_vec(&broken).unwrap();
        assert!(
            matches!(
                parse(&settings(), &signed(body, &now())).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{broken}"
        );
    }
    assert!(matches!(
        parse(&settings(), &signed(b"not json".to_vec(), &now())).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn сумма_не_сошлась_и_чужая_валюта() {
    let expected = rub(150_000);
    let sent = now();
    let under = parse(
        &settings(),
        &signed(
            event("payment.paid", "paid", "1499.99", "RUB", false),
            &sent,
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        check_paid_amount(expected, under.amount),
        AmountCheck::Underpaid { .. }
    ));
    let over = parse(
        &settings(),
        &signed(
            event("payment.paid", "paid", "1500.01", "RUB", false),
            &sent,
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        check_paid_amount(expected, over.amount),
        AmountCheck::Overpaid { .. }
    ));
    let dollars = parse(
        &settings(),
        &signed(
            event("payment.paid", "paid", "1500.00", "USD", false),
            &sent,
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        check_paid_amount(expected, dollars.amount),
        AmountCheck::CurrencyMismatch { .. }
    ));
    assert_eq!(
        check_paid_amount(expected, paid("paid").await.amount),
        AmountCheck::Ok
    );
    for (amount, currency) in [
        ("1500.001", "RUB"),
        ("", "RUB"),
        ("1500.00", "EUR"),
        ("1500.00", ""),
    ] {
        assert!(
            matches!(
                parse(
                    &settings(),
                    &signed(
                        event("payment.paid", "paid", amount, currency, false),
                        &sent
                    )
                )
                .await,
                Err(PaymentError::BadPayload(_))
            ),
            "{amount} {currency}"
        );
    }
}

#[tokio::test]
async fn тестовые_оплаты_засчитываются_только_в_тестовом_режиме() {
    let sent = now();
    let sandbox_body = event("payment.paid", "paid", "1500.00", "RUB", true);
    let skipped = parse(&settings(), &signed(sandbox_body.clone(), &sent))
        .await
        .unwrap();
    assert_eq!(skipped.status, ProviderStatus::Pending);
    assert_eq!(skipped.payment, None);
    let mut by_header = signed(
        event("payment.paid", "paid", "1500.00", "RUB", false),
        &sent,
    );
    by_header
        .headers
        .push(("X-Test-Mode".into(), "true".into()));
    assert_eq!(parse(&settings(), &by_header).await.unwrap().payment, None);
    let accepted = parse(&testing(), &signed(sandbox_body, &sent))
        .await
        .unwrap();
    assert_eq!(accepted.status, ProviderStatus::Paid);
    assert_eq!(accepted.payment, Some(PaymentRef::PublicId(PAYMENT)));
    let real = parse(
        &testing(),
        &signed(
            event("payment.paid", "paid", "1500.00", "RUB", false),
            &sent,
        ),
    )
    .await
    .unwrap();
    assert_eq!(real.status, ProviderStatus::Paid);
}

#[tokio::test]
async fn счёт_rollypay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payments"))
        .and(header("X-API-Key", KEY))
        .and(header_exists("X-Nonce"))
        .and(header("Content-Type", "application/json"))
        .and(body_json(json!({
            "amount": "1500.00",
            "payment_currency": "RUB",
            "order_id": PAYMENT.to_string(),
            "description": "Стандарт · 3 месяца",
            "customer_id": CLIENT.to_string(),
            "success_redirect_url": RETURN,
            "fail_redirect_url": RETURN
        })))
        .respond_with(created(&PAYMENT.to_string(), "1500.00"))
        .expect(1)
        .mount(&server)
        .await;
    let invoice = create(&server, &settings(), None).await.unwrap();
    assert_eq!(invoice.external_id.as_deref(), Some(PAY));
    assert_eq!(
        invoice.pay_url,
        "https://pay.rollypay.io/pay/tok_9f8e7d6c5b4a3210"
    );
    assert_eq!(
        invoice.expires_at,
        DateTime::parse_from_rfc3339("2026-11-18T12:30:00Z")
            .ok()
            .map(|t| t.with_timezone(&Utc))
    );
    assert_eq!(
        invoice.payload,
        json!({"payment_id": PAY, "status": "created", "test": false})
    );
    let sent = server.received_requests().await.unwrap();
    let raw = String::from_utf8_lossy(&sent[0].body).to_string();
    assert!(!raw.contains("client@example.com"), "{raw}");
    assert!(!raw.contains("tok_"), "{raw}");
}

#[tokio::test]
async fn способ_оплаты_и_тестовый_режим_уходят_в_счёт() {
    for (chosen, wire) in [
        (Some("sbp"), Some("sbp")),
        (Some("card"), Some("card")),
        (Some("intl_card"), Some("intl_card")),
        (Some("crypto"), Some("crypto")),
        (Some("page"), None),
        (None, None),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/payments"))
            .respond_with(created(&PAYMENT.to_string(), "1500.00"))
            .expect(1)
            .mount(&server)
            .await;
        create(&server, &settings(), chosen).await.unwrap();
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(
            body.get("payment_method").and_then(|m| m.as_str()),
            wire,
            "{chosen:?}"
        );
        assert_eq!(body.get("test"), None, "{chosen:?}");
    }
    let server = MockServer::start().await;
    for code in ["sberpay", "SBP", "", "erip"] {
        assert_eq!(
            create(&server, &settings(), Some(code)).await,
            Err(PaymentError::Rejected(
                "этот способ оплаты недоступен".into()
            )),
            "{code}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    Mock::given(method("POST"))
        .and(path("/api/v1/payments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "payment_id": PAY, "order_id": PAYMENT.to_string(), "status": "created",
            "pay_url": "https://pay.rollypay.io/pay/tok_test", "amount": "1500.00",
            "payment_currency": "RUB", "environment": "sandbox", "test": true
        })))
        .mount(&server)
        .await;
    let invoice = create(&server, &testing(), Some("card")).await.unwrap();
    assert_eq!(invoice.payload["test"], json!(true));
    assert_eq!(invoice.expires_at, None);
    let sent = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["test"], json!(true));
    assert_eq!(body["payment_method"], json!("card"));
    let dollars = RollyPay::with_api_base(server.uri())
        .create_invoice(&settings(), &invoice_for_usd())
        .await;
    assert_eq!(
        dollars,
        Err(PaymentError::UnsupportedCurrency("USD".into()))
    );
}

fn invoice_for_usd() -> InvoiceRequest {
    invoice(usd(500), None)
}

#[tokio::test]
async fn ответ_на_счёт_проверяется() {
    let other_order = Uuid::from_u128(99).to_string();
    for (answer, reason) in [
        (created(&other_order, "1500.00"), "другого заказа"),
        (created(&PAYMENT.to_string(), "1499.00"), "вместо"),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "payment_id": PAY, "order_id": PAYMENT.to_string(),
                "pay_url": "http://pay.rollypay.io/pay/tok"
            })),
            "ссылку на оплату",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "order_id": PAYMENT.to_string(), "pay_url": "https://pay.rollypay.io/pay/tok"
            })),
            "номер платежа",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(answer)
            .mount(&server)
            .await;
        let Err(PaymentError::BadPayload(text)) = create(&server, &settings(), None).await else {
            panic!("ответ принят: {reason}");
        };
        assert!(text.contains(reason), "{text}");
    }
}

#[tokio::test]
async fn ошибки_rollypay_понятны_оператору() {
    for (answer, expected) in [
        (
            problem(400, "payment cap exceeded"),
            PaymentError::Rejected("превышен лимит платежей кассы RollyPay".into()),
        ),
        (
            problem(400, "terminal not found"),
            PaymentError::Rejected("RollyPay не нашла кассу этого API-ключа".into()),
        ),
        (
            problem(
                400,
                "Сумма слишком мала для выбранного способа оплаты (для EUR — минимум 1 EUR)",
            ),
            PaymentError::Rejected(
                "Сумма слишком мала для выбранного способа оплаты (для EUR — минимум 1 EUR)".into(),
            ),
        ),
        (
            problem(401, "invalid api key"),
            PaymentError::Rejected(util::KEYS_REFUSED.into()),
        ),
        (
            problem(500, "unable to create payment"),
            PaymentError::Http { status: 500 },
        ),
        (
            problem(429, "too many requests"),
            PaymentError::Http { status: 429 },
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(answer)
            .mount(&server)
            .await;
        assert_eq!(
            create(&server, &settings(), None).await,
            Err(expected.clone())
        );
    }
}

#[tokio::test]
async fn у_каждого_запроса_свой_nonce_а_повтор_nonce_повторяется_с_новым() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .respond_with(problem(401, "nonce already used"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .and(header("X-API-Key", KEY))
        .respond_with(balance())
        .with_priority(2)
        .mount(&server)
        .await;
    let provider = RollyPay::with_api_base(server.uri());
    let check = provider
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert_eq!(check.status, CheckStatus::Ok, "{}", check.summary);
    let again = provider
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert_eq!(again.status, CheckStatus::Ok, "{}", again.summary);
    let sent = nonces(&server.received_requests().await.unwrap());
    assert_eq!(sent.len(), 3);
    for nonce in &sent {
        assert_eq!(nonce.get_version_num(), 4);
    }
    let mut unique = sent.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), sent.len());

    let stuck = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(problem(409, "Nonce already used"))
        .mount(&stuck)
        .await;
    assert_eq!(
        RollyPay::with_api_base(stuck.uri())
            .fetch_status(&settings(), PAY)
            .await,
        Err(PaymentError::Rejected(NONCE_REUSED.into()))
    );
    assert_eq!(stuck.received_requests().await.unwrap().len(), 2);
}

async fn api_payment(body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/payments/{PAY}")))
        .and(header("X-API-Key", KEY))
        .and(header_exists("X-Nonce"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn опрос_статуса_rollypay() {
    for (status, expected) in [
        ("created", ProviderStatus::Pending),
        ("processing", ProviderStatus::Pending),
        ("paid", ProviderStatus::Paid),
        ("expired", ProviderStatus::Canceled),
        ("canceled", ProviderStatus::Canceled),
        ("refunded", ProviderStatus::Refunded),
    ] {
        let server = api_payment(json!({
            "payment_id": PAY, "order_id": PAYMENT.to_string(), "amount": "1500.00",
            "payment_currency": "RUB", "status": status,
            "pay_url": "https://pay.rollypay.io/pay/tok_1", "rate_final": "93.89",
            "paid_at": "2026-11-18T12:05:32Z"
        }))
        .await;
        let outcome = RollyPay::with_api_base(server.uri())
            .fetch_status(&settings(), PAY)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.amount, Some(rub(150_000)), "{status}");
        assert_eq!(outcome.provider_txid.as_deref(), Some(PAY), "{status}");
    }
    let sandbox = api_payment(json!({
        "payment_id": PAY, "order_id": PAYMENT.to_string(), "amount": "1500.00",
        "payment_currency": "RUB", "status": "paid", "environment": "sandbox", "test": true
    }))
    .await;
    let provider = RollyPay::with_api_base(sandbox.uri());
    let ignored = provider
        .fetch_status(&settings(), PAY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ignored.status, ProviderStatus::Pending);
    let counted = provider
        .fetch_status(&testing(), PAY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counted.status, ProviderStatus::Paid);
    let other = api_payment(json!({
        "payment_id": "pay_other", "amount": "1500.00", "payment_currency": "RUB", "status": "paid"
    }))
    .await;
    assert!(matches!(
        RollyPay::with_api_base(other.uri())
            .fetch_status(&settings(), PAY)
            .await,
        Err(PaymentError::BadPayload(_))
    ));
    let empty = MockServer::start().await;
    assert_eq!(
        RollyPay::with_api_base(empty.uri())
            .fetch_status(&settings(), PAY)
            .await,
        Err(PaymentError::Rejected("платёж не найден в RollyPay".into()))
    );
    assert!(matches!(
        RollyPay::with_api_base(empty.uri())
            .fetch_status(&settings(), "../balance")
            .await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn проверка_подключения_rollypay_по_балансу_кассы() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .and(header("X-API-Key", KEY))
        .and(header_exists("X-Nonce"))
        .respond_with(balance())
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .respond_with(problem(401, "invalid api key"))
        .with_priority(10)
        .mount(&server)
        .await;
    let provider = RollyPay::with_api_base(server.uri());
    let hooked = CheckContext {
        webhook_url: Some("https://panel.example.com/api/webhooks/pay/rollypay".into()),
    };
    let ok = provider.check_connection(&settings(), &hooked).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    assert_eq!(ok.summary, "API-ключ принят RollyPay");
    assert!(ok.details[0].contains("d290f1ee-6c54-4b01-90e6-d701748f0851"));
    assert!(ok.details.last().unwrap().contains("Ключ подписи"));
    assert!(!ok.details.iter().any(|d| d.contains("тестовый режим")));
    assert!(!ok.details.iter().any(|d| d.contains("Часы")));
    let sandbox = provider
        .check_connection(&testing(), &CheckContext::default())
        .await;
    assert!(
        sandbox
            .details
            .iter()
            .any(|d| d.contains("Включён тестовый режим"))
    );
    assert!(
        sandbox
            .details
            .iter()
            .any(|d| d.contains("Адрес панели не задан"))
    );
    let mut wrong = settings();
    wrong.set("api_key", json!("rpk_live_wrong"));
    let rejected = provider.check_connection(&wrong, &hooked).await;
    assert_eq!(rejected.status, CheckStatus::KeysRejected);
    assert_eq!(rejected.summary, "RollyPay не приняла API-ключ");
    for request in server.received_requests().await.unwrap() {
        assert_eq!(request.method, wiremock::http::Method::GET);
        assert!(request.body.is_empty());
    }
    let down = failing_api(502).await;
    let unreachable = RollyPay::with_api_base(down.uri())
        .check_connection(&settings(), &hooked)
        .await;
    assert_eq!(unreachable.status, CheckStatus::Unreachable);
    let closed = RollyPay::with_api_base(closed_api())
        .check_connection(&settings(), &hooked)
        .await;
    assert_eq!(closed.status, CheckStatus::Unreachable);
}

#[tokio::test]
async fn проверка_подключения_замечает_расхождение_часов() {
    let server = MockServer::start().await;
    let behind = (Utc::now() - Duration::minutes(30) - Duration::seconds(20))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .respond_with(balance().insert_header("Date", behind.as_str()))
        .mount(&server)
        .await;
    let check = RollyPay::with_api_base(server.uri())
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert_eq!(check.status, CheckStatus::Ok);
    let drift = check
        .details
        .iter()
        .find(|d| d.contains("Часы сервера панели"))
        .unwrap();
    assert!(drift.contains("31 мин"), "{drift}");
}

#[test]
fn настройки_и_способы_rollypay() {
    let provider = RollyPay::new();
    assert_eq!(provider.id(), "rollypay");
    assert_eq!(provider.currencies(), &[Currency::Rub]);
    assert!(provider.accepts_http_webhooks());
    assert!(!provider.verifies_webhooks_by_api());
    assert!(!provider.enabled_by_default());
    assert!(provider.is_configured(&settings()));
    assert!(provider.is_configured(&testing()));
    let schema = provider.settings_schema();
    let secrets: Vec<&str> = schema
        .iter()
        .filter(|f| f.is_secret())
        .map(|f| f.key)
        .collect();
    assert_eq!(secrets, ["api_key", "signing_secret"]);
    let flag = schema.iter().find(|f| f.key == "test_mode").unwrap();
    assert!(!flag.required);
    for broken in [
        config(&[("api_key", json!(KEY))]),
        config(&[("signing_secret", json!(SECRET))]),
        config(&[
            ("api_key", json!("rpk live")),
            ("signing_secret", json!(SECRET)),
        ]),
        config(&[("api_key", json!(KEY)), ("signing_secret", json!("ключ"))]),
        config(&[
            ("api_key", json!("k".repeat(513))),
            ("signing_secret", json!(SECRET)),
        ]),
    ] {
        assert!(!provider.is_configured(&broken), "{broken:?}");
        assert!(matches!(
            provider.validate_settings(&broken),
            Err(PaymentError::NotConfigured(_))
        ));
    }
    let trimmed = config(&[
        ("api_key", json!(format!("  {KEY}\n"))),
        ("signing_secret", json!(SECRET)),
    ]);
    assert!(provider.is_configured(&trimmed));
    let listed: Vec<(&str, &str)> = provider
        .methods(Currency::Rub)
        .iter()
        .map(|m| (m.code, m.title))
        .collect();
    assert_eq!(
        listed,
        [
            ("sbp", "СБП"),
            ("card", "Карта"),
            ("intl_card", "Иностранная карта"),
            ("crypto", "Криптовалюта"),
            ("page", "Все способы"),
        ]
    );
    assert!(provider.methods(Currency::Usd).is_empty());
    for method in provider.methods(Currency::Rub) {
        assert!(
            crate::english::text(method.title).is_some(),
            "{}",
            method.title
        );
        assert!(
            crate::english::text(method.hint).is_some(),
            "{}",
            method.hint
        );
    }
    let label = provider.client_label(Currency::Rub, &settings());
    assert_eq!((label.title, label.hint), ("СБП", "Через приложение банка"));
}

const TERMINAL: &str = "d290f1ee-6c54-4b01-90e6-d701748f0851";
const PANEL_HOOK: &str = "https://panel.example.com/api/webhooks/pay/rollypay";

async fn terminal_api(callback: Option<serde_json::Value>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .and(header("X-API-Key", KEY))
        .respond_with(balance())
        .mount(&server)
        .await;
    if let Some(callback) = callback {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/terminals/{TERMINAL}")))
            .and(header("X-API-Key", KEY))
            .and(header_exists("X-Nonce"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": TERMINAL, "name": "VPN", "callback_url": callback,
                "success_redirect_url": "", "conversion_mode": "instant"
            })))
            .mount(&server)
            .await;
    }
    server
}

async fn hooks_of(server: &MockServer) -> ConnectionCheck {
    RollyPay::with_api_base(server.uri())
        .check_connection(
            &settings(),
            &CheckContext {
                webhook_url: Some(PANEL_HOOK.into()),
            },
        )
        .await
}

#[tokio::test]
async fn проверка_подключения_сверяет_адрес_уведомлений_кассы() {
    let same = hooks_of(&terminal_api(Some(json!(format!("{PANEL_HOOK}/")))).await).await;
    assert_eq!(same.status, CheckStatus::Ok, "{}", same.summary);
    assert_eq!(
        same.hooks,
        [HookCheck {
            label: "callback_url".into(),
            state: HookState::Same,
            address: None,
            note: None,
        }]
    );
    assert!(!same.details.iter().any(|d| d.contains("callback_url")));
    let factory =
        hooks_of(&terminal_api(Some(json!("https://example.com/payments/callback"))).await).await;
    assert_eq!(factory.status, CheckStatus::Ok);
    assert_eq!(factory.hooks[0].state, HookState::Other);
    assert_eq!(
        factory.hooks[0].address.as_deref(),
        Some("https://example.com/payments/callback")
    );
    assert!(
        factory.hooks[0]
            .note
            .as_deref()
            .is_some_and(|n| n.contains("заглушка")),
        "{:?}",
        factory.hooks[0]
    );
    let foreign = hooks_of(
        &terminal_api(Some(json!(
            "https://old-panel.example.org/api/webhooks/rollypay?key=secret"
        )))
        .await,
    )
    .await;
    assert_eq!(foreign.hooks[0].state, HookState::Other);
    assert_eq!(
        foreign.hooks[0].address.as_deref(),
        Some("https://old-panel.example.org/api/webhooks/rollypay")
    );
    assert_eq!(foreign.hooks[0].note, None);
    for empty in [json!(""), json!(null)] {
        let off = hooks_of(&terminal_api(Some(empty.clone())).await).await;
        assert_eq!(off.hooks[0].state, HookState::Off, "{empty}");
        assert_eq!(off.hooks[0].address, None, "{empty}");
    }
    let hidden = terminal_api(None).await;
    let unknown = hooks_of(&hidden).await;
    assert_eq!(unknown.status, CheckStatus::Ok);
    assert!(unknown.hooks.is_empty());
    assert!(
        unknown
            .details
            .iter()
            .any(|d| d.contains("callback_url") && d.contains("example.com")),
        "{:?}",
        unknown.details
    );
    for request in hidden.received_requests().await.unwrap() {
        assert_eq!(request.method, wiremock::http::Method::GET);
    }
    let silent = RollyPay::with_api_base(hidden.uri())
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert!(silent.hooks.is_empty());
    let calls = hidden.received_requests().await.unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.url.path().starts_with("/api/v1/terminals"))
            .count(),
        1
    );
}

fn with_terminal(terminal: &str) -> ProviderConfig {
    let mut settings = settings();
    settings.set("terminal_id", json!(terminal));
    settings
}

fn paid_object() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "payment_id": PAY, "order_id": PAYMENT.to_string(), "amount": "1500.00",
        "payment_currency": "RUB", "status": "paid"
    }))
}

#[test]
fn id_кассы_в_настройках_rollypay() {
    let provider = RollyPay::new();
    let field = provider
        .settings_schema()
        .into_iter()
        .find(|f| f.key == "terminal_id")
        .unwrap();
    assert_eq!(field.label, "ID кассы (Terminal ID)");
    assert!(!field.required);
    assert!(!field.is_secret());
    assert!(field.hint.contains("UUID"));
    for good in [
        TERMINAL.to_string(),
        TERMINAL.to_uppercase(),
        format!(" {TERMINAL} "),
        "  ".into(),
    ] {
        assert!(provider.is_configured(&with_terminal(&good)), "{good}");
    }
    for bad in [
        "abc",
        "d290f1ee6c544b0190e6d701748f0851",
        "{d290f1ee-6c54-4b01-90e6-d701748f0851}",
        "d290f1ee-6c54-4b01-90e6-d701748f085g",
        "d290f1ee-6c54-4b01-90e6-d701748f0851&status=paid",
    ] {
        assert!(!provider.is_configured(&with_terminal(bad)), "{bad}");
        let Err(PaymentError::NotConfigured(reason)) =
            provider.validate_settings(&with_terminal(bad))
        else {
            panic!("принят ID кассы {bad}");
        };
        assert!(reason.contains("UUID"), "{reason}");
    }
    assert_eq!(
        scoped("/api/v1/balance", &with_terminal(TERMINAL)),
        format!("/api/v1/balance?terminal_id={TERMINAL}")
    );
    assert_eq!(scoped("/api/v1/balance", &settings()), "/api/v1/balance");
    assert_eq!(
        scoped("/api/v1/balance", &with_terminal("abc")),
        "/api/v1/balance"
    );
}

#[tokio::test]
async fn id_кассы_уходит_во_все_запросы() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payments"))
        .and(body_partial_json(json!({
            "terminal_id": TERMINAL, "order_id": PAYMENT.to_string(), "amount": "1500.00"
        })))
        .respond_with(created(&PAYMENT.to_string(), "1500.00"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/payments/{PAY}")))
        .and(query_param("terminal_id", TERMINAL))
        .respond_with(paid_object())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .and(query_param("terminal_id", TERMINAL))
        .and(header("X-API-Key", KEY))
        .respond_with(balance())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/terminals/{TERMINAL}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": TERMINAL, "callback_url": PANEL_HOOK
        })))
        .expect(1)
        .mount(&server)
        .await;
    let config = with_terminal(TERMINAL);
    let provider = RollyPay::with_api_base(server.uri());
    create(&server, &config, Some("sbp")).await.unwrap();
    let polled = provider.fetch_status(&config, PAY).await.unwrap().unwrap();
    assert_eq!(polled.status, ProviderStatus::Paid);
    let check = hooks_of_with(&server, &config).await;
    assert_eq!(check.status, CheckStatus::Ok, "{}", check.summary);
    assert_eq!(check.hooks[0].state, HookState::Same);
    assert!(
        check.details[0].contains(&format!("ID кассы {TERMINAL} подходит к API-ключу")),
        "{:?}",
        check.details
    );
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent.len(), 4);
    for request in sent {
        let in_query = request
            .url
            .query_pairs()
            .any(|(k, v)| k == "terminal_id" && v == TERMINAL);
        let in_body = serde_json::from_slice::<serde_json::Value>(&request.body)
            .is_ok_and(|b| b["terminal_id"] == TERMINAL);
        let in_path = request.url.path() == format!("/api/v1/terminals/{TERMINAL}");
        assert!(in_query || in_body || in_path, "{}", request.url);
    }
}

async fn hooks_of_with(server: &MockServer, config: &ProviderConfig) -> ConnectionCheck {
    RollyPay::with_api_base(server.uri())
        .check_connection(
            config,
            &CheckContext {
                webhook_url: Some(PANEL_HOOK.into()),
            },
        )
        .await
}

#[tokio::test]
async fn без_id_кассы_запросы_как_раньше() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payments"))
        .respond_with(created(&PAYMENT.to_string(), "1500.00"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/payments/{PAY}")))
        .and(query_param_is_missing("terminal_id"))
        .respond_with(paid_object())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .and(query_param_is_missing("terminal_id"))
        .respond_with(balance())
        .expect(1)
        .mount(&server)
        .await;
    let provider = RollyPay::with_api_base(server.uri());
    create(&server, &settings(), None).await.unwrap();
    provider
        .fetch_status(&settings(), PAY)
        .await
        .unwrap()
        .unwrap();
    let check = provider
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert_eq!(check.status, CheckStatus::Ok, "{}", check.summary);
    assert!(
        check.details[0].contains(TERMINAL) && check.details[0].contains("Впишите этот ID"),
        "{:?}",
        check.details
    );
    let sent = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body.get("terminal_id"), None);
}

#[tokio::test]
async fn rollypay_просит_id_кассы() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(problem(400, "terminal_id is required"))
        .mount(&server)
        .await;
    let provider = RollyPay::with_api_base(server.uri());
    let check = provider
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert_eq!(check.status, CheckStatus::SettingsRejected);
    assert_eq!(
        check.summary,
        "RollyPay просит ID кассы: заполните поле «ID кассы (Terminal ID)»"
    );
    assert!(check.details[0].contains("UUID"), "{:?}", check.details);
    let required = Err(PaymentError::Rejected(TERMINAL_REQUIRED.into()));
    assert_eq!(
        create(&server, &settings(), None).await.map(|_| ()),
        required
    );
    assert_eq!(
        provider.fetch_status(&settings(), PAY).await.map(|_| ()),
        required
    );
    let insisted = provider
        .check_connection(&with_terminal(TERMINAL), &CheckContext::default())
        .await;
    assert_eq!(insisted.status, CheckStatus::SettingsRejected);
    assert!(
        insisted.summary.contains(TERMINAL) && insisted.summary.contains("terminal_id is required"),
        "{}",
        insisted.summary
    );
}

#[tokio::test]
async fn id_кассы_не_подходит_к_ключу() {
    let other = "5f1d7c3a-1111-4222-8333-944455556666";
    for (status, text) in [
        (403, "forbidden"),
        (404, "not found"),
        (400, "terminal not found"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/balance"))
            .respond_with(problem(status, text))
            .mount(&server)
            .await;
        let check = RollyPay::with_api_base(server.uri())
            .check_connection(&with_terminal(other), &CheckContext::default())
            .await;
        assert_eq!(check.status, CheckStatus::SettingsRejected, "{status}");
        assert!(
            check.summary.contains(other) && check.summary.contains("не подходит к API-ключу"),
            "{}",
            check.summary
        );
    }
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .respond_with(problem(403, "forbidden"))
        .mount(&server)
        .await;
    let keys = RollyPay::with_api_base(server.uri())
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert_eq!(keys.status, CheckStatus::KeysRejected);
    let foreign = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .and(query_param("terminal_id", other))
        .respond_with(balance())
        .mount(&foreign)
        .await;
    let check = RollyPay::with_api_base(foreign.uri())
        .check_connection(&with_terminal(other), &CheckContext::default())
        .await;
    assert_eq!(check.status, CheckStatus::SettingsRejected);
    assert!(
        check.summary.contains(other) && check.summary.contains(TERMINAL),
        "{}",
        check.summary
    );
}
