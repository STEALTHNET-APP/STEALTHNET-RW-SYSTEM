use super::*;
use async_trait::async_trait;
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::{ClientId, OrderId, SquadId};
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::module::ModuleId;
use sn_domain::money::{Amount, Currency, Money, PayCurrency, Stars};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::payment::{
    NewPayment, Payment, PaymentStatus, ProviderId, RefundMethod, RefundSubscriptionAction,
    providers,
};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::settings::{keys, secret_keys};
use sn_domain::subscription::{AutoRenewState, RemnaMirror, SubscriptionKind, SubscriptionStatus};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, Tariff};
use sn_domain::traffic::TrafficPolicy;
use sn_modules::{
    BalanceChange, CoreApi, Module, ModuleContext, ModuleError, ModuleRegistry, ScheduledTask,
};
use sn_payments::providers::cryptopay_signature;
use sn_payments::{
    ClientLabel, Invoice, InvoiceRequest, PaymentError, PaymentProvider, PaymentResult,
    ProviderConfig, ProviderRegistry, ProviderStatus, RefundOutcome, RefundRequest, WebhookOutcome,
    WebhookReply, WebhookRequest,
};
use sn_remnawave::fake::FakeRemnawave;
use sn_remnawave::{InternalSquad, SquadInfo};
use sn_services::checkout::{CartRequest, Channel, CheckoutRequest, CheckoutResult};
use sn_services::inbox::{InboundWebhook, RefundCommand};
use sn_services::{Clock, EnvFallbacks, FixedClock, ServiceDeps, SystemClock};
use sn_store::testing::{TestDb, test_store};
use sn_store::{
    NewCategory, NewPeriod, NewSubscription, NewTariff, ProviderRefundStatus, SecretBox,
};
use std::sync::Mutex;

mod clock;
mod sessions;

const CRYPTO_TOKEN: &str = "4321:TEST";
const SQUAD: &str = "00000000-0000-0000-0000-000000000001";

struct Ticker {
    runs: Mutex<Vec<String>>,
}

#[async_trait]
impl Module for Ticker {
    fn id(&self) -> ModuleId {
        ModuleId::Broadcast
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: "digest",
            every: Duration::from_secs(3_600),
        }]
    }

    async fn run_task(&self, _ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        self.runs.lock().unwrap().push(task.to_string());
        Ok(())
    }
}

struct Setup {
    db: TestDb,
    services: Services,
    remna: Arc<FakeRemnawave>,
}

fn squad() -> SquadId {
    serde_json::from_value(serde_json::json!(SQUAD)).unwrap()
}

async fn setup(modules: Vec<Arc<dyn Module>>) -> Option<Setup> {
    setup_with_clock(modules, Arc::new(SystemClock)).await
}

async fn setup_with_clock(modules: Vec<Arc<dyn Module>>, clock: Arc<dyn Clock>) -> Option<Setup> {
    setup_full(modules, clock, ProviderRegistry::builtin()).await
}

async fn setup_full(
    modules: Vec<Arc<dyn Module>>,
    clock: Arc<dyn Clock>,
    payments: ProviderRegistry,
) -> Option<Setup> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(payments),
        modules: Arc::new(ModuleRegistry::new(modules).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock,
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                keys::PUBLIC_URL,
                &serde_json::json!("https://panel.example.com"),
                None,
            )
            .await
            .unwrap();
        sn_services::settings::write_secret(
            &mut conn,
            services.secrets(),
            &secret_keys::provider(providers::CRYPTOPAY, "api_token"),
            Some(CRYPTO_TOKEN),
            None,
        )
        .await
        .unwrap();
    }
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    remna.set_squads(vec![InternalSquad {
        uuid: squad().0,
        name: "Основной".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    services.set_remna(Some(remna.clone()));
    Some(Setup {
        db,
        services,
        remna,
    })
}

async fn client(services: &Services, telegram_id: i64) -> ClientId {
    let profile = TelegramProfile {
        id: telegram_id,
        username: None,
        first_name: Some("Ксения".into()),
        last_name: None,
        language_code: Some("ru".into()),
        is_premium: false,
    };
    services
        .clients()
        .ensure_telegram(&profile, RegistrationSource::Bot, None)
        .await
        .unwrap()
        .0
        .id
}

async fn tariff(services: &Services) -> Tariff {
    let mut conn = services.store().conn().await.unwrap();
    let category = conn
        .tariffs()
        .create_category(&NewCategory {
            name: "Стандарт".into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    conn.tariffs()
        .create(&NewTariff {
            traffic_addon: None,
            category_id: category.id,
            name: "Стандарт".into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![squad()],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(19_900),
                },
                stars: Some(Stars(150)),
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap()
}

async fn pending_crypto(
    services: &Services,
    client_id: ClientId,
    tariff: &Tariff,
    expires_at: DateTime<Utc>,
) -> Payment {
    pending_payment(
        services,
        client_id,
        tariff,
        expires_at,
        providers::CRYPTOPAY,
    )
    .await
}

async fn pending_payment(
    services: &Services,
    client_id: ClientId,
    tariff: &Tariff,
    expires_at: DateTime<Utc>,
    provider: &str,
) -> Payment {
    pending_payment_by(services, client_id, tariff, expires_at, provider, None).await
}

async fn pending_payment_by(
    services: &Services,
    client_id: ClientId,
    tariff: &Tariff,
    expires_at: DateTime<Utc>,
    provider: &str,
    method: Option<&str>,
) -> Payment {
    let period = &tariff.periods[0];
    let q = quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff,
        period_id: period.id,
        extra_devices: 0,
        sell_extra_devices: false,
        adjustments: &[],
        promotion: None,
    })
    .unwrap();
    let mut tx = services.store().begin().await.unwrap();
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
            method: method.map(str::to_string),
            amount: Amount::new(299, PayCurrency::Usd),
            expires_at: Some(expires_at),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    payment
}

fn crypto_hook(payment: &Payment) -> InboundWebhook {
    let body = serde_json::json!({
        "update_id": 7, "update_type": "invoice_paid", "request_date": "2026-11-16T14:52:00.000Z",
        "payload": {"invoice_id": 4242, "status": "paid", "currency_type": "fiat", "fiat": "USD",
                    "amount": "2.99", "payload": payment.public_id.to_string()}
    })
    .to_string();
    InboundWebhook {
        provider: providers::CRYPTOPAY.into(),
        request: WebhookRequest {
            headers: vec![(
                "crypto-pay-api-signature".into(),
                cryptopay_signature(CRYPTO_TOKEN, body.as_bytes()),
            )],
            body: body.into_bytes().into(),
            remote_ip: None,
            route_suffix: None,
            query: None,
        },
    }
}

async fn job_counts(services: &Services) -> Vec<(String, i64)> {
    let mut conn = services.store().conn().await.unwrap();
    conn.jobs().counts().await.unwrap()
}

#[tokio::test]
async fn событие_remnawave_обрабатывается_задачей() {
    let Some(db) = test_store().await else { return };
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: None,
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    services.set_remna_webhook_secret(Some("0123456789abcdef0123456789abcdef".into()));
    let body = br#"{"scope":"node","event":"node.connection_lost","timestamp":"2026-11-16T14:38:00.000Z","data":{"uuid":"00000000-0000-0000-0000-000000000002","name":"NL-2","address":"198.51.100.22","isConnected":false,"isDisabled":false}}"#;
    let signature = sn_remnawave::webhook::sign("0123456789abcdef0123456789abcdef", body);
    services
        .remna_sync()
        .accept_webhook(Some(&signature), body)
        .await
        .unwrap();
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    assert_eq!(worker.tick().await.unwrap(), 1);
    assert_eq!(worker.tick().await.unwrap(), 0);
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.jobs().counts().await.unwrap(),
        vec![("done".to_string(), 1)]
    );
    let feed = conn.remna_events().feed(None, 10).await.unwrap();
    assert!(feed[0].processed_at.is_some());
    assert_eq!(
        feed[0].subject_id.as_deref(),
        Some("00000000-0000-0000-0000-000000000002")
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn покупка_с_баланса_выдаётся_воркером() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 501).await;
    let tariff = tariff(services).await;
    services
        .change_balance(BalanceChange {
            client_id,
            wallet: Wallet::Main,
            amount: Money::new(500, Currency::Usd),
            kind: LedgerKind::TopUp,
            comment: None,
            idempotency_key: "test:topup".into(),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
    let result = services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: CartRequest {
                    tariff_id: tariff.id,
                    period_id: tariff.periods[0].id,
                    extra_devices: 0,
                    subscription_id: None,
                    promo_code: None,
                },
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(result, CheckoutResult::Completed { .. }));
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    worker.tick().await.unwrap();
    assert_eq!(setup.remna.users().len(), 1);
    let mut conn = services.store().conn().await.unwrap();
    let subscriptions = conn.subscriptions().by_client(client_id).await.unwrap();
    assert_eq!(subscriptions.len(), 1);
    assert!(subscriptions[0].remna.is_some());
    drop(conn);
    let counts = job_counts(services).await;
    assert!(counts.contains(&("done".to_string(), 1)), "{counts:?}");
    setup.db.cleanup().await;
}

#[tokio::test]
async fn периодическая_работа_не_чаще_интервала_и_пишет_итог() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let worker = Worker::new(setup.services.clone(), WorkerConfig::default());
    let report = worker
        .run_periodic(Periodic::Housekeeping)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report["expired_payments"], 0);
    assert_eq!(report["purged_webhooks"], 0);
    assert_eq!(report["removed_sessions"], 0);
    assert_eq!(report["refunds"]["settled"], 0);
    assert_eq!(report["refunds"]["failed"], 0);
    assert_eq!(report["released_devices"], 0);
    assert_eq!(
        worker.run_periodic(Periodic::Housekeeping).await.unwrap(),
        None
    );
    let state = periodic_state(&setup.services, Periodic::Housekeeping)
        .await
        .unwrap();
    assert!(state.last_started_at.is_some());
    assert!(state.last_success_at.is_some());
    assert_eq!(state.last_error, None);
    assert_eq!(state.last_report, Some(report));
    let renew = worker
        .run_periodic(Periodic::AutoRenew)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renew["due"], 0);
    let state = periodic_state(&setup.services, Periodic::AutoRenew)
        .await
        .unwrap();
    assert!(state.last_success_at.is_some());
    setup.db.cleanup().await;
}

#[tokio::test]
async fn просроченный_счёт_отменяется_с_заказом_а_поздняя_оплата_проходит() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 502).await;
    let tariff = tariff(services).await;
    let payment = pending_crypto(
        services,
        client_id,
        &tariff,
        services.now() - chrono::Duration::minutes(5),
    )
    .await;
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    let report = worker
        .run_periodic(Periodic::Housekeeping)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report["expired_payments"], 1);
    assert_eq!(report["canceled_orders"], 1);
    {
        let mut conn = services.store().conn().await.unwrap();
        assert_eq!(
            conn.payments().get(payment.id).await.unwrap().status,
            PaymentStatus::Canceled
        );
        assert_eq!(
            conn.orders().get(payment.order_id).await.unwrap().status,
            OrderStatus::Canceled
        );
    }
    let reply = services
        .inbox()
        .handle(crypto_hook(&payment))
        .await
        .unwrap();
    assert_eq!(reply.reply.status(), 200);
    worker.tick().await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    assert_eq!(
        conn.payments().get(payment.id).await.unwrap().status,
        PaymentStatus::Paid
    );
    assert_eq!(
        conn.orders().get(payment.order_id).await.unwrap().status,
        OrderStatus::Fulfilled
    );
    drop(conn);
    assert_eq!(setup.remna.users().len(), 1);
    setup.db.cleanup().await;
}

#[tokio::test]
async fn задачи_модулей_ставятся_раз_в_интервал() {
    let ticker = Arc::new(Ticker {
        runs: Mutex::new(Vec::new()),
    });
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let Some(setup) = setup_with_clock(vec![ticker.clone()], clock).await else {
        return;
    };
    let worker = Worker::new(setup.services.clone(), WorkerConfig::default());
    let first = worker.schedule_module_tasks().await.unwrap();
    assert_eq!(first["scheduled"], 1);
    let second = worker.schedule_module_tasks().await.unwrap();
    assert_eq!(second["scheduled"], 0);
    assert_eq!(worker.tick().await.unwrap(), 1);
    assert_eq!(*ticker.runs.lock().unwrap(), vec!["digest".to_string()]);
    assert_eq!(
        job_counts(&setup.services).await,
        vec![("done".to_string(), 1)]
    );
    setup.db.cleanup().await;
}

#[tokio::test]
async fn опрос_счёта_без_поддержки_у_кассы_завершается() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 503).await;
    let tariff = tariff(services).await;
    let payment = pending_payment(
        services,
        client_id,
        &tariff,
        services.now() + chrono::Duration::minutes(30),
        providers::YOOMONEY,
    )
    .await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.payments()
            .attach_invoice(
                payment.id,
                &sn_store::InvoiceUpdate {
                    pay_url: Some("https://yoomoney.ru/quickpay/confirm".into()),
                    provider_txid: Some("4242".into()),
                    expires_at: None,
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        conn.jobs()
            .enqueue(
                &JobPayload::PollPayment {
                    payment_id: payment.id,
                },
                None,
            )
            .await
            .unwrap();
    }
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    assert_eq!(worker.tick().await.unwrap(), 1);
    assert_eq!(job_counts(services).await, vec![("done".to_string(), 1)]);
    setup.db.cleanup().await;
}

struct Slow {
    calls: Mutex<usize>,
    pause: Duration,
}

#[async_trait]
impl JobHandler for Slow {
    async fn handle(
        &self,
        _services: &Services,
        _payload: &JobPayload,
        _attempt: u32,
    ) -> JobResult {
        *self.calls.lock().unwrap() += 1;
        tokio::time::sleep(self.pause).await;
        JobResult::Done
    }
}

struct Waiting;

#[async_trait]
impl JobHandler for Waiting {
    async fn handle(
        &self,
        _services: &Services,
        _payload: &JobPayload,
        _attempt: u32,
    ) -> JobResult {
        JobResult::Wait {
            after: chrono::Duration::zero(),
            error: "прокси не отвечает".into(),
        }
    }
}

#[tokio::test]
async fn ожидание_прокси_не_тратит_попытки_задачи() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    {
        let mut conn = setup.services.store().conn().await.unwrap();
        conn.jobs()
            .enqueue(&JobPayload::ProcessRemnaEvent { event_id: 7 }, None)
            .await
            .unwrap()
            .unwrap();
    }
    let worker = Worker::new(setup.services.clone(), WorkerConfig::default())
        .with_handler(Arc::new(Waiting));
    let runs = worker.tick().await.unwrap();
    assert!(
        runs > usize::try_from(JobKind::ProcessRemnaEvent.max_attempts()).unwrap(),
        "{runs}"
    );
    assert_eq!(
        job_counts(&setup.services).await,
        vec![("queued".to_string(), 1)]
    );
    let mut conn = setup.services.store().conn().await.unwrap();
    let claimed = conn.jobs().claim(1, Duration::from_secs(30)).await.unwrap();
    drop(conn);
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempts, 1);
    setup.db.cleanup().await;
}

#[tokio::test]
async fn долгая_задача_не_достаётся_второму_обработчику() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let slow = Arc::new(Slow {
        calls: Mutex::new(0),
        pause: Duration::from_millis(2_500),
    });
    {
        let mut conn = setup.services.store().conn().await.unwrap();
        conn.jobs()
            .enqueue(&JobPayload::ProcessRemnaEvent { event_id: 1 }, None)
            .await
            .unwrap()
            .unwrap();
    }
    let first = Worker::new(
        setup.services.clone(),
        WorkerConfig {
            lease: Duration::from_secs(1),
            ..WorkerConfig::default()
        },
    )
    .with_handler(slow.clone());
    let second =
        Worker::new(setup.services.clone(), WorkerConfig::default()).with_handler(slow.clone());
    let (a, b) = tokio::join!(first.tick(), async {
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        second.tick().await
    });
    assert_eq!(a.unwrap(), 1);
    assert_eq!(b.unwrap(), 0);
    assert_eq!(*slow.calls.lock().unwrap(), 1);
    assert_eq!(
        job_counts(&setup.services).await,
        vec![("done".to_string(), 1)]
    );
    setup.db.cleanup().await;
}

#[tokio::test]
async fn давно_истёкшая_подписка_завершается_периодической_работой() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 504).await;
    let subscription = {
        let mut conn = services.store().conn().await.unwrap();
        let identities: Vec<_> = conn
            .clients()
            .identities(client_id)
            .await
            .unwrap()
            .into_iter()
            .map(|i| (i.kind, i.value))
            .collect();
        assert!(conn.trials().claim(client_id, &identities).await.unwrap());
        let created = conn
            .subscriptions()
            .create(&NewSubscription {
                client_id,
                tariff_id: None,
                kind: SubscriptionKind::Trial,
                extra_devices: 0,
                autorenew: AutoRenewState {
                    enabled: true,
                    period_id: None,
                },
            })
            .await
            .unwrap();
        conn.subscriptions()
            .apply_mirror(
                created.id,
                &RemnaMirror {
                    status: Some(SubscriptionStatus::Expired),
                    expire_at: Some(services.now() - chrono::Duration::days(120)),
                    ..RemnaMirror::default()
                },
            )
            .await
            .unwrap();
        created
    };
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    for periodic in Periodic::ALL {
        worker.run_periodic(periodic).await.unwrap();
    }
    let mut conn = services.store().conn().await.unwrap();
    let current = conn.subscriptions().get(subscription.id).await.unwrap();
    assert!(current.ended_at.is_some());
    assert!(!current.autorenew.enabled);
    drop(conn);
    setup.db.cleanup().await;
}

#[tokio::test]
async fn оплаченный_заказ_после_исчерпания_попыток_выдаётся_досверкой() {
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let Some(setup) = setup_with_clock(Vec::new(), clock.clone()).await else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 505).await;
    let tariff = tariff(services).await;
    services
        .change_balance(BalanceChange {
            client_id,
            wallet: Wallet::Main,
            amount: Money::new(500, Currency::Usd),
            kind: LedgerKind::TopUp,
            comment: None,
            idempotency_key: "test:topup:505".into(),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
    let result = services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: CartRequest {
                    tariff_id: tariff.id,
                    period_id: tariff.periods[0].id,
                    extra_devices: 0,
                    subscription_id: None,
                    promo_code: None,
                },
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Completed { order_id } = result else {
        panic!("оплата с баланса должна проходить сразу: {result:?}");
    };
    {
        let mut conn = services.store().conn().await.unwrap();
        let claimed = conn
            .jobs()
            .claim(10, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        conn.jobs()
            .review(claimed[0].id, "попытки кончились")
            .await
            .unwrap();
    }
    clock.set(Utc::now() + chrono::Duration::hours(2));
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    for periodic in Periodic::ALL {
        let _ = worker.run_periodic(periodic).await;
    }
    worker.tick().await.unwrap();
    assert_eq!(setup.remna.users().len(), 1);
    let mut conn = services.store().conn().await.unwrap();
    let order = conn.orders().by_public_id(order_id).await.unwrap();
    assert_eq!(order.status, OrderStatus::Fulfilled);
    drop(conn);
    setup.db.cleanup().await;
}

#[tokio::test]
async fn досверка_ставит_задачу_оплаченному_заказу_без_неё_и_не_трогает_ручную_проверку() {
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let Some(setup) = setup_with_clock(Vec::new(), clock.clone()).await else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 507).await;
    let tariff = tariff(services).await;
    let orphan = pending_payment(
        services,
        client_id,
        &tariff,
        services.now() + chrono::Duration::hours(1),
        providers::CRYPTOPAY,
    )
    .await
    .order_id;
    let reviewed = pending_payment(
        services,
        client_id,
        &tariff,
        services.now() + chrono::Duration::hours(1),
        providers::CRYPTOPAY,
    )
    .await
    .order_id;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.orders()
            .transition(orphan, OrderStatus::Paid)
            .await
            .unwrap();
        conn.orders()
            .transition(reviewed, OrderStatus::Paid)
            .await
            .unwrap();
        conn.orders()
            .mark_review(reviewed, "сквада нет в панели")
            .await
            .unwrap();
    }
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    let early = worker
        .run_periodic(Periodic::Redeliver)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(early["requeued"], 0);
    clock.set(Utc::now() + chrono::Duration::minutes(10));
    let report = worker.redeliver().await.unwrap();
    assert_eq!(report["stalled"], 1);
    assert_eq!(report["requeued"], 1);
    assert_eq!(worker.redeliver().await.unwrap()["requeued"], 0);
    let mut conn = services.store().conn().await.unwrap();
    let claimed = conn
        .jobs()
        .claim(10, Duration::from_secs(60))
        .await
        .unwrap();
    let payloads: Vec<JobPayload> = claimed.iter().map(|j| j.typed().unwrap()).collect();
    assert_eq!(
        payloads,
        vec![JobPayload::FulfillOrder { order_id: orphan }]
    );
    let audit = conn
        .audit()
        .for_entity(sn_store::audit_entity::ORDER, orphan.0, 10)
        .await
        .unwrap();
    assert_eq!(audit[0].action, "order.redeliver");
    assert_eq!(audit[0].actor_kind, "system");
    drop(conn);
    setup.db.cleanup().await;
}

#[tokio::test]
async fn опрос_автосписания_с_неизвестным_исходом_без_карты_уходит_на_ручную_проверку() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 506).await;
    let tariff = tariff(services).await;
    let payment = pending_payment_by(
        services,
        client_id,
        &tariff,
        services.now() + chrono::Duration::days(2),
        providers::YOOKASSA,
        Some(sn_services::autorenew::AUTORENEW_METHOD),
    )
    .await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.jobs()
            .enqueue(
                &JobPayload::PollPayment {
                    payment_id: payment.id,
                },
                None,
            )
            .await
            .unwrap();
    }
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    assert_eq!(worker.tick().await.unwrap(), 1);
    assert_eq!(job_counts(services).await, vec![("review".to_string(), 1)]);
    let mut conn = services.store().conn().await.unwrap();
    assert_eq!(
        conn.payments().get(payment.id).await.unwrap().status,
        PaymentStatus::Pending
    );
    drop(conn);
    setup.db.cleanup().await;
}

struct RefundKassa;

#[async_trait]
impl PaymentProvider for RefundKassa {
    fn id(&self) -> &'static str {
        "refund_kassa"
    }

    fn title(&self) -> &'static str {
        "Касса с отложенными возвратами"
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

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        _config: &ProviderConfig,
        _request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        Err(PaymentError::Unsupported("счёт"))
    }

    async fn parse_webhook(
        &self,
        _config: &ProviderConfig,
        _request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::BadSignature)
    }

    async fn refund(
        &self,
        _config: &ProviderConfig,
        _request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        Ok(RefundOutcome {
            provider_refund_id: Some("kr-1".into()),
            completed: false,
        })
    }

    async fn fetch_refund(
        &self,
        _config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        Ok(Some(WebhookOutcome {
            event_key: format!("refund:{provider_refund_id}"),
            payment: None,
            provider_txid: None,
            status: ProviderStatus::Refunded,
            amount: None,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        }))
    }
}

#[tokio::test]
async fn опрос_оплаченного_автосписания_подтверждает_ожидающий_возврат() {
    let Some(setup) = setup_full(
        Vec::new(),
        Arc::new(SystemClock),
        ProviderRegistry::new(vec![Arc::new(RefundKassa)]),
    )
    .await
    else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 507).await;
    let tariff = tariff(services).await;
    let payment = pending_payment_by(
        services,
        client_id,
        &tariff,
        services.now() + chrono::Duration::days(2),
        RefundKassa.id(),
        Some(sn_services::autorenew::AUTORENEW_METHOD),
    )
    .await;
    {
        let mut tx = services.store().begin().await.unwrap();
        tx.payments()
            .transition(payment.id, PaymentStatus::Paid, Some("kx-507"), None)
            .await
            .unwrap();
        tx.orders()
            .transition(payment.order_id, OrderStatus::Paid)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let (admin, _) = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    let requested = services
        .inbox()
        .refund(
            payment.id,
            &RefundCommand {
                amount: None,
                method: RefundMethod::Provider,
                subscription_action: RefundSubscriptionAction::Keep,
                reason: Some("Клиент передумал".into()),
                key: None,
                outside: false,
            },
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    let result = CoreHandlers
        .handle(
            services,
            &JobPayload::PollPayment {
                payment_id: payment.id,
            },
            0,
        )
        .await;
    assert_eq!(result, JobResult::Done);
    let mut conn = services.store().conn().await.unwrap();
    assert_eq!(
        conn.payments().get(payment.id).await.unwrap().status,
        PaymentStatus::Refunded,
        "ожидающий возврат автосписания подтверждён опросом"
    );
    assert_eq!(
        conn.payments().provider_refunds(payment.id).await.unwrap()[0].status,
        ProviderRefundStatus::Succeeded
    );
    drop(conn);
    setup.db.cleanup().await;
}

struct SlowRefundKassa;

#[async_trait]
impl PaymentProvider for SlowRefundKassa {
    fn id(&self) -> &'static str {
        "slow_refund_kassa"
    }

    fn title(&self) -> &'static str {
        "Касса с долгими возвратами"
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

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        _config: &ProviderConfig,
        _request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        Err(PaymentError::Unsupported("счёт"))
    }

    async fn parse_webhook(
        &self,
        _config: &ProviderConfig,
        _request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::BadSignature)
    }

    async fn refund(
        &self,
        _config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("slow-{}", request.payment_id)),
            completed: false,
        })
    }

    async fn fetch_refund(
        &self,
        _config: &ProviderConfig,
        _provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        Ok(None)
    }
}

struct PendingRefund {
    setup: Setup,
    clock: Arc<FixedClock>,
    payment: Payment,
}

async fn stalled_order_with_pending_refund(telegram_id: i64) -> Option<PendingRefund> {
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let setup = setup_full(
        Vec::new(),
        clock.clone(),
        ProviderRegistry::new(vec![Arc::new(SlowRefundKassa)]),
    )
    .await?;
    let services = &setup.services;
    let client_id = client(services, telegram_id).await;
    let tariff = tariff(services).await;
    let payment = pending_payment_by(
        services,
        client_id,
        &tariff,
        services.now() + chrono::Duration::days(2),
        SlowRefundKassa.id(),
        None,
    )
    .await;
    {
        let mut tx = services.store().begin().await.unwrap();
        tx.payments()
            .transition(
                payment.id,
                PaymentStatus::Paid,
                Some(&format!("sx-{telegram_id}")),
                None,
            )
            .await
            .unwrap();
        tx.orders()
            .transition(payment.order_id, OrderStatus::Paid)
            .await
            .unwrap();
        let job = tx
            .jobs()
            .enqueue(
                &JobPayload::FulfillOrder {
                    order_id: payment.order_id,
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        let mut conn = services.store().conn().await.unwrap();
        let claimed = conn
            .jobs()
            .claim(10, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(claimed[0].id, job);
        conn.jobs().review(job, "попытки кончились").await.unwrap();
    }
    let (admin, _) = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    let requested = services
        .inbox()
        .refund(
            payment.id,
            &RefundCommand {
                amount: None,
                method: RefundMethod::Provider,
                subscription_action: RefundSubscriptionAction::Keep,
                reason: Some("не выдали, возвращаем".into()),
                key: None,
                outside: false,
            },
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    clock.set(Utc::now() + chrono::Duration::hours(2));
    Some(PendingRefund {
        setup,
        clock,
        payment,
    })
}

async fn settle_refund(pending: &PendingRefund, status: ProviderStatus) {
    let outcome = WebhookOutcome {
        event_key: format!("refund:slow-{}", pending.payment.id),
        payment: Some(sn_payments::PaymentRef::Payment(pending.payment.id)),
        provider_txid: None,
        status,
        amount: None,
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    };
    pending
        .setup
        .services
        .inbox()
        .apply_outcome(SlowRefundKassa.id(), &outcome)
        .await
        .unwrap();
}

async fn order_status(services: &Services, payment: &Payment) -> OrderStatus {
    let mut conn = services.store().conn().await.unwrap();
    conn.orders().get(payment.order_id).await.unwrap().status
}

#[tokio::test]
async fn досверка_не_выдаёт_заказ_с_ожидающим_возвратом_а_после_отказа_кассы_выдаёт() {
    let Some(pending) = stalled_order_with_pending_refund(591).await else {
        return;
    };
    let services = &pending.setup.services;
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    let report = worker.redeliver().await.unwrap();
    assert_eq!(report["requeued"], 0, "{report}");
    worker.tick().await.unwrap();
    assert_eq!(
        order_status(services, &pending.payment).await,
        OrderStatus::Paid
    );
    assert!(pending.setup.remna.users().is_empty());
    settle_refund(&pending, ProviderStatus::RefundDeclined).await;
    pending.clock.set(Utc::now() + chrono::Duration::hours(3));
    worker.redeliver().await.unwrap();
    worker.tick().await.unwrap();
    assert_eq!(
        order_status(services, &pending.payment).await,
        OrderStatus::Fulfilled
    );
    assert_eq!(pending.setup.remna.users().len(), 1);
    pending.setup.db.cleanup().await;
}

#[tokio::test]
async fn после_подтверждения_возврата_заказ_не_выдаётся() {
    let Some(pending) = stalled_order_with_pending_refund(592).await else {
        return;
    };
    let services = &pending.setup.services;
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    assert_eq!(worker.redeliver().await.unwrap()["requeued"], 0);
    settle_refund(&pending, ProviderStatus::Refunded).await;
    assert_eq!(
        order_status(services, &pending.payment).await,
        OrderStatus::Refunded
    );
    pending.clock.set(Utc::now() + chrono::Duration::hours(3));
    assert_eq!(worker.redeliver().await.unwrap()["requeued"], 0);
    worker.tick().await.unwrap();
    assert!(pending.setup.remna.users().is_empty());
    assert_eq!(
        order_status(services, &pending.payment).await,
        OrderStatus::Refunded
    );
    pending.setup.db.cleanup().await;
}

async fn buy(services: &Services, client_id: ClientId, tariff: &Tariff) -> OrderId {
    let subscription_id = {
        let mut conn = services.store().conn().await.unwrap();
        conn.subscriptions()
            .by_client(client_id)
            .await
            .unwrap()
            .first()
            .map(|s| s.id)
    };
    let result = services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: CartRequest {
                    tariff_id: tariff.id,
                    period_id: tariff.periods[0].id,
                    extra_devices: 0,
                    subscription_id,
                    promo_code: None,
                },
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Completed { order_id } = result else {
        panic!("оплата с баланса должна проходить сразу: {result:?}");
    };
    let mut conn = services.store().conn().await.unwrap();
    conn.orders().by_public_id(order_id).await.unwrap().id
}

#[tokio::test]
async fn брошенная_выдача_заказа_на_проверке_не_держит_следующие_заказы() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let services = &setup.services;
    let client_id = client(services, 593).await;
    let standard = tariff(services).await;
    services
        .change_balance(BalanceChange {
            client_id,
            wallet: Wallet::Main,
            amount: Money::new(5_000, Currency::Usd),
            kind: LedgerKind::TopUp,
            comment: None,
            idempotency_key: "test:topup:593".into(),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
    let worker = Worker::new(services.clone(), WorkerConfig::default());
    buy(services, client_id, &standard).await;
    worker.tick().await.unwrap();
    assert_eq!(setup.remna.users().len(), 1);
    let before = setup.remna.users()[0].expire_at;
    let missing: SquadId =
        serde_json::from_value(serde_json::json!("00000000-0000-0000-0000-000000000077")).unwrap();
    let wide = {
        let mut conn = services.store().conn().await.unwrap();
        conn.tariffs()
            .create(&NewTariff {
                traffic_addon: None,
                category_id: standard.category_id,
                name: "Стандарт плюс".into(),
                description: None,
                on_sale: true,
                sort: 1,
                devices: DevicePolicy {
                    included: 3,
                    extra: None,
                },
                traffic: TrafficPolicy::UNLIMITED,
                squads: vec![squad(), missing],
                external_squad: None,
                periods: vec![NewPeriod {
                    days: 30,
                    prices: CurrencyPrices {
                        usd: Some(399),
                        rub: Some(29_900),
                    },
                    stars: None,
                    sort: 0,
                    active: true,
                }],
            })
            .await
            .unwrap()
    };
    let held = buy(services, client_id, &wide).await;
    worker.tick().await.unwrap();
    assert_eq!(
        {
            let mut conn = services.store().conn().await.unwrap();
            conn.orders().get(held).await.unwrap().status
        },
        OrderStatus::Review
    );
    let mut squads = setup.remna.squads();
    squads.push(InternalSquad {
        uuid: missing.0,
        name: "Резерв".into(),
        view_position: 1,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    });
    setup.remna.set_squads(squads);
    services
        .fulfillment()
        .redeliver(held, Actor::System)
        .await
        .unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        loop {
            let job = conn
                .jobs()
                .claim(1, Duration::from_secs(60))
                .await
                .unwrap()
                .remove(0);
            assert_eq!(
                job.typed().unwrap(),
                JobPayload::FulfillOrder { order_id: held }
            );
            conn.jobs()
                .retry_leased(
                    &job,
                    "Remnawave недоступна",
                    Utc::now() - chrono::Duration::seconds(1),
                )
                .await
                .unwrap();
            if job.attempts + 1 == job.max_attempts {
                break;
            }
        }
    }
    setup
        .remna
        .fail_on("update_user", sn_remnawave::fake::Fault::Timeout);
    worker.tick().await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    let state = conn.orders().plan_state(held).await.unwrap();
    assert_eq!(state.stage(), sn_store::PlanStage::Stale);
    let reason = conn.orders().review_reason(held).await.unwrap().unwrap();
    assert!(
        reason.contains("выдача не завершилась за 12 попыток"),
        "{reason}"
    );
    assert!(reason.contains("Пересчитать и выдать"), "{reason}");
    assert!(
        conn.jobs()
            .active_fulfillment(held)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    assert_eq!(setup.remna.users()[0].expire_at, before);
    let next = buy(services, client_id, &standard).await;
    worker.tick().await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    assert_eq!(
        conn.orders().get(next).await.unwrap().status,
        OrderStatus::Fulfilled
    );
    assert_eq!(
        conn.orders().get(held).await.unwrap().status,
        OrderStatus::Review
    );
    drop(conn);
    assert_eq!(
        setup.remna.users()[0].expire_at,
        before + chrono::Duration::days(30)
    );
    setup.db.cleanup().await;
}

#[test]
fn состояние_периодической_работы_читается_из_json() {
    let at = DateTime::parse_from_rfc3339("2026-11-17T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let state = PeriodicState {
        last_started_at: Some(at),
        last_success_at: None,
        last_error: Some("Remnawave недоступна".into()),
        last_error_at: Some(at),
        last_report: Some(serde_json::json!({"due": 3})),
    };
    assert_eq!(PeriodicState::from_json(&state.to_json()), state);
    assert_eq!(
        PeriodicState::from_json(&serde_json::json!({})),
        PeriodicState::default()
    );
}

#[tokio::test]
async fn слежение_за_нодами_идёт_по_расписанию_и_пишет_переход_в_ленту() {
    let start = Utc::now();
    let clock = Arc::new(FixedClock::new(start));
    let Some(setup) = setup_with_clock(Vec::new(), clock.clone()).await else {
        return;
    };
    let uuid =
        serde_json::from_value(serde_json::json!("00000000-0000-4000-8000-0000000000de")).unwrap();
    let mut node = sn_remnawave::fake::sample_node(uuid, "Germany", "DE");
    setup.remna.set_nodes(vec![node.clone()]);
    let worker = Worker::new(setup.services.clone(), WorkerConfig::default());
    let first = worker
        .run_periodic(Periodic::NodeWatch)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first["nodes"], 1);
    assert_eq!(first["events"], 0);
    node.is_connected = false;
    node.last_status_change = Some(start + chrono::Duration::seconds(20));
    setup.remna.set_nodes(vec![node]);
    clock.set(start + chrono::Duration::seconds(10));
    assert!(
        worker
            .run_periodic(Periodic::NodeWatch)
            .await
            .unwrap()
            .is_none()
    );
    clock.set(start + chrono::Duration::seconds(30));
    let second = worker
        .run_periodic(Periodic::NodeWatch)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second["events"], 1);
    let state = periodic_state(&setup.services, Periodic::NodeWatch)
        .await
        .unwrap();
    assert_eq!(state.last_report, Some(second));
    let mut conn = setup.services.store().conn().await.unwrap();
    let feed = conn.remna_events().feed(None, 10).await.unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].event, "node.connection_lost");
    assert_eq!(
        feed[0].subject_id.as_deref(),
        Some("00000000-0000-4000-8000-0000000000de")
    );
    assert!(feed[0].processed_at.is_some());
    drop(conn);
    setup.services.set_remna(None);
    clock.set(start + chrono::Duration::seconds(120));
    let skipped = worker
        .run_periodic(Periodic::NodeWatch)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(skipped["skipped"], "not_configured");
    setup.db.cleanup().await;
}

#[tokio::test]
async fn задания_выполняются_пока_идёт_долгая_сверка() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let services = &setup.services;
    let owner = client(services, 701).await;
    setup
        .remna
        .insert_user(sn_remnawave::fake::sample_user(7001, "sn_7001"));
    let held = {
        let mut conn = services.store().conn().await.unwrap();
        let created = conn
            .subscriptions()
            .create(&NewSubscription {
                client_id: owner,
                tariff_id: None,
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState {
                    enabled: false,
                    period_id: None,
                },
            })
            .await
            .unwrap();
        conn.subscriptions()
            .link_remna(
                created.id,
                &sn_domain::subscription::RemnaLink {
                    user_id: sn_domain::ids::RemnaUserId(7001),
                    username: "sn_7001".into(),
                    short_uuid: Some("short7001".into()),
                },
            )
            .await
            .unwrap();
        created
    };
    let mut lock = services.store().begin().await.unwrap();
    lock.subscriptions().lock(held.id).await.unwrap();
    let buyer = client(services, 702).await;
    let tariff = tariff(services).await;
    services
        .change_balance(BalanceChange {
            client_id: buyer,
            wallet: Wallet::Main,
            amount: Money::new(500, Currency::Usd),
            kind: LedgerKind::TopUp,
            comment: None,
            idempotency_key: "test:topup:702".into(),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let config = WorkerConfig {
        poll_interval: Duration::from_millis(50),
        ..WorkerConfig::default()
    };
    let running = tokio::spawn(Worker::new(services.clone(), config).run(cancel.clone()));
    let until = Instant::now() + Duration::from_secs(10);
    while periodic_state(services, Periodic::Reconcile)
        .await
        .unwrap()
        .last_started_at
        .is_none()
    {
        assert!(Instant::now() < until, "сверка не началась");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    services
        .checkout()
        .start(
            buyer,
            &CheckoutRequest {
                cart: CartRequest {
                    tariff_id: tariff.id,
                    period_id: tariff.periods[0].id,
                    extra_devices: 0,
                    subscription_id: None,
                    promo_code: None,
                },
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let issued = {
            let mut conn = services.store().conn().await.unwrap();
            conn.subscriptions()
                .by_client(buyer)
                .await
                .unwrap()
                .iter()
                .any(|s| s.remna.is_some())
        };
        if issued {
            break;
        }
        assert!(Instant::now() < until, "заказ не выдан, пока сверка ждёт");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        periodic_state(services, Periodic::Reconcile)
            .await
            .unwrap()
            .last_success_at
            .is_none()
    );
    lock.rollback().await.unwrap();
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap();
    let state = periodic_state(services, Periodic::Reconcile).await.unwrap();
    assert!(state.last_success_at.is_some());
    setup.db.cleanup().await;
}

struct Blocker {
    started: std::sync::atomic::AtomicBool,
    finished: std::sync::atomic::AtomicBool,
    release: tokio::sync::Semaphore,
}

#[async_trait]
impl Module for Blocker {
    fn id(&self) -> ModuleId {
        ModuleId::Broadcast
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: "slow",
            every: Duration::from_secs(3_600),
        }]
    }

    async fn run_task(&self, _ctx: &ModuleContext, _task: &str) -> Result<(), ModuleError> {
        self.started
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _permit = self.release.acquire().await;
        self.finished
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn долгая_задача_модуля_не_держит_выдачу_заказов() {
    let blocker = Arc::new(Blocker {
        started: std::sync::atomic::AtomicBool::new(false),
        finished: std::sync::atomic::AtomicBool::new(false),
        release: tokio::sync::Semaphore::new(0),
    });
    let Some(setup) = setup(vec![blocker.clone()]).await else {
        return;
    };
    let services = &setup.services;
    let buyer = client(services, 703).await;
    let tariff = tariff(services).await;
    services
        .change_balance(BalanceChange {
            client_id: buyer,
            wallet: Wallet::Main,
            amount: Money::new(500, Currency::Usd),
            kind: LedgerKind::TopUp,
            comment: None,
            idempotency_key: "test:topup:703".into(),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let config = WorkerConfig {
        poll_interval: Duration::from_millis(50),
        ..WorkerConfig::default()
    };
    let running = tokio::spawn(Worker::new(services.clone(), config).run(cancel.clone()));
    let until = Instant::now() + Duration::from_secs(10);
    while !blocker.started.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(Instant::now() < until, "задача модуля не началась");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    services
        .checkout()
        .start(
            buyer,
            &CheckoutRequest {
                cart: CartRequest {
                    tariff_id: tariff.id,
                    period_id: tariff.periods[0].id,
                    extra_devices: 0,
                    subscription_id: None,
                    promo_code: None,
                },
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let issued = {
            let mut conn = services.store().conn().await.unwrap();
            conn.subscriptions()
                .by_client(buyer)
                .await
                .unwrap()
                .iter()
                .any(|s| s.remna.is_some())
        };
        if issued {
            break;
        }
        assert!(
            Instant::now() < until,
            "заказ не выдан, пока идёт задача модуля"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    blocker.release.add_permits(1);
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap();
    assert!(blocker.finished.load(std::sync::atomic::Ordering::SeqCst));
    setup.db.cleanup().await;
}

struct Broken;

#[async_trait]
impl JobHandler for Broken {
    async fn handle(
        &self,
        _services: &Services,
        _payload: &JobPayload,
        _attempt: u32,
    ) -> JobResult {
        panic!("сломанная задача");
    }
}

#[tokio::test]
async fn паника_задачи_не_останавливает_воркер() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    {
        let mut conn = setup.services.store().conn().await.unwrap();
        for event_id in [1, 2] {
            conn.jobs()
                .enqueue(&JobPayload::ProcessRemnaEvent { event_id }, None)
                .await
                .unwrap()
                .unwrap();
        }
    }
    let worker =
        Worker::new(setup.services.clone(), WorkerConfig::default()).with_handler(Arc::new(Broken));
    assert_eq!(worker.tick().await.unwrap(), 2);
    let counts = job_counts(&setup.services).await;
    assert!(
        counts.iter().all(|(status, _)| status != "done"),
        "{counts:?}"
    );
    assert_eq!(counts.iter().map(|(_, n)| *n).sum::<i64>(), 2, "{counts:?}");
    setup.db.cleanup().await;
}

#[tokio::test]
async fn воркер_запускает_каждую_периодическую_работу() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let cancel = CancellationToken::new();
    let running = tokio::spawn(
        Worker::new(setup.services.clone(), WorkerConfig::default()).run(cancel.clone()),
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut missing: Vec<&str> = Vec::new();
    loop {
        missing.clear();
        for periodic in Periodic::ALL {
            let state = periodic_state(&setup.services, periodic).await.unwrap();
            if state.last_started_at.is_none() {
                missing.push(periodic.name());
            }
        }
        if missing.is_empty() || tokio::time::Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    cancel.cancel();
    running.await.unwrap();
    assert!(
        missing.is_empty(),
        "периодические работы не запускались: {missing:?}"
    );
    setup.db.cleanup().await;
}
