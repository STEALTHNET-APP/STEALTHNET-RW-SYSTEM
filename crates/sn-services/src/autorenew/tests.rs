use super::*;
use crate::SystemClock;
use crate::checkout::testkit::*;
use crate::inbox::PollResult;
use crate::services::{EnvFallbacks, ServiceDeps};
use async_trait::async_trait;
use chrono::Duration as Days;
use sn_domain::ids::ClientId;
use sn_domain::order::{Order, OrderKind};
use sn_domain::payment::Payment;
use sn_domain::settings::keys;
use sn_modules::{BalanceChange, CoreApi, ModuleRegistry};
use sn_payments::providers::Balance;
use sn_payments::{
    ClientLabel, Invoice, PaymentError, PaymentResult, ProviderConfig, ProviderRegistry,
    WebhookRequest,
};
use sn_remnawave::fake::FakeRemnawave;
use sn_remnawave::{InternalSquad, SquadInfo};
use sn_store::SecretBox;
use sn_store::testing::test_store;
use std::collections::VecDeque;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct FlakyPay {
    script: Mutex<VecDeque<PaymentResult<ProviderStatus>>>,
    requests: Mutex<Vec<InvoiceRequest>>,
    status: Mutex<Option<ProviderStatus>>,
}

impl FlakyPay {
    pub(crate) fn answering(script: Vec<PaymentResult<ProviderStatus>>) -> Arc<Self> {
        Arc::new(FlakyPay {
            script: Mutex::new(script.into()),
            ..FlakyPay::default()
        })
    }

    pub(crate) fn requests(&self) -> Vec<InvoiceRequest> {
        self.requests.lock().unwrap().clone()
    }
}

fn flaky_txid(request: &InvoiceRequest) -> String {
    format!("fp-{}", request.payment_public_id)
}

#[async_trait]
impl PaymentProvider for FlakyPay {
    fn id(&self) -> &'static str {
        TEST_PAY
    }

    fn title(&self) -> &'static str {
        "Касса с обрывами"
    }

    fn currencies(&self) -> &'static [Currency] {
        &Currency::ALL
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Касса с обрывами",
        }
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn supports_recurring(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        _config: &ProviderConfig,
        _request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        Err(PaymentError::Unsupported("счета"))
    }

    async fn parse_webhook(
        &self,
        _config: &ProviderConfig,
        _request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::BadSignature)
    }

    async fn fetch_status(
        &self,
        _config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let requests = self.requests();
        let Some(request) = requests.iter().find(|r| flaky_txid(r) == external_id) else {
            return Err(PaymentError::Rejected("платёж не найден".into()));
        };
        Ok(self.status.lock().unwrap().map(|status| WebhookOutcome {
            event_key: format!("status:{external_id}"),
            payment: None,
            provider_txid: Some(external_id.to_string()),
            status,
            amount: Some(request.amount),
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        }))
    }

    async fn charge_saved(
        &self,
        _config: &ProviderConfig,
        request: &InvoiceRequest,
        _token: &str,
    ) -> PaymentResult<WebhookOutcome> {
        self.requests.lock().unwrap().push(request.clone());
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(PaymentError::Rejected("ответ не задан".into())));
        let status = next?;
        Ok(WebhookOutcome {
            event_key: format!("charge:{}", request.payment_public_id),
            payment: Some(PaymentRef::Payment(request.payment_id)),
            provider_txid: Some(flaky_txid(request)),
            status,
            amount: Some(request.amount),
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        })
    }
}

pub(crate) async fn flaky_kit(pay: Arc<FlakyPay>) -> Option<Kit> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![pay, Arc::new(Balance)])),
        modules: Arc::new(ModuleRegistry::new(Vec::new()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &serde_json::json!(PUBLIC_URL), None)
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    remna.set_squads(
        [1, 2, 3, 9]
            .into_iter()
            .map(|n| InternalSquad {
                uuid: Uuid::from_u128(n),
                name: format!("Сквад {n}"),
                view_position: 0,
                info: SquadInfo::default(),
                inbounds: Vec::new(),
            })
            .collect(),
    );
    services.set_remna(Some(remna.clone()));
    Some(Kit {
        db,
        services,
        remna,
        pay: Arc::new(TestPay::default()),
    })
}

async fn drain_to(kit: &Kit, client_id: ClientId, keep: i64) {
    let current = balance(&kit.services, client_id).await;
    kit.services
        .change_balance(BalanceChange {
            client_id,
            wallet: Wallet::Main,
            amount: Money::new(keep - current, Currency::Usd),
            kind: LedgerKind::Purchase,
            comment: None,
            idempotency_key: format!("drain:{}", Uuid::new_v4()),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
}

async fn due_subscription(
    kit: &Kit,
    client_id: ClientId,
    offset: Days,
    status: SubscriptionStatus,
) -> Subscription {
    let tariff = standard(&kit.services).await;
    let subscription = provision(kit, client_id, &tariff, 90).await;
    expire_in(kit, &subscription, moment(offset), status).await
}

pub(crate) async fn renewals(kit: &Kit, subscription: &Subscription) -> Vec<(Order, Vec<Payment>)> {
    let mut conn = kit.services.store().conn().await.unwrap();
    let orders = conn
        .orders()
        .for_client(subscription.client_id, Default::default())
        .await
        .unwrap()
        .items;
    let mut out = Vec::new();
    for order in orders
        .into_iter()
        .filter(|o| o.kind == OrderKind::Renewal && o.subscription_id == Some(subscription.id))
    {
        let payments = conn.payments().for_order(order.id).await.unwrap();
        out.push((order, payments));
    }
    out
}

async fn attempts(kit: &Kit, subscription: &Subscription) -> u32 {
    let cycle = subscription
        .mirror
        .expire_at
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.autorenew()
        .attempts(subscription.id, &cycle)
        .await
        .unwrap()
}

pub(crate) fn notifications(jobs: &[JobPayload], name: &str) -> usize {
    jobs.iter()
        .filter(|job| matches!(job, JobPayload::Notify { template, .. } if template == name))
        .count()
}

#[tokio::test]
async fn автопродление_списывает_с_баланса_один_раз_за_период() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 301).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    assert!(subscription.autorenew.enabled);
    let before = balance(&kit.services, client_id).await;
    let now = kit.services.now();
    let autorenew = kit.services.autorenew();
    assert_eq!(
        autorenew.run_due(now).await.unwrap(),
        AutoRenewReport {
            due: 1,
            charged: 1,
            reminded: 0,
            failed: 0
        }
    );
    assert_eq!(
        autorenew.run_due(now).await.unwrap(),
        AutoRenewReport {
            due: 1,
            charged: 0,
            reminded: 0,
            failed: 0
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, before - 699);
    let renewals = renewals(&kit, &subscription).await;
    assert_eq!(renewals.len(), 1);
    let (order, payments) = &renewals[0];
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(payments[0].provider.as_str(), providers::BALANCE);
    assert_eq!(payments[0].method.as_deref(), Some(AUTORENEW_METHOD));
    let mut conn = kit.services.store().conn().await.unwrap();
    let entries = conn
        .ledger()
        .entries(client_id, Default::default())
        .await
        .unwrap();
    assert_eq!(entries.items[0].kind, LedgerKind::AutoRenew);
    drop(conn);
    assert!(
        jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder { order_id: order.id })
    );
    let next_day = autorenew.run_due(now + Days::days(1)).await.unwrap();
    assert_eq!(
        next_day.charged, 0,
        "пока заказ не выдан, второго списания нет"
    );
    let expire = subscription.mirror.expire_at.unwrap();
    kit.services.fulfillment().run(order.id).await.unwrap();
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + Days::days(90)
    );
    assert_eq!(autorenew.run_due(now).await.unwrap().due, 0);
    assert_eq!(balance(&kit.services, client_id).await, before - 699);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn без_полной_суммы_списания_нет_только_напоминание() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 302).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    drain_to(&kit, client_id, 300).await;
    jobs(&kit.services).await;
    let now = kit.services.now();
    let autorenew = kit.services.autorenew();
    assert_eq!(
        autorenew.run_due(now).await.unwrap(),
        AutoRenewReport {
            due: 1,
            charged: 0,
            reminded: 1,
            failed: 0
        }
    );
    assert_eq!(autorenew.run_due(now).await.unwrap().reminded, 0);
    assert_eq!(attempts(&kit, &subscription).await, 1);
    let next_day = autorenew.run_due(now + Days::days(1)).await.unwrap();
    assert_eq!((next_day.charged, next_day.reminded), (0, 0));
    assert_eq!(attempts(&kit, &subscription).await, 2);
    assert_eq!(balance(&kit.services, client_id).await, 300);
    assert!(renewals(&kit, &subscription).await.is_empty());
    let queued = jobs(&kit.services).await;
    assert_eq!(notifications(&queued, "autorenew.reminder"), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заблокированная_подписка_не_списывается() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 303).await;
    let subscription = due_subscription(
        &kit,
        client_id,
        Days::hours(6),
        SubscriptionStatus::Disabled,
    )
    .await;
    let before = balance(&kit.services, client_id).await;
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!((report.due, report.charged), (1, 0));
    assert_eq!(balance(&kit.services, client_id).await, before);
    assert_eq!(attempts(&kit, &subscription).await, 1);
    assert!(renewals(&kit, &subscription).await.is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_срока_попыток_автопродление_выключается() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 304).await;
    let subscription =
        due_subscription(&kit, client_id, Days::days(-4), SubscriptionStatus::Expired).await;
    jobs(&kit.services).await;
    let before = balance(&kit.services, client_id).await;
    kit.services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    let mut conn = kit.services.store().conn().await.unwrap();
    let current = conn.subscriptions().get(subscription.id).await.unwrap();
    drop(conn);
    assert!(!current.autorenew.enabled);
    assert_eq!(
        current.autorenew.period_id,
        subscription.autorenew.period_id
    );
    assert_eq!(balance(&kit.services, client_id).await, before);
    let queued = jobs(&kit.services).await;
    assert_eq!(notifications(&queued, "autorenew.disabled"), 1);
    assert_eq!(
        kit.services
            .autorenew()
            .run_due(kit.services.now())
            .await
            .unwrap()
            .due,
        0
    );
    kit.db.cleanup().await;
}

async fn save_card(kit: &Kit, client_id: ClientId) {
    let sealed = kit
        .services
        .secrets()
        .unwrap()
        .seal_str(&saved_method_context(client_id, TEST_PAY), "pm-1")
        .unwrap();
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.saved_methods()
        .save(client_id, TEST_PAY, &sealed, Some("Visa •• 4242"))
        .await
        .unwrap();
}

#[tokio::test]
async fn сохранённая_карта_списывается_полной_суммой_без_баланса() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 305).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    drain_to(&kit, client_id, 100).await;
    save_card(&kit, client_id).await;
    *kit.pay.charge.lock().unwrap() = Some(ProviderStatus::Paid);
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!((report.charged, report.failed), (1, 0));
    let charges = kit.pay.charges.lock().unwrap().clone();
    assert_eq!(charges.len(), 1);
    assert_eq!(charges[0].1, "pm-1");
    assert_eq!(charges[0].0.amount, Money::new(699, Currency::Usd).into());
    assert_eq!(balance(&kit.services, client_id).await, 100);
    let renewals = renewals(&kit, &subscription).await;
    let (order, payments) = &renewals[0];
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(payments.len(), 1);
    assert_eq!(payments[0].provider.as_str(), TEST_PAY);
    assert_eq!(payments[0].status, PaymentStatus::Paid);
    assert_eq!(payments[0].method.as_deref(), Some(AUTORENEW_METHOD));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказ_карты_отменяет_заказ_и_предупреждает_клиента() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 306).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    drain_to(&kit, client_id, 0).await;
    save_card(&kit, client_id).await;
    jobs(&kit.services).await;
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!((report.charged, report.failed), (0, 1));
    let renewals = renewals(&kit, &subscription).await;
    let (order, payments) = &renewals[0];
    assert_eq!(order.status, OrderStatus::Canceled);
    assert_eq!(payments[0].status, PaymentStatus::Failed);
    let queued = jobs(&kit.services).await;
    assert_eq!(notifications(&queued, "autorenew.failed"), 1);
    assert_eq!(attempts(&kit, &subscription).await, 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn десять_тысяч_старых_истёкших_подписок_не_останавливают_автопродление() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 307).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    let before = balance(&kit.services, client_id).await;
    sqlx::query(
        "INSERT INTO subscriptions (client_id, kind, status, expire_at, autorenew_enabled) \
         SELECT $1, 'trial', 'expired', now() - interval '30 days' - make_interval(secs => g), g % 2 = 0 \
         FROM generate_series(1, 10000) AS g",
    )
    .bind(client_id.0)
    .execute(kit.services.store().pool())
    .await
    .unwrap();
    jobs(&kit.services).await;
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!(
        report,
        AutoRenewReport {
            due: 1,
            charged: 1,
            reminded: 0,
            failed: 0
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, before - 699);
    assert_eq!(renewals(&kit, &subscription).await.len(), 1);
    let queued = jobs(&kit.services).await;
    assert_eq!(notifications(&queued, "autorenew.disabled"), 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ожидание_ответа_кассы_сохраняет_номер_платежа_и_опрос_его_завершает() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 308).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    drain_to(&kit, client_id, 0).await;
    save_card(&kit, client_id).await;
    jobs(&kit.services).await;
    *kit.pay.charge.lock().unwrap() = Some(ProviderStatus::Pending);
    let now = kit.services.now();
    let report = kit.services.autorenew().run_due(now).await.unwrap();
    assert_eq!((report.charged, report.failed), (1, 0));
    let renewals = renewals(&kit, &subscription).await;
    let (order, payments) = &renewals[0];
    assert_eq!(order.status, OrderStatus::Pending);
    let payment = payments[0].clone();
    assert_eq!(payment.status, PaymentStatus::Pending);
    let txid = format!("tp-charge-{}", payment.id);
    assert_eq!(payment.provider_txid.as_deref(), Some(txid.as_str()));
    let expire_at = subscription.mirror.expire_at.unwrap();
    assert!(payment.expires_at.unwrap() > expire_at + Days::days(3));
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        let expired = conn
            .payments()
            .expire_stale(now + Days::hours(6), 100)
            .await
            .unwrap();
        assert!(!expired.contains(&payment.id));
    }
    *kit.pay.status.lock().unwrap() = Some(WebhookOutcome {
        event_key: format!("payment.succeeded:{txid}"),
        payment: None,
        provider_txid: Some(txid.clone()),
        status: ProviderStatus::Paid,
        amount: Some(payment.amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    });
    assert_eq!(
        kit.services.inbox().poll_payment(payment.id).await.unwrap(),
        PollResult::Settled
    );
    let renewals = self::renewals(&kit, &subscription).await;
    let (order, payments) = &renewals[0];
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(payments[0].status, PaymentStatus::Paid);
    assert!(
        jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder { order_id: order.id })
    );
    assert_eq!(kit.pay.charges.lock().unwrap().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn неизвестный_исход_списания_не_отменяется_по_сроку_счёта_и_не_списывается_снова() {
    let pay = FlakyPay::answering(vec![Err(PaymentError::Transport("обрыв связи".into()))]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let client_id = client(&kit.services, 309).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    drain_to(&kit, client_id, 0).await;
    save_card(&kit, client_id).await;
    let now = kit.services.now();
    let report = kit.services.autorenew().run_due(now).await.unwrap();
    assert_eq!((report.charged, report.failed), (1, 0));
    let renewals = renewals(&kit, &subscription).await;
    let (_, payments) = &renewals[0];
    let payment = payments[0].clone();
    assert_eq!(payment.status, PaymentStatus::Pending);
    assert_eq!(payment.provider_txid, None);
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        let expired = conn
            .payments()
            .expire_stale(now + Days::days(1), 100)
            .await
            .unwrap();
        assert!(!expired.contains(&payment.id));
    }
    kit.services
        .autorenew()
        .run_due(now + Days::days(1))
        .await
        .unwrap();
    assert_eq!(pay.requests().len(), 1);
    let mut conn = kit.services.store().conn().await.unwrap();
    assert_eq!(
        conn.payments().get(payment.id).await.unwrap().status,
        PaymentStatus::Pending
    );
    drop(conn);
    kit.db.cleanup().await;
}

pub(crate) async fn unknown_charge(kit: &Kit, telegram_id: i64) -> (Subscription, Payment) {
    let client_id = client(&kit.services, telegram_id).await;
    let subscription =
        due_subscription(kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    drain_to(kit, client_id, 0).await;
    save_card(kit, client_id).await;
    jobs(&kit.services).await;
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!((report.charged, report.failed), (1, 0));
    let payment = renewals(kit, &subscription).await[0].1[0].clone();
    assert_eq!(payment.status, PaymentStatus::Pending);
    (subscription, payment)
}

#[tokio::test]
async fn неизвестный_исход_выясняется_повтором_с_тем_же_ключом() {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Transport("обрыв связи".into())),
        Ok(ProviderStatus::Paid),
    ]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, payment) = unknown_charge(&kit, 340).await;
    assert_eq!(payment.provider_txid, None);
    let autorenew = kit.services.autorenew();
    assert_eq!(
        autorenew.check_charge(payment.id).await.unwrap(),
        Some(ChargeCheck::Settled)
    );
    let requests = pay.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].payment_public_id, payment.public_id);
    assert_eq!(requests[0].payment_public_id, requests[1].payment_public_id);
    assert_eq!(requests[0].amount, requests[1].amount);
    assert_eq!(requests[0].description, requests[1].description);
    assert_eq!(requests[0].client, requests[1].client);
    let renewals = renewals(&kit, &subscription).await;
    let (order, payments) = &renewals[0];
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(payments[0].status, PaymentStatus::Paid);
    assert_eq!(
        payments[0].provider_txid.as_deref(),
        Some(flaky_txid(&requests[0]).as_str())
    );
    assert!(
        jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder { order_id: order.id })
    );
    assert_eq!(
        autorenew.check_charge(payment.id).await.unwrap(),
        Some(ChargeCheck::Settled)
    );
    assert_eq!(pay.requests().len(), 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказ_кассы_при_повторе_отменяет_заказ_и_предупреждает_клиента() {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Http { status: 502 }),
        Ok(ProviderStatus::Canceled),
    ]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, payment) = unknown_charge(&kit, 341).await;
    assert_eq!(
        kit.services
            .autorenew()
            .check_charge(payment.id)
            .await
            .unwrap(),
        Some(ChargeCheck::Settled)
    );
    let renewals = renewals(&kit, &subscription).await;
    let (order, payments) = &renewals[0];
    assert_eq!(order.status, OrderStatus::Canceled);
    assert_eq!(payments[0].status, PaymentStatus::Canceled);
    let queued = jobs(&kit.services).await;
    assert_eq!(notifications(&queued, "autorenew.failed"), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn спорный_отказ_повтора_оставляет_платёж_до_ручной_проверки() {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Transport("обрыв связи".into())),
        Err(PaymentError::Rejected(
            "ключ идемпотентности уже использован с другим телом".into(),
        )),
    ]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, payment) = unknown_charge(&kit, 342).await;
    let checked = kit
        .services
        .autorenew()
        .check_charge(payment.id)
        .await
        .unwrap();
    assert!(
        matches!(&checked, Some(ChargeCheck::Unclear(reason)) if reason.contains("ключом")),
        "{checked:?}"
    );
    let renewals = renewals(&kit, &subscription).await;
    let (order, payments) = &renewals[0];
    assert_eq!(order.status, OrderStatus::Pending);
    assert_eq!(payments[0].status, PaymentStatus::Pending);
    assert_eq!(pay.requests().len(), 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повтор_после_окна_ключа_не_отправляется() {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Transport("обрыв связи".into())),
        Ok(ProviderStatus::Paid),
    ]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (_, payment) = unknown_charge(&kit, 343).await;
    sqlx::query("UPDATE payments SET created_at = now() - interval '21 hours' WHERE id = $1")
        .bind(payment.id.0)
        .execute(kit.services.store().pool())
        .await
        .unwrap();
    let checked = kit
        .services
        .autorenew()
        .check_charge(payment.id)
        .await
        .unwrap();
    assert!(
        matches!(checked, Some(ChargeCheck::Unclear(_))),
        "{checked:?}"
    );
    assert_eq!(pay.requests().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заблокированному_клиенту_повторный_запрос_списания_не_отправляется() {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Transport("обрыв связи".into())),
        Ok(ProviderStatus::Paid),
    ]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, payment) = unknown_charge(&kit, 345).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.clients()
            .set_status(
                subscription.client_id,
                sn_domain::client::ClientStatus::Blocked,
                Some("нарушение правил"),
            )
            .await
            .unwrap();
    }
    let checked = kit
        .services
        .autorenew()
        .check_charge(payment.id)
        .await
        .unwrap();
    assert!(
        matches!(&checked, Some(ChargeCheck::Unclear(reason)) if reason.contains("заблокирован")),
        "{checked:?}"
    );
    assert_eq!(pay.requests().len(), 1);
    kit.db.cleanup().await;
}

pub(crate) async fn payment_audit(
    kit: &Kit,
    payment: &Payment,
    action: &str,
) -> Vec<sn_store::AuditRecord> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.audit()
        .for_entity(sn_store::audit_entity::PAYMENT, payment.id.0, 20)
        .await
        .unwrap()
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

async fn stays_unclear(kit: &Kit, pay: &FlakyPay, payment: &Payment, word: &str) {
    let autorenew = kit.services.autorenew();
    for _ in 0..2 {
        let checked = autorenew.check_charge(payment.id).await.unwrap();
        assert!(
            matches!(&checked, Some(ChargeCheck::Unclear(reason)) if reason.contains(word)),
            "{checked:?}"
        );
    }
    assert_eq!(
        pay.requests().len(),
        1,
        "новый запрос на списание не отправлен"
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    assert_eq!(
        conn.payments().get(payment.id).await.unwrap().status,
        PaymentStatus::Pending
    );
    assert_eq!(
        conn.orders().get(payment.order_id).await.unwrap().status,
        OrderStatus::Pending
    );
    drop(conn);
    let audit = payment_audit(kit, payment, CHARGE_UNCLEAR_AUDIT).await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].actor_kind, "system");
    assert_eq!(
        audit[0].details["order_id"],
        serde_json::json!(payment.order_id)
    );
    assert!(
        audit[0].details["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(word)),
        "{audit:?}"
    );
}

async fn flaky_unknown_charge(
    telegram_id: i64,
) -> Option<(Kit, Arc<FlakyPay>, Subscription, Payment)> {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Transport("обрыв связи".into())),
        Ok(ProviderStatus::Paid),
    ]);
    let kit = flaky_kit(pay.clone()).await?;
    let (subscription, payment) = unknown_charge(&kit, telegram_id).await;
    assert_eq!(payment.provider_txid, None);
    Some((kit, pay, subscription, payment))
}

#[tokio::test]
async fn после_выключения_автопродления_повторный_запрос_списания_не_отправляется() {
    let Some((kit, pay, subscription, payment)) = flaky_unknown_charge(346).await else {
        return;
    };
    kit.services
        .subscriptions()
        .set_autorenew(
            subscription.id,
            false,
            None,
            crate::Actor::Client(subscription.client_id),
        )
        .await
        .unwrap();
    stays_unclear(&kit, &pay, &payment, "автопродление уже выключено").await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_изменения_срока_подписки_повторный_запрос_списания_не_отправляется() {
    let Some((kit, pay, subscription, payment)) = flaky_unknown_charge(347).await else {
        return;
    };
    expire_in(
        &kit,
        &subscription,
        moment(Days::days(45)),
        SubscriptionStatus::Active,
    )
    .await;
    stays_unclear(&kit, &pay, &payment, "срок подписки уже изменился").await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn для_завершённой_подписки_повторный_запрос_списания_не_отправляется() {
    let Some((kit, pay, subscription, payment)) = flaky_unknown_charge(348).await else {
        return;
    };
    sqlx::query("UPDATE subscriptions SET ended_at = now() WHERE id = $1")
        .bind(subscription.id.0)
        .execute(kit.services.store().pool())
        .await
        .unwrap();
    stays_unclear(&kit, &pay, &payment, "подписка уже завершена").await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn для_отключённой_подписки_повторный_запрос_списания_не_отправляется() {
    let Some((kit, pay, subscription, payment)) = flaky_unknown_charge(349).await else {
        return;
    };
    expire_in(
        &kit,
        &subscription,
        subscription.mirror.expire_at.unwrap(),
        SubscriptionStatus::Disabled,
    )
    .await;
    stays_unclear(&kit, &pay, &payment, "подписка заблокирована").await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пока_другой_оплаченный_заказ_подписки_не_выдан_повторный_запрос_списания_не_отправляется()
{
    let Some((kit, pay, subscription, payment)) = flaky_unknown_charge(350).await else {
        return;
    };
    let tariff = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.tariffs()
            .get(subscription.tariff_id.unwrap())
            .await
            .unwrap()
    };
    credit(&kit.services, subscription.client_id, 1_000).await;
    let manual = buy_with_balance(
        &kit.services,
        subscription.client_id,
        cart(&tariff, 90, Some(&subscription)),
    )
    .await;
    assert_eq!(manual.status, OrderStatus::Paid);
    assert_ne!(manual.id, payment.order_id);
    stays_unclear(&kit, &pay, &payment, "подписка уже оплачена другим заказом").await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказ_на_проверке_без_плана_в_работе_не_мешает_повторному_запросу_списания() {
    let Some((kit, pay, subscription, payment)) = flaky_unknown_charge(353).await else {
        return;
    };
    let tariff = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.tariffs()
            .get(subscription.tariff_id.unwrap())
            .await
            .unwrap()
    };
    credit(&kit.services, subscription.client_id, 1_000).await;
    let manual = buy_with_balance(
        &kit.services,
        subscription.client_id,
        cart(&tariff, 90, Some(&subscription)),
    )
    .await;
    let autorenew = kit.services.autorenew();
    let checked = autorenew.check_charge(payment.id).await.unwrap();
    assert!(
        matches!(&checked, Some(ChargeCheck::Unclear(reason)) if reason.contains("подписка уже оплачена другим заказом")),
        "{checked:?}"
    );
    assert_eq!(pay.requests().len(), 1);
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.orders()
            .mark_review(manual.id, "сквада нет в панели")
            .await
            .unwrap();
        assert!(
            !crate::fulfillment::holds_autorenew(&mut conn, manual.id)
                .await
                .unwrap()
        );
    }
    assert_eq!(
        autorenew.check_charge(payment.id).await.unwrap(),
        Some(ChargeCheck::Settled)
    );
    assert_eq!(pay.requests().len(), 2, "повторный запрос отправлен");
    let renewals = renewals(&kit, &subscription).await;
    let (charged, payments) = renewals
        .iter()
        .find(|(order, _)| order.id == payment.order_id)
        .unwrap();
    assert_eq!(charged.status, OrderStatus::Paid);
    assert_eq!(payments[0].status, PaymentStatus::Paid);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn без_записи_о_попытке_повторный_запрос_списания_не_отправляется() {
    let Some((kit, pay, subscription, payment)) = flaky_unknown_charge(352).await else {
        return;
    };
    sqlx::query("DELETE FROM autorenew_attempts WHERE subscription_id = $1 AND order_id = $2")
        .bind(subscription.id.0)
        .bind(payment.order_id.0)
        .execute(kit.services.store().pool())
        .await
        .unwrap();
    stays_unclear(
        &kit,
        &pay,
        &payment,
        "попытка автопродления по этому заказу не найдена",
    )
    .await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_выключения_автопродления_статус_по_номеру_платежа_всё_равно_запрашивается() {
    let pay = FlakyPay::answering(vec![Ok(ProviderStatus::Pending)]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, payment) = unknown_charge(&kit, 351).await;
    assert!(payment.provider_txid.is_some());
    kit.services
        .subscriptions()
        .set_autorenew(
            subscription.id,
            false,
            None,
            crate::Actor::Client(subscription.client_id),
        )
        .await
        .unwrap();
    let autorenew = kit.services.autorenew();
    *pay.status.lock().unwrap() = Some(ProviderStatus::Pending);
    assert_eq!(
        autorenew.check_charge(payment.id).await.unwrap(),
        Some(ChargeCheck::Pending)
    );
    *pay.status.lock().unwrap() = Some(ProviderStatus::Paid);
    assert_eq!(
        autorenew.check_charge(payment.id).await.unwrap(),
        Some(ChargeCheck::Settled)
    );
    assert_eq!(pay.requests().len(), 1);
    let renewals = renewals(&kit, &subscription).await;
    assert_eq!(renewals[0].0.status, OrderStatus::Paid);
    assert_eq!(renewals[0].1[0].status, PaymentStatus::Paid);
    assert!(
        payment_audit(&kit, &payment, CHARGE_UNCLEAR_AUDIT)
            .await
            .is_empty()
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ожидающее_списание_проверяется_по_статусу_кассы() {
    let pay = FlakyPay::answering(vec![Ok(ProviderStatus::Pending)]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, payment) = unknown_charge(&kit, 344).await;
    let txid = flaky_txid(&pay.requests()[0]);
    assert_eq!(payment.provider_txid.as_deref(), Some(txid.as_str()));
    let autorenew = kit.services.autorenew();
    *pay.status.lock().unwrap() = Some(ProviderStatus::Pending);
    assert_eq!(
        autorenew.check_charge(payment.id).await.unwrap(),
        Some(ChargeCheck::Pending)
    );
    *pay.status.lock().unwrap() = Some(ProviderStatus::Paid);
    assert_eq!(
        autorenew.check_charge(payment.id).await.unwrap(),
        Some(ChargeCheck::Settled)
    );
    assert_eq!(pay.requests().len(), 1);
    let renewals = renewals(&kit, &subscription).await;
    assert_eq!(renewals[0].0.status, OrderStatus::Paid);
    assert_eq!(renewals[0].1[0].status, PaymentStatus::Paid);
    let mut conn = kit.services.store().conn().await.unwrap();
    let topup = conn
        .payments()
        .for_client(subscription.client_id, Default::default())
        .await
        .unwrap();
    drop(conn);
    for other in topup.items.iter().filter(|p| p.id != payment.id) {
        assert_eq!(autorenew.check_charge(other.id).await.unwrap(), None);
    }
    kit.db.cleanup().await;
}

#[tokio::test]
async fn автопродление_проходит_все_страницы_выборки() {
    let Some(kit) = kit().await else { return };
    let mut due = Vec::new();
    for (telegram_id, hours) in [(331, 6), (332, 12), (333, 18)] {
        let client_id = client(&kit.services, telegram_id).await;
        due.push(
            due_subscription(
                &kit,
                client_id,
                Days::hours(hours),
                SubscriptionStatus::Active,
            )
            .await
            .id,
        );
    }
    let now = kit.services.now();
    let autorenew = kit.services.autorenew();
    let first = autorenew.due(now, 2).await.unwrap();
    assert_eq!(
        first.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![due[0], due[1]]
    );
    let rest = autorenew
        .due_after(now, DueCursor::of(&first[1]), 2)
        .await
        .unwrap();
    assert_eq!(rest.iter().map(|s| s.id).collect::<Vec<_>>(), vec![due[2]]);
    assert_eq!(
        autorenew.run_due_in_pages(now, 1).await.unwrap(),
        AutoRenewReport {
            due: 3,
            charged: 3,
            reminded: 0,
            failed: 0
        }
    );
    kit.db.cleanup().await;
}

async fn bare_subscription(
    kit: &Kit,
    client_id: ClientId,
    kind: &str,
    status: &str,
    expired_days_ago: i32,
) -> sn_domain::ids::SubscriptionId {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO subscriptions (client_id, kind, status, expire_at, autorenew_enabled) \
         VALUES ($1, $2, $3, now() - make_interval(days => $4), true) RETURNING id",
    )
    .bind(client_id.0)
    .bind(kind)
    .bind(status)
    .bind(expired_days_ago)
    .fetch_one(kit.services.store().pool())
    .await
    .unwrap();
    sn_domain::ids::SubscriptionId(id)
}

async fn claim_trial(kit: &Kit, client_id: ClientId) {
    let mut conn = kit.services.store().conn().await.unwrap();
    let identities: Vec<_> = conn
        .clients()
        .identities(client_id)
        .await
        .unwrap()
        .into_iter()
        .map(|i| (i.kind, i.value))
        .collect();
    assert!(conn.trials().claim(client_id, &identities).await.unwrap());
}

#[tokio::test]
async fn давно_истёкшие_подписки_завершаются_по_правилу() {
    let Some(kit) = kit().await else { return };
    let tried = client(&kit.services, 320).await;
    claim_trial(&kit, tried).await;
    let old_trial = bare_subscription(&kit, tried, "trial", "expired", 120).await;
    let blocked = bare_subscription(&kit, tried, "paid", "disabled", 200).await;
    let recent = bare_subscription(&kit, tried, "paid", "expired", 30).await;
    let stale_mirror = bare_subscription(&kit, tried, "paid", "active", 150).await;
    let newcomer = client(&kit.services, 321).await;
    let only = bare_subscription(&kit, newcomer, "paid", "expired", 120).await;
    let regular = client(&kit.services, 322).await;
    let live = bare_subscription(&kit, regular, "paid", "active", -20).await;
    let dead = bare_subscription(&kit, regular, "paid", "expired", 365).await;
    let paying = client(&kit.services, 323).await;
    claim_trial(&kit, paying).await;
    let invoiced = bare_subscription(&kit, paying, "paid", "expired", 120).await;
    sqlx::query(
        "INSERT INTO orders (client_id, subscription_id, kind, status, intent, quote, currency, total_minor) \
         VALUES ($1, $2, 'renewal', 'pending', '{}'::jsonb, '{}'::jsonb, 'USD', 0)",
    )
    .bind(paying.0)
    .bind(invoiced.0)
    .execute(kit.services.store().pool())
    .await
    .unwrap();
    let now = kit.services.now();
    let autorenew = kit.services.autorenew();
    assert_eq!(autorenew.end_expired(now).await.unwrap().ended, 3);
    assert_eq!(autorenew.end_expired(now).await.unwrap().ended, 0);
    let mut conn = kit.services.store().conn().await.unwrap();
    for (id, ended) in [
        (old_trial, true),
        (blocked, true),
        (dead, true),
        (recent, false),
        (stale_mirror, false),
        (only, false),
        (live, false),
        (invoiced, false),
    ] {
        let current = conn.subscriptions().get(id).await.unwrap();
        assert_eq!(current.ended_at.is_some(), ended, "подписка {id}");
        assert_eq!(current.autorenew.enabled, !ended, "подписка {id}");
    }
    let audit = conn
        .audit()
        .for_entity(sn_store::audit_entity::SUBSCRIPTION, old_trial.0, 10)
        .await
        .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].action, "subscription.ended");
    assert_eq!(audit[0].details["ended_after_days"], ENDED_AFTER_DAYS);
    drop(conn);
    kit.db.cleanup().await;
}

#[test]
fn удалённый_срок_заменяется_ближайшим_по_дням() {
    let tariff = Tariff {
        traffic_addon: None,
        id: sn_domain::ids::TariffId(1),
        category_id: sn_domain::ids::CategoryId(1),
        name: "Стандарт".into(),
        description: None,
        on_sale: true,
        sort: 0,
        periods: vec![
            sn_domain::tariff::TariffPeriod {
                id: sn_domain::ids::PeriodId(11),
                days: 30,
                prices: sn_domain::tariff::CurrencyPrices {
                    usd: Some(299),
                    rub: None,
                },
                stars: None,
                sort: 0,
                active: true,
            },
            sn_domain::tariff::TariffPeriod {
                id: sn_domain::ids::PeriodId(12),
                days: 90,
                prices: sn_domain::tariff::CurrencyPrices {
                    usd: Some(699),
                    rub: None,
                },
                stars: None,
                sort: 1,
                active: false,
            },
            sn_domain::tariff::TariffPeriod {
                id: sn_domain::ids::PeriodId(13),
                days: 365,
                prices: sn_domain::tariff::CurrencyPrices {
                    usd: Some(2399),
                    rub: None,
                },
                stars: None,
                sort: 2,
                active: true,
            },
        ],
        devices: sn_domain::tariff::DevicePolicy {
            included: 3,
            extra: None,
        },
        traffic: sn_domain::traffic::TrafficPolicy::UNLIMITED,
        squads: vec![squad(1)],
        external_squad: None,
        device_discounts: Vec::new(),
    };
    let subscription = Subscription {
        id: sn_domain::ids::SubscriptionId(1),
        public_id: Uuid::nil(),
        client_id: ClientId(1),
        tariff_id: Some(tariff.id),
        kind: sn_domain::subscription::SubscriptionKind::Paid,
        remna: None,
        extra_devices: 0,
        period_started_at: None,
        period_days: Some(90),
        autorenew: AutoRenewState {
            enabled: true,
            period_id: Some(sn_domain::ids::PeriodId(12)),
        },
        mirror: Default::default(),
        created_at: chrono::Utc::now(),
        ended_at: None,
        device_limit: None,
    };
    assert_eq!(
        renewal_period(&tariff, &subscription, Currency::Usd).map(|p| p.days),
        Some(30)
    );
    assert_eq!(renewal_period(&tariff, &subscription, Currency::Rub), None);
}

#[tokio::test]
async fn заказ_на_проверке_с_неприменённым_планом_не_держит_автопродление() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 351).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    let tariff = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.tariffs()
            .get(subscription.tariff_id.unwrap())
            .await
            .unwrap()
    };
    let stuck = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    kit.remna
        .fail_on("update_user", sn_remnawave::fake::Fault::Timeout);
    fulfillment.run(stuck.id).await.unwrap();
    let admin = crate::Actor::Admin(sn_domain::ids::AdminId(1));
    kit.services
        .clients()
        .block(client_id, Some("проверка"), admin)
        .await
        .unwrap();
    fulfillment.run(stuck.id).await.unwrap();
    kit.services
        .clients()
        .unblock(client_id, admin)
        .await
        .unwrap();
    assert_eq!(
        order(&kit.services, stuck.id).await.status,
        OrderStatus::Review
    );
    let now = kit.services.now();
    let autorenew = kit.services.autorenew();
    assert_eq!(autorenew.run_due(now).await.unwrap().charged, 1);
    let charged = renewals(&kit, &subscription)
        .await
        .into_iter()
        .map(|(order, _)| order)
        .find(|order| order.id != stuck.id)
        .unwrap();
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.orders()
            .mark_review(charged.id, "сквада нет в панели")
            .await
            .unwrap();
    }
    assert_eq!(
        autorenew
            .run_due(now + Days::days(1))
            .await
            .unwrap()
            .charged,
        0,
        "своё списание на ручной проверке держит следующее"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn при_выключенном_балансе_тратится_остаток_а_без_него_напоминание_продлить() {
    let Some(kit) = kit().await else { return };
    crate::checkout::balance_off_tests::balance_off(&kit).await;
    let rich = client(&kit.services, 351).await;
    let rich_subscription =
        due_subscription(&kit, rich, Days::hours(12), SubscriptionStatus::Active).await;
    drain_to(&kit, rich, 800).await;
    let poor = client(&kit.services, 352).await;
    let poor_subscription =
        due_subscription(&kit, poor, Days::hours(12), SubscriptionStatus::Active).await;
    drain_to(&kit, poor, 300).await;
    jobs(&kit.services).await;
    let autorenew = kit.services.autorenew();
    let report = autorenew.run_due(kit.services.now()).await.unwrap();
    assert_eq!((report.charged, report.reminded), (1, 1));
    assert_eq!(balance(&kit.services, rich).await, 101);
    assert_eq!(renewals(&kit, &rich_subscription).await.len(), 1);
    assert!(renewals(&kit, &poor_subscription).await.is_empty());
    let queued = jobs(&kit.services).await;
    let reminder = queued
        .iter()
        .find_map(|job| match job {
            JobPayload::Notify {
                client_id,
                template,
                params,
            } if *client_id == poor && template == "autorenew.reminder" => Some(params.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(reminder["balance_off"], serde_json::json!(true));
    assert!(!autorenew.has_saved_card(poor).await.unwrap());
    save_card(&kit, poor).await;
    assert!(autorenew.has_saved_card(poor).await.unwrap());
    kit.db.cleanup().await;
}

fn notices(queued: &[JobPayload], name: &str) -> Vec<(ClientId, serde_json::Value)> {
    queued
        .iter()
        .filter_map(|job| match job {
            JobPayload::Notify {
                client_id,
                template,
                params,
            } if template == name => Some((*client_id, params.clone())),
            _ => None,
        })
        .collect()
}

async fn autorenew_enabled(kit: &Kit, subscription: &Subscription) -> bool {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.subscriptions()
        .get(subscription.id)
        .await
        .unwrap()
        .autorenew
        .enabled
}

#[tokio::test]
async fn списание_после_выключения_автопродления_продлевает_подписку_и_предупреждает_клиента() {
    let Some(kit) = kit().await else { return };
    *kit.pay.charge.lock().unwrap() = Some(ProviderStatus::Pending);
    let (subscription, charge) = unknown_charge(&kit, 360).await;
    let client_id = subscription.client_id;
    let expire = subscription.mirror.expire_at.unwrap();
    kit.services
        .subscriptions()
        .set_autorenew(
            subscription.id,
            false,
            None,
            crate::Actor::Client(client_id),
        )
        .await
        .unwrap();
    let txid = charge.provider_txid.clone().unwrap();
    *kit.pay.status.lock().unwrap() = Some(WebhookOutcome {
        event_key: format!("payment.succeeded:{txid}"),
        payment: None,
        provider_txid: Some(txid),
        status: ProviderStatus::Paid,
        amount: Some(charge.amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    });
    assert_eq!(
        kit.services
            .autorenew()
            .check_charge(charge.id)
            .await
            .unwrap(),
        Some(ChargeCheck::Settled)
    );
    jobs(&kit.services).await;
    let done = kit
        .services
        .fulfillment()
        .run(charge.order_id)
        .await
        .unwrap();
    assert!(
        matches!(done, crate::fulfillment::FulfillmentOutcome::Done { .. }),
        "{done:?}"
    );
    let renewed = remna_user(&kit, &subscription).await.expire_at;
    assert_eq!(renewed, expire + Days::days(90));
    assert!(!autorenew_enabled(&kit, &subscription).await);
    let queued = jobs(&kit.services).await;
    assert_eq!(notifications(&queued, "order.fulfilled"), 1);
    let warned = notices(&queued, CHARGED_AFTER_OFF_TEMPLATE);
    assert_eq!(warned.len(), 1, "{queued:?}");
    let (to, params) = &warned[0];
    assert_eq!(*to, client_id);
    assert_eq!(params["amount"], "$6.99");
    assert_eq!(params["expire_at"], serde_json::json!(renewed));
    assert_eq!(
        params["subscription_id"],
        serde_json::json!(subscription.id)
    );
    assert_eq!(params["payment_id"], serde_json::json!(charge.id));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn продление_с_баланса_предупреждает_только_тех_кто_выключил_автопродление_до_выдачи() {
    let Some(kit) = kit().await else { return };
    let kept = client(&kit.services, 361).await;
    let dropped = client(&kit.services, 362).await;
    let mut subscriptions = Vec::new();
    for client_id in [kept, dropped] {
        subscriptions.push(
            due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await,
        );
    }
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!(report.charged, 2);
    kit.services
        .subscriptions()
        .set_autorenew(
            subscriptions[1].id,
            false,
            None,
            crate::Actor::Client(dropped),
        )
        .await
        .unwrap();
    jobs(&kit.services).await;
    for subscription in &subscriptions {
        let (order, payments) = renewals(&kit, subscription).await.remove(0);
        assert_eq!(payments[0].provider.as_str(), providers::BALANCE);
        let done = kit.services.fulfillment().run(order.id).await.unwrap();
        assert!(
            matches!(done, crate::fulfillment::FulfillmentOutcome::Done { .. }),
            "{done:?}"
        );
    }
    let queued = jobs(&kit.services).await;
    assert_eq!(notifications(&queued, "order.fulfilled"), 2);
    let warned = notices(&queued, CHARGED_AFTER_OFF_TEMPLATE);
    assert_eq!(warned.len(), 1, "{queued:?}");
    assert_eq!(warned[0].0, dropped);
    assert_eq!(
        warned[0].1["expire_at"],
        serde_json::json!(remna_user(&kit, &subscriptions[1]).await.expire_at)
    );
    assert!(autorenew_enabled(&kit, &subscriptions[0]).await);
    assert!(!autorenew_enabled(&kit, &subscriptions[1]).await);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn автопродление_во_время_технических_работ_списывает_как_обычно() {
    let Some(kit) = kit().await else { return };
    let mut settings = (*kit.services.settings()).clone();
    settings.maintenance.enabled = true;
    kit.services.replace_settings(settings);
    assert!(kit.services.maintenance().active());
    let client_id = client(&kit.services, 309).await;
    let subscription =
        due_subscription(&kit, client_id, Days::hours(12), SubscriptionStatus::Active).await;
    let before = balance(&kit.services, client_id).await;
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!((report.due, report.charged, report.failed), (1, 1, 0));
    assert_eq!(balance(&kit.services, client_id).await, before - 699);
    let renewals = renewals(&kit, &subscription).await;
    let (order, _) = &renewals[0];
    let expire = subscription.mirror.expire_at.unwrap();
    kit.services.fulfillment().run(order.id).await.unwrap();
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + Days::days(90)
    );
    kit.db.cleanup().await;
}
