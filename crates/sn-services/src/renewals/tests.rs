use super::*;
use crate::checkout::testkit::*;
use crate::checkout::{CartRequest, Channel, CheckoutRequest, CheckoutResult};
use crate::fulfillment::FulfillmentOutcome;
use crate::inbox::InboundWebhook;
use crate::services::{EnvFallbacks, ServiceDeps};
use crate::{SystemClock, settings};
use bytes::Bytes;
use serde_json::{Value, json};
use sn_domain::client::{IdentityKind, NewIdentity};
use sn_domain::payment::{PaymentStatus, providers};
use sn_domain::settings::{keys, secret_keys};
use sn_modules::ModuleRegistry;
use sn_payments::providers::{Balance, CryptoPay, Lava};
use sn_payments::{ProviderRegistry, WebhookRequest};
use sn_remnawave::fake::FakeRemnawave;
use sn_store::SecretBox;
use sn_store::testing::test_store;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WEBHOOK_KEY: &str = "lava-webhook-key-123";
const OFFER: &str = "836b9fc5-7ae9-4a27-9642-592bc44072b7";
const SUB_OFFER: &str = "9b2c3d4e-5f60-4a7b-8c9d-0e1f2a3b4c5d";
const SUB_PRODUCT: &str = "72d53efb-3696-469f-b856-f0d815748dd6";
const PARENT: &str = "c5a0cacc-3453-44b0-9532-aa492f1ba191";
const EMAIL: &str = "client@example.com";

fn renewal_id(n: u32) -> String {
    format!("d41db415-ad71-4f2a-8d8c-{n:012}")
}

async fn lava_kit(server: &MockServer) -> Option<Kit> {
    let db = test_store().await?;
    let pay = Arc::new(TestPay::default());
    let registry = ProviderRegistry::new(vec![
        Arc::new(CryptoPay::new()),
        pay.clone(),
        Arc::new(Lava::with_api_base(server.uri())),
        Arc::new(Balance),
    ]);
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(registry),
        modules: Arc::new(ModuleRegistry::new(Vec::new()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &json!(PUBLIC_URL), None)
            .await
            .unwrap();
        for (field, value) in [("api_key", "lava-api-key"), ("webhook_key", WEBHOOK_KEY)] {
            settings::write_secret(
                &mut conn,
                services.secrets(),
                &secret_keys::provider(providers::LAVA, field),
                Some(value),
                None,
            )
            .await
            .unwrap();
        }
    }
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    remna.set_squads(
        [1, 2, 3, 9]
            .into_iter()
            .map(|n| sn_remnawave::InternalSquad {
                uuid: Uuid::from_u128(n),
                name: format!("Сквад {n}"),
                view_position: 0,
                info: sn_remnawave::SquadInfo::default(),
                inbounds: Vec::new(),
            })
            .collect(),
    );
    services.set_remna(Some(remna.clone()));
    Some(Kit {
        db,
        services,
        remna,
        pay,
    })
}

async fn configure(kit: &Kit, recurring: bool, plans: Value) {
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::LAVA),
                &json!({
                    "enabled": true,
                    "config": {"offer_id": OFFER, "recurring": recurring, "plans": plans},
                    "methods": [{"code": "card"}, {"code": "subscription"}]
                }),
                None,
            )
            .await
            .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
}

fn plan(tariff: &Tariff, days: u32, amount: i64, periodicity: &str) -> Value {
    let period = tariff.periods.iter().find(|p| p.days == days).unwrap();
    json!({
        "product": SUB_PRODUCT, "offer": SUB_OFFER, "periodicity": periodicity, "name": "VPN Стандарт",
        "amount": amount, "currency": "USD", "tariff": tariff.id.0, "tariff_period": period.id.0
    })
}

async fn with_email(kit: &Kit, telegram: i64) -> ClientId {
    let id = client(&kit.services, telegram).await;
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.clients()
        .add_identity(
            id,
            &NewIdentity::new(IdentityKind::Email, EMAIL, true).unwrap(),
        )
        .await
        .unwrap();
    id
}

fn hook(body: Value) -> InboundWebhook {
    InboundWebhook {
        provider: providers::LAVA.into(),
        request: WebhookRequest {
            headers: vec![("X-Api-Key".into(), WEBHOOK_KEY.into())],
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
            remote_ip: "158.160.60.174".parse().ok(),
            route_suffix: None,
            query: None,
        },
    }
}

async fn deliver(kit: &Kit, body: Value) -> u16 {
    kit.services
        .inbox()
        .handle(hook(body))
        .await
        .unwrap()
        .reply
        .status()
}

async fn mount_invoice(server: &MockServer, answer: Value) {
    Mock::given(method("POST"))
        .and(path("/api/v3/invoice"))
        .and(body_partial_json(
            json!({"offerId": SUB_OFFER, "periodicity": "MONTHLY", "email": EMAIL}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(answer))
        .mount(server)
        .await;
}

async fn mount_first_paid(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/invoices/{PARENT}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": PARENT, "type": "RECURRING", "status": "COMPLETED",
            "receipt": {"amount": 2.99, "currency": "USD", "fee": 0.24}
        })))
        .mount(server)
        .await;
}

async fn mount_renewal(server: &MockServer, id: &str, days_after: i64, amount: f64) {
    let at = (Utc::now() + Duration::days(days_after)).to_rfc3339();
    Mock::given(method("GET"))
        .and(path(format!("/api/v2/invoices/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id, "type": "SUBSCRIPTION_RENEWAL", "datetime": at, "status": "COMPLETED",
            "receipt": {"amount": amount, "currency": "USD", "fee": 0.24},
            "parentInvoice": {"id": PARENT}
        })))
        .mount(server)
        .await;
}

fn renewal_event(id: &str) -> Value {
    json!({
        "eventType": "subscription.recurring.payment.success",
        "product": {"id": SUB_PRODUCT, "title": "VPN Стандарт"},
        "buyer": {"email": EMAIL},
        "contractId": id,
        "parentContractId": PARENT,
        "amount": 2.99,
        "currency": "USD",
        "timestamp": Utc::now().to_rfc3339(),
        "status": "subscription-active"
    })
}

async fn link_of(kit: &Kit) -> Link {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_as::<_, Link>(&format!(
        "SELECT {LINK_COLUMNS} FROM provider_subscriptions WHERE external_id = $1"
    ))
    .bind(PARENT)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn count(kit: &Kit, sql: &str) -> i64 {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn notices(kit: &Kit) -> Vec<String> {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_scalar::<_, String>("SELECT event_key FROM payment_notices ORDER BY id")
        .fetch_all(conn.conn())
        .await
        .unwrap()
}

async fn templates(kit: &Kit) -> Vec<String> {
    jobs(&kit.services)
        .await
        .into_iter()
        .filter_map(|job| match job {
            JobPayload::Notify { template, .. } => Some(template),
            _ => None,
        })
        .collect()
}

struct Subscribed {
    client: ClientId,
    tariff: Tariff,
    subscription: Subscription,
}

async fn subscribe(kit: &Kit, server: &MockServer) -> Subscribed {
    let tariff = standard(&kit.services).await;
    configure(kit, true, json!([plan(&tariff, 30, 299, "MONTHLY")])).await;
    let client = with_email(kit, 5_093_118_422).await;
    mount_invoice(
        server,
        json!({"id": PARENT, "status": "in-progress", "amountTotal": {"currency": "USD", "amount": 2.99}, "paymentUrl": "https://app.lava.top/payment/c5a0cacc"}),
    )
    .await;
    mount_first_paid(server).await;
    let quote = kit
        .services
        .checkout()
        .quote(client, &cart(&tariff, 30, None))
        .await
        .unwrap();
    let offered = quote
        .methods
        .iter()
        .find(|m| m.method.as_deref() == Some(RECURRING_METHOD))
        .expect("кнопка автопродления у срока с подпиской Lava.top");
    assert_eq!(
        offered.hint,
        "$2.99 сейчас и каждый месяц, пока не отключите"
    );
    let result = kit
        .services
        .checkout()
        .start(
            client,
            &CheckoutRequest {
                cart: cart(&tariff, 30, None),
                provider: providers::LAVA.into(),
                method: Some(RECURRING_METHOD.into()),
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { order_id, .. } = result else {
        panic!("ожидался счёт Lava.top: {result:?}");
    };
    let pending = link_of(kit).await;
    assert_eq!(pending.status(), LinkStatus::Pending);
    assert_eq!(pending.email, EMAIL);
    assert_eq!(pending.days, 30);
    assert_eq!(
        deliver(
            kit,
            json!({
                "eventType": "payment.success", "product": {"id": SUB_PRODUCT, "title": "VPN Стандарт"},
                "buyer": {"email": EMAIL}, "contractId": PARENT, "amount": 2.99, "currency": "USD",
                "timestamp": Utc::now().to_rfc3339(), "status": "subscription-active"
            })
        )
        .await,
        200
    );
    let order = order_by_public_id(&kit.services, order_id).await;
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = self::order(&kit.services, order.id).await;
    let subscription = self::subscription(&kit.services, &order).await;
    let user = remna_user(kit, &subscription).await;
    let paid_at = order.paid_at.unwrap();
    assert_eq!(
        user.expire_at,
        sn_domain::plan::graced_expire_at(
            sn_domain::plan::PlanKind::New,
            None,
            paid_at,
            30,
            Some(Grace::first())
        )
    );
    let link = link_of(kit).await;
    assert_eq!(link.status(), LinkStatus::Active);
    assert_eq!(link.subscription_id, Some(subscription.id.0));
    assert!(
        subscription.autorenew.enabled,
        "подписка считается подпиской с автопродлением, хотя продлевает Lava.top"
    );
    Subscribed {
        client,
        tariff,
        subscription,
    }
}

async fn run_renewal_order(kit: &Kit, txid: &str) -> Order {
    let mut conn = kit.services.store().conn().await.unwrap();
    let payment = conn
        .payments()
        .by_provider_txid(providers::LAVA, txid)
        .await
        .unwrap()
        .expect("платёж продления");
    drop(conn);
    assert_eq!(payment.status, PaymentStatus::Paid);
    let outcome = kit
        .services
        .fulfillment()
        .run(payment.order_id)
        .await
        .unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    order(&kit.services, payment.order_id).await
}

#[tokio::test]
async fn продление_lava_засчитывается_один_раз_при_повторах_параллели_и_сверке() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let subscribed = subscribe(&kit, &server).await;
    let sub = subscribed.subscription.id;

    let first = renewal_id(1);
    mount_renewal(&server, &first, 30, 2.99).await;
    let before = remna_user(&kit, &subscribed.subscription).await.expire_at;
    assert_eq!(deliver(&kit, renewal_event(&first)).await, 200);
    assert_eq!(deliver(&kit, renewal_event(&first)).await, 200);
    let renewed = run_renewal_order(&kit, &first).await;
    assert_eq!(renewed.kind, sn_domain::order::OrderKind::Renewal);
    assert_eq!(renewed.quote.total, Money::new(299, Currency::Usd));
    let now_sub = self::subscription(&kit.services, &renewed).await;
    assert_eq!(
        remna_user(&kit, &now_sub).await.expire_at,
        sn_domain::plan::graced_expire_at(
            sn_domain::plan::PlanKind::Renewal,
            Some(before),
            renewed.paid_at.unwrap(),
            30,
            Some(Grace::renewal())
        )
    );
    let txid_count = |txid: String| {
        format!(
            "SELECT count(*) FROM payments WHERE provider = 'lava' AND provider_txid = '{txid}'"
        )
    };
    assert_eq!(count(&kit, &txid_count(first.clone())).await, 1);

    let second = renewal_id(2);
    mount_renewal(&server, &second, 60, 2.99).await;
    let inbox = kit.services.inbox();
    let (a, b, c) = tokio::join!(
        inbox.handle(hook(renewal_event(&second))),
        inbox.handle(hook(renewal_event(&second))),
        inbox.handle(hook(renewal_event(&second)))
    );
    for answer in [a, b, c] {
        assert!(answer.is_ok(), "{answer:?}");
    }
    assert_eq!(count(&kit, &txid_count(second.clone())).await, 1);

    let third = renewal_id(3);
    mount_renewal(&server, &third, 90, 2.99).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/subscriptions/{PARENT}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": PARENT, "status": "COMPLETED", "periodicity": "MONTHLY", "subscriptionStatus": "ACTIVE",
            "expiredAt": (Utc::now() + Duration::days(120)).to_rfc3339(),
            "recurrentPayments": [
                {"id": first, "status": "COMPLETED", "amount": 2.99, "currency": "USD"},
                {"id": second, "status": "COMPLETED", "amount": 2.99, "currency": "USD"},
                {"id": third, "status": "COMPLETED", "amount": 2.99, "currency": "USD"}
            ]
        })))
        .mount(&server)
        .await;
    let renewals = kit.services.renewals();
    let (hooked, checked) = tokio::join!(
        inbox.handle(hook(renewal_event(&third))),
        renewals.check_subscription(sub)
    );
    assert!(hooked.is_ok(), "{hooked:?}");
    assert!(checked.is_ok(), "{checked:?}");
    assert_eq!(count(&kit, &txid_count(third.clone())).await, 1);
    assert_eq!(
        count(&kit, "SELECT count(*) FROM provider_subscription_charges").await,
        4,
        "первая оплата и три продления"
    );
    assert_eq!(
        count(
            &kit,
            "SELECT count(*) FROM orders WHERE channel = 'autorenew'"
        )
        .await,
        3
    );

    let soon = renewal_id(4);
    mount_renewal(&server, &soon, 92, 2.99).await;
    assert_eq!(deliver(&kit, renewal_event(&soon)).await, 200);
    assert_eq!(
        count(&kit, &txid_count(soon.clone())).await,
        0,
        "второе списание за неделю не продлевает само"
    );
    assert!(
        notices(&kit)
            .await
            .iter()
            .any(|k| k == &format!("renewal_repeat:{soon}"))
    );

    let view = kit.services.renewals().view(sub).await.unwrap().unwrap();
    assert_eq!(view.status, LinkStatus::Active);
    assert_eq!(view.charges, 4);
    assert_eq!(view.amount, Some(Money::new(299, Currency::Usd)));
    assert_eq!(view.email, "c***@example.com");
    assert_eq!(view.title, "Lava.top");
    let _ = subscribed.client;
}

#[tokio::test]
async fn отмена_без_ответа_lava_повторяется_и_закрывается() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let subscribed = subscribe(&kit, &server).await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/subscriptions"))
        .and(query_param("contractId", PARENT))
        .and(query_param("email", EMAIL))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/subscriptions"))
        .and(query_param("contractId", PARENT))
        .respond_with(ResponseTemplate::new(204))
        .with_priority(5)
        .mount(&server)
        .await;
    kit.services
        .subscriptions()
        .set_autorenew(
            subscribed.subscription.id,
            false,
            None,
            Actor::Client(subscribed.client),
        )
        .await
        .unwrap();
    let link = link_of(&kit).await;
    assert_eq!(
        link.status(),
        LinkStatus::Cancelling,
        "без ответа Lava.top не пишем «отключено»"
    );
    assert_eq!(link.cancel_attempts, 1);
    assert!(link.cancel_error.is_some());
    let view = kit
        .services
        .renewals()
        .view(subscribed.subscription.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.status, LinkStatus::Cancelling);
    let enabled = kit
        .services
        .subscriptions()
        .set_autorenew(
            subscribed.subscription.id,
            true,
            None,
            Actor::Client(subscribed.client),
        )
        .await;
    assert!(
        enabled.is_err(),
        "пока Lava.top не отменила, включить автопродление с баланса нельзя"
    );

    let report = kit
        .services
        .renewals()
        .sweep(Utc::now() + Duration::minutes(2))
        .await
        .unwrap();
    assert_eq!(report.checked, 1);
    assert_eq!(link_of(&kit).await.status(), LinkStatus::Cancelling);
    let report = kit
        .services
        .renewals()
        .sweep(Utc::now() + Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(report.cancelled, 1);
    let link = link_of(&kit).await;
    assert_eq!(link.status(), LinkStatus::Cancelled);
    assert_eq!(link.cancel_source.as_deref(), Some("client"));
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        let sub = conn
            .subscriptions()
            .get(subscribed.subscription.id)
            .await
            .unwrap();
        assert!(
            !sub.autorenew.enabled,
            "после отмены автопродление выключено и с баланса не спишется"
        );
    }
    assert!(
        !templates(&kit)
            .await
            .iter()
            .any(|t| t == CANCELLED_TEMPLATE),
        "клиент отключил сам: письма не нужно"
    );
    assert_eq!(
        deliver(
            &kit,
            json!({
                "eventType": "subscription.cancelled", "contractId": PARENT,
                "product": {"id": SUB_PRODUCT, "title": "VPN Стандарт"}, "buyer": {"email": EMAIL},
                "cancelledAt": Utc::now().to_rfc3339(), "willExpireAt": (Utc::now() + Duration::days(30)).to_rfc3339()
            })
        )
        .await,
        200
    );
    assert_eq!(link_of(&kit).await.status(), LinkStatus::Cancelled);
    let late = renewal_id(7);
    mount_renewal(&server, &late, 30, 2.99).await;
    assert_eq!(deliver(&kit, renewal_event(&late)).await, 200);
    assert_eq!(
        count(
            &kit,
            &format!("SELECT count(*) FROM payments WHERE provider_txid = '{late}'")
        )
        .await,
        1,
        "списание до отмены всё равно продлевает подписку"
    );
    assert!(
        templates(&kit)
            .await
            .iter()
            .any(|t| t == crate::autorenew::CHARGED_AFTER_OFF_TEMPLATE)
    );
}

#[tokio::test]
async fn неудача_и_отмена_со_стороны_lava_закрывают_автопродление() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let subscribed = subscribe(&kit, &server).await;
    let first = renewal_id(1);
    mount_renewal(&server, &first, 30, 2.99).await;
    assert_eq!(deliver(&kit, renewal_event(&first)).await, 200);
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/subscriptions/{PARENT}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": PARENT, "subscriptionStatus": "ACTIVE", "recurrentPayments": []
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/subscriptions/{PARENT}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": PARENT, "subscriptionStatus": "FAILED", "recurrentPayments": []
        })))
        .with_priority(5)
        .mount(&server)
        .await;
    let failed = || {
        json!({
            "eventType": "subscription.recurring.payment.failed", "contractId": renewal_id(2),
            "parentContractId": PARENT, "product": {"id": SUB_PRODUCT, "title": "VPN Стандарт"},
            "buyer": {"email": EMAIL}, "amount": 2.99, "currency": "USD", "status": "subscription-failed",
            "errorMessage": "Not sufficient funds", "timestamp": Utc::now().to_rfc3339()
        })
    };
    assert_eq!(deliver(&kit, failed()).await, 200);
    assert_eq!(
        link_of(&kit).await.status(),
        LinkStatus::Active,
        "пока Lava.top держит подписку, неудача не окончательная"
    );
    assert_eq!(deliver(&kit, failed()).await, 200);
    let link = link_of(&kit).await;
    assert_eq!(link.status(), LinkStatus::Failed);
    assert!(
        templates(&kit)
            .await
            .iter()
            .any(|t| t == TURNED_OFF_TEMPLATE)
    );
    assert!(
        kit.services
            .renewals()
            .view(subscribed.subscription.id)
            .await
            .unwrap()
            .is_some_and(|v| v.status == LinkStatus::Failed)
    );
    let quote = kit
        .services
        .checkout()
        .quote(
            subscribed.client,
            &cart(&subscribed.tariff, 30, Some(&subscribed.subscription)),
        )
        .await
        .unwrap();
    assert!(
        quote
            .methods
            .iter()
            .any(|m| m.method.as_deref() == Some(RECURRING_METHOD)),
        "после неудачи автопродление можно оформить заново"
    );
}

#[tokio::test]
async fn отмена_в_lava_по_номеру_продления_и_уведомление_клиента() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    subscribe(&kit, &server).await;
    let first = renewal_id(1);
    mount_renewal(&server, &first, 30, 2.99).await;
    assert_eq!(deliver(&kit, renewal_event(&first)).await, 200);
    let until = Utc::now() + Duration::days(60);
    assert_eq!(
        deliver(
            &kit,
            json!({
                "eventType": "subscription.cancelled", "contractId": first,
                "product": {"id": SUB_PRODUCT, "title": "VPN Стандарт"}, "buyer": {"email": EMAIL},
                "cancelledAt": Utc::now().to_rfc3339(), "willExpireAt": until.to_rfc3339()
            })
        )
        .await,
        200
    );
    let link = link_of(&kit).await;
    assert_eq!(link.status(), LinkStatus::Cancelled);
    assert_eq!(link.cancel_source.as_deref(), Some("provider"));
    assert_eq!(
        link.paid_until.map(|t| t.timestamp()),
        Some(until.timestamp())
    );
    assert!(
        templates(&kit)
            .await
            .iter()
            .any(|t| t == CANCELLED_TEMPLATE)
    );
}

#[tokio::test]
async fn продление_подписки_с_другим_тарифом_уходит_на_баланс_и_отменяется() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let subscribed = subscribe(&kit, &server).await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/subscriptions"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let other = {
        let category = category(&kit.services, "Другая").await;
        tariff(
            &kit.services,
            category,
            "Другой",
            sn_domain::traffic::TrafficPolicy::UNLIMITED,
            vec![squad(1)],
        )
        .await
    };
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE subscriptions SET tariff_id = $2 WHERE id = $1")
            .bind(subscribed.subscription.id.0)
            .bind(other.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let before = balance(&kit.services, subscribed.client).await;
    let first = renewal_id(1);
    mount_renewal(&server, &first, 30, 2.99).await;
    assert_eq!(deliver(&kit, renewal_event(&first)).await, 200);
    assert_eq!(
        balance(&kit.services, subscribed.client).await,
        before + 299
    );
    assert!(
        notices(&kit)
            .await
            .iter()
            .any(|k| k == &format!("renewal_balance:{first}"))
    );
    assert_eq!(link_of(&kit).await.status(), LinkStatus::Cancelling);
    kit.services
        .renewals()
        .sweep(Utc::now() + Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(link_of(&kit).await.status(), LinkStatus::Cancelled);
    assert_eq!(deliver(&kit, renewal_event(&first)).await, 200);
    assert_eq!(
        balance(&kit.services, subscribed.client).await,
        before + 299,
        "повтор не зачисляет второй раз"
    );
}

#[tokio::test]
async fn кнопка_автопродления_только_у_срока_с_той_же_ценой() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let tariff = standard(&kit.services).await;
    let client = with_email(&kit, 5_093_118_500).await;
    configure(&kit, true, json!([plan(&tariff, 30, 299, "MONTHLY")])).await;
    assert!(offers(&kit, client, cart(&tariff, 30, None)).await);
    assert!(
        !offers(&kit, client, cart(&tariff, 90, None)).await,
        "у срока без подписки Lava.top кнопки нет"
    );
    let mut with_devices = cart(&tariff, 30, None);
    with_devices.extra_devices = 1;
    assert!(
        !offers(&kit, client, with_devices).await,
        "доп. устройства в цену подписки не входят"
    );
    configure(&kit, true, json!([plan(&tariff, 30, 349, "MONTHLY")])).await;
    assert!(
        !offers(&kit, client, cart(&tariff, 30, None)).await,
        "цена в Lava.top другая"
    );
    configure(
        &kit,
        true,
        json!([plan(&tariff, 30, 299, "PERIOD_90_DAYS")]),
    )
    .await;
    assert!(
        !offers(&kit, client, cart(&tariff, 30, None)).await,
        "периодичность не подходит к сроку"
    );
    configure(&kit, false, json!([plan(&tariff, 30, 299, "MONTHLY")])).await;
    assert!(
        !offers(&kit, client, cart(&tariff, 30, None)).await,
        "автопродление выключено"
    );
}

async fn offers(kit: &Kit, client: ClientId, cart: CartRequest) -> bool {
    kit.services
        .checkout()
        .quote(client, &cart)
        .await
        .unwrap()
        .methods
        .iter()
        .any(|m| m.method.as_deref() == Some(RECURRING_METHOD))
}

#[tokio::test]
async fn с_подпиской_lava_баланс_не_списывается_и_устройства_не_докупаются() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let subscribed = subscribe(&kit, &server).await;
    credit(&kit.services, subscribed.client, 10_000).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE subscriptions SET autorenew_enabled = true WHERE id = $1")
            .bind(subscribed.subscription.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let expire = remna_user(&kit, &subscribed.subscription).await.expire_at;
    let report = kit
        .services
        .autorenew()
        .run_due(expire - Duration::hours(12))
        .await
        .unwrap();
    assert_eq!(report.charged, 0);
    assert_eq!(balance(&kit.services, subscribed.client).await, 10_000);
    let mut more = cart(&subscribed.tariff, 30, Some(&subscribed.subscription));
    more.extra_devices = 1;
    let refused = kit
        .services
        .checkout()
        .start(
            subscribed.client,
            &CheckoutRequest {
                cart: more,
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await;
    assert!(
        matches!(&refused, Err(ServiceError::Rejected { code, .. }) if *code == "renewal.devices"),
        "{refused:?}"
    );
    let quote = kit
        .services
        .checkout()
        .quote(
            subscribed.client,
            &cart(&subscribed.tariff, 30, Some(&subscribed.subscription)),
        )
        .await
        .unwrap();
    assert!(
        quote
            .methods
            .iter()
            .all(|m| m.method.as_deref() != Some(RECURRING_METHOD)),
        "второе автопродление на ту же подписку не предлагаем"
    );
}

#[tokio::test]
async fn чарджбэк_отменяет_подписку_lava() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let subscribed = subscribe(&kit, &server).await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/subscriptions"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/invoices"))
        .and(query_param("buyerEmail", EMAIL))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": PARENT, "type": "SUBSCRIPTION_FIRST_INVOICE", "status": "COMPLETED",
                "receipt": {"amount": 2.99, "currency": "USD"}, "buyer": {"email": EMAIL},
                "product": {"name": "VPN Стандарт", "offer": "Месяц"}
            }],
            "page": 1, "size": 20, "total": 1
        })))
        .mount(&server)
        .await;
    assert_eq!(
        deliver(
            &kit,
            json!({
                "event_id": "cb-1", "event_type": "chargeback.initiated", "created_at": Utc::now().to_rfc3339(),
                "data": {
                    "chargeback_id": "cb-1", "parent_invoice_id": PARENT, "amount": 2.99, "currency": "USD",
                    "product": {"product_name": "VPN Стандарт", "tier_id": SUB_OFFER},
                    "customer_email": EMAIL, "subscription_cancelled": false
                }
            })
        )
        .await,
        200
    );
    let link = link_of(&kit).await;
    assert_eq!(link.status(), LinkStatus::Cancelled);
    assert_eq!(link.cancel_source.as_deref(), Some("chargeback"));
    let _ = subscribed;
}

#[tokio::test]
async fn возврат_в_lava_сразу_сверяет_подписку() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let subscribed = subscribe(&kit, &server).await;
    Mock::given(method("GET"))
        .and(path("/api/v2/invoices"))
        .and(query_param("buyerEmail", EMAIL))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": PARENT, "type": "SUBSCRIPTION_FIRST_INVOICE", "status": "COMPLETED",
                "receipt": {"amount": 2.99, "currency": "USD"}, "buyer": {"email": EMAIL},
                "product": {"name": "VPN Стандарт", "offer": "Месяц"}
            }],
            "page": 1, "size": 20, "total": 1
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/subscriptions/{PARENT}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": PARENT, "subscriptionStatus": "CANCELLED", "recurrentPayments": []
        })))
        .mount(&server)
        .await;
    let due =
        "SELECT count(*) FROM provider_subscriptions WHERE status = 'active' AND check_at <= now()";
    assert_eq!(count(&kit, due).await, 0);
    assert_eq!(
        deliver(
            &kit,
            json!({
                "event_id": "rf-1", "event_type": "refund.success", "created_at": Utc::now().to_rfc3339(),
                "data": {
                    "refund_id": "rf-1", "refund_type": "full", "initiator": "creator",
                    "amount": 2.99, "currency": "USD",
                    "product": {"product_name": "VPN Стандарт", "tier_id": SUB_OFFER},
                    "customer_email": EMAIL, "parent_invoice_id": PARENT,
                    "balance_impact": {"debited_amount": 2.99, "currency": "USD"},
                    "subscription_cancelled": true
                }
            })
        )
        .await,
        200
    );
    assert_eq!(
        count(&kit, due).await,
        1,
        "после возврата сверка назначена сразу"
    );
    assert_eq!(link_of(&kit).await.status(), LinkStatus::Active);
    let report = kit.services.renewals().sweep(Utc::now()).await.unwrap();
    assert_eq!(report.checked, 1);
    let link = link_of(&kit).await;
    assert_eq!(link.status(), LinkStatus::Cancelled);
    assert_eq!(link.cancel_source.as_deref(), Some("provider"));
    assert!(
        !kit.services
            .store()
            .conn()
            .await
            .unwrap()
            .subscriptions()
            .get(subscribed.subscription.id)
            .await
            .unwrap()
            .autorenew
            .enabled
    );
}

#[tokio::test]
async fn годовая_подписка_lava_на_360_дней_даёт_доступ_до_следующего_списания() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let tariff = standard(&kit.services).await;
    let tariff = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE tariff_periods SET days = 360, price_usd_minor = 4320 WHERE tariff_id = $1 AND days = 90",
        )
        .bind(tariff.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
        conn.tariffs().get(tariff.id).await.unwrap()
    };
    configure(&kit, true, json!([plan(&tariff, 360, 4320, "PERIOD_YEAR")])).await;
    {
        let registry = kit.services.payments();
        let lava = registry.get(providers::LAVA).unwrap();
        let config = registry.config(providers::LAVA);
        let mut conn = kit.services.store().conn().await.unwrap();
        check_plans(&mut conn, lava.as_ref(), &config)
            .await
            .unwrap();
    }
    let client = with_email(&kit, 5_093_118_360).await;
    Mock::given(method("POST"))
        .and(path("/api/v3/invoice"))
        .and(body_partial_json(
            json!({"offerId": SUB_OFFER, "periodicity": "PERIOD_YEAR", "email": EMAIL}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": PARENT, "status": "in-progress", "amountTotal": {"currency": "USD", "amount": 43.2},
            "paymentUrl": "https://app.lava.top/payment/c5a0cacc"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/invoices/{PARENT}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": PARENT, "type": "RECURRING", "status": "COMPLETED",
            "receipt": {"amount": 43.2, "currency": "USD", "fee": 3.1}
        })))
        .mount(&server)
        .await;
    let quote = kit
        .services
        .checkout()
        .quote(client, &cart(&tariff, 360, None))
        .await
        .unwrap();
    let offered = quote
        .methods
        .iter()
        .find(|m| m.method.as_deref() == Some(RECURRING_METHOD))
        .expect("у срока на 360 дней есть кнопка годовой подписки Lava.top");
    assert_eq!(offered.hint, "$43.20 сейчас и раз в год, пока не отключите");
    let result = kit
        .services
        .checkout()
        .start(
            client,
            &CheckoutRequest {
                cart: cart(&tariff, 360, None),
                provider: providers::LAVA.into(),
                method: Some(RECURRING_METHOD.into()),
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { order_id, .. } = result else {
        panic!("ожидался счёт Lava.top: {result:?}");
    };
    assert_eq!(
        deliver(
            &kit,
            json!({
                "eventType": "payment.success", "product": {"id": SUB_PRODUCT, "title": "VPN Стандарт"},
                "buyer": {"email": EMAIL}, "contractId": PARENT, "amount": 43.2, "currency": "USD",
                "timestamp": Utc::now().to_rfc3339(), "status": "subscription-active"
            })
        )
        .await,
        200
    );
    let order = order_by_public_id(&kit.services, order_id).await;
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = self::order(&kit.services, order.id).await;
    let subscription = self::subscription(&kit.services, &order).await;
    let paid_at = order.paid_at.unwrap();
    let year = Cycle::Months(12);
    let first = remna_user(&kit, &subscription).await.expire_at;
    assert_eq!(
        first,
        sn_domain::plan::graced_expire_at(
            sn_domain::plan::PlanKind::New,
            None,
            paid_at,
            360,
            Some(Grace::first().every(Some(year), Some(paid_at)))
        )
    );
    assert!(
        first >= year.after(paid_at) + Duration::hours(48) - Duration::seconds(1),
        "доступ до следующего списания Lava.top и ещё двое суток, а не 360 дней: {first}"
    );
    let link = link_of(&kit).await;
    assert_eq!(link.status(), LinkStatus::Active);
    assert_eq!(link.days, 360);
    let next = link.next_charge_at.unwrap();
    assert!(
        (next - year.after(paid_at)).num_seconds().abs() <= 1,
        "{next}"
    );

    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE provider_subscription_charges SET charged_at = charged_at - interval '1 year' \
             WHERE provider_subscription_id = $1",
        )
        .bind(link.id)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let renewal = renewal_id(360);
    mount_renewal(&server, &renewal, 0, 43.2).await;
    let mut event = renewal_event(&renewal);
    event["amount"] = json!(43.2);
    assert_eq!(deliver(&kit, event).await, 200);
    let renewed = run_renewal_order(&kit, &renewal).await;
    let charged_at: DateTime<Utc> = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT charged_at FROM provider_subscription_charges WHERE order_id = $1",
        )
        .bind(renewed.id.0)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    };
    let now_sub = self::subscription(&kit.services, &renewed).await;
    let second = remna_user(&kit, &now_sub).await.expire_at;
    assert_eq!(
        second,
        sn_domain::plan::graced_expire_at(
            sn_domain::plan::PlanKind::Renewal,
            Some(first),
            renewed.paid_at.unwrap(),
            360,
            Some(Grace::renewal().every(Some(year), Some(charged_at)))
        )
    );
    let at = charged_at.min(renewed.paid_at.unwrap());
    assert_eq!(
        (second - first).num_days(),
        (year.after(at) - at).num_days(),
        "списание добавляет год Lava.top, а оставшееся не сгорает"
    );
    let next = link_of(&kit).await.next_charge_at.unwrap();
    assert!((next - year.after(at)).num_seconds().abs() <= 1, "{next}");
}

#[tokio::test]
async fn без_оффера_lava_предлагает_только_карту_с_автопродлением() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let tariff = standard(&kit.services).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::LAVA),
                &json!({
                    "enabled": true,
                    "config": {"recurring": true, "plans": [plan(&tariff, 30, 299, "MONTHLY")]},
                    "methods": [{"code": "card"}, {"code": "subscription"}]
                }),
                None,
            )
            .await
            .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
    let client = with_email(&kit, 5_093_118_700).await;
    let lava = |quote: &crate::checkout::QuoteView| -> Vec<Option<String>> {
        quote
            .methods
            .iter()
            .filter(|m| m.provider == providers::LAVA)
            .map(|m| m.method.clone())
            .collect()
    };
    let bound = kit
        .services
        .checkout()
        .quote(client, &cart(&tariff, 30, None))
        .await
        .unwrap();
    assert_eq!(lava(&bound), vec![Some(RECURRING_METHOD.to_string())]);
    let other = kit
        .services
        .checkout()
        .quote(client, &cart(&tariff, 90, None))
        .await
        .unwrap();
    assert!(lava(&other).is_empty(), "{:?}", lava(&other));
}

#[tokio::test]
async fn блокировка_клиента_отменяет_подписку_lava() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let subscribed = subscribe(&kit, &server).await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/subscriptions"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    kit.services
        .clients()
        .block(subscribed.client, Some("тест"), Actor::System)
        .await
        .unwrap();
    let link = link_of(&kit).await;
    assert_eq!(link.status(), LinkStatus::Cancelled);
    assert_eq!(link.cancel_source.as_deref(), Some("block"));
}

#[test]
fn проверка_после_списания_не_чаще_раза_в_сутки_и_повтор_через_шесть_часов() {
    let now = Utc::now();
    assert_eq!(
        next_check(now + Duration::days(30), now),
        now + Duration::hours(24)
    );
    assert_eq!(
        next_check(now - Duration::hours(10), now),
        now + Duration::hours(30 - 10)
    );
    assert_eq!(
        next_check(now - Duration::hours(40), now),
        now + Duration::hours(6)
    );
    assert!(quick_repeat(Some(now), now + Duration::days(6)));
    assert!(!quick_repeat(Some(now), now + Duration::days(28)));
    assert!(!quick_repeat(None, now));
    assert_eq!(mask_email("alex@example.com"), "a***@example.com");
}

async fn journal_id(kit: &Kit, event_key: &str) -> i64 {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM webhook_inbox WHERE source = 'lava' AND parsed->>'event_key' = $1",
    )
    .bind(event_key)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn legacy_contract(
    kit: &Kit,
    client: ClientId,
    tariff: &Tariff,
    subscription: &Subscription,
) -> i64 {
    let period = tariff.periods.iter().find(|p| p.days == 30).unwrap();
    let intent = serde_json::to_value(PurchaseIntent::Subscription {
        tariff_id: tariff.id,
        period_id: period.id,
        extra_devices: 0,
        target: Some(subscription.id),
    })
    .unwrap();
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO legacy_invoices (source, old_id, provider, old_provider, external_id, order_ref, \
         client_id, intent, label, amount_minor, currency, created_at, recurring) \
         VALUES ('stealthnet', 'pay-old-1', 'lava', 'lavatop', $1, 'ord-old-1', $2, $3, \
         'Тариф «Стандарт»: подписка Lava.top с ежемесячным списанием', 299, 'USD', \
         now() - interval '90 days', true) RETURNING id",
    )
    .bind(PARENT)
    .bind(client.0)
    .bind(intent)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn notice_of(kit: &Kit, txid: &str) -> (i64, Option<String>, Option<i64>) {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT id, resolution, resolved_by FROM payment_notices \
         WHERE provider = 'lava' AND event_key = $1 AND kind = 'manual_credit'",
    )
    .bind(format!("renewal:{txid}"))
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

#[tokio::test]
async fn списание_по_договору_старой_панели_проводится_повтором_из_журнала() {
    let server = MockServer::start().await;
    let Some(kit) = lava_kit(&server).await else {
        return;
    };
    let tariff = standard(&kit.services).await;
    configure(&kit, true, json!([plan(&tariff, 30, 299, "MONTHLY")])).await;
    let client = with_email(&kit, 5_093_118_501).await;
    let subscription = provision(&kit, client, &tariff, 30).await;
    let admin = kit
        .services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0;
    let inbox = kit.services.inbox();

    let first = renewal_id(11);
    let second = renewal_id(12);
    for charge in [&first, &second] {
        mount_renewal(&server, charge, 0, 2.99).await;
        assert_eq!(deliver(&kit, renewal_event(charge)).await, 200);
    }
    let first_id = journal_id(&kit, &format!("renewal:{first}")).await;
    let second_id = journal_id(&kit, &format!("renewal:{second}")).await;
    let waiting = inbox.journal_card(first_id).await.unwrap();
    assert_eq!(waiting.item.outcome, sn_store::InboxOutcome::Ignored);
    assert!(waiting.item.retry.available);
    assert!(
        waiting
            .advice
            .as_deref()
            .is_some_and(|text| text.contains("Обработать ещё раз")),
        "{:?}",
        waiting.advice
    );
    assert_eq!(
        count(
            &kit,
            "SELECT count(*) FROM payment_notices WHERE kind = 'manual_credit' AND resolved_at IS NULL"
        )
        .await,
        2
    );

    let contract = legacy_contract(&kit, client, &tariff, &subscription).await;
    let (by_hand, _, _) = notice_of(&kit, &second).await;
    kit.services
        .decisions()
        .close_credit(by_hand, admin, "зачислили вручную")
        .await
        .unwrap();
    let refused = inbox.retry_webhook(second_id, admin).await.unwrap_err();
    assert_eq!(refused.code(), crate::inbox::RETRY_NOTICE_CLOSED);
    assert!(
        refused.to_string().contains("зачислили вручную"),
        "{refused}"
    );
    assert_eq!(
        inbox.journal_card(second_id).await.unwrap().item.outcome,
        sn_store::InboxOutcome::Ignored
    );

    let before = remna_user(&kit, &subscription).await.expire_at;
    let card = inbox.retry_webhook(first_id, admin).await.unwrap();
    assert_eq!(card.item.outcome, sn_store::InboxOutcome::Accepted);
    assert!(card.item.legacy_invoice);
    assert!(!card.item.retry.available);
    let renewed = run_renewal_order(&kit, &first).await;
    assert_eq!(renewed.kind, sn_domain::order::OrderKind::Renewal);
    assert_eq!(renewed.subscription_id, Some(subscription.id));
    assert!(remna_user(&kit, &subscription).await.expire_at > before);
    assert_eq!(
        count(
            &kit,
            &format!("SELECT count(*) FROM webhook_inbox WHERE id = {first_id} AND legacy_invoice_id = {contract}")
        )
        .await,
        1
    );
    let (_, resolution, resolved_by) = notice_of(&kit, &first).await;
    assert_eq!(resolution.as_deref(), Some(crate::inbox::LEGACY_CREDITED));
    assert_eq!(resolved_by, Some(admin.0));
    assert!(inbox.retry_webhook(first_id, admin).await.is_err());
    let txid_count = |txid: &str| {
        format!(
            "SELECT count(*) FROM payments WHERE provider = 'lava' AND provider_txid = '{txid}'"
        )
    };
    assert_eq!(count(&kit, &txid_count(&first)).await, 1);
    assert_eq!(count(&kit, &txid_count(&second)).await, 0);

    let third = renewal_id(13);
    mount_renewal(&server, &third, 30, 2.99).await;
    assert_eq!(deliver(&kit, renewal_event(&third)).await, 200);
    assert_eq!(count(&kit, &txid_count(&third)).await, 1);
    let (_, resolution, resolved_by) = notice_of(&kit, &third).await;
    assert_eq!(resolution.as_deref(), Some(crate::inbox::LEGACY_CREDITED));
    assert_eq!(resolved_by, None);
    assert_eq!(
        count(
            &kit,
            "SELECT count(*) FROM payment_notices WHERE kind = 'manual_credit' AND resolved_at IS NULL"
        )
        .await,
        0,
        "списания по договору старой панели не просят зачислять вручную"
    );
}
