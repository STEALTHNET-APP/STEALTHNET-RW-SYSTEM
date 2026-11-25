use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use chrono::{DateTime, Duration, Utc};
use sn_domain::client::{IdentityKind, Locale, NewIdentity, RegistrationSource};
use sn_domain::ids::{CategoryId, ClientId, RemnaUserId, SubscriptionId, TariffId};
use sn_domain::money::Currency;
use sn_domain::paging::PageRequest;
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use std::collections::BTreeSet;

struct World {
    now: DateTime<Utc>,
    standard: TariffId,
    family: TariffId,
    unblock: TariffId,
    standard_category: CategoryId,
    unblock_category: CategoryId,
    regular: ClientId,
    family_client: ClientId,
    lapsed: ClientId,
    old: ClientId,
    silent: ClientId,
    inviter: ClientId,
    blocked: ClientId,
    gone: ClientId,
}

async fn person(
    db: &mut impl Db,
    source: RegistrationSource,
    locale: Locale,
    telegram: Option<i64>,
    email: Option<&str>,
) -> ClientId {
    let id = db
        .clients()
        .create(&NewClient {
            locale,
            registration_source: source,
            note: None,
        })
        .await
        .unwrap()
        .id;
    if let Some(telegram) = telegram {
        let identity =
            NewIdentity::new(IdentityKind::Telegram, &telegram.to_string(), true).unwrap();
        db.clients().add_identity(id, &identity).await.unwrap();
    }
    if let Some(email) = email {
        let identity = NewIdentity::new(IdentityKind::Email, email, true).unwrap();
        db.clients().add_identity(id, &identity).await.unwrap();
    }
    id
}

async fn plan(
    db: &mut impl Db,
    client: ClientId,
    tariff: Option<TariffId>,
    kind: SubscriptionKind,
    status: SubscriptionStatus,
    expire_at: DateTime<Utc>,
    autorenew: bool,
) -> SubscriptionId {
    let id = db
        .subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: tariff,
            kind,
            extra_devices: 0,
            autorenew: AutoRenewState {
                enabled: autorenew,
                period_id: None,
            },
        })
        .await
        .unwrap()
        .id;
    db.subscriptions()
        .link_remna(
            id,
            &RemnaLink {
                user_id: RemnaUserId(50_000 + id.0),
                username: format!("sn_filter_{}", id.0),
                short_uuid: Some(format!("filter{}", id.0)),
            },
        )
        .await
        .unwrap();
    db.subscriptions()
        .apply_mirror(
            id,
            &RemnaMirror {
                status: Some(status),
                expire_at: Some(expire_at),
                first_connected_at: Some(Utc::now()),
                ..RemnaMirror::default()
            },
        )
        .await
        .unwrap();
    id
}

async fn exec(db: &mut impl Db, sql: &str, id: i64) {
    sqlx::query(sql).bind(id).execute(db.conn()).await.unwrap();
}

async fn world(db: &mut impl Db) -> World {
    let now = Utc::now();
    let standard_category = category(db, "Стандарт").await;
    let unblock_category = category(db, "Unblock").await;
    let mut new = tariff(standard_category, vec![period(30, 299, 19_900, 150)], &[1]);
    new.name = "Стандарт".into();
    let standard = db.tariffs().create(&new).await.unwrap().id;
    new.name = "Семейный".into();
    let family = db.tariffs().create(&new).await.unwrap().id;
    let mut new = tariff(unblock_category, vec![period(30, 349, 24_900, 175)], &[2]);
    new.name = "Unblock".into();
    let unblock = db.tariffs().create(&new).await.unwrap().id;

    let regular = person(
        db,
        RegistrationSource::Bot,
        Locale::Ru,
        Some(7_100_001),
        None,
    )
    .await;
    plan(
        db,
        regular,
        Some(standard),
        SubscriptionKind::Paid,
        SubscriptionStatus::Active,
        now + Duration::days(20),
        true,
    )
    .await;
    paid(db, regular, "cryptopay", usd(500)).await;

    let family_client = person(
        db,
        RegistrationSource::Miniapp,
        Locale::En,
        None,
        Some("filter.family@example.com"),
    )
    .await;
    plan(
        db,
        family_client,
        Some(family),
        SubscriptionKind::Paid,
        SubscriptionStatus::Active,
        now + Duration::days(5),
        false,
    )
    .await;

    let lapsed = person(
        db,
        RegistrationSource::Web,
        Locale::Ru,
        Some(7_100_003),
        Some("filter.lapsed@example.com"),
    )
    .await;
    plan(
        db,
        lapsed,
        Some(unblock),
        SubscriptionKind::Paid,
        SubscriptionStatus::Expired,
        now - Duration::days(10),
        false,
    )
    .await;
    paid(db, lapsed, "cryptopay", usd(3_000)).await;
    sqlx::query(
        "INSERT INTO client_balances (client_id, wallet, currency, amount_minor) VALUES ($1, 'main', 'USD', 200)",
    )
    .bind(lapsed.0)
    .execute(db.conn())
    .await
    .unwrap();

    let old = person(
        db,
        RegistrationSource::Import,
        Locale::Ru,
        Some(7_100_004),
        None,
    )
    .await;
    plan(
        db,
        old,
        Some(standard),
        SubscriptionKind::Paid,
        SubscriptionStatus::Expired,
        now - Duration::days(60),
        false,
    )
    .await;

    let silent = person(
        db,
        RegistrationSource::Bot,
        Locale::Ru,
        Some(7_100_005),
        None,
    )
    .await;
    exec(
        db,
        "UPDATE clients SET bot_unreachable = true WHERE id = $1",
        silent.0,
    )
    .await;
    sqlx::query(
        "INSERT INTO client_utm (client_id, source, campaign, start_param, registration_source) \
         VALUES ($1, 'tiktok', 'winter', 's_tiktok__k_winter', 'bot')",
    )
    .bind(silent.0)
    .execute(db.conn())
    .await
    .unwrap();

    let inviter = person(
        db,
        RegistrationSource::Admin,
        Locale::Ru,
        Some(7_100_006),
        None,
    )
    .await;
    plan(
        db,
        inviter,
        None,
        SubscriptionKind::Trial,
        SubscriptionStatus::Active,
        now + Duration::days(3),
        false,
    )
    .await;
    sqlx::query("INSERT INTO referral_links (client_id, referrer_id) VALUES ($1, $2)")
        .bind(regular.0)
        .bind(inviter.0)
        .execute(db.conn())
        .await
        .unwrap();

    let blocked = person(
        db,
        RegistrationSource::Bot,
        Locale::Ru,
        Some(7_100_007),
        None,
    )
    .await;
    plan(
        db,
        blocked,
        Some(standard),
        SubscriptionKind::Paid,
        SubscriptionStatus::Active,
        now + Duration::days(30),
        false,
    )
    .await;
    exec(
        db,
        "UPDATE clients SET status = 'blocked', blocked_at = now() WHERE id = $1",
        blocked.0,
    )
    .await;

    let gone = person(
        db,
        RegistrationSource::Migration,
        Locale::Ru,
        Some(7_100_008),
        None,
    )
    .await;
    let ended = plan(
        db,
        gone,
        Some(unblock),
        SubscriptionKind::Paid,
        SubscriptionStatus::Active,
        now + Duration::days(30),
        false,
    )
    .await;
    exec(
        db,
        "UPDATE subscriptions SET ended_at = now() WHERE id = $1",
        ended.0,
    )
    .await;

    for (client, days) in [
        (regular, 40),
        (family_client, 10),
        (lapsed, 3),
        (old, 300),
        (silent, 2),
        (inviter, 90),
        (blocked, 50),
        (gone, 200),
    ] {
        sqlx::query("UPDATE clients SET created_at = $2 WHERE id = $1")
            .bind(client.0)
            .bind(now - Duration::days(days))
            .execute(db.conn())
            .await
            .unwrap();
    }

    World {
        now,
        standard,
        family,
        unblock,
        standard_category,
        unblock_category,
        regular,
        family_client,
        lapsed,
        old,
        silent,
        inviter,
        blocked,
        gone,
    }
}

async fn ids(db: &mut impl Db, now: DateTime<Utc>, filter: &ClientFilter) -> BTreeSet<ClientId> {
    db.clients()
        .search_at(
            filter,
            PageRequest::new(Some(100), None),
            now,
            Currency::Usd,
        )
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.client.id)
        .collect()
}

async fn matched(
    db: &mut impl Db,
    now: DateTime<Utc>,
    conditions: ClientConditions,
) -> BTreeSet<ClientId> {
    ids(
        db,
        now,
        &ClientFilter {
            conditions,
            query: None,
        },
    )
    .await
}

fn set<const N: usize>(list: [ClientId; N]) -> BTreeSet<ClientId> {
    list.into_iter().collect()
}

#[tokio::test]
async fn каждое_условие_отбирает_своих_клиентов() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let w = world(&mut conn).await;
    let all = set([
        w.regular,
        w.family_client,
        w.lapsed,
        w.old,
        w.silent,
        w.inviter,
        w.blocked,
        w.gone,
    ]);
    let none = ClientConditions::default();
    let cases: Vec<(&str, ClientConditions, BTreeSet<ClientId>)> = vec![
        ("без условий", none.clone(), all.clone()),
        (
            "тариф",
            ClientConditions {
                tariff: vec![w.standard],
                ..none.clone()
            },
            set([w.regular, w.old, w.blocked]),
        ),
        (
            "два тарифа",
            ClientConditions {
                tariff: vec![w.standard, w.family],
                ..none.clone()
            },
            set([w.regular, w.family_client, w.old, w.blocked]),
        ),
        (
            "законченная подписка не держит тариф",
            ClientConditions {
                tariff: vec![w.unblock],
                ..none.clone()
            },
            set([w.lapsed]),
        ),
        (
            "категория",
            ClientConditions {
                category: vec![w.standard_category],
                ..none.clone()
            },
            set([w.regular, w.family_client, w.old, w.blocked]),
        ),
        (
            "другая категория",
            ClientConditions {
                category: vec![w.unblock_category],
                ..none.clone()
            },
            set([w.lapsed]),
        ),
        (
            "состояние и тариф",
            ClientConditions {
                tariff: vec![w.standard],
                status: Some(ClientSegment::Active),
                ..none.clone()
            },
            set([w.regular]),
        ),
        (
            "закончится за 7 дней",
            ClientConditions {
                ends: Some(7),
                ..none.clone()
            },
            set([w.family_client, w.inviter]),
        ),
        (
            "закончилась за 30 дней",
            ClientConditions {
                ended: Some(30),
                ..none.clone()
            },
            set([w.lapsed]),
        ),
        (
            "закончилась за 90 дней",
            ClientConditions {
                ended: Some(90),
                ..none.clone()
            },
            set([w.lapsed, w.old]),
        ),
        (
            "автопродление включено",
            ClientConditions {
                autorenew: Some(true),
                ..none.clone()
            },
            set([w.regular]),
        ),
        (
            "автопродление выключено",
            ClientConditions {
                autorenew: Some(false),
                ..none.clone()
            },
            set([w.family_client, w.lapsed, w.old, w.inviter, w.blocked]),
        ),
        (
            "из бота",
            ClientConditions {
                source: vec![ClientOrigin::Bot],
                ..none.clone()
            },
            set([w.regular, w.silent, w.blocked]),
        ),
        (
            "перенос",
            ClientConditions {
                source: vec![ClientOrigin::Transfer],
                ..none.clone()
            },
            set([w.old, w.gone]),
        ),
        (
            "сайт и мини-приложение",
            ClientConditions {
                source: vec![ClientOrigin::Web, ClientOrigin::Miniapp],
                ..none.clone()
            },
            set([w.family_client, w.lapsed]),
        ),
        (
            "язык",
            ClientConditions {
                lang: Some(Locale::En),
                ..none.clone()
            },
            set([w.family_client]),
        ),
        (
            "дата регистрации",
            ClientConditions {
                from: Some(w.now - Duration::days(45)),
                to: Some(w.now - Duration::days(5)),
                ..none.clone()
            },
            set([w.regular, w.family_client]),
        ),
        (
            "платили",
            ClientConditions {
                paid: Some(true),
                ..none.clone()
            },
            set([w.regular, w.lapsed]),
        ),
        (
            "не платили",
            ClientConditions {
                paid: Some(false),
                ..none.clone()
            },
            set([
                w.family_client,
                w.old,
                w.silent,
                w.inviter,
                w.blocked,
                w.gone,
            ]),
        ),
        (
            "оплатили от 10",
            ClientConditions {
                paid_from: Some(10),
                ..none.clone()
            },
            set([w.lapsed]),
        ),
        (
            "платили и не больше 10",
            ClientConditions {
                paid: Some(true),
                paid_to: Some(10),
                ..none.clone()
            },
            set([w.regular]),
        ),
        (
            "оплатили от 1 до 100",
            ClientConditions {
                paid_from: Some(1),
                paid_to: Some(100),
                ..none.clone()
            },
            set([w.regular, w.lapsed]),
        ),
        (
            "деньги на балансе",
            ClientConditions {
                balance: Some(true),
                ..none.clone()
            },
            set([w.lapsed]),
        ),
        (
            "баланс пустой",
            ClientConditions {
                balance: Some(false),
                ..none.clone()
            },
            set([
                w.regular,
                w.family_client,
                w.old,
                w.silent,
                w.inviter,
                w.blocked,
                w.gone,
            ]),
        ),
        (
            "без Telegram",
            ClientConditions {
                telegram: Some(false),
                ..none.clone()
            },
            set([w.family_client]),
        ),
        (
            "с почтой",
            ClientConditions {
                email: Some(true),
                ..none.clone()
            },
            set([w.family_client, w.lapsed]),
        ),
        (
            "бот остановлен",
            ClientConditions {
                bot_stopped: Some(true),
                ..none.clone()
            },
            set([w.silent]),
        ),
        (
            "пришёл по приглашению",
            ClientConditions {
                invited: Some(true),
                ..none.clone()
            },
            set([w.regular]),
        ),
        (
            "приглашал друзей",
            ClientConditions {
                invites: Some(true),
                ..none.clone()
            },
            set([w.inviter]),
        ),
        (
            "метка без учёта регистра",
            ClientConditions {
                utm: Some("TikTok".into()),
                ..none.clone()
            },
            set([w.silent]),
        ),
        (
            "старт-параметр целиком",
            ClientConditions {
                utm: Some("s_tiktok__k_winter".into()),
                ..none.clone()
            },
            set([w.silent]),
        ),
        (
            "несколько условий вместе",
            ClientConditions {
                tariff: vec![w.standard, w.family],
                source: vec![ClientOrigin::Bot],
                telegram: Some(true),
                ..none.clone()
            },
            set([w.regular, w.blocked]),
        ),
    ];
    for (name, conditions, expected) in cases {
        assert_eq!(
            matched(&mut conn, w.now, conditions).await,
            expected,
            "{name}"
        );
    }
    let with_query = ids(
        &mut conn,
        w.now,
        &ClientFilter {
            conditions: ClientConditions {
                lang: Some(Locale::En),
                ..none.clone()
            },
            query: Some("filter.family@example.com".into()),
        },
    )
    .await;
    assert_eq!(with_query, set([w.family_client]));
    let nobody = ids(
        &mut conn,
        w.now,
        &ClientFilter {
            conditions: ClientConditions {
                lang: Some(Locale::Ru),
                ..none.clone()
            },
            query: Some("filter.family@example.com".into()),
        },
    )
    .await;
    assert!(nobody.is_empty());
}

#[tokio::test]
async fn счётчики_и_страницы_учитывают_условия() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let w = world(&mut conn).await;
    let filter = ClientFilter {
        conditions: ClientConditions {
            tariff: vec![w.standard],
            status: Some(ClientSegment::Active),
            ..ClientConditions::default()
        },
        query: None,
    };
    let counts = conn
        .clients()
        .segment_counts_where(&filter, w.now, Currency::Usd)
        .await
        .unwrap();
    assert_eq!(counts.total, 3, "состояние не сужает счётчики");
    assert_eq!(counts.active, 1);
    assert_eq!(counts.expired, 1);
    assert_eq!(counts.blocked, 1);
    assert_eq!(counts.trial, 0);
    let all = conn.clients().segment_counts(w.now).await.unwrap();
    assert_eq!(
        all,
        conn.clients()
            .segment_counts_where(&ClientFilter::default(), w.now, Currency::Usd)
            .await
            .unwrap()
    );
    assert_eq!(all.total, 8);

    let by_tariff = ClientFilter {
        conditions: ClientConditions {
            tariff: vec![w.standard],
            ..ClientConditions::default()
        },
        query: None,
    };
    let first = conn
        .clients()
        .search_at(
            &by_tariff,
            PageRequest::new(Some(2), None),
            w.now,
            Currency::Usd,
        )
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.client.id)
            .collect::<Vec<_>>(),
        vec![w.regular, w.blocked],
        "от новых к старым"
    );
    let next = first.next.expect("есть вторая страница");
    let second = conn
        .clients()
        .search_at(
            &by_tariff,
            PageRequest::new(Some(2), Some(next)),
            w.now,
            Currency::Usd,
        )
        .await
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.client.id)
            .collect::<Vec<_>>(),
        vec![w.old]
    );
    assert_eq!(second.next, None);
    let row = &first.items[0];
    assert_eq!(row.paid_total_minor, 500);
    assert_eq!(
        row.subscription.as_ref().and_then(|s| s.tariff_id),
        Some(w.standard)
    );
}

#[tokio::test]
async fn варианты_для_фильтров_берут_тарифы_категории_и_метки() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let w = world(&mut conn).await;
    exec(
        &mut conn,
        "UPDATE tariffs SET deleted_at = now() WHERE id = $1",
        w.family.0,
    )
    .await;
    let options = filter_options(conn.conn()).await.unwrap();
    let tariffs: Vec<(TariffId, bool)> =
        options.tariffs.iter().map(|t| (t.id, t.deleted)).collect();
    assert!(tariffs.contains(&(w.standard, false)));
    assert!(
        tariffs.contains(&(w.family, true)),
        "удалённый тариф с живой подпиской остаётся в списке"
    );
    assert!(tariffs.contains(&(w.unblock, false)));
    assert_eq!(
        options.categories.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![w.standard_category, w.unblock_category]
    );
    assert_eq!(
        options.utm,
        vec!["tiktok".to_string(), "winter".to_string()]
    );
    assert_eq!(
        options.tariff_names().get(&w.standard).map(String::as_str),
        Some("Стандарт")
    );
    exec(
        &mut conn,
        "UPDATE subscriptions SET ended_at = now() WHERE tariff_id = $1",
        w.family.0,
    )
    .await;
    let options = filter_options(conn.conn()).await.unwrap();
    assert!(!options.tariffs.iter().any(|t| t.id == w.family));
}
