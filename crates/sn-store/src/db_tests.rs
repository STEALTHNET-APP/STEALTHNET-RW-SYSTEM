use crate::testing::test_store;
use crate::*;
use chrono::{Duration, Utc};
use sn_domain::client::{IdentityKind, Locale, NewIdentity, RegistrationSource};
use sn_domain::ids::{ClientId, RemnaUserId, SquadId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Amount, Currency, Money, PayCurrency, Stars};
use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::payment::{NewPayment, PaymentStatus, ProviderId};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices};
use sn_domain::traffic::{GIB, TrafficPolicy, TrafficReset};
use uuid::Uuid;

async fn new_client(db: &mut impl Db) -> ClientId {
    db.clients()
        .create(&NewClient {
            locale: Locale::Ru,
            registration_source: RegistrationSource::Bot,
            note: None,
        })
        .await
        .unwrap()
        .id
}

fn sample_tariff(category: sn_domain::ids::CategoryId) -> NewTariff {
    NewTariff {
        traffic_addon: None,
        category_id: category,
        name: "Unblock".into(),
        description: Some("Для мобильного интернета".into()),
        on_sale: true,
        sort: 1,
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
        traffic: TrafficPolicy {
            limit_bytes: Some(300 * GIB),
            reset: TrafficReset::Month,
        },
        squads: vec![SquadId(Uuid::from_u128(3)), SquadId(Uuid::from_u128(1))],
        external_squad: None,
        periods: vec![
            NewPeriod {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(349),
                    rub: Some(24_900),
                },
                stars: Some(Stars(175)),
                sort: 0,
                active: true,
            },
            NewPeriod {
                days: 90,
                prices: CurrencyPrices {
                    usd: Some(899),
                    rub: Some(64_900),
                },
                stars: Some(Stars(450)),
                sort: 1,
                active: true,
            },
        ],
    }
}

#[tokio::test]
async fn миграции_применяются_и_повторно_не_нужны() {
    let Some(db) = test_store().await else { return };
    assert_eq!(db.store.pending_migrations().await.unwrap(), 0);
    let installed = db.store.installed_at().await.unwrap().unwrap();
    db.store.migrate().await.unwrap();
    db.store.ping().await.unwrap();
    assert_eq!(db.store.installed_at().await.unwrap(), Some(installed));
    assert!(installed <= chrono::Utc::now());
    let earlier = chrono::DateTime::parse_from_rfc3339("2025-03-01T08:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query(
        "UPDATE _sqlx_migrations SET installed_on = $1 \
         WHERE version = (SELECT max(version) FROM _sqlx_migrations)",
    )
    .bind(earlier)
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        db.store.installed_at().await.unwrap(),
        Some(earlier),
        "момент установки — самая ранняя миграция"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn настройки_и_секреты() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    conn.settings()
        .set_as("core.currency", &Currency::Rub, None)
        .await
        .unwrap();
    let currency: Option<Currency> = conn.settings().get_as("core.currency").await.unwrap();
    assert_eq!(currency, Some(Currency::Rub));
    let sealed_box = SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap();
    let sealed = sealed_box
        .seal_str("remnawave.token", "token-value")
        .unwrap();
    conn.secrets()
        .set("remnawave.token", &sealed, None)
        .await
        .unwrap();
    let stored = conn
        .secrets()
        .get("remnawave.token")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sealed_box.open_string("remnawave.token", &stored).unwrap(),
        "token-value"
    );
    assert_eq!(
        conn.secrets().keys("remnawave.").await.unwrap(),
        ["remnawave.token"]
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn идентичность_принадлежит_одному_клиенту() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let first = new_client(&mut conn).await;
    let second = new_client(&mut conn).await;
    let identity = NewIdentity::new(IdentityKind::Telegram, "5093118422", true).unwrap();
    conn.clients().add_identity(first, &identity).await.unwrap();
    let err = conn
        .clients()
        .add_identity(second, &identity)
        .await
        .unwrap_err();
    assert_eq!(err.constraint(), Some("client_identities_kind_value_key"));
    let found = conn
        .clients()
        .find_by_identity(IdentityKind::Telegram, "5093118422")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, first);
    assert!(
        conn.trials()
            .claim(first, &[(IdentityKind::Telegram, "5093118422".into())])
            .await
            .unwrap()
    );
    assert!(
        !conn
            .trials()
            .claim(second, &[(IdentityKind::Telegram, "5093118422".into())])
            .await
            .unwrap()
    );
    let other = [(IdentityKind::Email, "new@example.org".to_string())];
    assert!(conn.trials().was_claimed(first, &other).await.unwrap());
    assert!(!conn.trials().was_claimed(second, &other).await.unwrap());
    assert!(
        !conn.trials().claim(first, &other).await.unwrap(),
        "пробный период один на клиента, даже если сменилась почта"
    );
    assert!(conn.trials().claim(second, &other).await.unwrap());
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn тариф_сохраняется_со_сроками_и_сквадами() {
    let Some(db) = test_store().await else { return };
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: "Unblock".into(),
            sort: 1,
            visible: true,
        })
        .await
        .unwrap();
    let tariff = tx
        .tariffs()
        .create(&sample_tariff(category.id))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(tariff.periods.len(), 2);
    assert_eq!(tariff.squads[0], SquadId(Uuid::from_u128(3)));
    assert_eq!(tariff.traffic.limit_bytes, Some(300 * GIB));
    assert_eq!(tariff.devices.max_total(), 5);
    let mut conn = db.store.conn().await.unwrap();
    let listed = conn.tariffs().list(false).await.unwrap();
    assert_eq!(listed, vec![tariff]);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn заказ_и_платёж_проходят_статусы_идемпотентно() {
    let Some(db) = test_store().await else { return };
    let mut tx = db.store.begin().await.unwrap();
    let client = new_client(&mut tx).await;
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: "Unblock".into(),
            sort: 1,
            visible: true,
        })
        .await
        .unwrap();
    let tariff = tx
        .tariffs()
        .create(&sample_tariff(category.id))
        .await
        .unwrap();
    let period = tariff.periods[1].clone();
    let q = quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff: &tariff,
        period_id: period.id,
        extra_devices: 1,
        sell_extra_devices: true,
        adjustments: &[],
        promotion: None,
    })
    .unwrap();
    let order = tx
        .orders()
        .create(&NewOrder {
            client_id: client,
            subscription_id: None,
            kind: OrderKind::New,
            intent: PurchaseIntent::Subscription {
                tariff_id: tariff.id,
                period_id: period.id,
                extra_devices: 1,
                target: None,
            },
            quote: q.clone(),
            snapshot: Some(tariff.snapshot(&period)),
        })
        .await
        .unwrap();
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: order.id,
            client_id: client,
            provider: ProviderId::from("cryptopay"),
            method: None,
            amount: q.total.into(),
            expires_at: Some(Utc::now() - Duration::minutes(1)),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut conn = db.store.conn().await.unwrap();
    let expired = conn.payments().expire_stale(Utc::now(), 10).await.unwrap();
    assert_eq!(expired, vec![payment.id]);
    let late = conn
        .payments()
        .transition(payment.id, PaymentStatus::Paid, Some("inv-1"), None)
        .await
        .unwrap();
    assert!(late.applied());
    let again = conn
        .payments()
        .transition(payment.id, PaymentStatus::Paid, Some("inv-1"), None)
        .await
        .unwrap();
    assert!(!again.applied());
    let found = conn
        .payments()
        .by_provider_txid("cryptopay", "inv-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.amount, Amount::new(q.total.minor, PayCurrency::Usd));
    let (paid, changed) = conn
        .orders()
        .transition(order.id, OrderStatus::Paid)
        .await
        .unwrap();
    assert!(changed);
    assert!(paid.paid_at.is_some());
    assert_eq!(paid.quote, q);
    assert!(
        conn.orders()
            .transition(order.id, OrderStatus::Pending)
            .await
            .is_err()
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn баланс_не_уходит_в_минус_и_не_задваивается() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let client = new_client(&mut conn).await;
    let topup = NewLedgerEntry {
        client_id: client,
        wallet: Wallet::Main,
        amount: Money::new(420, Currency::Usd),
        kind: LedgerKind::TopUp,
        order_id: None,
        payment_id: None,
        admin_id: None,
        comment: None,
        idempotency_key: Some("topup:test:1".into()),
    };
    let first = conn.ledger().append(&topup).await.unwrap();
    assert!(!first.duplicate);
    assert_eq!(first.balance_after, Money::new(420, Currency::Usd));
    let repeat = conn.ledger().append(&topup).await.unwrap();
    assert!(repeat.duplicate);
    assert_eq!(repeat.entry_id, first.entry_id);
    let purchase = NewLedgerEntry {
        amount: Money::new(-699, Currency::Usd),
        kind: LedgerKind::Purchase,
        idempotency_key: Some("purchase:test:1".into()),
        ..topup.clone()
    };
    assert!(matches!(
        conn.ledger().append(&purchase).await,
        Err(StoreError::InsufficientFunds)
    ));
    assert_eq!(
        conn.ledger()
            .balance(client, Wallet::Main, Currency::Usd)
            .await
            .unwrap(),
        Money::new(420, Currency::Usd)
    );
    drop(conn);
    let mut tx = db.store.begin().await.unwrap();
    assert!(matches!(
        tx.ledger().append(&purchase).await,
        Err(StoreError::InsufficientFunds)
    ));
    let ok = NewLedgerEntry {
        amount: Money::new(-420, Currency::Usd),
        ..purchase
    };
    assert_eq!(
        tx.ledger().append(&ok).await.unwrap().balance_after.minor,
        0
    );
    tx.commit().await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.ledger()
            .entries(client, sn_domain::paging::PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn очередь_задач_с_дедупом_и_арендой() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let payload = JobPayload::ProcessRemnaEvent { event_id: 42 };
    let id = conn.jobs().enqueue(&payload, None).await.unwrap().unwrap();
    assert!(conn.jobs().enqueue(&payload, None).await.unwrap().is_none());
    let claimed = conn
        .jobs()
        .claim(10, std::time::Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, id);
    assert_eq!(claimed[0].typed().unwrap(), payload);
    assert!(
        conn.jobs()
            .claim(10, std::time::Duration::from_secs(60))
            .await
            .unwrap()
            .is_empty()
    );
    conn.jobs().complete(id).await.unwrap();
    assert_eq!(
        conn.jobs().counts().await.unwrap(),
        vec![("done".to_string(), 1)]
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn повтор_вебхука_и_события_remnawave_отсекаются() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let hook = NewInboundWebhook {
        source: "cryptopay".into(),
        dedupe_key: "invoice_paid:1".into(),
        remote_ip: Some("203.0.113.7".into()),
        headers: serde_json::json!({}),
        body: Some(b"{}".to_vec()),
        parsed: None,
        event: None,
    };
    let first = conn.webhooks().record(&hook).await.unwrap();
    assert!(!first.duplicate);
    conn.webhooks()
        .finish(first.id, InboxOutcome::Accepted, None, None, None)
        .await
        .unwrap();
    let second = conn.webhooks().record(&hook).await.unwrap();
    assert!(second.duplicate);
    assert_eq!(second.outcome, InboxOutcome::Accepted);
    let event = NewRemnaEvent {
        body_sha256: vec![7; 32],
        scope: "user".into(),
        event: "user.modified".into(),
        occurred_at: Utc::now(),
        subject_kind: Some("user".into()),
        subject_id: Some("42".into()),
        payload: serde_json::json!({}),
    };
    assert!(conn.remna_events().insert(&event).await.unwrap().is_some());
    assert!(conn.remna_events().insert(&event).await.unwrap().is_none());
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn зеркало_подписки_не_откатывается_старым_событием() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let client = new_client(&mut conn).await;
    let sub = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: None,
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    conn.subscriptions()
        .link_remna(
            sub.id,
            &RemnaLink {
                user_id: RemnaUserId(77),
                username: "sn_test".into(),
                short_uuid: Some("abc".into()),
            },
        )
        .await
        .unwrap();
    let now = Utc::now();
    let fresh = RemnaMirror {
        status: Some(SubscriptionStatus::Limited),
        remna_updated_at: Some(now),
        ..RemnaMirror::default()
    };
    assert!(
        conn.subscriptions()
            .apply_mirror(sub.id, &fresh)
            .await
            .unwrap()
    );
    let stale = RemnaMirror {
        status: Some(SubscriptionStatus::Active),
        remna_updated_at: Some(now - Duration::minutes(5)),
        ..RemnaMirror::default()
    };
    assert!(
        !conn
            .subscriptions()
            .apply_mirror(sub.id, &stale)
            .await
            .unwrap()
    );
    let stored = conn
        .subscriptions()
        .by_remna_user(RemnaUserId(77))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.mirror.status, Some(SubscriptionStatus::Limited));
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn состояние_модулей_не_перетирает_выбор_оператора() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.modules()
            .ensure_defaults(&[("promo", true), ("gifts", false)])
            .await
            .unwrap(),
        2
    );
    conn.modules()
        .set_enabled("gifts", true, None)
        .await
        .unwrap();
    assert_eq!(
        conn.modules()
            .ensure_defaults(&[("promo", true), ("gifts", false)])
            .await
            .unwrap(),
        0
    );
    let states = conn.modules().all().await.unwrap();
    assert!(states.iter().any(|s| s.module == "gifts" && s.enabled));
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn служебные_таблицы_выдачи_и_бота() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let client = new_client(&mut conn).await;
    let sealed_box = SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap();
    let first = sealed_box.seal_str("card", "pm_1").unwrap();
    let second = sealed_box.seal_str("card", "pm_2").unwrap();
    conn.saved_methods()
        .save(client, "yookassa", &first, Some("Visa *1234"))
        .await
        .unwrap();
    conn.saved_methods()
        .save(client, "yookassa", &second, Some("Mir *5678"))
        .await
        .unwrap();
    let active = conn
        .saved_methods()
        .active(client, "yookassa")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sealed_box.open_string("card", &active.token).unwrap(),
        "pm_2"
    );
    let sub = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: None,
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    assert!(
        conn.autorenew()
            .record_attempt(
                sub.id,
                "2027-02-14",
                1,
                AttemptResult::Insufficient,
                None,
                None
            )
            .await
            .unwrap()
    );
    assert!(
        !conn
            .autorenew()
            .record_attempt(sub.id, "2027-02-14", 1, AttemptResult::Charged, None, None)
            .await
            .unwrap()
    );
    assert_eq!(
        conn.autorenew()
            .attempts(sub.id, "2027-02-14")
            .await
            .unwrap(),
        1
    );
    assert!(
        conn.autorenew()
            .mark_notified(client, "expiring", "2027-02-14")
            .await
            .unwrap()
    );
    assert!(
        !conn
            .autorenew()
            .mark_notified(client, "expiring", "2027-02-14")
            .await
            .unwrap()
    );
    conn.bot_state()
        .set("offset", &serde_json::json!(42))
        .await
        .unwrap();
    assert_eq!(
        conn.bot_state().get("offset").await.unwrap(),
        Some(serde_json::json!(42))
    );
    conn.bot_state()
        .set_dialog(
            5,
            &serde_json::json!({"await": "promo"}),
            Utc::now() + Duration::minutes(30),
        )
        .await
        .unwrap();
    assert!(conn.bot_state().dialog(5).await.unwrap().is_some());
    conn.bot_state().clear_dialog(5).await.unwrap();
    assert!(conn.bot_state().dialog(5).await.unwrap().is_none());
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn возраст_записи_состояния_считается_по_часам_базы_в_миг_чтения() {
    let Some(db) = test_store().await else { return };
    let key = "worker.возраст";
    let mut early = db.store.begin().await.unwrap();
    assert_eq!(early.bot_state().get_aged(key).await.unwrap(), None);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut conn = db.store.conn().await.unwrap();
    conn.bot_state()
        .set(key, &serde_json::json!({ "last": 7 }))
        .await
        .unwrap();
    let (value, age) = early.bot_state().get_aged(key).await.unwrap().unwrap();
    assert_eq!(value, serde_json::json!({ "last": 7 }));
    assert!(
        age >= Duration::zero() && age < Duration::seconds(30),
        "запись из другой транзакции кажется записью из будущего: {age}"
    );
    early.rollback().await.unwrap();
    for (shift, expected) in [("-90 seconds", 90), ("2 hours", -7_200)] {
        sqlx::query(
            "UPDATE bot_state SET updated_at = clock_timestamp() + $2::interval WHERE key = $1",
        )
        .bind(key)
        .bind(shift)
        .execute(conn.conn())
        .await
        .unwrap();
        let (_, age) = conn.bot_state().get_aged(key).await.unwrap().unwrap();
        assert!((age.num_seconds() - expected).abs() <= 30, "{shift}: {age}");
    }
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn ложные_отметки_антибота_снимаются_миграцией() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let cases = [
        ("migration", Some(1_000), vec!["burst"], None),
        (
            "import",
            Some(8_000_000_000_i64),
            vec!["young_account", "no_username", "burst"],
            None,
        ),
        (
            "migration",
            Some(2_000),
            vec!["burst", "disposable_email"],
            Some(vec!["disposable_email"]),
        ),
        (
            "import",
            Some(3_000),
            vec!["self_invite"],
            Some(vec!["self_invite"]),
        ),
        ("import", Some(8_100_000_000), vec!["mass_invites"], None),
        ("bot", Some(4_000), vec!["burst"], Some(vec!["burst"])),
        ("bot", Some(5_000), vec!["mass_invites"], None),
        ("web", None, vec!["mass_invites"], None),
        (
            "miniapp",
            Some(6_000),
            vec!["mass_invites", "self_invite"],
            Some(vec!["self_invite"]),
        ),
        (
            "bot",
            Some(8_200_000_000),
            vec!["mass_invites"],
            Some(vec!["mass_invites"]),
        ),
        (
            "bot",
            Some(7_000),
            vec!["mass_invites", "young_account"],
            Some(vec!["mass_invites", "young_account"]),
        ),
    ];
    sqlx::query(
        "WITH made AS (INSERT INTO clients (locale, registration_source, created_at) \
           VALUES ('ru', 'bot', now() - interval '400 days') RETURNING id) \
         INSERT INTO client_identities (client_id, kind, value, verified, created_at) \
         SELECT id, 'telegram', '7000000000', true, now() - interval '400 days' FROM made",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    let mut made = Vec::new();
    for (source, telegram, reasons, _) in &cases {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO clients (locale, registration_source) VALUES ('ru', $1) RETURNING id",
        )
        .bind(source)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        if let Some(telegram) = telegram {
            sqlx::query(
                "INSERT INTO client_identities (client_id, kind, value, verified) \
                 VALUES ($1, 'telegram', $2, true)",
            )
            .bind(id)
            .bind(telegram.to_string())
            .execute(conn.conn())
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO antibot_marks (client_id, reasons) VALUES ($1, $2)")
            .bind(id)
            .bind(serde_json::json!(reasons))
            .execute(conn.conn())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO antibot_events (client_id, action, reasons, dedupe) \
             VALUES ($1, 'marked', $2, 'marked:' || $1)",
        )
        .bind(id)
        .bind(serde_json::json!(reasons))
        .execute(conn.conn())
        .await
        .unwrap();
        made.push(id);
    }
    let script = include_str!("../migrations/0570_antibot_checkup.sql");
    let cleanup = &script[script.find("WITH setup").unwrap()..];
    for _ in 0..2 {
        sqlx::raw_sql(cleanup).execute(conn.conn()).await.unwrap();
    }
    for (id, (source, telegram, reasons, kept)) in made.iter().zip(&cases) {
        let mark: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT reasons FROM antibot_marks WHERE client_id = $1")
                .bind(id)
                .fetch_optional(conn.conn())
                .await
                .unwrap();
        let again: bool = sqlx::query_scalar(
            "SELECT dedupe IS NULL FROM antibot_events WHERE client_id = $1 AND action = 'marked'",
        )
        .bind(id)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        let case = format!("{source} {telegram:?} {reasons:?}");
        assert_eq!(
            mark,
            kept.as_ref().map(|kept| serde_json::json!(kept)),
            "{case}"
        );
        assert_eq!(again, kept.is_none(), "{case}");
    }
    drop(conn);
    db.cleanup().await;
}
