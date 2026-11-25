use super::*;
use sn_domain::money::{Amount, Money, PayCurrency};
use sn_domain::payment::{Payment, PaymentStatus, RefundMethod, RefundSubscriptionAction};
use sn_domain::pricing::{TopUpLimits, topup_quote};
use sn_store::{InvoiceUpdate, NewRefund, SessionChannel};

async fn topup_payment(h: &Http, client_id: ClientId, minor: i64, method: Option<&str>) -> Payment {
    let amount = Money::new(minor, Currency::Usd);
    let quote = topup_quote(
        amount,
        TopUpLimits {
            min: Money::new(100, Currency::Usd),
            max: Money::new(100_000, Currency::Usd),
        },
    )
    .unwrap();
    let mut tx = h.services().store().begin().await.unwrap();
    let order = tx
        .orders()
        .create(&NewOrder {
            client_id,
            subscription_id: None,
            kind: OrderKind::TopUp,
            intent: PurchaseIntent::TopUp { amount },
            quote,
            snapshot: None,
        })
        .await
        .unwrap();
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: order.id,
            client_id,
            provider: ProviderId::from(providers::CRYPTOPAY),
            method: method.map(str::to_string),
            amount: amount.into(),
            expires_at: None,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    payment
}

async fn move_to(h: &Http, payment: &Payment, to: PaymentStatus) {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.payments()
        .transition(payment.id, to, None, None)
        .await
        .unwrap();
}

async fn invoice(h: &Http, payment: &Payment, tag: &str, expires_at: DateTime<Utc>) {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.payments()
        .attach_invoice(
            payment.id,
            &InvoiceUpdate {
                pay_url: Some(format!("https://pay.crypt.bot/raw-{tag}")),
                provider_txid: Some(format!("txid-{tag}")),
                expires_at: Some(expires_at),
                payload: json!({ "token": format!("payload-{tag}") }),
            },
        )
        .await
        .unwrap();
}

async fn miniapp_token(h: &Http, client: ClientId) -> String {
    h.services()
        .auth()
        .client_session_for(client, SessionChannel::Miniapp, &Default::default(), false)
        .await
        .unwrap()
        .token
}

fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn история_оплат_только_свои_постранично_и_без_данных_провайдера() {
    let Some(h) = http().await else { return };
    let tariff = h.tariff().await;
    let alice = h.client(5_093_118_501).await;
    let bob = h.client(5_093_118_502).await;

    let bought = h.pending_payment(alice, &tariff).await;
    move_to(&h, &bought, PaymentStatus::Paid).await;
    let topped = topup_payment(&h, alice, 1_000, Some("usdt")).await;
    move_to(&h, &topped, PaymentStatus::Paid).await;
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.refunds()
            .create(&NewRefund {
                bonus_minor: 0,
                payment_id: topped.id,
                amount: Amount::new(300, PayCurrency::Usd),
                method: RefundMethod::Balance,
                subscription_action: RefundSubscriptionAction::Keep,
                reason: Some("ошибка оператора".into()),
                admin_id: None,
                provider_refund_id: Some("refund-secret".into()),
            })
            .await
            .unwrap();
    }
    move_to(&h, &topped, PaymentStatus::PartiallyRefunded).await;
    let dropped = h.pending_payment(alice, &tariff).await;
    move_to(&h, &dropped, PaymentStatus::Canceled).await;
    let late = topup_payment(&h, alice, 2_000, Some("repeat")).await;
    invoice(&h, &late, "late", start() - Duration::minutes(5)).await;
    let open = h.pending_payment(alice, &tariff).await;
    invoice(&h, &open, "open", start() + Duration::minutes(30)).await;
    let foreign = h.pending_payment(bob, &tariff).await;
    invoice(&h, &foreign, "foreign", start() + Duration::minutes(30)).await;

    let token = h.client_token(alice).await;
    let (status, page) = h.get("/api/client/payments", &token).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let expected: Vec<String> = [&open, &late, &dropped, &topped, &bought]
        .iter()
        .map(|p| p.public_id.to_string())
        .collect();
    assert_eq!(ids(&page), expected);
    assert!(page["next"].is_null());
    let raw = page.to_string();
    for secret in [
        "raw-open",
        "raw-late",
        "txid-open",
        "payload-open",
        "refund-secret",
        "ошибка оператора",
        foreign.public_id.to_string().as_str(),
    ] {
        assert!(!raw.contains(secret), "в ответе есть {secret}: {raw}");
    }
    for field in ["provider_txid", "provider_payload", "order_id", "client_id"] {
        assert!(!raw.contains(field), "в ответе есть поле {field}");
    }

    let items = page["items"].as_array().unwrap();
    assert_eq!(items[0]["status"], "pending");
    assert_eq!(items[0]["pay_url"], format!("/api/pay/{}", open.public_id));
    assert_eq!(items[0]["method"], "Криптовалюта");
    assert_eq!(items[0]["purchase"]["kind"], "new");
    assert_eq!(items[0]["purchase"]["tariff"], "Стандарт");
    assert_eq!(items[0]["purchase"]["days"], 90);
    assert_eq!(items[0]["purchase"]["months"], 3);
    assert_eq!(items[1]["status"], "pending");
    assert!(items[1]["pay_url"].is_null(), "время оплаты вышло");
    assert_eq!(items[1]["purchase"]["kind"], "top_up");
    assert_eq!(items[1]["method"], "Криптовалюта");
    assert_eq!(items[2]["status"], "canceled");
    assert!(items[2]["pay_url"].is_null());
    assert_eq!(items[3]["status"], "partially_refunded");
    assert_eq!(
        items[3]["amount"],
        json!({"minor": 1000, "currency": "USD"})
    );
    assert_eq!(
        items[3]["refunded"],
        json!({"minor": 300, "currency": "USD"})
    );
    assert_eq!(items[3]["purchase"]["label"], "Пополнение баланса");
    assert_eq!(items[3]["method"], "Криптовалюта, USDT");
    assert_eq!(items[4]["status"], "paid");
    assert!(items[4]["refunded"].is_null());
    assert!(items[4]["paid_at"].is_string());

    let mut walked = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            Some(before) => format!("/api/client/payments?limit=2&before={before}"),
            None => "/api/client/payments?limit=2".to_string(),
        };
        let (status, page) = h.get(&path, &token).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let chunk = ids(&page);
        assert!(chunk.len() <= 2);
        walked.extend(chunk);
        match page["next"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(walked, expected);

    let (status, refused) = h
        .get(
            &format!("/api/client/payments?before={}", foreign.public_id),
            &token,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");

    let other = h.client_token(bob).await;
    let (status, page) = h.get("/api/client/payments", &other).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&page), [foreign.public_id.to_string()]);
    let (status, _) = h
        .get(&format!("/api/client/payments/{}", open.public_id), &other)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["payments_count"], 2);
    h.db.cleanup().await;
}

#[tokio::test]
async fn пополнение_отдаёт_суммы_и_способы_своего_канала_и_прячется_с_выключенным_балансом() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/cryptopay",
            &owner,
            json!({
                "enabled": true,
                "config": {"api_token": "1234:AAA"},
                "channels": {"cabinet": true, "bot": true, "miniapp": false}
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({"topup": {"min_minor": 1_000, "max_minor": 3_000}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let client = h.client(5_093_118_503).await;
    let cabinet = h.client_token(client).await;
    let mini = miniapp_token(&h, client).await;
    let (status, options) = h.get("/api/client/topup", &cabinet).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["currency"], "USD");
    assert_eq!(options["balance"], json!({"minor": 0, "currency": "USD"}));
    assert_eq!(options["min"], json!({"minor": 1000, "currency": "USD"}));
    assert_eq!(options["max"], json!({"minor": 3000, "currency": "USD"}));
    let presets: Vec<i64> = options["presets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["minor"].as_i64().unwrap())
        .collect();
    assert_eq!(presets, [1_000, 2_000]);
    assert_eq!(
        options["methods"],
        json!([{"provider": "cryptopay", "method": null, "title": "Криптовалюта", "hint": "USDT, TON, BTC"}])
    );

    let (status, options) = h.get("/api/client/topup", &mini).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["methods"], json!([]));
    let (status, refused) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &mini,
            json!({"amount": "15", "provider": "cryptopay"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "checkout.method_unavailable");
    let (status, refused) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &cabinet,
            json!({"amount": "5", "provider": "cryptopay", "return_url": "https://evil.example/"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"]["code"], "balance.topup_out_of_range");

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/balance",
            &owner,
            json!({"enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for token in [&cabinet, &mini] {
        let (status, refused) = h.get("/api/client/topup", token).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
        assert_eq!(refused["error"]["code"], "balance.disabled");
        let (_, me) = h.get("/api/client/me", token).await;
        assert_eq!(me["balance_enabled"], false);
    }
    let (status, history) = h.get("/api/client/payments", &cabinet).await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history["items"], json!([]));
    h.db.cleanup().await;
}
