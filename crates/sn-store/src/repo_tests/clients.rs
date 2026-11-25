use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use chrono::{Duration, Utc};
use sn_domain::client::RegistrationSource;
use sn_domain::events::DomainEvent;
use sn_domain::ids::ClientId;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::paging::PageRequest;
use sn_domain::subscription::{SubscriptionKind, SubscriptionStatus};
use std::collections::HashSet;

async fn found(
    db: &mut impl Db,
    segment: Option<ClientSegment>,
    query: Option<&str>,
) -> Vec<ClientId> {
    db.clients()
        .search_at(
            &ClientFilter {
                conditions: ClientConditions {
                    status: segment,
                    ..ClientConditions::default()
                },
                query: query.map(str::to_string),
            },
            PageRequest::default(),
            Utc::now(),
            Currency::Usd,
        )
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.client.id)
        .collect()
}

fn active(kind: SubscriptionKind, days: i64, connected: bool) -> Sub {
    Sub {
        kind,
        status: Some(SubscriptionStatus::Active),
        expire_at: Some(Utc::now() + Duration::days(days)),
        connected,
    }
}

#[tokio::test]
async fn клиенты_ищутся_по_сегментам_и_строке() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let now = Utc::now();
    let alexey = client(
        &mut conn,
        &Person {
            telegram: Some(5_093_118_422),
            username: Some("morozov_a"),
            first_name: Some("Алексей"),
            last_name: Some("Морозов"),
            email: Some("a.morozov@example.com"),
        },
    )
    .await;
    let alexey_sub =
        subscription(&mut conn, alexey, active(SubscriptionKind::Paid, 90, true)).await;
    let ksenia = client(
        &mut conn,
        &Person {
            telegram: Some(1_001),
            username: Some("ks_smirnova"),
            first_name: Some("Ксения"),
            last_name: Some("Смирнова"),
            email: None,
        },
    )
    .await;
    subscription(&mut conn, ksenia, active(SubscriptionKind::Paid, 2, true)).await;
    let maria = client(
        &mut conn,
        &Person {
            email: Some("m.lebedeva@example.com"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(&mut conn, maria, active(SubscriptionKind::Trial, 2, false)).await;
    let igor = client(
        &mut conn,
        &Person {
            telegram: Some(1_002),
            username: Some("igor_nv"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(
        &mut conn,
        igor,
        Sub {
            status: Some(SubscriptionStatus::Limited),
            ..active(SubscriptionKind::Paid, 60, true)
        },
    )
    .await;
    let artem = client(
        &mut conn,
        &Person {
            telegram: Some(1_003),
            username: Some("artem_p"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(
        &mut conn,
        artem,
        Sub {
            status: Some(SubscriptionStatus::Expired),
            ..active(SubscriptionKind::Paid, -5, true)
        },
    )
    .await;
    let oleg = client(
        &mut conn,
        &Person {
            telegram: Some(1_004),
            username: Some("oleg_s"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(
        &mut conn,
        oleg,
        Sub {
            status: Some(SubscriptionStatus::Disabled),
            ..active(SubscriptionKind::Paid, 30, true)
        },
    )
    .await;
    conn.clients()
        .set_status(
            oleg,
            sn_domain::client::ClientStatus::Blocked,
            Some("Злоупотребление трафиком"),
        )
        .await
        .unwrap();
    let bare = client(
        &mut conn,
        &Person {
            telegram: Some(7_412),
            ..Person::NOBODY
        },
    )
    .await;
    let alena = client(
        &mut conn,
        &Person {
            telegram: Some(1_005),
            username: Some("alena"),
            first_name: Some("Алёна"),
            last_name: Some("Ёлкина"),
            email: None,
        },
    )
    .await;
    subscription(
        &mut conn,
        alena,
        Sub {
            status: Some(SubscriptionStatus::Expired),
            ..active(SubscriptionKind::Paid, -40, true)
        },
    )
    .await;
    let alena_sub = subscription(&mut conn, alena, active(SubscriptionKind::Paid, 20, true)).await;
    let gone = client(
        &mut conn,
        &Person {
            telegram: Some(1_006),
            username: Some("gone"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(&mut conn, gone, active(SubscriptionKind::Paid, 20, true)).await;
    conn.clients().soft_delete(gone).await.unwrap();
    let pending = client(
        &mut conn,
        &Person {
            telegram: Some(1_007),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(
        &mut conn,
        pending,
        Sub {
            status: None,
            expire_at: None,
            ..active(SubscriptionKind::Paid, 0, false)
        },
    )
    .await;
    let missing = client(
        &mut conn,
        &Person {
            telegram: Some(1_008),
            ..Person::NOBODY
        },
    )
    .await;
    let lost = subscription(&mut conn, missing, active(SubscriptionKind::Paid, 20, true)).await;
    conn.subscriptions().mark_missing(lost, true).await.unwrap();

    conn.ledger()
        .append(&NewLedgerEntry {
            client_id: alexey,
            wallet: Wallet::Main,
            amount: Money::new(420, Currency::Usd),
            kind: LedgerKind::TopUp,
            order_id: None,
            payment_id: None,
            admin_id: None,
            comment: None,
            idempotency_key: Some("topup:alexey".into()),
        })
        .await
        .unwrap();
    paid(&mut conn, alexey, "cryptopay", usd(699)).await;
    paid(&mut conn, alexey, "cryptopay", usd(640)).await;
    paid(&mut conn, alexey, "balance", usd(300)).await;
    paid(
        &mut conn,
        alexey,
        "stars",
        Amount::new(350, PayCurrency::Xtr),
    )
    .await;
    payment(&mut conn, alexey, "cryptopay", usd(999)).await;

    let counts = conn.clients().segment_counts(now).await.unwrap();
    assert_eq!(
        counts,
        SegmentCounts {
            total: 10,
            active: 3,
            trial: 1,
            no_subscription: 3,
            expiring_soon: 2,
            never_connected: 1,
            expired: 1,
            limited: 1,
            blocked: 1,
        }
    );
    assert_eq!(counts.get(Some(ClientSegment::Limited)), 1);

    assert_eq!(
        found(&mut conn, Some(ClientSegment::Active), None).await,
        [alena, ksenia, alexey]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::ExpiringSoon), None).await,
        [maria, ksenia]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Trial), None).await,
        [maria]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::NeverConnected), None).await,
        [maria]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Expired), None).await,
        [artem]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Limited), None).await,
        [igor]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Blocked), None).await,
        [oleg]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::NoSubscription), None).await,
        [missing, pending, bare]
    );
    assert_eq!(found(&mut conn, None, None).await.len(), 10);

    for (query, expected) in [
        ("морозов", vec![alexey]),
        ("МОРОЗОВ алексей", vec![alexey]),
        ("алена елкина", vec![alena]),
        ("@MOROZOV", vec![alexey]),
        ("@morozov_a", vec![alexey]),
        ("@%", vec![]),
        ("5093118422", vec![alexey]),
        ("7412", vec![bare]),
        ("a.morozov@example", vec![alexey]),
        ("M.Lebedeva@example.com", vec![maria]),
        ("нет такого", vec![]),
    ] {
        assert_eq!(
            found(&mut conn, None, Some(query)).await,
            expected,
            "{query}"
        );
    }
    assert_eq!(
        found(&mut conn, None, Some(&igor.0.to_string())).await,
        [igor]
    );
    let alexey_client = conn.clients().get(alexey).await.unwrap();
    assert_eq!(
        found(&mut conn, None, Some(&alexey_client.public_id.to_string())).await,
        [alexey]
    );
    let short = format!("https://sub.example.com/short{}", alexey_sub.0);
    assert_eq!(found(&mut conn, None, Some(&short)).await, [alexey]);
    assert_eq!(
        found(&mut conn, None, Some(&format!("short{}", alexey_sub.0))).await,
        [alexey]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Active), Some("ксения")).await,
        [ksenia]
    );
    assert!(
        found(&mut conn, Some(ClientSegment::Expired), Some("ксения"))
            .await
            .is_empty()
    );

    let rows = conn
        .clients()
        .search_at(
            &ClientFilter {
                conditions: ClientConditions {
                    status: Some(ClientSegment::Active),
                    ..ClientConditions::default()
                },
                query: None,
            },
            PageRequest::default(),
            now,
            Currency::Usd,
        )
        .await
        .unwrap()
        .items;
    let first = rows.iter().find(|r| r.client.id == alexey).unwrap();
    assert_eq!(first.name.as_deref(), Some("Алексей Морозов"));
    assert_eq!(first.telegram_id.as_deref(), Some("5093118422"));
    assert_eq!(first.telegram_username.as_deref(), Some("morozov_a"));
    assert_eq!(first.email.as_deref(), Some("a.morozov@example.com"));
    assert_eq!(first.balance_minor, 420);
    assert_eq!(first.paid_total_minor, 1_339);
    assert_eq!(first.subscription.as_ref().map(|s| s.id), Some(alexey_sub));
    let two = rows.iter().find(|r| r.client.id == alena).unwrap();
    assert_eq!(two.subscription.as_ref().map(|s| s.id), Some(alena_sub));

    let mut seen = HashSet::new();
    let mut after = None;
    let mut pages = 0;
    loop {
        let page = conn
            .clients()
            .search_at(
                &ClientFilter::default(),
                PageRequest::new(Some(3), after),
                now,
                Currency::Usd,
            )
            .await
            .unwrap();
        pages += 1;
        for item in &page.items {
            assert!(seen.insert(item.client.id));
        }
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!((seen.len(), pages), (10, 4));
    assert!(!seen.contains(&gone));

    let by_default = conn
        .clients()
        .search(&ClientFilter::default(), PageRequest::default())
        .await
        .unwrap();
    assert_eq!(by_default.items.len(), 10);

    let sub_public = conn.subscriptions().get(alena_sub).await.unwrap().public_id;
    assert_eq!(
        found(&mut conn, None, Some(&sub_public.to_string())).await,
        [alena]
    );
    let twin = client(
        &mut conn,
        &Person {
            telegram: Some(igor.0),
            ..Person::NOBODY
        },
    )
    .await;
    assert_eq!(
        found(&mut conn, None, Some(&igor.0.to_string())).await,
        [twin, igor]
    );
    assert!(found(&mut conn, None, Some("1006")).await.is_empty());
    assert!(
        found(&mut conn, Some(ClientSegment::Active), Some("igor_nv"))
            .await
            .is_empty()
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn история_клиента_одной_лентой_без_потерь_на_границе_страниц() {
    let Some(db) = test_store().await else { return };
    let mut tx = db.store.begin().await.unwrap();
    let alexey = client(&mut tx, &Person::NOBODY).await;
    let other = client(&mut tx, &Person::NOBODY).await;
    let sub = subscription(&mut tx, alexey, active(SubscriptionKind::Paid, 90, true)).await;
    let payment_id = paid(&mut tx, alexey, "cryptopay", usd(699)).await;
    tx.events()
        .push(&DomainEvent::ClientRegistered {
            client_id: alexey,
            source: RegistrationSource::Bot,
            start_param: None,
        })
        .await
        .unwrap();
    tx.audit()
        .record(&AuditEntry {
            actor_kind: ActorKind::Admin,
            actor_id: None,
            action: "subscription.add_days".into(),
            entity_kind: Some(audit_entity::SUBSCRIPTION.into()),
            entity_id: Some(sub.0),
            details: serde_json::json!({"days": 7}),
            ip: None,
        })
        .await
        .unwrap();
    tx.audit()
        .record(&AuditEntry {
            actor_kind: ActorKind::Client,
            actor_id: Some(alexey.0),
            action: "cabinet.login".into(),
            entity_kind: None,
            entity_id: None,
            details: serde_json::json!({}),
            ip: None,
        })
        .await
        .unwrap();
    tx.audit()
        .record(&AuditEntry {
            actor_kind: ActorKind::Admin,
            actor_id: None,
            action: "client.note".into(),
            entity_kind: Some(audit_entity::CLIENT.into()),
            entity_id: Some(other.0),
            details: serde_json::json!({}),
            ip: None,
        })
        .await
        .unwrap();
    paid(&mut tx, other, "cryptopay", usd(100)).await;
    tx.commit().await.unwrap();

    let mut conn = db.store.conn().await.unwrap();
    let later = paid(&mut conn, alexey, "cryptopay", usd(640)).await;
    assert_eq!(conn.clients().history_count(alexey, true).await.unwrap(), 5);

    let first = conn
        .clients()
        .history(alexey, PageRequest::new(Some(1), None), true)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(matches!(&first.items[0].entry, ClientHistoryEntry::Payment(p) if p.id == later));
    assert!(first.next.is_some());

    let second = conn
        .clients()
        .history(alexey, PageRequest::new(Some(2), first.next), true)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 4);
    assert!(
        second
            .items
            .iter()
            .all(|item| item.at == second.items[0].at)
    );
    assert_eq!(second.next, None);
    let kinds: Vec<&str> = second
        .items
        .iter()
        .map(|item| match &item.entry {
            ClientHistoryEntry::Audit(_) => "audit",
            ClientHistoryEntry::Event(_) => "event",
            ClientHistoryEntry::Payment(_) => "payment",
            ClientHistoryEntry::Order(_) => "order",
        })
        .collect();
    assert_eq!(kinds, ["audit", "audit", "event", "payment"]);
    assert!(
        second.items.iter().any(
            |item| matches!(&item.entry, ClientHistoryEntry::Payment(p) if p.id == payment_id)
        )
    );
    let json = serde_json::to_value(&second.items[0]).unwrap();
    assert_eq!(json["entry"]["type"], "audit");

    let empty = conn
        .clients()
        .history(
            alexey,
            PageRequest::new(Some(50), Some(second.items[0].at.timestamp_micros())),
            true,
        )
        .await
        .unwrap();
    assert!(empty.items.is_empty());
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn аудит_в_истории_клиента_по_разу_и_без_чужих_записей() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let alexey = client(&mut conn, &Person::NOBODY).await;
    let other = client(&mut conn, &Person::NOBODY).await;
    let payment = paid(&mut conn, alexey, "cryptopay", usd(500)).await;
    let entry = |actor_kind, actor_id: Option<i64>, action: &str, kind: &str, id: i64| AuditEntry {
        actor_kind,
        actor_id,
        action: action.into(),
        entity_kind: Some(kind.into()),
        entity_id: Some(id),
        details: serde_json::json!({}),
        ip: None,
    };
    for record in [
        entry(
            ActorKind::Client,
            Some(alexey.0),
            "client.totp_enabled",
            audit_entity::CLIENT,
            alexey.0,
        ),
        entry(
            ActorKind::System,
            None,
            "payment.amount_mismatch",
            audit_entity::PAYMENT,
            payment.0,
        ),
        entry(
            ActorKind::Client,
            Some(other.0),
            "client.totp_enabled",
            audit_entity::CLIENT,
            other.0,
        ),
        entry(
            ActorKind::Admin,
            Some(alexey.0),
            "client.note",
            audit_entity::CLIENT,
            other.0,
        ),
        entry(
            ActorKind::Client,
            Some(alexey.0),
            "client.oauth_linked",
            audit_entity::ADMIN,
            other.0,
        ),
    ] {
        conn.audit().record(&record).await.unwrap();
    }
    let mut seen = Vec::new();
    for money in [true, false] {
        let history = conn
            .clients()
            .history(alexey, PageRequest::new(Some(50), None), money)
            .await
            .unwrap();
        let mut audit: Vec<String> = history
            .items
            .iter()
            .filter_map(|item| match &item.entry {
                ClientHistoryEntry::Audit(record) => Some(record.action.clone()),
                _ => None,
            })
            .collect();
        audit.sort();
        assert_eq!(
            conn.clients().history_count(alexey, money).await.unwrap(),
            i64::try_from(history.items.len()).unwrap()
        );
        seen.push(audit);
    }
    assert_eq!(
        seen[0],
        [
            "client.oauth_linked",
            "client.totp_enabled",
            "payment.amount_mismatch"
        ]
    );
    assert_eq!(seen[1], ["client.oauth_linked", "client.totp_enabled"]);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn история_без_повторов_и_сводка_для_карточки() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let inviter = client(
        &mut conn,
        &Person {
            telegram: Some(700_100),
            username: Some("ivan_p"),
            first_name: Some("Иван"),
            last_name: Some("Петров"),
            ..Person::NOBODY
        },
    )
    .await;
    let alexey = client(&mut conn, &Person::NOBODY).await;
    let sub = subscription(&mut conn, alexey, active(SubscriptionKind::Paid, 90, true)).await;
    sqlx::query("INSERT INTO referral_links (client_id, referrer_id) VALUES ($1, $2)")
        .bind(alexey.0)
        .bind(inviter.0)
        .execute(conn.conn())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO client_sessions (client_id, token_hash, channel, expires_at, last_seen_at) \
         VALUES ($1, '\\x01'::bytea, 'cabinet', now() + interval '1 day', now() - interval '1 hour'), \
         ($1, '\\x02'::bytea, 'cabinet', now() + interval '1 day', now() - interval '3 hours')",
    )
    .bind(alexey.0)
    .execute(conn.conn())
    .await
    .unwrap();
    let card_paid = paid(&mut conn, alexey, "cryptopay", usd(699)).await;
    paid(&mut conn, alexey, "balance", usd(299)).await;
    payment(&mut conn, alexey, "cryptopay", usd(100)).await;
    order(&mut conn, alexey, usd(500)).await;
    for event in [
        DomainEvent::ClientRegistered {
            client_id: alexey,
            source: RegistrationSource::Bot,
            start_param: Some("ref_ivan".into()),
        },
        DomainEvent::SubscriptionStatusChanged {
            subscription_id: sub,
            client_id: alexey,
            from: Some(SubscriptionStatus::Active),
            to: SubscriptionStatus::Expired,
        },
        DomainEvent::SubscriptionStatusChanged {
            subscription_id: sub,
            client_id: alexey,
            from: Some(SubscriptionStatus::Expired),
            to: SubscriptionStatus::Active,
        },
        DomainEvent::ClientUnblocked { client_id: alexey },
    ] {
        conn.events().push(&event).await.unwrap();
    }

    let page = conn
        .clients()
        .history(alexey, PageRequest::new(Some(50), None), true)
        .await
        .unwrap();
    let mut seen: Vec<String> = page
        .items
        .iter()
        .map(|item| match &item.entry {
            ClientHistoryEntry::Payment(p) => format!("payment:{}", p.provider),
            ClientHistoryEntry::Order(o) => format!("order:{}", o.id.0),
            ClientHistoryEntry::Event(e) => match &e.event {
                DomainEvent::SubscriptionStatusChanged { to, .. } => format!("status:{to:?}"),
                other => other.kind().to_string(),
            },
            ClientHistoryEntry::Audit(a) => a.action.clone(),
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            "client_registered",
            "payment:balance",
            "payment:cryptopay",
            "status:Expired"
        ]
    );
    assert_eq!(conn.clients().history_count(alexey, true).await.unwrap(), 4);
    assert_eq!(
        conn.clients().history_count(alexey, false).await.unwrap(),
        2
    );
    assert!(
        page.items
            .iter()
            .any(|item| matches!(&item.entry, ClientHistoryEntry::Payment(p) if p.id == card_paid))
    );

    let stats = conn
        .clients()
        .card_stats(alexey, Currency::Usd)
        .await
        .unwrap();
    assert_eq!((stats.paid_total_minor, stats.payments_count), (699, 1));
    assert_eq!(stats.referrals_count, 0);
    let referrer = stats.referrer.expect("пригласивший");
    assert_eq!(referrer.client_id, inviter);
    assert_eq!(referrer.name.as_deref(), Some("Иван Петров"));
    assert_eq!(referrer.telegram_username.as_deref(), Some("ivan_p"));
    let seen_at = stats.cabinet_seen_at.expect("вход в кабинет");
    assert!(Utc::now() - seen_at < Duration::minutes(70));
    assert_eq!(stats.miniapp_seen_at, None);

    let inviter_stats = conn
        .clients()
        .card_stats(inviter, Currency::Usd)
        .await
        .unwrap();
    assert_eq!(inviter_stats.referrals_count, 1);
    assert_eq!(inviter_stats.referrer, None);
    assert_eq!(
        (inviter_stats.paid_total_minor, inviter_stats.payments_count),
        (0, 0)
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn второй_фактор_клиента_включается_отсекает_повтор_и_сбрасывается() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let id = client(&mut conn, &Person::NOBODY).await;
    assert!(conn.clients().totp(id).await.unwrap().is_none());
    assert!(!conn.clients().use_totp_step(id, 10).await.unwrap());
    assert!(!conn.clients().clear_totp(id).await.unwrap());
    let sealed = SealedSecret {
        nonce: vec![1; 12],
        ciphertext: vec![2; 40],
    };
    let at = Utc::now();
    conn.clients().enable_totp(id, &sealed, at).await.unwrap();
    let totp = conn.clients().totp(id).await.unwrap().unwrap();
    assert_eq!(totp.secret, sealed);
    assert_eq!(
        totp.enabled_at.map(|t| t.timestamp_micros()),
        Some(at.timestamp_micros())
    );
    assert!(conn.clients().use_totp_step(id, 100).await.unwrap());
    assert!(!conn.clients().use_totp_step(id, 100).await.unwrap());
    assert!(!conn.clients().use_totp_step(id, 99).await.unwrap());
    assert!(conn.clients().use_totp_step(id, 101).await.unwrap());
    let other = SealedSecret {
        nonce: vec![3; 12],
        ciphertext: vec![4; 40],
    };
    conn.clients().enable_totp(id, &other, at).await.unwrap();
    assert_eq!(
        conn.clients().totp(id).await.unwrap().unwrap().secret,
        other
    );
    assert!(conn.clients().use_totp_step(id, 50).await.unwrap());
    for (channel, token) in [
        (SessionChannel::Cabinet, b"cabinet-token".to_vec()),
        (SessionChannel::Miniapp, b"miniapp-token".to_vec()),
    ] {
        conn.sessions()
            .create_client(
                id,
                channel,
                SessionScope::Full,
                &NewSession {
                    token_hash: token,
                    expires_at: Utc::now() + Duration::days(1),
                    ip: None,
                    user_agent: None,
                },
            )
            .await
            .unwrap();
    }
    assert!(conn.clients().clear_totp(id).await.unwrap());
    assert!(conn.clients().totp(id).await.unwrap().is_none());
    assert!(!conn.clients().clear_totp(id).await.unwrap());
    assert_eq!(
        conn.sessions()
            .revoke_client_channel(id, SessionChannel::Cabinet)
            .await
            .unwrap(),
        1
    );
    assert!(
        conn.sessions()
            .client_by_token(b"cabinet-token")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        conn.sessions()
            .client_by_token(b"miniapp-token")
            .await
            .unwrap()
            .is_some()
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn клиенты_идут_от_новых_аккаунтов_к_старым_и_листаются_без_повторов() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let mut made = Vec::new();
    for days in [400_i64, 5, 90, 5, 0, 30, 5] {
        let id = client(&mut conn, &Person::NOBODY).await;
        sqlx::query(
            "UPDATE clients SET created_at = date_trunc('day', now()) - make_interval(days => $2) \
             WHERE id = $1",
        )
        .bind(id.0)
        .bind(i32::try_from(days).unwrap())
        .execute(conn.conn())
        .await
        .unwrap();
        made.push((days, id));
    }
    let mut wanted = made.clone();
    wanted.sort_by_key(|(days, id)| (*days, std::cmp::Reverse(id.0)));
    let wanted: Vec<ClientId> = wanted.into_iter().map(|(_, id)| id).collect();
    assert_eq!(found(&mut conn, None, None).await, wanted);

    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = conn
            .clients()
            .search(&ClientFilter::default(), PageRequest::new(Some(2), after))
            .await
            .unwrap();
        seen.extend(page.items.iter().map(|item| item.client.id));
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, wanted);

    let middle = wanted[2];
    sqlx::query("UPDATE clients SET deleted_at = now() WHERE id = $1")
        .bind(middle.0)
        .execute(conn.conn())
        .await
        .unwrap();
    let rest = conn
        .clients()
        .search(
            &ClientFilter::default(),
            PageRequest::new(Some(50), Some(middle.0)),
        )
        .await
        .unwrap();
    let rest: Vec<ClientId> = rest.items.iter().map(|item| item.client.id).collect();
    assert_eq!(rest, wanted[3..]);
    drop(conn);
    db.cleanup().await;
}
