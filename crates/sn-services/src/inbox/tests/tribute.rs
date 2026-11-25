use super::*;
use crate::decisions::Scope;
use sn_payments::providers::{Tribute, tribute_signature};

const TRIBUTE_KEY: &str = "trb-scenario-key";
const BUYER: i64 = 5_093_118_422;

async fn tribute_kit(products: serde_json::Value, plans: serde_json::Value) -> Option<Kit> {
    let kit = kassa_kit_with(
        Arc::new(Kassa::default()),
        Arc::new(SystemClock),
        vec![Arc::new(Tribute::new())],
    )
    .await?;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        settings::write_secret(
            &mut conn,
            kit.services.secrets(),
            &secret_keys::provider(providers::TRIBUTE, "api_key"),
            Some(TRIBUTE_KEY),
            None,
        )
        .await
        .unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::TRIBUTE),
                &serde_json::json!({
                    "enabled": true,
                    "config": {"products": products, "plans": plans}
                }),
                None,
            )
            .await
            .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
    Some(kit)
}

fn products() -> serde_json::Value {
    serde_json::json!([
        {"id": 11, "name": "Пополнение $5", "amount": 500, "currency": "USD", "link": "https://t.me/tribute/app?startapp=p11"},
        {"id": 12, "name": "Пополнение $10", "amount": 1000, "currency": "USD", "link": "https://t.me/tribute/app?startapp=p12"}
    ])
}

fn hook(body: &str) -> InboundWebhook {
    InboundWebhook {
        provider: providers::TRIBUTE.into(),
        request: WebhookRequest {
            headers: vec![(
                "trbt-signature".into(),
                tribute_signature(TRIBUTE_KEY, body.as_bytes()),
            )],
            body: Bytes::from(body.to_string()),
            remote_ip: "203.0.113.9".parse().ok(),
            route_suffix: None,
            query: None,
        },
    }
}

fn purchase(id: i64, telegram: i64, minor: i64, sent: &str) -> String {
    serde_json::json!({
        "name": "new_digital_product",
        "created_at": "2026-11-18T10:00:00Z",
        "sent_at": sent,
        "payload": {
            "product_id": 11, "product_name": "Пополнение", "amount": minor, "currency": "usd",
            "telegram_user_id": telegram, "purchase_id": id, "transaction_id": id * 10,
            "purchase_created_at": "2026-11-18T10:00:00Z"
        }
    })
    .to_string()
}

fn refund(id: i64, minor: i64) -> String {
    serde_json::json!({
        "name": "digital_product_refunded",
        "created_at": "2026-11-18T12:00:00Z",
        "sent_at": "2026-11-18T12:00:00Z",
        "payload": {
            "product_id": 11, "product_name": "Пополнение", "amount": minor, "currency": "usd",
            "telegram_user_id": BUYER, "purchase_id": id, "transaction_id": id * 10,
            "refund_reason": "telegram_refund", "refunded_at": "2026-11-18T12:00:00Z"
        }
    })
    .to_string()
}

fn subscription_event(name: &str, period: i64, expires: &str, minor: i64) -> String {
    serde_json::json!({
        "name": name,
        "created_at": "2026-11-18T10:00:00Z",
        "sent_at": "2026-11-18T10:00:00Z",
        "payload": {
            "subscription_name": "VPN", "subscription_id": 1644, "period_id": period,
            "period": "monthly", "type": "regular", "price": minor, "amount": minor - 30,
            "currency": "usd", "trb_user_id": "T-31326", "telegram_user_id": BUYER,
            "channel_id": 614, "channel_name": "vpn", "expires_at": expires
        }
    })
    .to_string()
}

async fn status(kit: &Kit, body: &str) -> u16 {
    kit.services
        .inbox()
        .handle(hook(body))
        .await
        .unwrap()
        .reply
        .status()
}

async fn payment(kit: &Kit, id: PaymentId) -> Payment {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().get(id).await.unwrap()
}

#[tokio::test]
async fn tribute_покупка_пополняет_баланс_и_сама_оплачивает_заказ() {
    let Some(kit) = tribute_kit(products(), serde_json::json!([])).await else {
        return;
    };
    let client_id = client(&kit.services, BUYER).await;
    let tariff = standard(&kit.services).await;
    let quote = kit
        .services
        .checkout()
        .quote_in(client_id, &cart(&tariff, 30, None), Channel::Bot)
        .await
        .unwrap();
    let offered = quote
        .methods
        .iter()
        .find(|m| m.provider == providers::TRIBUTE)
        .expect("Tribute должен быть среди способов");
    assert_eq!(offered.amount, Some(Amount::new(500, PayCurrency::Usd)));
    let started = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(&tariff, 30, None),
                provider: providers::TRIBUTE.into(),
                method: None,
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect {
        payment_id,
        order_id,
        ..
    } = started
    else {
        panic!("ожидалась ссылка на товар Tribute");
    };
    let waiting = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    assert_eq!(
        waiting.pay_url.as_deref(),
        Some("https://t.me/tribute/app?startapp=p11")
    );
    assert_eq!(waiting.provider_txid, None);

    let body = purchase(901, BUYER, 500, "2026-11-18T10:00:01Z");
    assert_eq!(status(&kit, &body).await, 200);
    let resent = purchase(901, BUYER, 500, "2026-11-18T10:05:00Z");
    assert_eq!(status(&kit, &resent).await, 200);

    let topup = by_txid(&kit, providers::TRIBUTE, "purchase:901")
        .await
        .expect("покупка Tribute записана пополнением");
    assert_eq!(topup.status, PaymentStatus::Paid);
    assert_eq!(topup.amount, Amount::new(500, PayCurrency::Usd));
    assert_eq!(
        order(&kit.services, topup.order_id).await.kind,
        OrderKind::TopUp
    );
    let bought = order_by_public_id(&kit.services, order_id).await;
    assert_eq!(bought.status, OrderStatus::Paid);
    assert_eq!(balance(&kit.services, client_id).await, 500 - 299);
    assert_eq!(
        payment(&kit, waiting.id).await.status,
        PaymentStatus::Canceled
    );
    let paid_with: Vec<String> = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments()
            .for_order(bought.id)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.status == PaymentStatus::Paid)
            .map(|p| p.provider.as_str().to_string())
            .collect()
    };
    assert_eq!(paid_with, [providers::BALANCE]);
    let outcomes: Vec<String> = inbox_rows(&kit)
        .await
        .into_iter()
        .map(|row| row.outcome)
        .collect();
    assert_eq!(outcomes, ["accepted", "duplicate"]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn tribute_пополнение_идёт_на_сумму_товара_и_не_задваивается() {
    let Some(kit) = tribute_kit(products(), serde_json::json!([])).await else {
        return;
    };
    let client_id = client(&kit.services, BUYER).await;
    let options = kit
        .services
        .checkout()
        .topup_options(client_id, Channel::Miniapp)
        .await
        .unwrap();
    let method = options
        .methods
        .iter()
        .find(|m| m.provider == providers::TRIBUTE)
        .unwrap();
    assert_eq!(
        method.amounts,
        Some(vec![
            Money::new(500, Currency::Usd),
            Money::new(1000, Currency::Usd)
        ])
    );
    let result = kit
        .services
        .checkout()
        .topup(
            client_id,
            Money::new(700, Currency::Usd),
            providers::TRIBUTE,
            None,
            Channel::Miniapp,
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидалась ссылка на товар");
    };
    let waiting = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    assert_eq!(waiting.amount, Amount::new(1000, PayCurrency::Usd));
    assert_eq!(
        waiting.pay_url.as_deref(),
        Some("https://t.me/tribute/app?startapp=p12")
    );
    assert_eq!(
        status(&kit, &purchase(902, BUYER, 1000, "2026-11-18T10:00:01Z")).await,
        200
    );
    let paid = payment(&kit, waiting.id).await;
    assert_eq!(paid.status, PaymentStatus::Paid);
    assert_eq!(paid.provider_txid.as_deref(), Some("purchase:902"));
    assert_eq!(balance(&kit.services, client_id).await, 1000);

    let refunded = refund(902, 1000);
    assert_eq!(status(&kit, &refunded).await, 200);
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        payment(&kit, waiting.id).await.status,
        PaymentStatus::Refunded
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn tribute_без_telegram_кнопки_нет_а_чужой_покупатель_ждёт_решения() {
    let Some(kit) = tribute_kit(products(), serde_json::json!([])).await else {
        return;
    };
    let web = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.clients()
            .create(&sn_store::NewClient {
                locale: sn_domain::client::Locale::Ru,
                registration_source: sn_domain::client::RegistrationSource::Web,
                note: None,
            })
            .await
            .unwrap()
            .id
    };
    let options = kit
        .services
        .checkout()
        .topup_options(web, Channel::Cabinet)
        .await
        .unwrap();
    assert!(
        options
            .methods
            .iter()
            .all(|m| m.provider != providers::TRIBUTE)
    );
    assert!(matches!(
        kit.services
            .checkout()
            .topup(
                web,
                Money::new(500, Currency::Usd),
                providers::TRIBUTE,
                None,
                Channel::Cabinet,
            )
            .await,
        Err(ServiceError::Rejected { .. })
    ));

    assert_eq!(
        status(
            &kit,
            &purchase(903, 77_000_001, 500, "2026-11-18T10:00:01Z")
        )
        .await,
        200
    );
    assert_eq!(
        by_txid(&kit, providers::TRIBUTE, "purchase:903").await,
        None
    );
    let decisions = kit
        .services
        .decisions()
        .summary(Scope {
            money: true,
            refunds: false,
        })
        .await
        .unwrap();
    let credits = decisions.credits.expect("группа денег без решения");
    assert_eq!(credits.count, 1);
    assert_eq!(credits.total, Some(Amount::new(500, PayCurrency::Usd)));
    let waiting = kit.services.decisions().credits(10).await.unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].provider_title, "Tribute");
    assert!(waiting[0].text.contains("77000001"), "{}", waiting[0].text);
    let owner = admin(&kit).await;
    assert!(matches!(
        kit.services
            .decisions()
            .close_credit(waiting[0].id, owner, "  ")
            .await,
        Err(ServiceError::Rejected { .. })
    ));
    kit.services
        .decisions()
        .close_credit(waiting[0].id, owner, "Зачислено вручную клиенту №5")
        .await
        .unwrap();
    assert!(
        kit.services
            .decisions()
            .credits(10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        kit.services
            .decisions()
            .close_credit(waiting[0].id, owner, "ещё раз")
            .await,
        Err(ServiceError::Rejected { .. })
    ));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn tribute_подписка_выдаёт_тариф_продлевает_его_а_без_привязки_идёт_на_баланс() {
    let Some(kit) = tribute_kit(products(), serde_json::json!([])).await else {
        return;
    };
    let tariff = standard(&kit.services).await;
    let month = tariff.periods.iter().find(|p| p.days == 30).unwrap().id;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::TRIBUTE),
                &serde_json::json!({
                    "enabled": true,
                    "config": {
                        "products": products(),
                        "plans": [{"period": 1547, "tariff": tariff.id, "tariff_period": month}]
                    }
                }),
                None,
            )
            .await
            .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
    let client_id = client(&kit.services, BUYER).await;

    let first = subscription_event("new_subscription", 1547, "2026-12-18T10:00:00Z", 299);
    assert_eq!(status(&kit, &first).await, 200);
    let paid = by_txid(
        &kit,
        providers::TRIBUTE,
        "sub:1644:5093118422:2026-12-18T10:00:00Z",
    )
    .await
    .expect("оплата подписки записана");
    let granted = order(&kit.services, paid.order_id).await;
    assert_eq!(granted.status, OrderStatus::Paid);
    assert!(matches!(
        granted.intent,
        PurchaseIntent::Subscription { tariff_id, period_id, .. }
            if tariff_id == tariff.id && period_id == month
    ));
    let done = kit.services.fulfillment().run(granted.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let subscription_id = order(&kit.services, granted.id)
        .await
        .subscription_id
        .expect("подписка выдана");

    let renewed = subscription_event("renewed_subscription", 1547, "2027-01-18T10:00:00Z", 299);
    assert_eq!(status(&kit, &renewed).await, 200);
    let again = by_txid(
        &kit,
        providers::TRIBUTE,
        "sub:1644:5093118422:2027-01-18T10:00:00Z",
    )
    .await
    .expect("продление записано");
    let renewal = order(&kit.services, again.order_id).await;
    assert_ne!(renewal.id, granted.id);
    assert_eq!(renewal.subscription_id, Some(subscription_id));
    assert!(matches!(
        renewal.intent,
        PurchaseIntent::Subscription { target: Some(target), .. } if target == subscription_id
    ));

    let cancelled = subscription_event("cancelled_subscription", 1547, "2027-01-18T10:00:00Z", 299);
    assert_eq!(status(&kit, &cancelled).await, 200);
    assert_eq!(balance(&kit.services, client_id).await, 0);

    let unbound = subscription_event("new_subscription", 9999, "2026-12-20T10:00:00Z", 450);
    assert_eq!(status(&kit, &unbound).await, 200);
    let topup = by_txid(
        &kit,
        providers::TRIBUTE,
        "sub:1644:5093118422:2026-12-20T10:00:00Z",
    )
    .await
    .unwrap();
    assert_eq!(
        order(&kit.services, topup.order_id).await.kind,
        OrderKind::TopUp
    );
    assert_eq!(topup.amount, Amount::new(420, PayCurrency::Usd));
    assert_eq!(balance(&kit.services, client_id).await, 420);
    assert_eq!(paid.amount, Amount::new(269, PayCurrency::Usd));

    let mut free: serde_json::Value = serde_json::from_str(&subscription_event(
        "new_subscription",
        1547,
        "2026-12-25T10:00:00Z",
        299,
    ))
    .unwrap();
    free["payload"]["amount"] = serde_json::json!(0);
    assert_eq!(status(&kit, &free.to_string()).await, 200);
    assert_eq!(
        by_txid(
            &kit,
            providers::TRIBUTE,
            "sub:1644:5093118422:2026-12-25T10:00:00Z"
        )
        .await,
        None
    );
    assert_eq!(balance(&kit.services, client_id).await, 420);
    let waiting = kit.services.decisions().credits(10).await.unwrap();
    assert_eq!(waiting.len(), 1);
    assert!(
        waiting[0].text.contains("нулевой суммой"),
        "{}",
        waiting[0].text
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn tribute_возврат_потраченного_пополнения_уходит_на_разбор() {
    let Some(kit) = tribute_kit(products(), serde_json::json!([])).await else {
        return;
    };
    let client_id = client(&kit.services, BUYER).await;
    let tariff = standard(&kit.services).await;
    assert_eq!(
        status(&kit, &purchase(904, BUYER, 500, "2026-11-18T10:00:01Z")).await,
        200
    );
    assert_eq!(balance(&kit.services, client_id).await, 500);
    buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    assert_eq!(balance(&kit.services, client_id).await, 201);
    assert_eq!(status(&kit, &refund(904, 500)).await, 200);
    assert_eq!(balance(&kit.services, client_id).await, 0);
    let waiting = kit.services.decisions().credits(10).await.unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].kind, NoticeKind::RefundReported);
    assert_eq!(waiting[0].amount, Some(Amount::new(299, PayCurrency::Usd)));
    assert_eq!(waiting[0].client_id, Some(client_id));
    assert!(
        waiting[0].text.contains("не хватило"),
        "{}",
        waiting[0].text
    );
    kit.db.cleanup().await;
}

async fn promise(kit: &Kit, products: serde_json::Value, percent: i64) {
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::TRIBUTE),
                &serde_json::json!({
                    "enabled": true,
                    "config": {"products": products, "plans": []},
                    "topup_bonus": {"enabled": percent > 0, "percent": percent}
                }),
                None,
            )
            .await
            .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
}

#[tokio::test]
async fn tribute_бонус_только_за_пополнение_а_не_за_покупку_тарифа() {
    let Some(kit) = tribute_kit(products(), serde_json::json!([])).await else {
        return;
    };
    promise(&kit, products(), 10).await;
    let client_id = client(&kit.services, BUYER).await;
    let tariff = standard(&kit.services).await;
    kit.services
        .checkout()
        .topup(
            client_id,
            Money::new(700, Currency::Usd),
            providers::TRIBUTE,
            None,
            Channel::Miniapp,
        )
        .await
        .unwrap();
    promise(&kit, products(), 0).await;
    status(&kit, &purchase(911, BUYER, 1000, "2026-11-18T10:00:01Z")).await;
    status(&kit, &purchase(911, BUYER, 1000, "2026-11-18T10:05:00Z")).await;
    assert_eq!(balance(&kit.services, client_id).await, 1_100);

    promise(&kit, products(), 10).await;
    status(&kit, &purchase(912, BUYER, 500, "2026-11-18T11:00:01Z")).await;
    assert_eq!(balance(&kit.services, client_id).await, 1_650);

    kit.services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(&tariff, 30, None),
                provider: providers::TRIBUTE.into(),
                method: None,
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await
        .unwrap();
    status(&kit, &purchase(913, BUYER, 500, "2026-11-18T12:00:01Z")).await;
    assert_eq!(balance(&kit.services, client_id).await, 1_650 + 500 - 299);

    status(&kit, &refund(911, 1000)).await;
    status(&kit, &refund(911, 1000)).await;
    assert_eq!(
        balance(&kit.services, client_id).await,
        1_650 + 500 - 299 - 1_100
    );
    kit.db.cleanup().await;
}
