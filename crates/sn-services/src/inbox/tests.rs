use super::*;
use crate::checkout::testkit::*;
use crate::checkout::{Channel, CheckoutRequest, CheckoutResult};
use crate::fulfillment::FulfillmentOutcome;
use crate::services::{EnvFallbacks, ServiceDeps};
use crate::{SystemClock, settings};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use sn_domain::ids::ClientId;
use sn_domain::money::{Currency, PayCurrency};
use sn_domain::order::{NewOrder, PurchaseIntent};
use sn_domain::payment::{NewPayment, ProviderId};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::settings::{SettingField, keys, secret_keys};
use sn_domain::tariff::Tariff;
use sn_modules::ModuleRegistry;
use sn_payments::providers::{Balance, CryptoPay, Overpay, cryptopay_signature};
use sn_payments::{
    ClientLabel, Invoice, InvoiceRequest, PaymentProvider, PaymentResult, ProviderConfig,
    ProviderRegistry, RefundOutcome,
};
use sn_remnawave::fake::FakeRemnawave;
use sn_store::SecretBox;
use sn_store::testing::test_store;
use std::sync::{Arc, Mutex};

mod actions;
mod blocked;
mod bonus;
mod gate;
mod holds;
mod journal;
mod notices;
mod polling;
mod resume;
mod tribute;

const KASSA: &str = "kassa";
const KASSA_SECRET: &str = "kassa-secret";

#[derive(Default)]
struct Kassa {
    refund_completed: Mutex<bool>,
    refund_fails: Mutex<bool>,
    refund_lost: Mutex<bool>,
    refund_keys: Mutex<Vec<Uuid>>,
    refunds: Mutex<Vec<RefundRequest>>,
    status: Mutex<Option<WebhookOutcome>>,
    lookups: Mutex<Vec<String>>,
    refund_status: Mutex<Option<WebhookOutcome>>,
    finds_by_payment: Mutex<bool>,
    knows_order_only: Mutex<bool>,
}

#[async_trait::async_trait]
impl PaymentProvider for Kassa {
    fn id(&self) -> &'static str {
        KASSA
    }

    fn title(&self) -> &'static str {
        "Касса с подтверждением возвратов"
    }

    fn currencies(&self) -> &'static [Currency] {
        &Currency::ALL
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Тестовая касса",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![SettingField::secret("secret", "Секрет", "Кабинет кассы")]
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    fn supports_refund_polling(&self) -> bool {
        true
    }

    fn polls_by_payment_id(&self) -> bool {
        *self.finds_by_payment.lock().unwrap()
    }

    fn status_by_order(&self) -> bool {
        *self.knows_order_only.lock().unwrap()
    }

    async fn create_invoice(
        &self,
        _config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        Ok(Invoice {
            pay_url: format!("https://kassa.example.com/{}", request.payment_public_id),
            external_id: Some(format!("k-{}", request.payment_id)),
            expires_at: None,
            payload: serde_json::json!({}),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let secret = config.require("secret")?;
        if request.header("x-kassa-secret") != Some(secret) {
            return Err(PaymentError::BadSignature);
        }
        let body: serde_json::Value = serde_json::from_slice(&request.body)
            .map_err(|e| PaymentError::BadPayload(e.to_string()))?;
        let text = |key: &str| {
            body.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let status = match text("status").as_deref() {
            Some("paid") => ProviderStatus::Paid,
            Some("refunded") => ProviderStatus::Refunded,
            _ => ProviderStatus::Pending,
        };
        Ok(WebhookOutcome {
            event_key: text("event").unwrap_or_default(),
            payment: text("payment")
                .and_then(|p| Uuid::parse_str(&p).ok())
                .map(PaymentRef::PublicId),
            provider_txid: text("txid"),
            status,
            amount: body
                .get("amount")
                .and_then(serde_json::Value::as_i64)
                .map(|minor| Amount::new(minor, PayCurrency::Usd)),
            saved_method: None,
            reply: WebhookReply::Json {
                status: 200,
                body: serde_json::json!({ "code": 0 }),
            },
            signal: None,
        })
    }

    async fn fetch_status(
        &self,
        _config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        self.lookups.lock().unwrap().push(external_id.to_string());
        Ok(self.status.lock().unwrap().clone())
    }

    async fn refund(
        &self,
        _config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        self.refund_keys.lock().unwrap().push(request.key);
        if *self.refund_fails.lock().unwrap() {
            return Err(PaymentError::Transport("касса не отвечает".into()));
        }
        let mut refunds = self.refunds.lock().unwrap();
        refunds.push(request.clone());
        if *self.refund_lost.lock().unwrap() {
            return Err(PaymentError::Transport("ответ кассы потерян".into()));
        }
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("kr-{}", refunds.len())),
            completed: *self.refund_completed.lock().unwrap(),
        })
    }

    async fn fetch_refund(
        &self,
        _config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        self.lookups
            .lock()
            .unwrap()
            .push(format!("refund:{provider_refund_id}"));
        Ok(self.refund_status.lock().unwrap().clone())
    }
}

async fn kassa_kit(kassa: Arc<Kassa>) -> Option<Kit> {
    kassa_kit_with_clock(kassa, Arc::new(SystemClock)).await
}

async fn kassa_kit_with_clock(kassa: Arc<Kassa>, clock: Arc<dyn crate::Clock>) -> Option<Kit> {
    kassa_kit_with(kassa, clock, Vec::new()).await
}

async fn kassa_kit_with(
    kassa: Arc<Kassa>,
    clock: Arc<dyn crate::Clock>,
    extra: Vec<Arc<dyn PaymentProvider>>,
) -> Option<Kit> {
    let db = test_store().await?;
    let pay = Arc::new(TestPay::default());
    let mut providers: Vec<Arc<dyn PaymentProvider>> = vec![
        Arc::new(CryptoPay::new()),
        pay.clone(),
        kassa,
        Arc::new(Overpay::new()),
        Arc::new(Balance),
    ];
    providers.extend(extra);
    let registry = ProviderRegistry::new(providers);
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(registry),
        modules: Arc::new(ModuleRegistry::new(Vec::new()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock,
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &serde_json::json!(PUBLIC_URL), None)
            .await
            .unwrap();
        for (provider, field, value) in [
            (providers::CRYPTOPAY, "api_token", CRYPTO_TOKEN),
            (KASSA, "secret", KASSA_SECRET),
        ] {
            settings::write_secret(
                &mut conn,
                services.secrets(),
                &secret_keys::provider(provider, field),
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

async fn switch_provider(kit: &Kit, provider: &str, enabled: bool) {
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(provider),
                &serde_json::json!({ "enabled": enabled }),
                None,
            )
            .await
            .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
}

fn refund_command(
    amount: Option<i64>,
    action: RefundSubscriptionAction,
    reason: Option<&str>,
) -> RefundCommand {
    RefundCommand {
        amount: amount.map(|minor| Amount::new(minor, PayCurrency::Usd)),
        method: RefundMethod::Provider,
        subscription_action: action,
        reason: reason.map(str::to_string),
        key: None,
        outside: false,
    }
}

async fn provider_refunds(kit: &Kit, payment: PaymentId) -> Vec<ProviderRefund> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().provider_refunds(payment).await.unwrap()
}

async fn kassa_topup(kit: &Kit, client_id: ClientId, minor: i64) -> Payment {
    let result = kit
        .services
        .checkout()
        .topup(
            client_id,
            Money::new(minor, Currency::Usd),
            KASSA,
            None,
            Channel::Cabinet,
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().by_public_id(payment_id).await.unwrap()
}

fn kassa_hook(secret: &str, body: &serde_json::Value) -> InboundWebhook {
    InboundWebhook {
        provider: KASSA.into(),
        request: WebhookRequest {
            headers: vec![("x-kassa-secret".into(), secret.into())],
            body: Bytes::from(body.to_string()),
            remote_ip: "203.0.113.9".parse().ok(),
            route_suffix: None,
            query: None,
        },
    }
}

fn kassa_reply() -> WebhookReply {
    WebhookReply::Json {
        status: 200,
        body: serde_json::json!({ "code": 0 }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct InboxRow {
    source: String,
    dedupe_key: String,
    outcome: String,
    body: Option<Vec<u8>>,
    payment_id: Option<i64>,
}

async fn inbox_rows(kit: &Kit) -> Vec<InboxRow> {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT source, dedupe_key, outcome, body, payment_id FROM webhook_inbox ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn admin(kit: &Kit) -> AdminId {
    kit.services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

fn paid_by(payment: &Payment, txid: &str) -> WebhookOutcome {
    WebhookOutcome {
        event_key: format!("paid:{txid}"),
        payment: Some(PaymentRef::PublicId(payment.public_id)),
        provider_txid: Some(txid.into()),
        status: ProviderStatus::Paid,
        amount: Some(payment.amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    }
}

async fn by_txid(kit: &Kit, provider: &str, txid: &str) -> Option<Payment> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments()
        .by_provider_txid(provider, txid)
        .await
        .unwrap()
}

fn notifications(queued: &[JobPayload], client_id: ClientId) -> Vec<String> {
    queued
        .iter()
        .filter_map(|job| match job {
            JobPayload::Notify {
                client_id: to,
                template,
                ..
            } if *to == client_id => Some(template.clone()),
            _ => None,
        })
        .collect()
}

async fn refunded_events(kit: &Kit, client_id: ClientId) -> usize {
    events(&kit.services, client_id)
        .await
        .into_iter()
        .filter(|e| matches!(e, DomainEvent::PaymentRefunded { .. }))
        .count()
}

async fn pending(
    kit: &Kit,
    client_id: ClientId,
    tariff: &Tariff,
    provider: &str,
    amount: Amount,
    expires_at: Option<DateTime<Utc>>,
) -> Payment {
    let period = tariff.periods.iter().find(|p| p.days == 90).unwrap();
    let q = quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff,
        period_id: period.id,
        extra_devices: 0,
        sell_extra_devices: true,
        adjustments: &[],
        promotion: None,
    })
    .unwrap();
    let mut tx = kit.services.store().begin().await.unwrap();
    let order = tx
        .orders()
        .create(&NewOrder {
            client_id,
            subscription_id: None,
            kind: OrderKind::New,
            intent: PurchaseIntent::Subscription {
                tariff_id: tariff.id,
                period_id: period.id,
                extra_devices: 0,
                target: None,
            },
            quote: q,
            snapshot: Some(tariff.snapshot(period)),
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
            expires_at,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    payment
}

async fn extra_payment(
    kit: &Kit,
    order_id: OrderId,
    client_id: ClientId,
    provider: &str,
    amount: Amount,
) -> Payment {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments()
        .create(&NewPayment {
            order_id,
            client_id,
            provider: ProviderId::from(provider),
            method: None,
            amount,
            expires_at: None,
        })
        .await
        .unwrap()
}

fn crypto_hook(payment: Uuid, amount: &str, update_id: i64) -> InboundWebhook {
    let body = serde_json::json!({
        "update_id": update_id, "update_type": "invoice_paid", "request_date": "2026-11-16T14:52:00.000Z",
        "payload": {"invoice_id": 9001, "status": "paid", "currency_type": "fiat", "fiat": "USD",
                    "amount": amount, "payload": payment.to_string()}
    })
    .to_string();
    InboundWebhook {
        provider: providers::CRYPTOPAY.into(),
        request: WebhookRequest {
            headers: vec![(
                "crypto-pay-api-signature".into(),
                cryptopay_signature(CRYPTO_TOKEN, body.as_bytes()),
            )],
            body: Bytes::from(body),
            remote_ip: None,
            route_suffix: None,
            query: None,
        },
    }
}

fn outcome(payment: &Payment, status: ProviderStatus) -> WebhookOutcome {
    WebhookOutcome {
        event_key: format!("{status:?}:{}", payment.id),
        payment: Some(PaymentRef::Payment(payment.id)),
        provider_txid: payment
            .provider_txid
            .clone()
            .or_else(|| Some(format!("tx-{}", payment.id))),
        status,
        amount: Some(payment.amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    }
}

async fn invoice(kit: &Kit, client_id: ClientId, tariff: &Tariff) -> Payment {
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(tariff, 90, None),
                provider: TEST_PAY.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().by_public_id(payment_id).await.unwrap()
}

async fn topup(kit: &Kit, client_id: ClientId, minor: i64) -> Payment {
    let result = kit
        .services
        .checkout()
        .topup(
            client_id,
            Money::new(minor, Currency::Usd),
            TEST_PAY,
            None,
            Channel::Bot,
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().by_public_id(payment_id).await.unwrap()
}

async fn payment(kit: &Kit, id: PaymentId) -> Payment {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().get(id).await.unwrap()
}

async fn audit_actions(kit: &Kit, payment: PaymentId) -> Vec<String> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.audit()
        .for_entity("payment", payment.0, 20)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect()
}

#[tokio::test]
async fn два_параллельных_вебхука_дают_одну_задачу_и_одну_выдачу() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 101).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(
        &kit,
        client_id,
        &tariff,
        providers::CRYPTOPAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    let inbox = kit.services.inbox();
    let (first, second) = tokio::join!(
        inbox.handle(crypto_hook(payment.public_id, "6.99", 1)),
        inbox.handle(crypto_hook(payment.public_id, "6.99", 2)),
    );
    assert_eq!(first.unwrap().reply.status(), 200);
    assert_eq!(second.unwrap().reply.status(), 200);
    let queued = jobs(&kit.services).await;
    assert_eq!(
        queued,
        vec![JobPayload::FulfillOrder {
            order_id: payment.order_id
        }]
    );
    let paid_events = events(&kit.services, client_id)
        .await
        .into_iter()
        .filter(|e| matches!(e, DomainEvent::PaymentPaid { .. }))
        .count();
    assert_eq!(paid_events, 1);
    let fulfillment = kit.services.fulfillment();
    let done = fulfillment.run(payment.order_id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let again = fulfillment.run(payment.order_id).await.unwrap();
    assert!(matches!(again, FulfillmentOutcome::Done { .. }));
    assert_eq!(kit.remna.users().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn поздняя_оплата_отменённого_счёта_принимается() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 102).await;
    let tariff = standard(&kit.services).await;
    let now = kit.services.now();
    let stale = pending(
        &kit,
        client_id,
        &tariff,
        TEST_PAY,
        Amount::new(699, PayCurrency::Usd),
        Some(now - chrono::Duration::hours(1)),
    )
    .await;
    {
        let mut tx = kit.services.store().begin().await.unwrap();
        assert_eq!(
            tx.payments().expire_stale(now, 10).await.unwrap(),
            vec![stale.id]
        );
        tx.orders()
            .transition(stale.order_id, OrderStatus::Canceled)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let result = kit
        .services
        .inbox()
        .apply_outcome(TEST_PAY, &outcome(&stale, ProviderStatus::Paid))
        .await
        .unwrap();
    assert_eq!(
        result,
        AcceptResult::Accepted {
            order_id: stale.order_id,
            payment_id: stale.id
        }
    );
    assert_eq!(payment(&kit, stale.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, stale.order_id).await.status,
        OrderStatus::Paid
    );
    assert_eq!(
        jobs(&kit.services).await,
        vec![JobPayload::FulfillOrder {
            order_id: stale.order_id
        }]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пополнение_зачисляется_один_раз_и_клиент_получает_одно_уведомление() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 103).await;
    let payment = topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    let paid = outcome(&payment, ProviderStatus::Paid);
    assert!(matches!(
        inbox.apply_outcome(TEST_PAY, &paid).await.unwrap(),
        AcceptResult::Accepted { .. }
    ));
    assert_eq!(
        inbox.apply_outcome(TEST_PAY, &paid).await.unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: payment.id
        }
    );
    let by_txid = WebhookOutcome {
        payment: Some(PaymentRef::ProviderTxid(
            payment.provider_txid.clone().unwrap(),
        )),
        ..paid.clone()
    };
    assert_eq!(
        inbox.apply_outcome(TEST_PAY, &by_txid).await.unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: payment.id
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Fulfilled
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let entries = conn
        .ledger()
        .entries(client_id, Default::default())
        .await
        .unwrap();
    assert_eq!(entries.items.len(), 1);
    assert_eq!(entries.items[0].payment_id, Some(payment.id));
    drop(conn);
    assert_eq!(
        jobs(&kit.services).await,
        vec![JobPayload::Notify {
            client_id,
            template: TOPUP_TEMPLATE.into(),
            params: serde_json::json!({
                "amount": payment.amount.as_money().unwrap(),
                "order_id": payment.order_id,
                "payment_id": payment.id,
            }),
        }]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_по_уведомлению_сторнирует_пополнение() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 104).await;
    let payment = topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(TEST_PAY, &outcome(&payment, ProviderStatus::Paid))
        .await
        .unwrap();
    let partial = WebhookOutcome {
        event_key: "refund:1".into(),
        ..outcome(
            &payment,
            ProviderStatus::PartiallyRefunded {
                refunded: Amount::new(300, PayCurrency::Usd),
            },
        )
    };
    assert_eq!(
        inbox.apply_outcome(TEST_PAY, &partial).await.unwrap(),
        AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: PaymentStatus::PartiallyRefunded
        }
    );
    assert_eq!(
        inbox.apply_outcome(TEST_PAY, &partial).await.unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: payment.id
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 700);
    let second = WebhookOutcome {
        event_key: "refund:2".into(),
        ..partial.clone()
    };
    assert_eq!(
        inbox.apply_outcome(TEST_PAY, &second).await.unwrap(),
        AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: PaymentStatus::PartiallyRefunded
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 400);
    assert_eq!(
        inbox
            .apply_outcome(TEST_PAY, &outcome(&payment, ProviderStatus::Refunded))
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: PaymentStatus::Refunded
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Refunded
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let refunds = conn.refunds().for_payment(payment.id).await.unwrap();
    assert_eq!(
        refunds.iter().map(|r| r.amount.minor).collect::<Vec<_>>(),
        vec![300, 300, 400]
    );
    assert!(refunds.iter().all(|r| r.method == RefundMethod::Provider));
    drop(conn);
    let refunded = events(&kit.services, client_id)
        .await
        .into_iter()
        .filter(|e| matches!(e, DomainEvent::PaymentRefunded { .. }))
        .count();
    assert_eq!(refunded, 3);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_пополнения_не_уводит_баланс_в_минус() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 110).await;
    let tariff = standard(&kit.services).await;
    let payment = topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(TEST_PAY, &outcome(&payment, ProviderStatus::Paid))
        .await
        .unwrap();
    buy_with_balance(&kit.services, client_id, cart(&tariff, 90, None)).await;
    assert_eq!(balance(&kit.services, client_id).await, 301);
    inbox
        .apply_outcome(TEST_PAY, &outcome(&payment, ProviderStatus::Refunded))
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert!(
        audit_actions(&kit, payment.id)
            .await
            .contains(&"refund.topup_shortfall".to_string())
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn сохранённая_карта_хранится_зашифрованной() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 105).await;
    let tariff = standard(&kit.services).await;
    let payment = invoice(&kit, client_id, &tariff).await;
    let paid = WebhookOutcome {
        saved_method: Some(SavedMethod {
            token: "pm-123".into(),
            title: Some("Visa •• 4242".into()),
        }),
        ..outcome(&payment, ProviderStatus::Paid)
    };
    kit.services
        .inbox()
        .apply_outcome(TEST_PAY, &paid)
        .await
        .unwrap();
    let mut conn = kit.services.store().conn().await.unwrap();
    let record = conn
        .saved_methods()
        .active(client_id, TEST_PAY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.title.as_deref(), Some("Visa •• 4242"));
    assert_ne!(record.token.ciphertext, b"pm-123".to_vec());
    let token = kit
        .services
        .secrets()
        .unwrap()
        .open_string(&saved_method_context(client_id, TEST_PAY), &record.token)
        .unwrap();
    assert_eq!(token, "pm-123");
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повторная_оплата_оплаченного_заказа_идёт_на_баланс_а_звёзды_на_проверку() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 106).await;
    let tariff = standard(&kit.services).await;
    let first = pending(
        &kit,
        client_id,
        &tariff,
        TEST_PAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    let second = extra_payment(
        &kit,
        first.order_id,
        client_id,
        TEST_PAY,
        Amount::new(699, PayCurrency::Usd),
    )
    .await;
    let stars = extra_payment(
        &kit,
        first.order_id,
        client_id,
        providers::STARS,
        Amount::new(350, PayCurrency::Xtr),
    )
    .await;
    let inbox = kit.services.inbox();
    for payment in [&first, &second] {
        assert!(matches!(
            inbox
                .apply_outcome(TEST_PAY, &outcome(payment, ProviderStatus::Paid))
                .await
                .unwrap(),
            AcceptResult::Accepted { .. }
        ));
    }
    assert!(matches!(
        inbox
            .apply_outcome(providers::STARS, &outcome(&stars, ProviderStatus::Paid))
            .await
            .unwrap(),
        AcceptResult::Accepted { .. }
    ));
    assert_eq!(balance(&kit.services, client_id).await, 699);
    assert_eq!(
        audit_actions(&kit, second.id).await,
        vec!["payment.repeat_credited".to_string()]
    );
    assert_eq!(
        audit_actions(&kit, stars.id).await,
        vec!["payment.repeat_review".to_string()]
    );
    assert_eq!(payment(&kit, stars.id).await.status, PaymentStatus::Paid);
    let queued = jobs(&kit.services).await;
    assert_eq!(queued.len(), 3, "{queued:?}");
    assert_eq!(
        queued[0],
        JobPayload::FulfillOrder {
            order_id: first.order_id
        }
    );
    assert_eq!(
        notifications(&queued, client_id),
        vec![
            REPEAT_CREDITED_TEMPLATE.to_string(),
            REPEAT_REVIEW_TEMPLATE.to_string()
        ]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn админ_возвращает_покупку_на_баланс_или_через_кассу() {
    let Some(kit) = kit().await else { return };
    let (admin, _) = kit
        .services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    let client_id = client(&kit.services, 107).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let bought = buy_with_balance(&kit.services, client_id, cart(&tariff, 90, None)).await;
    let balance_payment = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments()
            .for_order(bought.id)
            .await
            .unwrap()
            .remove(0)
    };
    let inbox = kit.services.inbox();
    let to_balance = RefundCommand {
        amount: None,
        method: RefundMethod::Balance,
        subscription_action: RefundSubscriptionAction::Keep,
        reason: Some("Клиент передумал".into()),
        key: None,
        outside: false,
    };
    let refunded = inbox
        .refund(balance_payment.id, &to_balance, admin)
        .await
        .unwrap();
    assert_eq!(refunded.status, PaymentStatus::Refunded);
    assert_eq!(refunded.amount, Amount::new(699, PayCurrency::Usd));
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    assert_eq!(
        order(&kit.services, bought.id).await.status,
        OrderStatus::Refunded
    );
    assert_eq!(
        inbox
            .refund(balance_payment.id, &to_balance, admin)
            .await
            .unwrap_err()
            .code(),
        "refund.not_paid"
    );
    let card = invoice(&kit, client_id, &tariff).await;
    inbox
        .apply_outcome(TEST_PAY, &outcome(&card, ProviderStatus::Paid))
        .await
        .unwrap();
    let partial = inbox
        .refund(
            card.id,
            &RefundCommand {
                amount: Some(Amount::new(200, PayCurrency::Usd)),
                method: RefundMethod::Provider,
                subscription_action: RefundSubscriptionAction::Keep,
                reason: None,
                key: None,
                outside: false,
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(partial.status, PaymentStatus::PartiallyRefunded);
    assert_eq!(kit.pay.refunds.lock().unwrap().len(), 1);
    let echoed = WebhookOutcome {
        event_key: format!("refund:rf-{}", card.id),
        ..outcome(
            &card,
            ProviderStatus::PartiallyRefunded {
                refunded: Amount::new(200, PayCurrency::Usd),
            },
        )
    };
    assert_eq!(
        inbox.apply_outcome(TEST_PAY, &echoed).await.unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: card.id
        }
    );
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        assert_eq!(conn.refunds().for_payment(card.id).await.unwrap().len(), 1);
    }
    assert_eq!(
        kit.pay.refunds.lock().unwrap()[0].amount,
        Amount::new(200, PayCurrency::Usd)
    );
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    let topped = topup(&kit, client_id, 500).await;
    inbox
        .apply_outcome(TEST_PAY, &outcome(&topped, ProviderStatus::Paid))
        .await
        .unwrap();
    assert_eq!(
        inbox
            .refund(topped.id, &to_balance, admin)
            .await
            .unwrap_err()
            .code(),
        "refund.topup_to_balance"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ручная_отметка_оплаты_запрещена_для_звёзд() {
    let Some(kit) = kit().await else { return };
    let (admin, _) = kit
        .services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    let client_id = client(&kit.services, 108).await;
    let tariff = standard(&kit.services).await;
    let stars = pending(
        &kit,
        client_id,
        &tariff,
        providers::STARS,
        Amount::new(350, PayCurrency::Xtr),
        None,
    )
    .await;
    let inbox = kit.services.inbox();
    assert_eq!(
        inbox
            .mark_paid(stars.id, admin, None)
            .await
            .unwrap_err()
            .code(),
        "payment.manual_stars"
    );
    let card = pending(
        &kit,
        client_id,
        &tariff,
        TEST_PAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    assert!(matches!(
        inbox
            .mark_paid(card.id, admin, Some("оплатил переводом"))
            .await
            .unwrap(),
        AcceptResult::Accepted { .. }
    ));
    assert_eq!(payment(&kit, card.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        audit_actions(&kit, card.id).await,
        vec!["payment.mark_paid".to_string()]
    );
    assert_eq!(
        jobs(&kit.services).await,
        vec![JobPayload::FulfillOrder {
            order_id: card.order_id
        }]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn опрос_счёта_проводит_оплату_без_вебхука() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 109).await;
    let tariff = standard(&kit.services).await;
    let payment = invoice(&kit, client_id, &tariff).await;
    let inbox = kit.services.inbox();
    assert_eq!(
        inbox.poll_payment(payment.id).await.unwrap(),
        PollResult::Unsupported
    );
    *kit.pay.status.lock().unwrap() = Some(WebhookOutcome {
        payment: None,
        ..outcome(&payment, ProviderStatus::Pending)
    });
    assert_eq!(
        inbox.poll_payment(payment.id).await.unwrap(),
        PollResult::Pending
    );
    *kit.pay.status.lock().unwrap() = Some(WebhookOutcome {
        payment: None,
        ..outcome(&payment, ProviderStatus::Paid)
    });
    assert_eq!(
        inbox.poll_payment(payment.id).await.unwrap(),
        PollResult::Settled
    );
    assert_eq!(
        self::payment(&kit, payment.id).await.status,
        PaymentStatus::Paid
    );
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Paid
    );
    assert_eq!(
        inbox.poll_payment(payment.id).await.unwrap(),
        PollResult::Settled
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказ_кассы_отменяет_неоплаченный_заказ() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 111).await;
    let tariff = standard(&kit.services).await;
    let payment = invoice(&kit, client_id, &tariff).await;
    let inbox = kit.services.inbox();
    assert_eq!(
        inbox
            .apply_outcome(TEST_PAY, &outcome(&payment, ProviderStatus::Failed))
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: PaymentStatus::Failed
        }
    );
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Canceled
    );
    assert!(matches!(
        inbox
            .apply_outcome(TEST_PAY, &outcome(&payment, ProviderStatus::Refunded))
            .await
            .unwrap(),
        AcceptResult::Ignored { .. }
    ));
    kit.db.cleanup().await;
}

#[test]
fn отказ_подлинного_уведомления_получает_свой_исход() {
    for code in [
        "payment.txid_mismatch",
        "payment.provider_mismatch",
        "payment.refund_currency",
    ] {
        assert_eq!(rejection_outcome(code), InboxOutcome::RejectedBusiness);
    }
    assert_eq!(
        rejection_outcome("payment.amount_mismatch"),
        InboxOutcome::AmountMismatch
    );
    assert!(!InboxOutcome::RejectedBusiness.is_final());
    assert!(!InboxOutcome::RejectedBusiness.is_unverified());
}

#[test]
fn возврат_из_админки_узнаётся_в_уведомлении_кассы() {
    assert!(same_refund("rf-1", "refund:rf-1"));
    assert!(same_refund("1601", "refund:1601"));
    assert!(same_refund("op-part", "tx-7:refund:op-part"));
    assert!(same_refund("refund:9", "refund:9"));
    assert!(!same_refund("1600", "refund:1601"));
    assert!(!same_refund("", "refund:"));
}

#[tokio::test]
async fn повторная_оплата_по_той_же_ссылке_с_другой_транзакцией_зачисляется_на_баланс() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 120).await;
    let tariff = standard(&kit.services).await;
    let first = pending(
        &kit,
        client_id,
        &tariff,
        TEST_PAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    let inbox = kit.services.inbox();
    assert_eq!(
        inbox
            .apply_outcome(TEST_PAY, &paid_by(&first, "op-1"))
            .await
            .unwrap(),
        AcceptResult::Accepted {
            order_id: first.order_id,
            payment_id: first.id
        }
    );
    let again = inbox
        .apply_outcome(TEST_PAY, &paid_by(&first, "op-2"))
        .await
        .unwrap();
    let repeat = by_txid(&kit, TEST_PAY, "op-2")
        .await
        .expect("повторная оплата не сохранена");
    assert_eq!(
        again,
        AcceptResult::Accepted {
            order_id: first.order_id,
            payment_id: repeat.id
        }
    );
    assert_ne!(repeat.id, first.id);
    assert_eq!(repeat.order_id, first.order_id);
    assert_eq!(repeat.status, PaymentStatus::Paid);
    assert_eq!(repeat.amount, first.amount);
    assert_eq!(balance(&kit.services, client_id).await, 699);
    assert_eq!(
        audit_actions(&kit, repeat.id).await,
        vec!["payment.repeat_credited".to_string()]
    );
    assert_eq!(
        inbox
            .apply_outcome(TEST_PAY, &paid_by(&first, "op-2"))
            .await
            .unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: repeat.id
        }
    );
    assert_eq!(
        inbox
            .apply_outcome(TEST_PAY, &paid_by(&first, "op-1"))
            .await
            .unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: first.id
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 699);
    assert_eq!(
        payment(&kit, first.id).await.provider_txid.as_deref(),
        Some("op-1")
    );
    let paid_events = events(&kit.services, client_id)
        .await
        .into_iter()
        .filter(|e| matches!(e, DomainEvent::PaymentPaid { .. }))
        .count();
    assert_eq!(paid_events, 2);
    let queued = jobs(&kit.services).await;
    assert_eq!(
        queued
            .iter()
            .filter(|j| matches!(j, JobPayload::FulfillOrder { .. }))
            .count(),
        1
    );
    assert_eq!(
        notifications(&queued, client_id),
        vec!["payment.repeat_credited".to_string()]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повторная_оплата_звёздами_уходит_на_ручную_проверку() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 121).await;
    let tariff = standard(&kit.services).await;
    let stars = pending(
        &kit,
        client_id,
        &tariff,
        providers::STARS,
        Amount::new(350, PayCurrency::Xtr),
        None,
    )
    .await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(providers::STARS, &paid_by(&stars, "charge-1"))
        .await
        .unwrap();
    let again = inbox
        .apply_outcome(providers::STARS, &paid_by(&stars, "charge-2"))
        .await
        .unwrap();
    let repeat = by_txid(&kit, providers::STARS, "charge-2")
        .await
        .expect("повторная оплата звёздами не сохранена");
    assert_eq!(
        again,
        AcceptResult::Accepted {
            order_id: stars.order_id,
            payment_id: repeat.id
        }
    );
    assert_eq!(repeat.amount, Amount::new(350, PayCurrency::Xtr));
    assert_eq!(repeat.status, PaymentStatus::Paid);
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        audit_actions(&kit, repeat.id).await,
        vec!["payment.repeat_review".to_string()]
    );
    let queued = jobs(&kit.services).await;
    assert_eq!(
        notifications(&queued, client_id),
        vec!["payment.repeat_review".to_string()]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повторная_оплата_возвращённого_платежа_не_теряется() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 122).await;
    let tariff = standard(&kit.services).await;
    let first = pending(
        &kit,
        client_id,
        &tariff,
        TEST_PAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(TEST_PAY, &paid_by(&first, "op-1"))
        .await
        .unwrap();
    let refunded = WebhookOutcome {
        event_key: "refund:r-1".into(),
        status: ProviderStatus::Refunded,
        ..paid_by(&first, "op-1")
    };
    assert_eq!(
        inbox.apply_outcome(TEST_PAY, &refunded).await.unwrap(),
        AcceptResult::StatusChanged {
            payment_id: first.id,
            status: PaymentStatus::Refunded
        }
    );
    let again = inbox
        .apply_outcome(TEST_PAY, &paid_by(&first, "op-2"))
        .await
        .unwrap();
    let repeat = by_txid(&kit, TEST_PAY, "op-2")
        .await
        .expect("повторная оплата возвращённого платежа не сохранена");
    assert_eq!(
        again,
        AcceptResult::Accepted {
            order_id: first.order_id,
            payment_id: repeat.id
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 699);
    assert_eq!(
        payment(&kit, first.id).await.status,
        PaymentStatus::Refunded
    );
    assert_eq!(
        order(&kit.services, first.order_id).await.status,
        OrderStatus::Refunded
    );
    assert_eq!(
        audit_actions(&kit, repeat.id).await,
        vec!["payment.repeat_credited".to_string()]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_без_подтверждения_кассы_ждёт_и_удерживает_сумму_на_балансе() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 130).await;
    let topped = kassa_topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &outcome(&topped, ProviderStatus::Paid))
        .await
        .unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    let command = RefundCommand {
        amount: None,
        method: RefundMethod::Provider,
        subscription_action: RefundSubscriptionAction::Keep,
        reason: Some("Клиент передумал".into()),
        key: None,
        outside: false,
    };
    let requested = inbox.refund(topped.id, &command, admin).await.unwrap();
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    assert_eq!(requested.status, PaymentStatus::Paid);
    assert_eq!(requested.amount, Amount::new(1_000, PayCurrency::Usd));
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(payment(&kit, topped.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, topped.order_id).await.status,
        OrderStatus::Fulfilled
    );
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        assert!(
            conn.refunds()
                .for_payment(topped.id)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(refunded_events(&kit, client_id).await, 0);
    assert_eq!(
        inbox
            .refund(topped.id, &command, admin)
            .await
            .unwrap_err()
            .code(),
        "refund.pending"
    );
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    let confirmed = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        ..outcome(&topped, ProviderStatus::Refunded)
    };
    assert_eq!(
        inbox.apply_outcome(KASSA, &confirmed).await.unwrap(),
        AcceptResult::StatusChanged {
            payment_id: topped.id,
            status: PaymentStatus::Refunded
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        payment(&kit, topped.id).await.status,
        PaymentStatus::Refunded
    );
    assert_eq!(
        order(&kit.services, topped.order_id).await.status,
        OrderStatus::Refunded
    );
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        let refunds = conn.refunds().for_payment(topped.id).await.unwrap();
        assert_eq!(refunds.len(), 1);
        assert_eq!(refunds[0].amount, Amount::new(1_000, PayCurrency::Usd));
        assert_eq!(refunds[0].provider_refund_id.as_deref(), Some("kr-1"));
        assert_eq!(refunds[0].admin_id, Some(admin));
        assert_eq!(refunds[0].reason.as_deref(), Some("Клиент передумал"));
    }
    assert_eq!(refunded_events(&kit, client_id).await, 1);
    assert_eq!(
        inbox.apply_outcome(KASSA, &confirmed).await.unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: topped.id
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn тело_поддельного_вебхука_не_сохраняется() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 140).await;
    let tariff = standard(&kit.services).await;
    let pending_payment = pending(
        &kit,
        client_id,
        &tariff,
        providers::CRYPTOPAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    let mut forged = crypto_hook(pending_payment.public_id, "6.99", 1);
    forged.request.headers = vec![("crypto-pay-api-signature".into(), "00".repeat(32))];
    let inbox = kit.services.inbox();
    assert_eq!(inbox.handle(forged).await.unwrap().reply.status(), 401);
    let rows = inbox_rows(&kit).await;
    assert!(rows.iter().all(|r| r.body.is_none()), "{rows:?}");
    assert_eq!(
        payment(&kit, pending_payment.id).await.status,
        PaymentStatus::Pending
    );
    assert_eq!(
        inbox
            .handle(crypto_hook(pending_payment.public_id, "6.99", 2))
            .await
            .unwrap()
            .reply
            .status(),
        200
    );
    let accepted: Vec<InboxRow> = inbox_rows(&kit)
        .await
        .into_iter()
        .filter(|r| r.outcome == "accepted")
        .collect();
    assert_eq!(accepted.len(), 1);
    assert!(accepted[0].body.is_some());
    assert_eq!(accepted[0].payment_id, Some(pending_payment.id.0));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оплата_счёта_выставленного_до_выключения_системы_проходит() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 141).await;
    let tariff = standard(&kit.services).await;
    let pending_payment = pending(
        &kit,
        client_id,
        &tariff,
        providers::CRYPTOPAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    switch_provider(&kit, providers::CRYPTOPAY, false).await;
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart(&tariff, 90, None))
        .await
        .unwrap();
    assert!(
        view.methods
            .iter()
            .all(|m| m.provider != providers::CRYPTOPAY),
        "{:?}",
        view.methods
    );
    let refused = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(&tariff, 90, None),
                provider: providers::CRYPTOPAY.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "checkout.method_unavailable");
    let reply = kit
        .services
        .inbox()
        .handle(crypto_hook(pending_payment.public_id, "6.99", 1))
        .await
        .unwrap();
    assert_eq!(reply.reply.status(), 200);
    assert_eq!(
        payment(&kit, pending_payment.id).await.status,
        PaymentStatus::Paid
    );
    let rows = inbox_rows(&kit).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].outcome, "accepted");
    assert_eq!(rows[0].payment_id, Some(pending_payment.id.0));
    assert!(rows[0].body.is_some());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn подтверждение_возврата_после_выключения_системы_проходит() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 143).await;
    let topped = kassa_topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &outcome(&topped, ProviderStatus::Paid))
        .await
        .unwrap();
    let requested = inbox
        .refund(
            topped.id,
            &refund_command(None, RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    switch_provider(&kit, KASSA, false).await;
    assert_eq!(
        kit.services
            .checkout()
            .topup(
                client_id,
                Money::new(500, Currency::Usd),
                KASSA,
                None,
                Channel::Cabinet,
            )
            .await
            .unwrap_err()
            .code(),
        "checkout.method_unavailable"
    );
    let body = serde_json::json!({
        "event": "refund:kr-1",
        "payment": topped.public_id.to_string(),
        "txid": topped.provider_txid,
        "status": "refunded",
        "amount": 1_000,
    });
    let reply = inbox.handle(kassa_hook(KASSA_SECRET, &body)).await.unwrap();
    assert_eq!(reply.reply, kassa_reply());
    assert_eq!(
        payment(&kit, topped.id).await.status,
        PaymentStatus::Refunded
    );
    assert_eq!(
        provider_refunds(&kit, topped.id).await[0].status,
        ProviderRefundStatus::Succeeded
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn вебхук_ненастроенной_системы_получает_404_и_отказ_пишется_без_тела() {
    let clock = Arc::new(crate::FixedClock::new(moment(chrono::Duration::zero())));
    let Some(kit) = kassa_kit_with_clock(Arc::new(Kassa::default()), clock).await else {
        return;
    };
    let hook = |provider: &str, fill: u8, ip: &str| InboundWebhook {
        provider: provider.into(),
        request: WebhookRequest {
            headers: vec![("user-agent".into(), "probe/1.0".into())],
            body: Bytes::from(vec![fill; 4096]),
            remote_ip: ip.parse().ok(),
            route_suffix: None,
            query: None,
        },
    };
    let inbox = kit.services.inbox();
    for _ in 0..2 {
        let reply = inbox
            .handle(hook(providers::OVERPAY, b'x', "198.51.100.7"))
            .await
            .unwrap();
        assert_eq!(reply.reply.status(), 404);
    }
    let rows = inbox_rows(&kit).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].source, providers::OVERPAY);
    assert_eq!(rows[0].outcome, "not_accepted");
    assert_eq!(rows[0].body, None);
    assert_eq!(rows[0].payment_id, None);
    assert!(rows[0].dedupe_key.starts_with("rejected:"));
    let unknown = inbox
        .handle(hook("nope", b'x', "198.51.100.7"))
        .await
        .unwrap();
    assert_eq!(unknown.reply.status(), 404);
    assert_eq!(inbox_rows(&kit).await.len(), 1);
    let internal = inbox
        .handle(hook(providers::BALANCE, b'x', "198.51.100.7"))
        .await
        .unwrap();
    assert_eq!(internal.reply.status(), 404);
    let rows = inbox_rows(&kit).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1].source, providers::BALANCE);
    assert_eq!(rows[1].outcome, "not_accepted");
    assert_eq!(rows[1].body, None);
    for fill in 0..40u8 {
        let reply = inbox
            .handle(hook(providers::OVERPAY, fill, "198.51.100.8"))
            .await
            .unwrap();
        assert_eq!(reply.reply.status(), 404);
    }
    let rows = inbox_rows(&kit).await;
    assert_eq!(rows.len(), 2 + REJECTED_WEBHOOKS.limit as usize, "{rows:?}");
    assert!(rows.iter().all(|r| r.body.is_none()));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn слишком_большое_тело_вебхука_отклоняется_без_записи() {
    let Some(kit) = kit().await else { return };
    let mut hook = crypto_hook(Uuid::new_v4(), "6.99", 1);
    hook.request.body = Bytes::from(vec![b' '; 200_000]);
    let reply = kit.services.inbox().handle(hook).await.unwrap();
    assert_eq!(reply.reply.status(), 413);
    assert!(inbox_rows(&kit).await.is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn старые_записи_вебхуков_удаляются() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 142).await;
    let tariff = standard(&kit.services).await;
    let pending_payment = pending(
        &kit,
        client_id,
        &tariff,
        providers::CRYPTOPAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    let now = kit.services.now();
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        for (key, outcome, age_days) in [
            ("old-accepted", "accepted", 40),
            ("old-rejected", "rejected_signature", 4),
            ("fresh-accepted", "accepted", 1),
            ("fresh-rejected", "rejected_payload", 1),
        ] {
            sqlx::query(
                "INSERT INTO webhook_inbox (source, dedupe_key, outcome, body, received_at) \
                 VALUES ('cryptopay', $1, $2, $3, $4)",
            )
            .bind(key)
            .bind(outcome)
            .bind(Some(b"{}".to_vec()))
            .bind(now - chrono::Duration::days(age_days))
            .execute(conn.conn())
            .await
            .unwrap();
        }
    }
    assert_eq!(
        kit.services
            .inbox()
            .handle(crypto_hook(pending_payment.public_id, "6.99", 1))
            .await
            .unwrap()
            .reply
            .status(),
        200
    );
    let keys: Vec<String> = inbox_rows(&kit)
        .await
        .into_iter()
        .map(|r| r.dedupe_key)
        .collect();
    assert_eq!(keys.len(), 3, "{keys:?}");
    assert!(keys.contains(&"fresh-accepted".to_string()));
    assert!(keys.contains(&"fresh-rejected".to_string()));
    assert!(!keys.contains(&"old-accepted".to_string()));
    assert!(!keys.contains(&"old-rejected".to_string()));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn подлинное_уведомление_отклонённое_проверкой_хранится_тридцать_дней() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 152).await;
    let tariff = standard(&kit.services).await;
    let foreign = invoice(&kit, client_id, &tariff).await;
    let topped = kassa_topup(&kit, client_id, 500).await;
    let inbox = kit.services.inbox();
    let provider_mismatch = serde_json::json!({
        "event": "pay:1",
        "payment": foreign.public_id.to_string(),
        "txid": "k-foreign",
        "status": "paid",
        "amount": 699,
    });
    let txid_mismatch = serde_json::json!({
        "event": "pay:2",
        "payment": topped.public_id.to_string(),
        "txid": "k-other",
        "status": "paid",
        "amount": 500,
    });
    for body in [&provider_mismatch, &txid_mismatch] {
        assert_eq!(
            inbox
                .handle(kassa_hook(KASSA_SECRET, body))
                .await
                .unwrap()
                .reply,
            kassa_reply()
        );
    }
    for id in [foreign.id, topped.id] {
        assert_eq!(payment(&kit, id).await.status, PaymentStatus::Pending);
    }
    let rows = inbox_rows(&kit).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.iter()
            .all(|r| r.outcome == "rejected_business" && r.body.is_some()),
        "{rows:?}"
    );
    assert_eq!(
        inbox
            .handle(kassa_hook("guess", &provider_mismatch))
            .await
            .unwrap()
            .reply
            .status(),
        401
    );
    let age = |days: i32| {
        let store = kit.services.store().clone();
        async move {
            let mut conn = store.conn().await.unwrap();
            sqlx::query(
                "UPDATE webhook_inbox SET received_at = received_at - make_interval(days => $1)",
            )
            .bind(days)
            .execute(conn.conn())
            .await
            .unwrap();
        }
    };
    age(10).await;
    assert_eq!(inbox.purge_webhooks().await.unwrap(), 1);
    let rows = inbox_rows(&kit).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|r| r.outcome == "rejected_business"));
    age(WEBHOOK_RETENTION_DAYS as i32).await;
    assert_eq!(inbox.purge_webhooks().await.unwrap(), 2);
    assert!(inbox_rows(&kit).await.is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повтор_уже_обработанного_вебхука_получает_ответ_в_формате_кассы() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 150).await;
    let topped = kassa_topup(&kit, client_id, 500).await;
    let body = serde_json::json!({
        "event": "pay:1",
        "payment": topped.public_id.to_string(),
        "txid": topped.provider_txid,
        "status": "paid",
        "amount": 500,
    });
    let inbox = kit.services.inbox();
    let first = inbox.handle(kassa_hook(KASSA_SECRET, &body)).await.unwrap();
    assert_eq!(first.reply, kassa_reply());
    let second = inbox.handle(kassa_hook(KASSA_SECRET, &body)).await.unwrap();
    assert_eq!(second.reply, kassa_reply());
    assert_eq!(balance(&kit.services, client_id).await, 500);
    let forged = inbox.handle(kassa_hook("guess", &body)).await.unwrap();
    assert_eq!(forged.reply.status(), 401);
    let rows = inbox_rows(&kit).await;
    assert_eq!(
        rows.iter().filter(|r| r.body.is_some()).count(),
        1,
        "{rows:?}"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказ_кассы_в_возврате_снимает_ожидание_и_позволяет_вернуть_снова() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 131).await;
    let topped = kassa_topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &outcome(&topped, ProviderStatus::Paid))
        .await
        .unwrap();
    let requested = inbox
        .refund(
            topped.id,
            &refund_command(Some(600), RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    assert_eq!(requested.refund_id, None);
    let declined = WebhookOutcome {
        event_key: format!("{}:refund:kr-1", topped.provider_txid.clone().unwrap()),
        amount: Some(Amount::new(600, PayCurrency::Usd)),
        ..outcome(&topped, ProviderStatus::RefundDeclined)
    };
    assert_eq!(
        inbox.apply_outcome(KASSA, &declined).await.unwrap(),
        AcceptResult::StatusChanged {
            payment_id: topped.id,
            status: PaymentStatus::Paid
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    assert_eq!(payment(&kit, topped.id).await.status, PaymentStatus::Paid);
    assert_eq!(refunded_events(&kit, client_id).await, 0);
    let requests = provider_refunds(&kit, topped.id).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].status, ProviderRefundStatus::Declined);
    assert_eq!(requests[0].id, requested.request_id.unwrap());
    assert!(requests[0].failure_reason.is_some());
    assert!(
        audit_actions(&kit, topped.id)
            .await
            .contains(&"payment.refund_declined".to_string())
    );
    assert!(matches!(
        inbox.apply_outcome(KASSA, &declined).await.unwrap(),
        AcceptResult::Ignored { .. }
    ));
    *kassa.refund_completed.lock().unwrap() = true;
    let refunded = inbox
        .refund(
            topped.id,
            &refund_command(None, RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert!(!refunded.pending);
    assert!(refunded.refund_id.is_some());
    assert_eq!(refunded.status, PaymentStatus::Refunded);
    assert_eq!(refunded.amount, Amount::new(1_000, PayCurrency::Usd));
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(kassa.refunds.lock().unwrap().len(), 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn частичные_ожидающие_возвраты_не_превышают_оплату() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 135).await;
    let topped = kassa_topup(&kit, client_id, 1_000).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &outcome(&topped, ProviderStatus::Paid))
        .await
        .unwrap();
    for minor in [300, 500] {
        assert!(
            inbox
                .refund(
                    topped.id,
                    &refund_command(Some(minor), RefundSubscriptionAction::Keep, None),
                    admin,
                )
                .await
                .unwrap()
                .pending
        );
    }
    assert_eq!(balance(&kit.services, client_id).await, 200);
    assert_eq!(
        inbox
            .refund(
                topped.id,
                &refund_command(Some(300), RefundSubscriptionAction::Keep, None),
                admin,
            )
            .await
            .unwrap_err()
            .code(),
        "refund.pending"
    );
    assert_eq!(kassa.refunds.lock().unwrap().len(), 2);
    assert_eq!(balance(&kit.services, client_id).await, 200);
    let confirm = |id: &str, minor: i64| WebhookOutcome {
        event_key: format!("refund:{id}"),
        ..outcome(
            &topped,
            ProviderStatus::PartiallyRefunded {
                refunded: Amount::new(minor, PayCurrency::Usd),
            },
        )
    };
    assert_eq!(
        inbox
            .apply_outcome(KASSA, &confirm("kr-2", 500))
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: topped.id,
            status: PaymentStatus::PartiallyRefunded
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 200);
    assert_eq!(
        inbox
            .apply_outcome(KASSA, &confirm("kr-1", 300))
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: topped.id,
            status: PaymentStatus::PartiallyRefunded
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 200);
    let requests = provider_refunds(&kit, topped.id).await;
    assert!(
        requests
            .iter()
            .all(|r| r.status == ProviderRefundStatus::Succeeded && r.refund_id.is_some())
    );
    assert_eq!(refunded_events(&kit, client_id).await, 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отключение_подписки_при_возврате_ждёт_подтверждения_кассы() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 132).await;
    let tariff = standard(&kit.services).await;
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(&tariff, 90, None),
                provider: KASSA.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let paid = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &outcome(&paid, ProviderStatus::Paid))
        .await
        .unwrap();
    let done = kit.services.fulfillment().run(paid.order_id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let bought = order(&kit.services, paid.order_id).await;
    let subscription = subscription(&kit.services, &bought).await;
    let requested = inbox
        .refund(
            paid.id,
            &refund_command(None, RefundSubscriptionAction::Disable, Some("Не подошёл")),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    assert_eq!(requested.subscription_error, None);
    assert_eq!(
        remna_user(&kit, &subscription).await.status,
        sn_remnawave::UserStatus::Active
    );
    let confirmed = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        ..outcome(&paid, ProviderStatus::Refunded)
    };
    assert_eq!(
        inbox.apply_outcome(KASSA, &confirmed).await.unwrap(),
        AcceptResult::StatusChanged {
            payment_id: paid.id,
            status: PaymentStatus::Refunded
        }
    );
    assert_eq!(
        remna_user(&kit, &subscription).await.status,
        sn_remnawave::UserStatus::Disabled
    );
    assert_eq!(
        order(&kit.services, paid.order_id).await.status,
        OrderStatus::Refunded
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let actions: Vec<String> = conn
        .audit()
        .for_entity("subscription", subscription.id.0, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect();
    assert_eq!(actions, vec!["subscription.refund_action".to_string()]);
    let refunds = conn.refunds().for_payment(paid.id).await.unwrap();
    assert_eq!(
        refunds[0].subscription_action,
        RefundSubscriptionAction::Disable
    );
    assert_eq!(refunds[0].reason.as_deref(), Some("Не подошёл"));
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_с_отключением_уже_отключённой_подписки_проходит_без_ошибки() {
    let kassa = Arc::new(Kassa::default());
    *kassa.refund_completed.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 133).await;
    let tariff = standard(&kit.services).await;
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(&tariff, 30, None),
                provider: KASSA.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let paid = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &outcome(&paid, ProviderStatus::Paid))
        .await
        .unwrap();
    let done = kit.services.fulfillment().run(paid.order_id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let bought = order(&kit.services, paid.order_id).await;
    let subscription = subscription(&kit.services, &bought).await;
    let subscription = set_user(&kit, &subscription, |user| {
        user.status = sn_remnawave::UserStatus::Disabled;
    })
    .await;
    let refunded = inbox
        .refund(
            paid.id,
            &refund_command(
                None,
                RefundSubscriptionAction::Disable,
                Some("Повторная жалоба"),
            ),
            admin,
        )
        .await
        .unwrap();
    assert!(!refunded.pending);
    assert_eq!(refunded.subscription_error, None);
    assert_eq!(refunded.status, PaymentStatus::Refunded);
    assert_eq!(
        remna_user(&kit, &subscription).await.status,
        sn_remnawave::UserStatus::Disabled
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let actions: Vec<String> = conn
        .audit()
        .for_entity("subscription", subscription.id.0, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect();
    assert_eq!(actions, vec!["subscription.refund_action".to_string()]);
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ожидающий_возврат_уточняется_опросом_кассы() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 134).await;
    let first = kassa_topup(&kit, client_id, 1_000).await;
    let second = kassa_topup(&kit, client_id, 400).await;
    let inbox = kit.services.inbox();
    for topped in [&first, &second] {
        inbox
            .apply_outcome(KASSA, &outcome(topped, ProviderStatus::Paid))
            .await
            .unwrap();
        inbox
            .refund(
                topped.id,
                &refund_command(None, RefundSubscriptionAction::Keep, None),
                admin,
            )
            .await
            .unwrap();
    }
    assert_eq!(balance(&kit.services, client_id).await, 0);
    *kassa.refund_status.lock().unwrap() = Some(WebhookOutcome {
        event_key: "refund:kr-1".into(),
        payment: None,
        provider_txid: None,
        status: ProviderStatus::Pending,
        amount: None,
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    });
    assert_eq!(
        inbox.poll_payment(first.id).await.unwrap(),
        PollResult::Pending
    );
    assert_eq!(
        kassa.lookups.lock().unwrap().clone(),
        vec!["refund:kr-1".to_string()]
    );
    *kassa.refund_status.lock().unwrap() = Some(WebhookOutcome {
        event_key: "refund:kr-1".into(),
        payment: None,
        provider_txid: None,
        status: ProviderStatus::Refunded,
        amount: Some(Amount::new(1_000, PayCurrency::Usd)),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    });
    assert_eq!(
        inbox.poll_payment(first.id).await.unwrap(),
        PollResult::Settled
    );
    assert_eq!(
        payment(&kit, first.id).await.status,
        PaymentStatus::Refunded
    );
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(
        inbox.poll_payment(first.id).await.unwrap(),
        PollResult::Settled
    );
    *kassa.refund_status.lock().unwrap() = Some(WebhookOutcome {
        event_key: "refund:kr-2".into(),
        payment: None,
        provider_txid: None,
        status: ProviderStatus::RefundDeclined,
        amount: Some(Amount::new(400, PayCurrency::Usd)),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    });
    assert_eq!(
        inbox.poll_refunds(10).await.unwrap(),
        RefundPollReport::default()
    );
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE provider_refunds SET created_at = created_at - interval '10 minutes'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    assert_eq!(
        inbox.poll_refunds(10).await.unwrap(),
        RefundPollReport {
            settled: 1,
            ..RefundPollReport::default()
        }
    );
    assert_eq!(payment(&kit, second.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        provider_refunds(&kit, second.id).await[0].status,
        ProviderRefundStatus::Declined
    );
    assert_eq!(balance(&kit.services, client_id).await, 400);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_повторной_оплаты_сторнирует_баланс_и_не_трогает_заказ() {
    let Some(kit) = kit().await else { return };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 123).await;
    let tariff = standard(&kit.services).await;
    let first = pending(
        &kit,
        client_id,
        &tariff,
        TEST_PAY,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    let inbox = kit.services.inbox();
    for txid in ["op-1", "op-2"] {
        inbox
            .apply_outcome(TEST_PAY, &paid_by(&first, txid))
            .await
            .unwrap();
    }
    let repeat = by_txid(&kit, TEST_PAY, "op-2").await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 699);
    let to_balance = RefundCommand {
        amount: None,
        method: RefundMethod::Balance,
        subscription_action: RefundSubscriptionAction::Keep,
        reason: None,
        key: None,
        outside: false,
    };
    assert_eq!(
        inbox
            .refund(repeat.id, &to_balance, admin)
            .await
            .unwrap_err()
            .code(),
        "refund.topup_to_balance"
    );
    let refunded = inbox
        .refund(
            repeat.id,
            &refund_command(None, RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    assert_eq!(refunded.status, PaymentStatus::Refunded);
    assert_eq!(balance(&kit.services, client_id).await, 0);
    assert_eq!(payment(&kit, first.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, first.order_id).await.status,
        OrderStatus::Paid
    );
    assert_eq!(
        kit.pay.refunds.lock().unwrap()[0].provider_txid,
        "op-2".to_string()
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_звёзд_за_повторную_оплату_относится_к_повторному_платежу() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 124).await;
    let tariff = standard(&kit.services).await;
    let stars = pending(
        &kit,
        client_id,
        &tariff,
        providers::STARS,
        Amount::new(350, PayCurrency::Xtr),
        None,
    )
    .await;
    let inbox = kit.services.inbox();
    for charge in ["charge-1", "charge-2"] {
        inbox
            .apply_outcome(providers::STARS, &paid_by(&stars, charge))
            .await
            .unwrap();
    }
    let repeat = by_txid(&kit, providers::STARS, "charge-2").await.unwrap();
    let refunded = WebhookOutcome {
        event_key: "refunded_payment:charge-2".into(),
        status: ProviderStatus::Refunded,
        ..paid_by(&stars, "charge-2")
    };
    assert_eq!(
        inbox
            .apply_outcome(providers::STARS, &refunded)
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: repeat.id,
            status: PaymentStatus::Refunded
        }
    );
    assert_eq!(payment(&kit, stars.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, stars.order_id).await.status,
        OrderStatus::Paid
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повторная_оплата_через_вебхук_видна_в_журнале_вебхуков() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 151).await;
    let topped = kassa_topup(&kit, client_id, 500).await;
    let inbox = kit.services.inbox();
    for (event, txid) in [
        ("pay:1", topped.provider_txid.clone().unwrap()),
        ("pay:2", "k-again".to_string()),
    ] {
        let body = serde_json::json!({
            "event": event,
            "payment": topped.public_id.to_string(),
            "txid": txid,
            "status": "paid",
            "amount": 500,
        });
        assert_eq!(
            inbox
                .handle(kassa_hook(KASSA_SECRET, &body))
                .await
                .unwrap()
                .reply,
            kassa_reply()
        );
    }
    let repeat = by_txid(&kit, KASSA, "k-again").await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    let paid_ids: Vec<Option<i64>> = inbox_rows(&kit)
        .await
        .into_iter()
        .map(|r| r.payment_id)
        .collect();
    assert_eq!(paid_ids, vec![Some(topped.id.0), Some(repeat.id.0)]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn автосписание_в_ожидании_подтверждается_по_номеру_транзакции() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let client_id = client(&kit.services, 133).await;
    let tariff = standard(&kit.services).await;
    let now = kit.services.now();
    let charge = pending(
        &kit,
        client_id,
        &tariff,
        KASSA,
        Amount::new(699, PayCurrency::Usd),
        Some(now - chrono::Duration::minutes(5)),
    )
    .await;
    assert_eq!(charge.provider_txid, None);
    let inbox = kit.services.inbox();
    inbox.track_pending(charge.id, "ch-77").await.unwrap();
    inbox.track_pending(charge.id, "ch-77").await.unwrap();
    assert_eq!(
        payment(&kit, charge.id).await.provider_txid.as_deref(),
        Some("ch-77")
    );
    assert_eq!(
        inbox
            .track_pending(charge.id, "ch-78")
            .await
            .unwrap_err()
            .code(),
        "payment.txid_mismatch"
    );
    *kassa.status.lock().unwrap() = Some(WebhookOutcome {
        payment: None,
        ..outcome(&charge, ProviderStatus::Pending)
    });
    assert_eq!(
        inbox.confirm_by_txid(charge.id, "ch-77").await.unwrap(),
        PollResult::Pending
    );
    {
        let mut tx = kit.services.store().begin().await.unwrap();
        assert_eq!(
            tx.payments().expire_stale(now, 10).await.unwrap(),
            vec![charge.id]
        );
        tx.orders()
            .transition(charge.order_id, OrderStatus::Canceled)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(
        inbox.poll_payment(charge.id).await.unwrap(),
        PollResult::Settled
    );
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Canceled
    );
    *kassa.status.lock().unwrap() = Some(WebhookOutcome {
        payment: None,
        provider_txid: Some("ch-77".into()),
        ..outcome(&charge, ProviderStatus::Paid)
    });
    assert_eq!(
        inbox.confirm_by_txid(charge.id, "ch-77").await.unwrap(),
        PollResult::Settled
    );
    assert_eq!(
        kassa.lookups.lock().unwrap().clone(),
        vec!["ch-77".to_string(), "ch-77".to_string()]
    );
    assert_eq!(payment(&kit, charge.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, charge.order_id).await.status,
        OrderStatus::Paid
    );
    assert!(
        jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder {
                order_id: charge.order_id
            })
    );
    assert_eq!(
        inbox.confirm_by_txid(charge.id, "ch-77").await.unwrap(),
        PollResult::Settled
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказы_с_одного_адреса_записываются_не_больше_предела() {
    let clock = Arc::new(crate::FixedClock::new(moment(chrono::Duration::zero())));
    let Some(kit) = kassa_kit_with_clock(Arc::new(Kassa::default()), clock).await else {
        return;
    };
    let inbox = kit.services.inbox();
    for update in 0..40 {
        let mut forged = crypto_hook(Uuid::new_v4(), "6.99", update);
        forged.request.headers = vec![("crypto-pay-api-signature".into(), "00".repeat(32))];
        forged.request.remote_ip = "198.51.100.200".parse().ok();
        assert_eq!(inbox.handle(forged).await.unwrap().reply.status(), 401);
    }
    let rows = inbox_rows(&kit).await;
    assert_eq!(rows.len(), REJECTED_WEBHOOKS.limit as usize);
    assert!(rows.iter().all(|r| r.body.is_none()
        && r.outcome == "rejected_signature"
        && r.dedupe_key.starts_with("rejected:")));
    let mut other = crypto_hook(Uuid::new_v4(), "6.99", 99);
    other.request.headers = vec![("crypto-pay-api-signature".into(), "00".repeat(32))];
    other.request.remote_ip = "198.51.100.201".parse().ok();
    assert_eq!(inbox.handle(other).await.unwrap().reply.status(), 401);
    assert_eq!(
        inbox_rows(&kit).await.len(),
        REJECTED_WEBHOOKS.limit as usize + 1
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказы_с_адресов_одной_ipv6_сети_считаются_вместе() {
    let clock = Arc::new(crate::FixedClock::new(moment(chrono::Duration::zero())));
    let Some(kit) = kassa_kit_with_clock(Arc::new(Kassa::default()), clock).await else {
        return;
    };
    let inbox = kit.services.inbox();
    for host in 1..=40 {
        let mut forged = crypto_hook(Uuid::new_v4(), "6.99", host);
        forged.request.headers = vec![("crypto-pay-api-signature".into(), "00".repeat(32))];
        forged.request.remote_ip = format!("2001:db8:1:2::{host:x}").parse().ok();
        assert_eq!(inbox.handle(forged).await.unwrap().reply.status(), 401);
    }
    assert_eq!(
        inbox_rows(&kit).await.len(),
        REJECTED_WEBHOOKS.limit as usize
    );
    let mut other = crypto_hook(Uuid::new_v4(), "6.99", 99);
    other.request.headers = vec![("crypto-pay-api-signature".into(), "00".repeat(32))];
    other.request.remote_ip = "2001:db8:1:3::1".parse().ok();
    assert_eq!(inbox.handle(other).await.unwrap().reply.status(), 401);
    assert_eq!(
        inbox_rows(&kit).await.len(),
        REJECTED_WEBHOOKS.limit as usize + 1
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn чистка_журнала_вебхуков_для_воркера() {
    let Some(kit) = kit().await else { return };
    let now = kit.services.now();
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        for n in 0..3 {
            sqlx::query(
                "INSERT INTO webhook_inbox (source, dedupe_key, outcome, body, received_at) \
                 VALUES ('cryptopay', $1, 'accepted', NULL, $2)",
            )
            .bind(format!("old-{n}"))
            .bind(now - chrono::Duration::days(WEBHOOK_RETENTION_DAYS + 1))
            .execute(conn.conn())
            .await
            .unwrap();
        }
    }
    assert_eq!(kit.services.inbox().purge_webhooks().await.unwrap(), 3);
    assert!(inbox_rows(&kit).await.is_empty());
    assert_eq!(kit.services.inbox().purge_webhooks().await.unwrap(), 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ожидающее_списание_запоминает_транзакцию_для_опроса() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let client_id = client(&kit.services, 136).await;
    let tariff = standard(&kit.services).await;
    let now = kit.services.now();
    let charge = pending(
        &kit,
        client_id,
        &tariff,
        KASSA,
        Amount::new(699, PayCurrency::Usd),
        Some(now + chrono::Duration::hours(1)),
    )
    .await;
    let inbox = kit.services.inbox();
    let waiting = WebhookOutcome {
        payment: Some(PaymentRef::Payment(charge.id)),
        provider_txid: Some("ch-5".into()),
        ..outcome(&charge, ProviderStatus::Pending)
    };
    assert!(matches!(
        inbox.apply_outcome(KASSA, &waiting).await.unwrap(),
        AcceptResult::Ignored { .. }
    ));
    assert_eq!(
        payment(&kit, charge.id).await.provider_txid.as_deref(),
        Some("ch-5")
    );
    let by_webhook = WebhookOutcome {
        payment: Some(PaymentRef::PublicId(charge.public_id)),
        provider_txid: Some("ch-6".into()),
        ..waiting.clone()
    };
    inbox.apply_outcome(KASSA, &by_webhook).await.unwrap();
    assert_eq!(
        payment(&kit, charge.id).await.provider_txid.as_deref(),
        Some("ch-5")
    );
    *kassa.status.lock().unwrap() = Some(WebhookOutcome {
        payment: None,
        ..waiting.clone()
    });
    assert_eq!(
        inbox.poll_payment(charge.id).await.unwrap(),
        PollResult::Pending
    );
    *kassa.status.lock().unwrap() = Some(WebhookOutcome {
        payment: None,
        status: ProviderStatus::Paid,
        ..waiting
    });
    assert_eq!(
        inbox.poll_payment(charge.id).await.unwrap(),
        PollResult::Settled
    );
    assert_eq!(
        kassa.lookups.lock().unwrap().clone(),
        vec!["ch-5".to_string(), "ch-5".to_string()]
    );
    assert_eq!(payment(&kit, charge.id).await.status, PaymentStatus::Paid);
    kit.db.cleanup().await;
}
