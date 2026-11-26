use crate::checkout::CartRequest;
use crate::inbox::InboundWebhook;
use crate::remna_sync::RemnaWebhookResult;
use crate::*;
use bytes::Bytes;
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::{CategoryId, ClientId};
use sn_domain::jobs::JobPayload;
use sn_domain::money::{Amount, Currency, Money, PayCurrency, Stars};
use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::payment::{NewPayment, PaymentStatus, ProviderId, providers};
use sn_domain::plan::PlanKind;
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::settings::{keys, secret_keys};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, Tariff};
use sn_domain::traffic::TrafficPolicy;
use sn_modules::ModuleRegistry;
use sn_payments::providers::cryptopay_signature;
use sn_payments::{ProviderRegistry, WebhookRequest};
use sn_remnawave::fake::FakeRemnawave;
use sn_store::testing::{TestDb, test_store};
use sn_store::{Db, NewCategory, NewPeriod, NewTariff, SecretBox};
use std::sync::Arc;
use uuid::Uuid;

const CRYPTO_TOKEN: &str = "4321:TEST";

async fn setup() -> Option<(TestDb, Services)> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
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
        settings::write_secret(
            &mut conn,
            services.secrets(),
            secret_keys::REMNA_WEBHOOK_SECRET,
            Some("0123456789abcdef0123456789abcdef"),
            None,
        )
        .await
        .unwrap();
    }
    services.reload().await.unwrap();
    services.set_remna(Some(Arc::new(FakeRemnawave::new())));
    Some((db, services))
}

async fn client(services: &Services) -> ClientId {
    let profile = TelegramProfile {
        id: 5_093_118_422,
        username: Some("morozov_a".into()),
        first_name: Some("Алексей".into()),
        last_name: None,
        language_code: Some("ru".into()),
        is_premium: false,
    };
    let (client, created) = services
        .clients()
        .ensure_telegram(&profile, RegistrationSource::Bot, Some("ref_ABC"))
        .await
        .unwrap();
    assert!(created);
    let (again, created_again) = services
        .clients()
        .ensure_telegram(&profile, RegistrationSource::Miniapp, None)
        .await
        .unwrap();
    assert!(!created_again);
    assert_eq!(again.id, client.id);
    client.id
}

async fn identity_version(services: &Services, telegram_id: i64) -> (String, serde_json::Value) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT xmin::text, meta FROM client_identities WHERE kind = 'telegram' AND value = $1",
    )
    .bind(telegram_id.to_string())
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

#[tokio::test]
async fn повторное_обновление_из_telegram_не_переписывает_тот_же_профиль() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let mut profile = TelegramProfile {
        id: 5_093_118_423,
        username: Some("lebedeva_m".into()),
        first_name: Some("Мария".into()),
        last_name: None,
        language_code: Some("ru".into()),
        is_premium: false,
    };
    let (client, created) = services
        .clients()
        .ensure_telegram(&profile, RegistrationSource::Bot, None)
        .await
        .unwrap();
    assert!(created);
    let (version, _) = identity_version(&services, profile.id).await;
    let (again, created) = services
        .clients()
        .ensure_telegram(&profile, RegistrationSource::Bot, None)
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(again.id, client.id);
    assert_eq!(identity_version(&services, profile.id).await.0, version);
    profile.username = Some("maria_l".into());
    services
        .clients()
        .ensure_telegram(&profile, RegistrationSource::Bot, None)
        .await
        .unwrap();
    let (changed, meta) = identity_version(&services, profile.id).await;
    assert_ne!(changed, version);
    assert_eq!(meta["username"], "maria_l");
    let mut conn = services.store().conn().await.unwrap();
    let words: String = sqlx::query_scalar(
        "SELECT search_text FROM client_identities WHERE kind = 'telegram' AND value = $1",
    )
    .bind(profile.id.to_string())
    .fetch_one(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert!(words.contains("maria_l"), "{words}");
    db.cleanup().await;
}

async fn tariff(services: &Services, category: &str) -> Tariff {
    let mut tx = services.store().begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: category.into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    let tariff = tx
        .tariffs()
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
            squads: vec![sn_domain::ids::SquadId(Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 90,
                prices: CurrencyPrices {
                    usd: Some(699),
                    rub: Some(49_900),
                },
                stars: Some(Stars(350)),
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tariff
}

async fn pending_payment(
    services: &Services,
    client_id: ClientId,
    tariff: &Tariff,
) -> (Uuid, Money) {
    let period = &tariff.periods[0];
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
            quote: q.clone(),
            snapshot: Some(tariff.snapshot(period)),
        })
        .await
        .unwrap();
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: order.id,
            client_id,
            provider: ProviderId::from(providers::CRYPTOPAY),
            method: None,
            amount: q.total.into(),
            expires_at: None,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (payment.public_id, q.total)
}

fn crypto_hook(payment: Uuid, amount: &str, token: &str) -> InboundWebhook {
    let body = serde_json::json!({
        "update_id": 1, "update_type": "invoice_paid", "request_date": "2026-11-16T14:52:00.000Z",
        "payload": {"invoice_id": 9001, "status": "paid", "currency_type": "fiat", "fiat": "USD",
                    "amount": amount, "payload": payment.to_string()}
    })
    .to_string();
    InboundWebhook {
        provider: providers::CRYPTOPAY.into(),
        request: WebhookRequest {
            headers: vec![(
                "crypto-pay-api-signature".into(),
                cryptopay_signature(token, body.as_bytes()),
            )],
            body: Bytes::from(body),
            remote_ip: None,
            route_suffix: None,
            query: None,
        },
    }
}

#[tokio::test]
async fn вебхук_оплаты_проводит_платёж_и_ставит_выдачу_один_раз() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let client_id = client(&services).await;
    let tariff = tariff(&services, "Стандарт").await;
    let (payment_id, total) = pending_payment(&services, client_id, &tariff).await;
    assert_eq!(total, Money::new(699, Currency::Usd));

    let forged = services
        .inbox()
        .handle(crypto_hook(payment_id, "6.99", "чужой токен"))
        .await
        .unwrap();
    assert_eq!(forged.reply.status(), 401);

    let accepted = services
        .inbox()
        .handle(crypto_hook(payment_id, "6.99", CRYPTO_TOKEN))
        .await
        .unwrap();
    assert_eq!(accepted.reply.status(), 200);
    let replay = services
        .inbox()
        .handle(crypto_hook(payment_id, "6.99", CRYPTO_TOKEN))
        .await
        .unwrap();
    assert_eq!(replay.reply.status(), 200);

    let mut conn = db.store.conn().await.unwrap();
    let payment = conn.payments().by_public_id(payment_id).await.unwrap();
    assert_eq!(payment.status, PaymentStatus::Paid);
    assert_eq!(payment.provider_txid.as_deref(), Some("9001"));
    let order = conn.orders().get(payment.order_id).await.unwrap();
    assert_eq!(order.status, OrderStatus::Paid);
    let jobs = conn
        .jobs()
        .claim(10, std::time::Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        jobs[0].typed().unwrap(),
        JobPayload::FulfillOrder { order_id: order.id }
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn недоплата_не_засчитывается() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let client_id = client(&services).await;
    let tariff = tariff(&services, "Стандарт").await;
    let (payment_id, _) = pending_payment(&services, client_id, &tariff).await;
    let reply = services
        .inbox()
        .handle(crypto_hook(payment_id, "6.98", CRYPTO_TOKEN))
        .await
        .unwrap();
    assert_eq!(reply.reply.status(), 200);
    let mut conn = db.store.conn().await.unwrap();
    let payment = conn.payments().by_public_id(payment_id).await.unwrap();
    assert_eq!(payment.status, PaymentStatus::Pending);
    assert_eq!(payment.amount, Amount::new(699, PayCurrency::Usd));
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn расчёт_цены_с_валютой_установки_и_способами_оплаты() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let client_id = client(&services).await;
    let tariff = tariff(&services, "Стандарт").await;
    let view = services
        .checkout()
        .quote(
            client_id,
            &CartRequest {
                tariff_id: tariff.id,
                period_id: tariff.periods[0].id,
                extra_devices: 0,
                subscription_id: None,
                promo_code: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::New);
    assert_eq!(view.quote.total, Money::new(699, Currency::Usd));
    let methods: Vec<&str> = view.methods.iter().map(|m| m.provider).collect();
    assert_eq!(methods.first(), Some(&providers::CRYPTOPAY));
    assert_eq!(methods.last(), Some(&providers::BALANCE));
    assert!(view.methods.last().unwrap().unavailable.is_some());
    let catalog = services.checkout().catalog().await.unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].category.id, CategoryId(tariff.category_id.0));
    db.cleanup().await;
}

#[tokio::test]
async fn вебхук_remnawave_принимается_один_раз() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let body = br#"{"scope":"service","event":"service.panel_started","timestamp":"2026-01-07T11:57:29.426Z","data":{"panelVersion":"3.4.11"}}"#;
    let signature = sn_remnawave::webhook::sign("0123456789abcdef0123456789abcdef", body);
    let sync = services.remna_sync();
    assert!(matches!(
        sync.accept_webhook(Some(&signature), body).await.unwrap(),
        RemnaWebhookResult::Accepted { .. }
    ));
    assert_eq!(
        sync.accept_webhook(Some(&signature), body).await.unwrap(),
        RemnaWebhookResult::Duplicate
    );
    assert_eq!(
        sync.accept_webhook(Some("deadbeef"), body).await.unwrap(),
        RemnaWebhookResult::BadSignature
    );
    db.cleanup().await;
}

#[tokio::test]
async fn владелец_создаётся_и_входит() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let auth = services.auth();
    let (id, created) = auth
        .upsert_owner("Owner@Example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    assert!(created);
    let login = auth
        .admin_login(
            "owner@example.com",
            "очень-надёжный-пароль",
            None,
            &Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(login.principal.admin_id, id);
    let principal = auth.admin_session(&login.token).await.unwrap();
    assert_eq!(principal.admin_id, id);
    assert!(matches!(
        auth.admin_login(
            "owner@example.com",
            "неверный-пароль-123",
            None,
            &Default::default()
        )
        .await,
        Err(ServiceError::Unauthorized)
    ));
    let (_, created_again) = auth
        .upsert_owner("owner@example.com", "новый-надёжный-пароль")
        .await
        .unwrap();
    assert!(!created_again);
    assert!(matches!(
        auth.admin_session(&login.token).await,
        Err(ServiceError::Unauthorized)
    ));
    db.cleanup().await;
}
