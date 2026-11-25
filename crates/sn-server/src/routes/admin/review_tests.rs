use crate::{AppState, ServerConfig, router};
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use sn_domain::access::{AdminRole, Permission};
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::{AdminId, ClientId, OrderId, PeriodId, TariffId};
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::payment::{
    NewPayment, Payment, PaymentStatus, ProviderId, RefundSubscriptionAction, providers,
};
use sn_domain::plan::FulfillmentPlan;
use sn_domain::pricing::Quote;
use sn_domain::subscription::{AutoRenewState, SubscriptionKind};
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_services::{EnvFallbacks, ServiceDeps, Services, SystemClock};
use sn_store::testing::{TestDb, test_store};
use sn_store::{
    AuditRecord, Db, NewAdmin, NewProviderRefund, NewSubscription, PlanStage, ProviderRefundStatus,
    SecretBox, audit_entity,
};
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "очень-надёжный-пароль";

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

    async fn send(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn login(&self, email: &str) -> String {
        let (status, body) = self
            .send(
                Method::POST,
                "/api/admin/auth/login",
                None,
                Some(json!({ "email": email, "password": PASSWORD })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().unwrap().to_string()
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

    async fn module_order(&self, client_id: ClientId, provider: &str, amount: Amount) -> Payment {
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
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id,
                provider: ProviderId::from(provider),
                method: None,
                amount,
                expires_at: None,
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        payment
    }

    async fn paid_payment(&self, client_id: ClientId, txid: &str) -> Payment {
        let invoice = self
            .module_order(
                client_id,
                providers::PLATEGA,
                Amount::new(699, PayCurrency::Usd),
            )
            .await;
        let mut tx = self.services().store().begin().await.unwrap();
        tx.payments()
            .transition(invoice.id, PaymentStatus::Paid, Some(txid), None)
            .await
            .unwrap();
        tx.orders()
            .transition(invoice.order_id, OrderStatus::Paid)
            .await
            .unwrap();
        tx.orders()
            .transition(invoice.order_id, OrderStatus::Fulfilled)
            .await
            .unwrap();
        let paid = tx.payments().get(invoice.id).await.unwrap();
        tx.commit().await.unwrap();
        paid
    }

    async fn refund_request(
        &self,
        payment: &Payment,
        admin: AdminId,
        action: RefundSubscriptionAction,
    ) -> i64 {
        let mut conn = self.services().store().conn().await.unwrap();
        conn.payments()
            .create_provider_refund(&NewProviderRefund {
                bonus_minor: 0,
                payment_id: payment.id,
                amount: payment.amount,
                subscription_action: action,
                reason: Some("Не подошёл".into()),
                admin_id: Some(admin),
                provider_refund_id: Some(format!("rf-{}", payment.id.0)),
                hold_entry_id: None,
                requested_at: self.services().now(),
            })
            .await
            .unwrap()
            .id
    }

    async fn failed_action(&self, payment: &Payment, admin: AdminId) -> i64 {
        let request = self
            .refund_request(payment, admin, RefundSubscriptionAction::Disable)
            .await;
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

    async fn review_order(&self, client_id: ClientId) -> OrderId {
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
        order.id
    }

    async fn stale_renewal(&self, client_id: ClientId) -> OrderId {
        let plan: FulfillmentPlan = serde_json::from_value(json!({
            "kind": "renewal",
            "action": {"type": "update", "user_id": 77},
            "target": {
                "expire_at": "2027-03-16T00:00:00Z",
                "squads": ["00000000-0000-0000-0000-000000000001"],
                "external_squad": null,
                "traffic_limit_bytes": 0,
                "traffic_strategy": "NO_RESET",
                "hwid_device_limit": 3,
                "tag": null
            },
            "reset_traffic": false,
            "activate": false,
            "period_started_at": "2026-11-16T14:52:00Z",
            "period_days": 120,
            "base_expire_at": "2027-02-14T00:00:00Z"
        }))
        .unwrap();
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
                quote: flat_quote(Money::new(299, Currency::Usd), 30),
                snapshot: None,
            })
            .await
            .unwrap();
        tx.orders()
            .transition(order.id, OrderStatus::Paid)
            .await
            .unwrap();
        tx.orders().save_plan(order.id, &plan).await.unwrap();
        tx.orders()
            .mark_review(order.id, "срок подписки в Remnawave изменился")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        order.id
    }

    async fn seed_stars_review(&self, charge: &str, payer: i64, payment: &Payment) {
        let payload = json!({
            "event": "successful_payment",
            "invoice_payload": payment.public_id.to_string(),
            "total_amount": payment.amount.minor - 1,
            "currency": "XTR",
            "telegram_payment_charge_id": charge,
            "telegram_user_id": payer,
        });
        let row = format!(
            "{charge}\tsuccessful_payment\t{payload}\tupdate\treview\t1\t2026-11-17 09:00:00+00\tсумма не сошлась\t{}\n",
            payment.id.0
        );
        let mut conn = self.services().store().conn().await.unwrap();
        let mut copy = conn
            .conn()
            .copy_in_raw(
                "COPY bot_star_events (charge_id, event, payload, source, status, attempts, \
                 next_attempt_at, last_error, payment_id) FROM STDIN",
            )
            .await
            .unwrap();
        copy.send(row.as_bytes()).await.unwrap();
        assert_eq!(copy.finish().await.unwrap(), 1);
    }

    async fn audit(&self, entity: &str, id: i64) -> Vec<AuditRecord> {
        let mut conn = self.services().store().conn().await.unwrap();
        conn.audit().for_entity(entity, id, 20).await.unwrap()
    }

    async fn plan_stage(&self, order_id: OrderId) -> PlanStage {
        let mut conn = self.services().store().conn().await.unwrap();
        conn.orders().plan_state(order_id).await.unwrap().stage()
    }

    async fn order_status(&self, order_id: OrderId) -> OrderStatus {
        let mut conn = self.services().store().conn().await.unwrap();
        conn.orders().get(order_id).await.unwrap().status
    }

    async fn payment_status(&self, payment: &Payment) -> PaymentStatus {
        let mut conn = self.services().store().conn().await.unwrap();
        conn.payments().get(payment.id).await.unwrap().status
    }
}

fn by_admin<'a>(records: &'a [AuditRecord], action: &str, admin: AdminId) -> Vec<&'a AuditRecord> {
    records
        .iter()
        .filter(|record| {
            record.action == action
                && record.actor_kind == "admin"
                && record.actor_id == Some(admin.0)
        })
        .collect()
}

fn all_but(permission: Permission) -> Vec<Permission> {
    Permission::ALL
        .into_iter()
        .filter(|p| *p != permission)
        .collect()
}

#[tokio::test]
async fn ручной_разбор_заказов_и_звёзд_требует_именно_права_деньги_и_пишет_аудит_с_админом() {
    let Some(h) = harness().await else { return };
    let (outsider, _) = h
        .staff(
            "no-money@example.com",
            AdminRole::ReadOnly,
            &all_but(Permission::Money),
        )
        .await;
    let (cashier, cashier_id) = h
        .staff(
            "money@example.com",
            AdminRole::ReadOnly,
            &[Permission::Money],
        )
        .await;
    let client = h.client(8101).await;
    let plain = h.review_order(client).await;
    let recalculated = h.stale_renewal(client).await;
    let closed = h.stale_renewal(client).await;
    let invoice = h
        .module_order(client, providers::STARS, Amount::new(350, PayCurrency::Xtr))
        .await;
    h.seed_stars_review("stx-8101", 8101, &invoice).await;
    h.seed_stars_review("stx-8102", 8101, &invoice).await;
    let (status, page) = h
        .send(Method::GET, "/api/admin/stars/review", Some(&cashier), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let star_ids: Vec<i64> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_i64().unwrap())
        .collect();
    assert_eq!(star_ids.len(), 2);
    h.services()
        .set_remna(Some(Arc::new(sn_remnawave::fake::FakeRemnawave::new())));
    let note = json!({ "note": "Срок в панели уже учитывает оплату" });
    let comment = json!({ "comment": "Разобрали с клиентом в поддержке" });
    let calls: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, "/api/admin/orders/review".into(), None),
        (
            Method::POST,
            format!("/api/admin/orders/{}/redeliver", plain.0),
            None,
        ),
        (
            Method::POST,
            format!("/api/admin/orders/{}/recalculate", recalculated.0),
            None,
        ),
        (
            Method::POST,
            format!("/api/admin/orders/{}/mark-fulfilled", closed.0),
            Some(note.clone()),
        ),
        (Method::GET, "/api/admin/stars/review".into(), None),
        (
            Method::POST,
            format!("/api/admin/stars/review/{}/refund", star_ids[0]),
            None,
        ),
        (
            Method::POST,
            format!("/api/admin/stars/review/{}/close", star_ids[1]),
            Some(comment.clone()),
        ),
    ];
    for (method, path, body) in &calls {
        let (status, reply) = h
            .send(method.clone(), path, Some(&outsider), body.clone())
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {reply}");
        assert_eq!(reply["error"]["code"], "auth.forbidden", "{path}");
    }
    assert_eq!(h.plan_stage(recalculated).await, PlanStage::Stale);
    assert_eq!(h.order_status(closed).await, OrderStatus::Review);
    for order in [plain, recalculated, closed] {
        assert!(h.audit(audit_entity::ORDER, order.0).await.is_empty());
    }
    assert!(
        h.audit(audit_entity::PAYMENT, invoice.id.0)
            .await
            .is_empty()
    );
    let mut replies = Vec::new();
    for (method, path, body) in &calls {
        replies.push(
            h.send(method.clone(), path, Some(&cashier), body.clone())
                .await,
        );
    }
    for (index, (status, reply)) in replies.iter().enumerate() {
        if index == 5 {
            assert_eq!(*status, StatusCode::FORBIDDEN, "{reply}");
            assert_eq!(reply["error"]["code"], "auth.forbidden");
        } else {
            assert_eq!(*status, StatusCode::OK, "{}: {reply}", calls[index].1);
        }
    }
    let (refunder, _) = h
        .staff(
            "money-refunds@example.com",
            AdminRole::ReadOnly,
            &[Permission::Money, Permission::Refunds],
        )
        .await;
    let (status, reply) = h
        .send(Method::POST, &calls[5].1, Some(&refunder), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{reply}");
    assert_eq!(reply["error"]["code"], "stars.bot_token_missing");
    assert_eq!(replies[0].1.as_array().unwrap().len(), 3);
    assert_eq!(replies[4].1["total"], 2);
    assert_eq!(h.plan_stage(recalculated).await, PlanStage::None);
    assert_eq!(h.order_status(closed).await, OrderStatus::Fulfilled);
    let redelivered = h.audit(audit_entity::ORDER, plain.0).await;
    assert_eq!(
        by_admin(&redelivered, "order.redeliver", cashier_id).len(),
        1
    );
    let reset = h.audit(audit_entity::ORDER, recalculated.0).await;
    assert_eq!(by_admin(&reset, "order.recalculate", cashier_id).len(), 1);
    let marked = h.audit(audit_entity::ORDER, closed.0).await;
    let marked = by_admin(&marked, "order.mark_fulfilled", cashier_id);
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0].details["note"], note["note"]);
    let stars = h.audit(audit_entity::PAYMENT, invoice.id.0).await;
    let stars = by_admin(&stars, "payment.stars_review_closed", cashier_id);
    assert_eq!(stars.len(), 1);
    assert_eq!(stars[0].details["comment"], comment["comment"]);
    assert_eq!(stars[0].details["event_id"], star_ids[1]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn ручные_действия_с_заявками_на_возврат_требуют_права_возвраты() {
    let Some(h) = harness().await else { return };
    let (outsider, outsider_id) = h
        .staff(
            "no-refunds@example.com",
            AdminRole::ReadOnly,
            &all_but(Permission::Refunds),
        )
        .await;
    let client = h.client(8201).await;
    let waiting = h.paid_payment(client, "tx-8201").await;
    let request = h
        .refund_request(&waiting, outsider_id, RefundSubscriptionAction::Keep)
        .await;
    let broken = h.paid_payment(client, "tx-8202").await;
    let action = h.failed_action(&broken, outsider_id).await;
    let text = json!({ "note": "проверено", "reason": "проверено" });
    for (method, path) in [
        (Method::GET, "/api/admin/refunds/pending".to_string()),
        (Method::GET, "/api/admin/refunds/actions".to_string()),
        (Method::POST, format!("/api/admin/refunds/{request}/cancel")),
        (
            Method::POST,
            format!("/api/admin/refunds/{request}/confirm"),
        ),
        (
            Method::POST,
            format!("/api/admin/refunds/{action}/action/retry"),
        ),
        (
            Method::POST,
            format!("/api/admin/refunds/{action}/action/close"),
        ),
    ] {
        let (status, reply) = h
            .send(method.clone(), &path, Some(&outsider), Some(text.clone()))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {reply}");
        let (status, _) = h.send(method, &path, None, Some(text.clone())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    assert_eq!(h.payment_status(&waiting).await, PaymentStatus::Paid);
    for payment in [&waiting, &broken] {
        assert!(
            h.audit(audit_entity::PAYMENT, payment.id.0)
                .await
                .is_empty()
        );
    }
    h.db.cleanup().await;
}

#[tokio::test]
async fn зависшая_заявка_на_возврат_видна_в_списке_отменяется_и_подтверждается_вручную() {
    let Some(h) = harness().await else { return };
    let (refunder, refunder_id) = h
        .staff(
            "refunds@example.com",
            AdminRole::ReadOnly,
            &[Permission::Refunds],
        )
        .await;
    let client = h.client(8301).await;
    let canceled = h.paid_payment(client, "tx-8301").await;
    let confirmed = h.paid_payment(client, "tx-8302").await;
    let first = h
        .refund_request(&canceled, refunder_id, RefundSubscriptionAction::Keep)
        .await;
    let second = h
        .refund_request(&confirmed, refunder_id, RefundSubscriptionAction::Keep)
        .await;
    let (status, list) = h
        .send(
            Method::GET,
            "/api/admin/refunds/pending",
            Some(&refunder),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list.as_array().unwrap();
    assert_eq!(items.len(), 2, "{list}");
    assert_eq!(items[0]["request_id"], first);
    assert_eq!(items[0]["payment_id"], canceled.id.0);
    assert_eq!(items[0]["amount"], json!({"minor": 699, "currency": "USD"}));
    assert_eq!(items[0]["subscription_action"], "keep");
    assert_eq!(items[0]["requested_by"], refunder_id.0);
    assert_eq!(items[0]["held"], false);
    assert_eq!(items[1]["request_id"], second);
    let cancel = format!("/api/admin/refunds/{first}/cancel");
    for body in [None, Some(json!({})), Some(json!({ "reason": "  " }))] {
        let (status, reply) = h.send(Method::POST, &cancel, Some(&refunder), body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{reply}");
        assert_eq!(reply["error"]["code"], "refund.request_note");
    }
    let (status, reply) = h
        .send(
            Method::POST,
            &cancel,
            Some(&refunder),
            Some(json!({ "reason": "Касса не ответила за неделю" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply, json!({ "ok": true, "request_id": first }));
    assert_eq!(h.payment_status(&canceled).await, PaymentStatus::Paid);
    let audit = h.audit(audit_entity::PAYMENT, canceled.id.0).await;
    let audit = by_admin(&audit, "payment.refund_canceled", refunder_id);
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].details["reason"], "Касса не ответила за неделю");
    let (status, reply) = h
        .send(
            Method::POST,
            &cancel,
            Some(&refunder),
            Some(json!({ "reason": "ещё раз" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{reply}");
    assert_eq!(reply["error"]["code"], "refund.request_resolved");
    let confirm = format!("/api/admin/refunds/{second}/confirm");
    let (status, reply) = h.send(Method::POST, &confirm, Some(&refunder), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{reply}");
    assert_eq!(reply["error"]["code"], "refund.request_note");
    let (status, reply) = h
        .send(
            Method::POST,
            &confirm,
            Some(&refunder),
            Some(json!({ "note": "Возврат виден в кабинете кассы" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["request_id"], second);
    assert_eq!(reply["result"]["type"], "status_changed");
    assert_eq!(reply["result"]["status"], "refunded");
    assert_eq!(h.payment_status(&confirmed).await, PaymentStatus::Refunded);
    assert_eq!(
        h.order_status(confirmed.order_id).await,
        OrderStatus::Refunded
    );
    let audit = h.audit(audit_entity::PAYMENT, confirmed.id.0).await;
    let manual = by_admin(&audit, "payment.refund_confirmed_manually", refunder_id);
    assert_eq!(manual.len(), 1);
    assert_eq!(manual[0].details["note"], "Возврат виден в кабинете кассы");
    let (status, list) = h
        .send(
            Method::GET,
            "/api/admin/refunds/pending",
            Some(&refunder),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([]));
    let (status, _) = h
        .send(
            Method::POST,
            "/api/admin/refunds/999999/cancel",
            Some(&refunder),
            Some(json!({ "reason": "нет такой" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}

#[tokio::test]
async fn несработавшее_действие_с_подпиской_видно_в_списке_повторяется_и_закрывается() {
    let Some(h) = harness().await else { return };
    let (refunder, refunder_id) = h
        .staff(
            "refunds@example.com",
            AdminRole::ReadOnly,
            &[Permission::Refunds, Permission::ClientsRead],
        )
        .await;
    let client = h.client(8401).await;
    let retried = h.paid_payment(client, "tx-8401").await;
    let closed = h.paid_payment(client, "tx-8402").await;
    let first = h.failed_action(&retried, refunder_id).await;
    let second = h.failed_action(&closed, refunder_id).await;
    let (status, dashboard) = h
        .send(Method::GET, "/api/admin/dashboard", Some(&refunder), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["refund_actions_failed"], 2);
    let (status, list) = h
        .send(
            Method::GET,
            "/api/admin/refunds/actions",
            Some(&refunder),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list.as_array().unwrap();
    assert_eq!(items.len(), 2, "{list}");
    assert_eq!(items[0]["request_id"], second);
    assert_eq!(items[0]["payment_id"], closed.id.0);
    assert_eq!(items[0]["order_id"], closed.order_id.0);
    assert_eq!(items[0]["client_id"], client.0);
    assert_eq!(items[0]["action"], "disable");
    assert_eq!(items[0]["attempts"], 1);
    assert_eq!(items[0]["error"], "Remnawave не отвечает");
    let retry = format!("/api/admin/refunds/{first}/action/retry");
    let (status, reply) = h.send(Method::POST, &retry, Some(&refunder), None).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(
        reply,
        json!({ "ok": true, "request_id": first, "outcome": "done" })
    );
    let audit = h.audit(audit_entity::PAYMENT, retried.id.0).await;
    assert_eq!(
        by_admin(&audit, "subscription.refund_action_retry", refunder_id).len(),
        1
    );
    let (status, reply) = h.send(Method::POST, &retry, Some(&refunder), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{reply}");
    assert_eq!(reply["error"]["code"], "refund.action_not_failed");
    let close = format!("/api/admin/refunds/{second}/action/close");
    let (status, reply) = h.send(Method::POST, &close, Some(&refunder), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{reply}");
    assert_eq!(reply["error"]["code"], "refund.action_note");
    let (status, reply) = h
        .send(
            Method::POST,
            &close,
            Some(&refunder),
            Some(json!({ "note": "Подписку отключили в панели вручную" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let audit = h.audit(audit_entity::PAYMENT, closed.id.0).await;
    let audit = by_admin(&audit, "subscription.refund_action_closed", refunder_id);
    assert_eq!(audit.len(), 1);
    assert_eq!(
        audit[0].details["note"],
        "Подписку отключили в панели вручную"
    );
    let (status, reply) = h
        .send(
            Method::POST,
            &close,
            Some(&refunder),
            Some(json!({ "note": "ещё раз" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{reply}");
    assert_eq!(reply["error"]["code"], "refund.action_closed");
    let (status, list) = h
        .send(
            Method::GET,
            "/api/admin/refunds/actions",
            Some(&refunder),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([]));
    let (status, dashboard) = h
        .send(Method::GET, "/api/admin/dashboard", Some(&refunder), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["refund_actions_failed"], 0);
    h.db.cleanup().await;
}

#[tokio::test]
async fn главная_показывает_сколько_оплат_звёздами_ждёт_разбора() {
    let Some(h) = harness().await else { return };
    let (viewer, _) = h
        .staff(
            "viewer@example.com",
            AdminRole::ReadOnly,
            &[Permission::Money],
        )
        .await;
    let client = h.client(8501).await;
    let invoice = h
        .module_order(client, providers::STARS, Amount::new(350, PayCurrency::Xtr))
        .await;
    let (status, dashboard) = h
        .send(Method::GET, "/api/admin/dashboard", Some(&viewer), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["stars_review"], 0);
    h.seed_stars_review("stx-8501", 8501, &invoice).await;
    h.seed_stars_review("stx-8502", 8501, &invoice).await;
    let (status, dashboard) = h
        .send(Method::GET, "/api/admin/dashboard", Some(&viewer), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["stars_review"], 2);
    let (status, page) = h
        .send(Method::GET, "/api/admin/stars/review", Some(&viewer), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let id = page["items"][0]["id"].as_i64().unwrap();
    let (status, reply) = h
        .send(
            Method::POST,
            &format!("/api/admin/stars/review/{id}/close"),
            Some(&viewer),
            Some(json!({ "comment": "Продлили вручную" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let (status, dashboard) = h
        .send(Method::GET, "/api/admin/dashboard", Some(&viewer), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["stars_review"], 1);
    h.db.cleanup().await;
}
