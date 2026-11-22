use crate::checkout::{CartRequest, Channel, CheckoutRequest, CheckoutResult};
use crate::services::{EnvFallbacks, ServiceDeps, Services};
use crate::{SystemClock, settings};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{CategoryId, ClientId, OrderId, SquadId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::money::{Currency, Money, Stars};
use sn_domain::order::Order;
use sn_domain::payment::providers;
use sn_domain::settings::{TrialSettings, keys, secret_keys};
use sn_domain::subscription::{Subscription, SubscriptionStatus};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices, Tariff};
use sn_domain::traffic::TrafficPolicy;
use sn_modules::{BalanceChange, CoreApi, Module, ModuleRegistry};
use sn_payments::providers::{Balance, CryptoPay};
use sn_payments::{
    ClientLabel, Invoice, InvoiceRequest, PayMethod, PaymentError, PaymentProvider, PaymentRef,
    PaymentResult, ProviderConfig, ProviderRegistry, ProviderStatus, RefundOutcome, RefundRequest,
    WebhookOutcome, WebhookReply, WebhookRequest,
};
use sn_remnawave::fake::FakeRemnawave;
use sn_remnawave::{InternalSquad, RemnaUser, SquadInfo};
use sn_store::testing::{TestDb, test_store};
use sn_store::{Db, NewCategory, NewPeriod, NewTariff, SecretBox};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub(crate) const CRYPTO_TOKEN: &str = "4321:TEST";
pub(crate) const TEST_PAY: &str = "testpay";
pub(crate) const PUBLIC_URL: &str = "https://panel.example.com";

pub(crate) fn moment(offset: chrono::Duration) -> DateTime<Utc> {
    DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap() + offset
}

pub(crate) fn squad(n: u128) -> SquadId {
    SquadId(Uuid::from_u128(n))
}

#[derive(Default)]
pub(crate) struct TestPay {
    pub invoices: Mutex<Vec<InvoiceRequest>>,
    pub fail_invoice: Mutex<bool>,
    pub status: Mutex<Option<WebhookOutcome>>,
    pub charge: Mutex<Option<ProviderStatus>>,
    pub charges: Mutex<Vec<(InvoiceRequest, String)>>,
    pub refunds: Mutex<Vec<RefundRequest>>,
    pub canceled: Mutex<Vec<String>>,
}

#[async_trait]
impl PaymentProvider for TestPay {
    fn id(&self) -> &'static str {
        TEST_PAY
    }

    fn title(&self) -> &'static str {
        "Тестовая касса"
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

    fn methods(&self, _currency: Currency) -> Vec<PayMethod> {
        vec![
            PayMethod::new("sbp", "СБП", "Через приложение банка"),
            PayMethod::new("card", "Карта", "Visa, Mastercard, МИР"),
            PayMethod::new("loan", "Рассрочка", "От 30 долларов").min_minor(3_000),
        ]
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn supports_recurring(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        _config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        if *self.fail_invoice.lock().unwrap() {
            return Err(PaymentError::Rejected("касса недоступна".into()));
        }
        self.invoices.lock().unwrap().push(request.clone());
        Ok(Invoice {
            pay_url: format!("https://pay.example.com/{}", request.payment_public_id),
            external_id: Some(format!("tp-{}", request.payment_id)),
            expires_at: None,
            payload: serde_json::json!({}),
        })
    }

    async fn cancel_invoice(
        &self,
        _config: &ProviderConfig,
        invoice: &sn_payments::IssuedInvoice<'_>,
    ) -> PaymentResult<()> {
        self.canceled
            .lock()
            .unwrap()
            .push(invoice.external_id.unwrap_or_default().to_string());
        Ok(())
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
        _external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        Ok(self.status.lock().unwrap().clone())
    }

    async fn charge_saved(
        &self,
        _config: &ProviderConfig,
        request: &InvoiceRequest,
        token: &str,
    ) -> PaymentResult<WebhookOutcome> {
        self.charges
            .lock()
            .unwrap()
            .push((request.clone(), token.to_string()));
        let Some(status) = *self.charge.lock().unwrap() else {
            return Err(PaymentError::Rejected("карта отклонена".into()));
        };
        Ok(WebhookOutcome {
            event_key: format!("charge:{}", request.payment_id),
            payment: Some(PaymentRef::Payment(request.payment_id)),
            provider_txid: Some(format!("tp-charge-{}", request.payment_id)),
            status,
            amount: Some(request.amount),
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        })
    }

    async fn refund(
        &self,
        _config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        self.refunds.lock().unwrap().push(request.clone());
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("rf-{}", request.payment_id)),
            completed: true,
        })
    }
}

pub(crate) struct Kit {
    pub db: TestDb,
    pub services: Services,
    pub remna: Arc<FakeRemnawave>,
    pub pay: Arc<TestPay>,
}

pub(crate) async fn kit() -> Option<Kit> {
    kit_with(Vec::new()).await
}

pub(crate) async fn kit_with(modules: Vec<Arc<dyn Module>>) -> Option<Kit> {
    let db = test_store().await?;
    let pay = Arc::new(TestPay::default());
    let registry = ProviderRegistry::new(vec![
        Arc::new(CryptoPay::new()),
        pay.clone(),
        Arc::new(Balance),
    ]);
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(registry),
        modules: Arc::new(ModuleRegistry::new(modules).unwrap()),
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
        conn.settings()
            .set(
                &keys::provider(providers::CRYPTOPAY),
                &serde_json::json!({"enabled": true}),
                None,
            )
            .await
            .unwrap();
        settings::write_secret(
            &mut conn,
            services.secrets(),
            &secret_keys::provider(providers::CRYPTOPAY, "api_token"),
            Some(CRYPTO_TOKEN),
            None,
        )
        .await
        .unwrap();
        conn.settings()
            .set_as(
                keys::TRIAL,
                &TrialSettings {
                    enabled: true,
                    days: 3,
                    squads: vec![squad(9)],
                    traffic: TrafficPolicy::UNLIMITED,
                    devices: 1,
                    plans: Vec::new(),
                },
                None,
            )
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
        pay,
    })
}

pub(crate) async fn client(services: &Services, telegram_id: i64) -> ClientId {
    let profile = TelegramProfile {
        id: telegram_id,
        username: Some(format!("user{telegram_id}")),
        first_name: Some("Алексей".into()),
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

pub(crate) async fn category(services: &Services, name: &str) -> CategoryId {
    let mut conn = services.store().conn().await.unwrap();
    conn.tariffs()
        .create_category(&NewCategory {
            name: name.into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap()
        .id
}

pub(crate) async fn tariff(
    services: &Services,
    category_id: CategoryId,
    name: &str,
    traffic: TrafficPolicy,
    squads: Vec<SquadId>,
) -> Tariff {
    let mut conn = services.store().conn().await.unwrap();
    conn.tariffs()
        .create(&NewTariff {
            traffic_addon: None,
            category_id,
            name: name.into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: Some(ExtraDevices {
                    max: 2,
                    monthly: CurrencyPrices {
                        usd: Some(99),
                        rub: Some(4_900),
                    },
                    monthly_stars: Some(Stars(50)),
                }),
            },
            traffic,
            squads,
            external_squad: None,
            periods: vec![
                NewPeriod {
                    days: 30,
                    prices: CurrencyPrices {
                        usd: Some(299),
                        rub: Some(19_900),
                    },
                    stars: Some(Stars(150)),
                    sort: 0,
                    active: true,
                },
                NewPeriod {
                    days: 90,
                    prices: CurrencyPrices {
                        usd: Some(699),
                        rub: Some(49_900),
                    },
                    stars: Some(Stars(350)),
                    sort: 1,
                    active: true,
                },
            ],
        })
        .await
        .unwrap()
}

pub(crate) async fn standard(services: &Services) -> Tariff {
    let category = category(services, "Стандарт").await;
    tariff(
        services,
        category,
        "Стандарт",
        TrafficPolicy::UNLIMITED,
        vec![squad(1), squad(2)],
    )
    .await
}

pub(crate) async fn credit(services: &Services, client_id: ClientId, minor: i64) {
    services
        .change_balance(BalanceChange {
            client_id,
            wallet: Wallet::Main,
            amount: Money::new(minor, Currency::Usd),
            kind: LedgerKind::TopUp,
            comment: None,
            idempotency_key: format!("test:{}", Uuid::new_v4()),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
}

pub(crate) async fn balance(services: &Services, client_id: ClientId) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    conn.ledger()
        .balance(client_id, Wallet::Main, Currency::Usd)
        .await
        .unwrap()
        .minor
}

pub(crate) fn cart(tariff: &Tariff, days: u32, subscription: Option<&Subscription>) -> CartRequest {
    CartRequest {
        tariff_id: tariff.id,
        period_id: tariff.periods.iter().find(|p| p.days == days).unwrap().id,
        extra_devices: 0,
        subscription_id: subscription.map(|s| s.id),
        promo_code: None,
    }
}

pub(crate) async fn buy_with_balance(
    services: &Services,
    client_id: ClientId,
    cart: CartRequest,
) -> Order {
    let result = services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart,
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Completed { order_id } = result else {
        panic!("оплата с баланса должна проходить сразу: {result:?}");
    };
    order_by_public_id(services, order_id).await
}

pub(crate) async fn order_by_public_id(services: &Services, public_id: Uuid) -> Order {
    let mut conn = services.store().conn().await.unwrap();
    conn.orders().by_public_id(public_id).await.unwrap()
}

pub(crate) async fn order(services: &Services, id: OrderId) -> Order {
    let mut conn = services.store().conn().await.unwrap();
    conn.orders().get(id).await.unwrap()
}

pub(crate) async fn subscription(services: &Services, order: &Order) -> Subscription {
    let id = order.subscription_id.expect("у заказа нет подписки");
    let mut conn = services.store().conn().await.unwrap();
    conn.subscriptions().get(id).await.unwrap()
}

pub(crate) async fn provision(
    kit: &Kit,
    client_id: ClientId,
    tariff: &Tariff,
    days: u32,
) -> Subscription {
    credit(&kit.services, client_id, 10_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(tariff, days, None)).await;
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(outcome, crate::fulfillment::FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = self::order(&kit.services, order.id).await;
    subscription(&kit.services, &order).await
}

pub(crate) async fn remna_user(kit: &Kit, subscription: &Subscription) -> RemnaUser {
    let link = subscription.remna.as_ref().expect("подписка не привязана");
    kit.remna
        .user(link.user_id)
        .expect("нет пользователя в Remnawave")
}

pub(crate) async fn set_user(
    kit: &Kit,
    subscription: &Subscription,
    change: impl FnOnce(&mut RemnaUser),
) -> Subscription {
    let mut user = remna_user(kit, subscription).await;
    change(&mut user);
    user.updated_at = Utc::now();
    kit.remna.insert_user(user.clone());
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.subscriptions()
        .apply_mirror(subscription.id, &user.mirror(None, Utc::now()))
        .await
        .unwrap();
    conn.subscriptions().get(subscription.id).await.unwrap()
}

pub(crate) async fn expire_in(
    kit: &Kit,
    subscription: &Subscription,
    at: DateTime<Utc>,
    status: SubscriptionStatus,
) -> Subscription {
    set_user(kit, subscription, |user| {
        user.expire_at = at;
        user.status = status.into();
    })
    .await
}

pub(crate) async fn jobs(services: &Services) -> Vec<JobPayload> {
    let mut conn = services.store().conn().await.unwrap();
    conn.jobs()
        .claim(100, std::time::Duration::from_secs(30))
        .await
        .unwrap()
        .into_iter()
        .map(|job| job.typed().unwrap())
        .collect()
}

pub(crate) async fn events(services: &Services, client_id: ClientId) -> Vec<DomainEvent> {
    let mut conn = services.store().conn().await.unwrap();
    let mut events: Vec<DomainEvent> = conn
        .events()
        .for_client(client_id, 200)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.event)
        .collect();
    events.reverse();
    events
}
