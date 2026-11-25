use crate::{AppState, ServerConfig, router};
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use chrono::Duration;
use serde_json::{Value, json};
use sn_domain::access::{AdminRole, Permission};
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::{AdminId, ClientId, PeriodId, TariffId};
use sn_domain::jobs::JobPayload;
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::payment::{
    AUTORENEW_METHOD, NewPayment, Payment, PaymentStatus, ProviderId, RefundSubscriptionAction,
    providers,
};
use sn_domain::pricing::Quote;
use sn_domain::subscription::{AutoRenewState, SubscriptionKind};
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_services::{EnvFallbacks, ServiceDeps, Services, SystemClock};
use sn_store::testing::{TestDb, test_store};
use sn_store::{Db, NewAdmin, NewProviderRefund, NewSubscription, ProviderRefundStatus, SecretBox};
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "очень-надёжный-пароль";
const UNCLEAR: &str = "исход автосписания так и не выяснен: проверьте платёж в кабинете кассы";

struct Harness {
    db: TestDb,
    state: AppState,
}

async fn harness() -> Option<Harness> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    Some(Harness {
        db,
        state: AppState::new(ServerConfig::default(), services),
    })
}

fn flat_quote(total: Money, days: u32) -> Quote {
    Quote {
        currency: total.currency,
        lines: Vec::new(),
        subtotal: total,
        discount: Money::zero(total.currency),
        total,
        stars: None,
        days,
        extra_devices: 0,
    }
}

impl Harness {
    fn services(&self) -> &Services {
        &self.state.services
    }

    async fn send(&self, method: Method, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = router(self.state.clone())
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn get(&self, path: &str, token: &str) -> Value {
        let (status, body) = self.send(Method::GET, path, Some(token)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        body
    }

    async fn login(&self, email: &str) -> String {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/admin/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "email": email, "password": PASSWORD }).to_string(),
            ))
            .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        body["token"].as_str().unwrap().to_string()
    }

    async fn owner(&self) -> String {
        self.services()
            .auth()
            .upsert_owner("owner@example.com", PASSWORD)
            .await
            .unwrap();
        self.login("owner@example.com").await
    }

    async fn staff(&self, email: &str, role: AdminRole, extra: &[Permission]) -> (String, AdminId) {
        let hash = sn_services::auth::hash_password(PASSWORD).unwrap();
        let id = {
            let mut conn = self.services().store().conn().await.unwrap();
            let created = conn
                .admins()
                .create(&NewAdmin {
                    email: email.into(),
                    password_hash: hash,
                    role,
                    must_change_password: false,
                })
                .await
                .unwrap();
            conn.admins()
                .set_role(created.id, role, extra)
                .await
                .unwrap();
            created.id
        };
        (self.login(email).await, id)
    }

    async fn client(&self, telegram_id: i64) -> ClientId {
        self.services()
            .clients()
            .ensure_telegram(
                &TelegramProfile {
                    id: telegram_id,
                    username: None,
                    first_name: Some("Алексей".into()),
                    last_name: None,
                    language_code: Some("ru".into()),
                    is_premium: false,
                },
                RegistrationSource::Bot,
                None,
            )
            .await
            .unwrap()
            .0
            .id
    }

    async fn charge(
        &self,
        client_id: ClientId,
        minor: i64,
        job: &str,
        reason: Option<&str>,
        closes_in_hours: i64,
    ) -> Payment {
        let total = Money::new(minor, Currency::Usd);
        let now = self.services().now();
        let mut tx = self.services().store().begin().await.unwrap();
        let subscription = tx
            .subscriptions()
            .create(&NewSubscription {
                client_id,
                tariff_id: None,
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState::default(),
            })
            .await
            .unwrap();
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id,
                subscription_id: Some(subscription.id),
                kind: OrderKind::Renewal,
                intent: PurchaseIntent::Subscription {
                    tariff_id: TariffId(1),
                    period_id: PeriodId(1),
                    extra_devices: 0,
                    target: Some(subscription.id),
                },
                quote: flat_quote(total, 30),
                snapshot: None,
            })
            .await
            .unwrap();
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id,
                provider: ProviderId::from(providers::YOOKASSA),
                method: Some(AUTORENEW_METHOD.into()),
                amount: total.into(),
                expires_at: Some(now + Duration::hours(closes_in_hours)),
            })
            .await
            .unwrap();
        let job_id = tx
            .jobs()
            .enqueue(
                &JobPayload::PollPayment {
                    payment_id: payment.id,
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        let mut conn = self.services().store().conn().await.unwrap();
        let reason = reason.unwrap_or_default();
        match job {
            "review" => conn.jobs().review(job_id, reason).await.unwrap(),
            _ => conn.jobs().retry(job_id, reason, now).await.unwrap(),
        }
        payment
    }

    async fn review_order(&self, client_id: ClientId) {
        let amount = Money::new(699, Currency::Usd);
        let mut tx = self.services().store().begin().await.unwrap();
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id,
                subscription_id: None,
                kind: OrderKind::TopUp,
                intent: PurchaseIntent::TopUp { amount },
                quote: flat_quote(amount, 0),
                snapshot: None,
            })
            .await
            .unwrap();
        tx.orders()
            .transition(order.id, OrderStatus::Paid)
            .await
            .unwrap();
        tx.orders()
            .mark_review(order.id, "модуль не подтвердил заказ")
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn stars_review(&self, charge: &str, payer: i64, stars: i64) {
        let payload = json!({
            "event": "successful_payment",
            "total_amount": stars,
            "currency": "XTR",
            "telegram_payment_charge_id": charge,
            "telegram_user_id": payer,
        });
        let row = format!(
            "{charge}\tsuccessful_payment\t{payload}\tupdate\treview\t1\t2026-11-17 09:00:00+00\tсумма не совпала со счётом\n"
        );
        let mut conn = self.services().store().conn().await.unwrap();
        let mut copy = conn
            .conn()
            .copy_in_raw(
                "COPY bot_star_events (charge_id, event, payload, source, status, attempts, \
                 next_attempt_at, last_error) FROM STDIN",
            )
            .await
            .unwrap();
        copy.send(row.as_bytes()).await.unwrap();
        assert_eq!(copy.finish().await.unwrap(), 1);
    }

    async fn paid_payment(&self, client_id: ClientId, txid: &str) -> Payment {
        let total = Money::new(699, Currency::Usd);
        let mut tx = self.services().store().begin().await.unwrap();
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id,
                subscription_id: None,
                kind: OrderKind::Module,
                intent: PurchaseIntent::Module {
                    module: sn_domain::module::ModuleId::Options,
                    kind: "тест".into(),
                    payload: json!({}),
                },
                quote: flat_quote(total, 0),
                snapshot: None,
            })
            .await
            .unwrap();
        let invoice = tx
            .payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id,
                provider: ProviderId::from(providers::PLATEGA),
                method: None,
                amount: Amount::new(699, PayCurrency::Usd),
                expires_at: None,
            })
            .await
            .unwrap();
        tx.payments()
            .transition(invoice.id, PaymentStatus::Paid, Some(txid), None)
            .await
            .unwrap();
        tx.orders()
            .transition(order.id, OrderStatus::Paid)
            .await
            .unwrap();
        tx.orders()
            .transition(order.id, OrderStatus::Fulfilled)
            .await
            .unwrap();
        let paid = tx.payments().get(invoice.id).await.unwrap();
        tx.commit().await.unwrap();
        paid
    }

    async fn refund_request(&self, payment: &Payment, admin: AdminId, hours_ago: i64) -> i64 {
        let requested_at = self.services().now() - Duration::hours(hours_ago);
        let mut conn = self.services().store().conn().await.unwrap();
        conn.payments()
            .create_provider_refund(&NewProviderRefund {
                bonus_minor: 0,
                payment_id: payment.id,
                amount: payment.amount,
                subscription_action: RefundSubscriptionAction::Disable,
                reason: Some("Не подошёл".into()),
                admin_id: Some(admin),
                provider_refund_id: Some(format!("rf-{}", payment.id.0)),
                hold_entry_id: None,
                requested_at,
            })
            .await
            .unwrap()
            .id
    }

    async fn failed_action(&self, payment: &Payment, admin: AdminId) -> i64 {
        let request = self.refund_request(payment, admin, 0).await;
        let now = self.services().now();
        let mut conn = self.services().store().conn().await.unwrap();
        assert!(
            conn.payments()
                .resolve_provider_refund(request, ProviderRefundStatus::Succeeded, None, None, now)
                .await
                .unwrap()
        );
        assert!(
            conn.payments()
                .arm_refund_action(request, now)
                .await
                .unwrap()
        );
        assert!(
            conn.payments()
                .fail_refund_action(request, "Remnawave не отвечает", None)
                .await
                .unwrap()
        );
        request
    }
}

#[tokio::test]
async fn список_списаний_с_карты_на_проверке_показывает_клиента_сумму_и_причину() {
    let Some(h) = harness().await else { return };
    let (viewer, _) = h
        .staff(
            "money@example.com",
            AdminRole::ReadOnly,
            &[Permission::Money],
        )
        .await;
    let (support, _) = h
        .staff("support@example.com", AdminRole::Support, &[])
        .await;
    let client = h.client(9101).await;
    let waiting = h.charge(client, 499, "review", Some(UNCLEAR), 30).await;
    h.charge(client, 299, "queued", Some("ждём ответа кассы"), 30)
        .await;
    h.charge(client, 199, "review", Some("старое списание"), -1)
        .await;
    let (status, _) = h
        .send(Method::GET, "/api/admin/charges/review", Some(&support))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let list = h.get("/api/admin/charges/review", &viewer).await;
    let items = list.as_array().unwrap();
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["payment_id"], waiting.id.0);
    assert_eq!(items[0]["order_id"], waiting.order_id.0);
    assert_eq!(items[0]["client_id"], client.0);
    assert_eq!(items[0]["client"]["name"], "Алексей");
    assert_eq!(
        items[0]["amount"],
        json!({ "minor": 499, "currency": "USD" })
    );
    assert_eq!(items[0]["provider"], "yookassa");
    assert_eq!(items[0]["provider_title"], "ЮKassa");
    assert_eq!(items[0]["reason"], UNCLEAR);
    assert_eq!(items[0]["order"]["kind"], "renewal");
    assert_eq!(items[0]["order"]["days"], 30);
    assert!(items[0]["charged_at"].is_string(), "{list}");
    assert!(items[0]["review_since"].is_string(), "{list}");
    assert!(items[0]["closes_at"].is_string(), "{list}");
    let newer = h.charge(client, 599, "review", None, 30).await;
    let list = h.get("/api/admin/charges/review", &viewer).await;
    assert_eq!(list.as_array().unwrap().len(), 2, "{list}");
    assert_eq!(list[0]["payment_id"], newer.id.0);
    assert_eq!(list[0]["reason"], Value::Null);
    let list = h.get("/api/admin/charges/review?limit=1", &viewer).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let mut conn = h.services().store().conn().await.unwrap();
    assert_eq!(conn.payments().charges_in_review().await.unwrap(), 2);
    drop(conn);
    h.db.cleanup().await;
}

#[tokio::test]
async fn счётчик_у_денег_в_меню_считает_только_дела_которые_админ_вправе_решить() {
    let Some(h) = harness().await else { return };
    let owner = h.owner().await;
    let (money, _) = h
        .staff(
            "money@example.com",
            AdminRole::ReadOnly,
            &[Permission::Money],
        )
        .await;
    let (refunds, refunder) = h
        .staff(
            "refunds@example.com",
            AdminRole::ReadOnly,
            &[Permission::Refunds],
        )
        .await;
    let (support, _) = h
        .staff("support@example.com", AdminRole::Support, &[])
        .await;
    for token in [&owner, &money, &refunds, &support] {
        let badges = h.get("/api/admin/badges", token).await;
        assert_eq!(badges["money"], 0, "{badges}");
        assert_eq!(badges["support"], 0, "{badges}");
    }
    let client = h.client(9201).await;
    h.review_order(client).await;
    h.charge(client, 499, "review", Some(UNCLEAR), 30).await;
    h.stars_review("stx-9201", 9201, 450).await;
    let refunded = h.paid_payment(client, "tx-9201").await;
    let action = h.failed_action(&refunded, refunder).await;
    let stuck = h.paid_payment(client, "tx-9202").await;
    h.refund_request(&stuck, refunder, 25).await;
    let fresh = h.paid_payment(client, "tx-9203").await;
    h.refund_request(&fresh, refunder, 1).await;
    for (token, expected) in [(&owner, 5), (&money, 3), (&refunds, 2), (&support, 0)] {
        let badges = h.get("/api/admin/badges", token).await;
        assert_eq!(badges["money"], expected, "{badges}");
    }

    let dashboard = h.get("/api/admin/dashboard", &money).await;
    let decisions = &dashboard["decisions"];
    assert_eq!(decisions["orders"]["count"], 1, "{decisions}");
    assert!(decisions["orders"]["since"].is_string(), "{decisions}");
    assert_eq!(
        decisions["orders"]["oldest"]["reason"],
        "модуль не подтвердил заказ"
    );
    assert_eq!(decisions["orders"]["oldest"]["client"]["name"], "Алексей");
    assert_eq!(decisions["charges"]["count"], 1);
    assert_eq!(
        decisions["charges"]["total"],
        json!({ "minor": 499, "currency": "USD" })
    );
    assert_eq!(decisions["charges"]["oldest"]["reason"], UNCLEAR);
    assert_eq!(decisions["stars"]["count"], 1);
    assert_eq!(
        decisions["stars"]["oldest"]["amount"],
        json!({ "minor": 450, "currency": "XTR" })
    );
    assert_eq!(decisions["stars"]["oldest"]["client_id"], client.0);
    assert_eq!(
        decisions["stars"]["oldest"]["reason"],
        "сумма не совпала со счётом"
    );
    assert_eq!(decisions["refund_actions"], Value::Null);
    assert_eq!(decisions["refund_requests"], Value::Null);

    let (status, dashboard) = h
        .send(Method::GET, "/api/admin/dashboard", Some(&refunds))
        .await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    let decisions = &dashboard["decisions"];
    assert_eq!(decisions["orders"], Value::Null, "{decisions}");
    assert_eq!(decisions["charges"], Value::Null);
    assert_eq!(decisions["stars"], Value::Null);
    assert_eq!(decisions["refund_actions"]["count"], 1);
    assert_eq!(decisions["refund_actions"]["oldest"]["action"], "disable");
    assert_eq!(
        decisions["refund_actions"]["oldest"]["reason"],
        "Remnawave не отвечает"
    );
    assert_eq!(decisions["refund_requests"]["count"], 1);
    assert_eq!(
        decisions["refund_requests"]["oldest"]["client_id"],
        client.0
    );

    let list = h
        .get("/api/admin/refunds/pending?stuck=true", &refunds)
        .await;
    let items = list.as_array().unwrap();
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["payment_id"], stuck.id.0);
    assert_eq!(items[0]["stuck"], true);
    assert_eq!(items[0]["client_id"], client.0);
    assert_eq!(items[0]["client"]["name"], "Алексей");
    assert_eq!(items[0]["provider_title"], "Platega");
    assert_eq!(items[0]["order"]["kind"], "module");
    let list = h.get("/api/admin/refunds/pending", &refunds).await;
    assert_eq!(list.as_array().unwrap().len(), 2, "{list}");
    assert_eq!(list[1]["stuck"], false);
    let list = h.get("/api/admin/refunds/actions", &refunds).await;
    assert_eq!(list[0]["request_id"], action);
    assert_eq!(list[0]["client"]["name"], "Алексей");
    assert_eq!(list[0]["provider_title"], "Platega");

    {
        let mut conn = h.services().store().conn().await.unwrap();
        assert!(conn.payments().finish_refund_action(action).await.unwrap());
    }
    let badges = h.get("/api/admin/badges", &refunds).await;
    assert_eq!(badges["money"], 1, "{badges}");
    let badges = h.get("/api/admin/badges", &owner).await;
    assert_eq!(badges["money"], 4, "{badges}");
    h.db.cleanup().await;
}
