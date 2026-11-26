use super::auto::{self, Delay, Localized, RuleDraft, Trigger, Unit};
use super::tests::{
    FakeMail, FakeSender, admin, code, context_with, draft, fast, fast_mail, held_context, json_of,
    mail_client, owner, png, spawn_engine, spawn_mail, tg_client, wait_status,
};
use super::*;
use crate::registry::ModuleRegistry;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sn_domain::access::AdminRole;
use sn_domain::client::NewIdentity;
use sn_store::Store;
use sn_store::testing::test_store;
use std::time::Instant;
use tower::ServiceExt;

async fn now_of(store: &Store) -> DateTime<Utc> {
    let mut conn = store.conn().await.unwrap();
    crate::catalog::db_now(&mut conn).await.unwrap()
}

async fn sub(store: &Store, client: ClientId, kind: &str, expire_at: DateTime<Utc>) -> i64 {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_scalar(
        "INSERT INTO subscriptions (client_id, kind, expire_at, first_connected_at) \
         VALUES ($1, $2, $3, CASE WHEN $2 = 'trial' THEN NULL ELSE now() END) RETURNING id",
    )
    .bind(client.0)
    .bind(kind)
    .bind(expire_at)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn run(store: &Store, sql: &str, id: i64) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(sql)
        .bind(id)
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn run_at(store: &Store, sql: &str, id: i64, at: DateTime<Utc>) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(sql)
        .bind(id)
        .bind(at)
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn paid_order(store: &Store, client: ClientId, subscription: Option<i64>, at: DateTime<Utc>) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO orders (client_id, subscription_id, kind, status, intent, quote, currency, total_minor, \
         paid_at, fulfilled_at) VALUES ($1, $2, 'new', 'fulfilled', '{}', '{}', 'USD', 500, $3, $3)",
    )
    .bind(client.0)
    .bind(subscription)
    .bind(at)
    .execute(conn.conn())
    .await
    .unwrap();
}

fn rule_draft(trigger: Trigger, days: u16, channel: Channel) -> RuleDraft {
    RuleDraft {
        trigger,
        amount: days,
        unit: Unit::Days,
        percent: None,
        with_autorenew: false,
        channel,
        subject: (channel != Channel::Bot).then(|| "О подписке".to_string()),
        text: format!("<b>Правило</b> {}", trigger.as_str()),
        image_id: None,
        buttons: vec![ButtonSpec {
            text: "Продлить".into(),
            url: None,
            section: Some(BotSection::Buy),
            style: None,
            emoji_id: None,
        }],
        en: None,
        enabled: true,
        audience: None,
    }
}

async fn case_clients(store: &Store, rule: i64) -> Vec<(i64, bool)> {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_as(
        "SELECT client_id, queued FROM auto_broadcast_cases WHERE rule_id = $1 ORDER BY client_id, case_key",
    )
    .bind(rule)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn fired(store: &Store, rule: i64) -> Vec<i64> {
    case_clients(store, rule)
        .await
        .into_iter()
        .map(|(client, _)| client)
        .collect()
}

async fn queue(store: &Store, rule: i64) -> Vec<(String, i64, String)> {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_as(
        "SELECT r.channel, r.client_id, r.state FROM broadcast_recipients r \
         JOIN broadcasts b ON b.id = r.broadcast_id WHERE b.rule_id = $1 ORDER BY r.channel, r.client_id",
    )
    .bind(rule)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn batches(store: &Store, rule: i64) -> Vec<(i64, String, i32)> {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_as("SELECT id, text, rule_version FROM broadcasts WHERE rule_id = $1 ORDER BY id")
        .bind(rule)
        .fetch_all(conn.conn())
        .await
        .unwrap()
}

async fn settle(store: &Store, rule: i64) {
    let started = Instant::now();
    loop {
        let mut conn = store.conn().await.unwrap();
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM broadcast_recipients r JOIN broadcasts b ON b.id = r.broadcast_id \
             WHERE b.rule_id = $1 AND (r.state IN ('pending', 'sending') OR b.status = 'sending')",
        )
        .bind(rule)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        drop(conn);
        if left == 0 {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "очередь правила {rule} не разошлась"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn ids(clients: &[ClientId]) -> Vec<i64> {
    let mut out: Vec<i64> = clients.iter().map(|c| c.0).collect();
    out.sort_unstable();
    out
}

#[test]
fn сроки_событий_пишутся_по_русски_и_проверяются() {
    let days = |n| Delay::days(n);
    assert_eq!(
        Trigger::Expiring.timing(days(1), None),
        "за 1 день до окончания"
    );
    assert_eq!(
        Trigger::Expiring.timing(days(3), None),
        "за 3 дня до окончания"
    );
    assert_eq!(
        Trigger::Expiring.timing(days(5), None),
        "за 5 дней до окончания"
    );
    assert_eq!(
        Trigger::Expired.timing(days(0), None),
        "сразу после окончания"
    );
    assert_eq!(
        Trigger::TrialEnded.timing(days(21), None),
        "через 21 день после окончания"
    );
    assert_eq!(
        Trigger::NoPurchase.timing(days(2), None),
        "через 2 дня после регистрации"
    );
    assert_eq!(
        Trigger::NotConnected.timing(days(11), None),
        "через 11 дней после покупки"
    );
    assert_eq!(
        Trigger::TrialExpiring.timing(Delay::hours(6), None),
        "за 6 часов до окончания"
    );
    assert_eq!(
        Trigger::NoSubscription.timing(Delay::hours(1), None),
        "через 1 час после регистрации"
    );
    assert_eq!(
        Trigger::NoSubscription.timing(Delay::hours(22), None),
        "через 22 часа после регистрации"
    );
    assert_eq!(
        Trigger::TrafficLow.timing(days(0), Some(85)),
        "когда израсходовано 85% трафика"
    );
    for trigger in Trigger::ALL {
        assert_eq!(Trigger::parse(trigger.as_str()), Some(trigger));
        assert_eq!(
            serde_json::to_value(trigger).unwrap(),
            serde_json::json!(trigger.as_str())
        );
    }
    assert_eq!(Trigger::Expiring.min_amount(), 1);
    assert_eq!(Trigger::Expired.min_amount(), 0);
    assert!(Trigger::TrafficLow.by_percent());
    assert_eq!(Delay::hours(6).span(), Span::hours(6));
    assert_eq!(Delay::days(2).span(), Span::days(2));
}

#[tokio::test]
async fn скорое_окончание_срабатывает_один_раз_в_свой_срок_и_снова_после_продления() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context_with(store, Some(sender.clone()), None);
    let now = now_of(store).await;
    let due = tg_client(store, 9301).await;
    let trial = tg_client(store, 9302).await;
    let later = tg_client(store, 9303).await;
    let stale = tg_client(store, 9304).await;
    let renewing = tg_client(store, 9305).await;
    let gone = tg_client(store, 9306).await;
    let due_sub = sub(store, due, "paid", now + Span::days(3) - Span::hours(1)).await;
    sub(store, trial, "trial", now + Span::days(2) + Span::hours(1)).await;
    sub(store, later, "paid", now + Span::days(3) + Span::hours(2)).await;
    sub(store, stale, "paid", now + Span::days(1)).await;
    let renewing_sub = sub(
        store,
        renewing,
        "paid",
        now + Span::days(2) + Span::hours(12),
    )
    .await;
    run(
        store,
        "UPDATE subscriptions SET autorenew_enabled = true WHERE id = $1",
        renewing_sub,
    )
    .await;
    sub(store, gone, "paid", now - Span::hours(1)).await;

    let rule = create_rule(&ctx, &rule_draft(Trigger::Expiring, 3, Channel::Bot), None)
        .await
        .unwrap();
    assert_eq!(rule.timing, "за 3 дня до окончания");
    let probe = rule_reach(
        &ctx,
        &ReachQuery {
            trigger: Trigger::Expiring,
            amount: 3,
            unit: Unit::Days,
            percent: None,
            with_autorenew: false,
            id: Some(rule.id),
            audience: None,
        },
    )
    .await
    .unwrap();
    assert_eq!((probe.cases, probe.recipients), (1, 1));

    let first = auto::check(&ctx, now).await.unwrap();
    assert_eq!((first.cases, first.queued), (1, 1));
    assert_eq!(fired(store, rule.id).await, ids(&[due]));
    let again = auto::check(&ctx, now).await.unwrap();
    assert_eq!(again.cases, 0);

    let restarted = context_with(store, Some(sender.clone()), None);
    let after = auto::check(&restarted, now + Span::hours(3)).await.unwrap();
    assert_eq!(after.cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[due, later]));

    let engine = spawn_engine(&ctx, &sender, fast());
    settle(store, rule.id).await;
    engine.stop().await;
    assert_eq!(sender.chats(), vec![9301, 9303]);
    assert_eq!(
        auto::check(&restarted, now + Span::hours(3))
            .await
            .unwrap()
            .cases,
        0
    );

    let view = auto::rule(&ctx, rule.id).await.unwrap();
    assert_eq!(view.stats.cases, 2);
    assert_eq!(view.stats.progress.sent, 2);
    assert!(!fired(store, rule.id).await.contains(&trial.0));
    assert!(view.stats.last_sent_at.is_some());

    run_at(
        store,
        "UPDATE subscriptions SET expire_at = $2 WHERE id = $1",
        due_sub,
        now + Span::days(33),
    )
    .await;
    let renewed = auto::check(&ctx, now + Span::days(30) + Span::hours(1))
        .await
        .unwrap();
    assert_eq!(renewed.cases, 1);
    let keys: Vec<(i64, bool)> = case_clients(store, rule.id).await;
    assert_eq!(keys.iter().filter(|(c, _)| *c == due.0).count(), 2);

    let mut with_renewal = rule_draft(Trigger::Expiring, 3, Channel::Bot);
    with_renewal.with_autorenew = true;
    update_rule(&ctx, rule.id, &with_renewal, None)
        .await
        .unwrap();
    let chosen = auto::check(&ctx, now).await.unwrap();
    assert_eq!(chosen.cases, 1);
    assert!(fired(store, rule.id).await.contains(&renewing.0));
    assert!(!fired(store, rule.id).await.contains(&stale.0));
    assert!(!fired(store, rule.id).await.contains(&gone.0));
    db.cleanup().await;
}

#[tokio::test]
async fn закончилась_и_не_продлена_через_заданные_дни() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let lapsed = tg_client(store, 9401).await;
    let soon = tg_client(store, 9402).await;
    let moved = tg_client(store, 9403).await;
    let trial = tg_client(store, 9404).await;
    let old = tg_client(store, 9405).await;
    let disabled = tg_client(store, 9406).await;
    sub(store, lapsed, "paid", now - Span::days(2) - Span::hours(1)).await;
    sub(store, soon, "paid", now - Span::days(1)).await;
    sub(store, moved, "paid", now - Span::days(2) - Span::hours(1)).await;
    sub(store, moved, "paid", now + Span::days(20)).await;
    sub(store, trial, "trial", now - Span::days(2) - Span::hours(1)).await;
    sub(store, old, "paid", now - Span::days(5)).await;
    let off = sub(
        store,
        disabled,
        "paid",
        now - Span::days(2) - Span::hours(1),
    )
    .await;
    run(
        store,
        "UPDATE subscriptions SET status = 'disabled' WHERE id = $1",
        off,
    )
    .await;
    let rule = create_rule(&ctx, &rule_draft(Trigger::Expired, 2, Channel::Bot), None)
        .await
        .unwrap();
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[lapsed]));
    assert_eq!(
        auto::check(&ctx, now + Span::hours(23))
            .await
            .unwrap()
            .cases,
        0
    );
    assert_eq!(
        auto::check(&ctx, now + Span::days(1) + Span::hours(1))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[lapsed, soon]));
    db.cleanup().await;
}

#[tokio::test]
async fn пробный_период_закончился_без_покупки() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let idle = tg_client(store, 9501).await;
    let buyer = tg_client(store, 9502).await;
    let upgraded = tg_client(store, 9503).await;
    let trying = tg_client(store, 9504).await;
    sub(store, idle, "trial", now - Span::hours(1)).await;
    let bought = sub(store, buyer, "trial", now - Span::hours(1)).await;
    paid_order(store, buyer, Some(bought), now - Span::hours(2)).await;
    sub(store, upgraded, "trial", now - Span::hours(1)).await;
    sub(store, upgraded, "paid", now + Span::days(29)).await;
    sub(store, trying, "trial", now + Span::days(1)).await;
    let rule = create_rule(
        &ctx,
        &rule_draft(Trigger::TrialEnded, 0, Channel::Bot),
        None,
    )
    .await
    .unwrap();
    assert_eq!(rule.timing, "сразу после окончания");
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[idle]));
    assert_eq!(
        auto::check(&ctx, now + Span::days(1) + Span::hours(1))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[idle, trying]));
    db.cleanup().await;
}

#[tokio::test]
async fn нет_покупок_после_регистрации() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let quiet = tg_client(store, 9601).await;
    let buyer = tg_client(store, 9602).await;
    let fresh = tg_client(store, 9603).await;
    let moved = tg_client(store, 9604).await;
    let trial = tg_client(store, 9605).await;
    for (client, at) in [
        (quiet, now - Span::days(3) - Span::hours(1)),
        (buyer, now - Span::days(3) - Span::hours(1)),
        (fresh, now - Span::days(2)),
        (moved, now - Span::days(3) - Span::hours(1)),
        (trial, now - Span::days(3) - Span::hours(1)),
    ] {
        run_at(
            store,
            "UPDATE clients SET created_at = $2 WHERE id = $1",
            client.0,
            at,
        )
        .await;
    }
    paid_order(store, buyer, None, now - Span::days(1)).await;
    run(
        store,
        "UPDATE clients SET registration_source = 'migration' WHERE id = $1",
        moved.0,
    )
    .await;
    sub(store, trial, "trial", now + Span::days(1)).await;
    let rule = create_rule(
        &ctx,
        &rule_draft(Trigger::NoPurchase, 3, Channel::Bot),
        None,
    )
    .await
    .unwrap();
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 2);
    assert_eq!(fired(store, rule.id).await, ids(&[quiet, trial]));
    assert_eq!(
        auto::check(&ctx, now + Span::days(1) + Span::hours(1))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[quiet, fresh, trial]));
    db.cleanup().await;
}

#[tokio::test]
async fn нет_подключений_после_покупки() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let silent = tg_client(store, 9701).await;
    let online = tg_client(store, 9702).await;
    let recent = tg_client(store, 9703).await;
    let imported = tg_client(store, 9704).await;
    let lapsed = tg_client(store, 9705).await;
    let mut subs = Vec::new();
    for (client, expire) in [
        (silent, now + Span::days(28)),
        (online, now + Span::days(28)),
        (recent, now + Span::days(29)),
        (imported, now + Span::days(28)),
        (lapsed, now - Span::hours(1)),
    ] {
        let id = sub(store, client, "paid", expire).await;
        run(
            store,
            "UPDATE subscriptions SET first_connected_at = NULL WHERE id = $1",
            id,
        )
        .await;
        subs.push(id);
    }
    run(
        store,
        "UPDATE subscriptions SET first_connected_at = now() WHERE id = $1",
        subs[1],
    )
    .await;
    paid_order(
        store,
        silent,
        Some(subs[0]),
        now - Span::days(2) - Span::hours(1),
    )
    .await;
    paid_order(
        store,
        online,
        Some(subs[1]),
        now - Span::days(2) - Span::hours(1),
    )
    .await;
    paid_order(store, recent, Some(subs[2]), now - Span::days(1)).await;
    paid_order(
        store,
        lapsed,
        Some(subs[4]),
        now - Span::days(2) - Span::hours(1),
    )
    .await;
    let rule = create_rule(
        &ctx,
        &rule_draft(Trigger::NotConnected, 2, Channel::Bot),
        None,
    )
    .await
    .unwrap();
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[silent]));
    assert_eq!(
        auto::check(&ctx, now + Span::days(1) + Span::hours(1))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[silent, recent]));
    db.cleanup().await;
}

#[tokio::test]
async fn автоматическая_рассылка_не_пишет_на_почту_привязанную_без_кода() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mail = FakeMail::new();
    let ctx = context_with(store, None, Some(mail));
    let now = now_of(store).await;
    let client = mail_client(store, None, None).await;
    {
        let mut conn = store.conn().await.unwrap();
        conn.clients()
            .add_identity(
                client,
                &NewIdentity::email_without_code("raw@example.com").unwrap(),
            )
            .await
            .unwrap();
    }
    sub(store, client, "paid", now + Span::days(3) - Span::hours(2)).await;
    let query = ReachQuery {
        trigger: Trigger::Expiring,
        amount: 3,
        unit: Unit::Days,
        percent: None,
        with_autorenew: false,
        id: None,
        audience: None,
    };
    let probe = rule_reach(&ctx, &query).await.unwrap();
    assert_eq!((probe.cases, probe.recipients, probe.emails), (1, 0, 0));
    run(
        store,
        "UPDATE client_identities SET verified = true WHERE kind = 'email' AND client_id = $1",
        client.0,
    )
    .await;
    let probe = rule_reach(&ctx, &query).await.unwrap();
    assert_eq!((probe.cases, probe.emails), (1, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn остановившие_бота_и_отписавшиеся_не_получают() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let mail = FakeMail::new();
    let ctx = context_with(store, Some(sender.clone()), Some(mail.clone()));
    let now = now_of(store).await;
    let both = mail_client(store, Some(9801), Some("both@example.com")).await;
    let none = mail_client(store, Some(9802), Some("gone@example.com")).await;
    let mailbox = mail_client(store, Some(9803), Some("mail@example.com")).await;
    let left = mail_client(store, None, Some("left@example.com")).await;
    let chat = mail_client(store, Some(9805), None).await;
    for client in [none, mailbox] {
        run(
            store,
            "UPDATE clients SET bot_unreachable = true WHERE id = $1",
            client.0,
        )
        .await;
    }
    {
        let mut conn = store.conn().await.unwrap();
        sqlx::query(
            "INSERT INTO broadcast_unsubscribes (email) VALUES ('gone@example.com'), ('left@example.com')",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    for client in [both, none, mailbox, left, chat] {
        sub(store, client, "paid", now + Span::days(3) - Span::hours(2)).await;
    }
    let rule = create_rule(&ctx, &rule_draft(Trigger::Expiring, 3, Channel::Both), None)
        .await
        .unwrap();
    let probe = rule_reach(
        &ctx,
        &ReachQuery {
            trigger: Trigger::Expiring,
            amount: 3,
            unit: Unit::Days,
            percent: None,
            with_autorenew: false,
            id: None,
            audience: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (
            probe.cases,
            probe.recipients,
            probe.stopped,
            probe.emails,
            probe.unsubscribed
        ),
        (5, 2, 2, 2, 2)
    );
    let report = auto::check(&ctx, now).await.unwrap();
    assert_eq!((report.cases, report.queued), (5, 3));
    assert_eq!(
        case_clients(store, rule.id).await,
        vec![
            (both.0, true),
            (none.0, false),
            (mailbox.0, true),
            (left.0, false),
            (chat.0, true)
        ]
    );
    let bot = spawn_engine(&ctx, &sender, fast());
    let letters = spawn_mail(&ctx, &mail, fast_mail());
    settle(store, rule.id).await;
    bot.stop().await;
    letters.stop().await;
    assert_eq!(sender.chats(), vec![9801, 9805]);
    let mut to: Vec<String> = mail.mailed().into_iter().map(|m| m.to).collect();
    to.sort();
    assert_eq!(to, vec!["both@example.com", "mail@example.com"]);
    assert!(mail.mailed().iter().all(|m| m.unsubscribe.is_some()));
    let view = auto::rule(&ctx, rule.id).await.unwrap();
    assert_eq!(
        (
            view.stats.cases,
            view.stats.unreachable,
            view.stats.progress.sent
        ),
        (5, 2, 4)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn выключение_останавливает_очередь_а_сломанная_почта_не_мешает_боту() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let mail = FakeMail::new();
    let ctx = context_with(store, Some(sender.clone()), Some(mail.clone()));
    let now = now_of(store).await;
    let first = mail_client(store, Some(9901), Some("one@example.com")).await;
    sub(store, first, "paid", now + Span::days(3) - Span::hours(2)).await;
    let rule = create_rule(&ctx, &rule_draft(Trigger::Expiring, 3, Channel::Both), None)
        .await
        .unwrap();
    auto::check(&ctx, now).await.unwrap();
    let off = switch_rule(&ctx, rule.id, false, None).await.unwrap();
    assert!(!off.enabled);
    assert_eq!(
        queue(store, rule.id).await,
        vec![
            ("bot".to_string(), first.0, "skipped".to_string()),
            ("email".to_string(), first.0, "skipped".to_string()),
        ]
    );
    let second = mail_client(store, Some(9902), Some("two@example.com")).await;
    sub(store, second, "paid", now + Span::days(3) - Span::hours(1)).await;
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 0);
    switch_rule(&ctx, rule.id, true, None).await.unwrap();
    *mail.ready.lock().unwrap() = Err("почта выключена".into());
    let report = auto::check(&ctx, now).await.unwrap();
    assert_eq!((report.cases, report.queued), (1, 1));
    let rows = queue(store, rule.id).await;
    assert!(rows.contains(&("bot".to_string(), second.0, "pending".to_string())));
    assert!(
        !rows
            .iter()
            .any(|(lane, client, _)| lane == "email" && *client == second.0)
    );
    let view = auto::rule(&ctx, rule.id).await.unwrap();
    assert!(
        view.notice
            .as_deref()
            .is_some_and(|n| n.contains("Письма по правилу не уходят")),
        "{:?}",
        view.notice
    );
    let mailed = create_rule(&ctx, &rule_draft(Trigger::Expired, 1, Channel::Email), None)
        .await
        .unwrap_err();
    assert_eq!(code(mailed), "broadcast.mail");
    let mut quiet = rule_draft(Trigger::Expired, 1, Channel::Email);
    quiet.enabled = false;
    let parked = create_rule(&ctx, &quiet, None).await.unwrap();
    assert!(!parked.enabled);
    assert_eq!(
        code(switch_rule(&ctx, parked.id, true, None).await.unwrap_err()),
        "broadcast.mail"
    );
    let mut wrong = rule_draft(Trigger::Expiring, 0, Channel::Bot);
    wrong.enabled = false;
    assert_eq!(
        code(create_rule(&ctx, &wrong, None).await.unwrap_err()),
        "broadcast.invalid"
    );
    wrong.amount = DAYS_MAX + 1;
    assert_eq!(
        code(create_rule(&ctx, &wrong, None).await.unwrap_err()),
        "broadcast.invalid"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn правка_текста_идёт_новой_пачкой_а_картинка_правила_не_удаляется() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let image = upload_image(&ctx, &png(640, 480), None).await.unwrap();
    let mut draft = rule_draft(Trigger::Expiring, 3, Channel::Bot);
    draft.image_id = Some(image.id);
    let rule = create_rule(&ctx, &draft, None).await.unwrap();
    run(
        store,
        "UPDATE broadcast_images SET created_at = now() - interval '3 days' WHERE id = $1",
        image.id,
    )
    .await;
    cleanup(&ctx).await.unwrap();
    assert!(image_file(&ctx, image.id).await.is_ok());
    let a = tg_client(store, 9951).await;
    sub(store, a, "paid", now + Span::days(3) - Span::hours(2)).await;
    auto::check(&ctx, now).await.unwrap();
    draft.text = "Новый текст".into();
    update_rule(&ctx, rule.id, &draft, None).await.unwrap();
    let b = tg_client(store, 9952).await;
    sub(store, b, "paid", now + Span::days(3) - Span::hours(1)).await;
    auto::check(&ctx, now).await.unwrap();
    let made = batches(store, rule.id).await;
    assert_eq!(made.len(), 2);
    assert_eq!(made[0].1, "<b>Правило</b> expiring");
    assert_eq!((made[1].1.as_str(), made[1].2), ("Новый текст", 2));
    draft.amount = 4;
    update_rule(&ctx, rule.id, &draft, None).await.unwrap();
    let c = tg_client(store, 9953).await;
    sub(store, c, "paid", now + Span::days(4) - Span::hours(1)).await;
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(batches(store, rule.id).await.len(), 2);
    delete_rule(&ctx, rule.id, None).await.unwrap();
    let mut conn = store.conn().await.unwrap();
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM broadcasts WHERE rule_id = $1")
        .bind(rule.id)
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(kept, 2);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn админка_ведёт_правила_и_не_путает_их_с_разовыми() {
    let Some(db) = test_store().await else {
        return;
    };
    owner(&db).await;
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context_with(store, Some(sender.clone()), None);
    let now = now_of(store).await;
    let quiet = tg_client(store, 9971).await;
    run_at(
        store,
        "UPDATE clients SET created_at = $2 WHERE id = $1",
        quiet.0,
        now - Span::days(3) - Span::hours(1),
    )
    .await;
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx.clone());
    let call = |method: &str, uri: &str, role: AdminRole, body: Option<serde_json::Value>| {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/broadcast{uri}"))
            .extension(admin(role));
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        request
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap()
    };
    let denied = router
        .clone()
        .oneshot(call("GET", "/auto", AdminRole::Support, None))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let empty = router
        .clone()
        .oneshot(call("GET", "/auto", AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(json_of(empty).await["items"], serde_json::json!([]));
    let bad = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto",
            AdminRole::Admin,
            Some(serde_json::json!({ "trigger": "expiring", "days": 0, "text": "x" })),
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_of(bad).await["error"]["code"], "broadcast.invalid");
    let reach = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto/reach",
            AdminRole::Admin,
            Some(serde_json::json!({ "trigger": "no_purchase", "days": 3 })),
        ))
        .await
        .unwrap();
    assert_eq!(reach.status(), StatusCode::OK);
    assert_eq!(json_of(reach).await["recipients"], 1);
    let created = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto",
            AdminRole::Admin,
            Some(serde_json::json!({
                "trigger": "no_purchase",
                "days": 3,
                "text": "Привет! Попробуйте <b>месяц</b> со скидкой",
                "buttons": [{ "text": "Купить", "section": "buy" }],
                "enabled": true
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json_of(created).await;
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["title"], "Нет покупок после регистрации");
    assert_eq!(created["timing"], "через 3 дня после регистрации");
    assert_eq!(created["enabled"], true);
    let edited = router
        .clone()
        .oneshot(call(
            "PUT",
            &format!("/auto/{id}"),
            AdminRole::Admin,
            Some(serde_json::json!({
                "trigger": "no_purchase",
                "days": 5,
                "text": "Привет!",
                "enabled": true
            })),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);
    assert_eq!(
        json_of(edited).await["timing"],
        "через 5 дней после регистрации"
    );
    let back = router
        .clone()
        .oneshot(call(
            "PUT",
            &format!("/auto/{id}"),
            AdminRole::Admin,
            Some(serde_json::json!({ "trigger": "no_purchase", "days": 3, "text": "Привет!", "enabled": true })),
        ))
        .await
        .unwrap();
    assert_eq!(back.status(), StatusCode::OK);
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    let listed = router
        .clone()
        .oneshot(call("GET", "/auto", AdminRole::Admin, None))
        .await
        .unwrap();
    let listed = json_of(listed).await;
    assert_eq!(listed["items"][0]["id"], id);
    assert_eq!(listed["items"][0]["stats"]["cases"], 1);
    assert_eq!(listed["items"][0]["stats"]["progress"]["pending"], 1);
    let once = router
        .clone()
        .oneshot(call("GET", "/broadcasts", AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(json_of(once).await["items"], serde_json::json!([]));
    let batch = batches(store, id).await[0].0;
    let hidden = router
        .clone()
        .oneshot(call(
            "GET",
            &format!("/broadcasts/{batch}"),
            AdminRole::Admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let stopped = router
        .clone()
        .oneshot(call(
            "POST",
            &format!("/auto/{id}/switch"),
            AdminRole::Admin,
            Some(serde_json::json!({ "enabled": false })),
        ))
        .await
        .unwrap();
    let stopped = json_of(stopped).await;
    assert_eq!(stopped["enabled"], false);
    assert_eq!(stopped["stats"]["progress"]["skipped"], 1);
    let removed = router
        .clone()
        .oneshot(call(
            "DELETE",
            &format!("/auto/{id}"),
            AdminRole::Admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);
    let missing = router
        .clone()
        .oneshot(call("GET", &format!("/auto/{id}"), AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let empty = router
        .clone()
        .oneshot(call("GET", "/auto", AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(json_of(empty).await["items"], serde_json::json!([]));
    db.cleanup().await;
}

async fn traffic(store: &Store, subscription: i64, used_gb: i64, limit_gb: Option<i64>) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "UPDATE subscriptions SET traffic_used_bytes = $2 * 1073741824, \
         traffic_limit_bytes = $3 * 1073741824 WHERE id = $1",
    )
    .bind(subscription)
    .bind(used_gb)
    .bind(limit_gb)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn english_clients(store: &Store, on: bool) {
    let mut conn = store.conn().await.unwrap();
    conn.settings()
        .set(
            sn_domain::settings::keys::LANGUAGES,
            &serde_json::json!({ "english": on, "default": "ru" }),
            None,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn не_взял_ни_пробный_ни_подписку_через_часы() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let quiet = tg_client(store, 9811).await;
    let trial = tg_client(store, 9812).await;
    let buyer = tg_client(store, 9813).await;
    let fresh = tg_client(store, 9814).await;
    let moved = tg_client(store, 9815).await;
    for (client, at) in [
        (quiet, now - Span::hours(2) - Span::minutes(10)),
        (trial, now - Span::hours(2) - Span::minutes(10)),
        (buyer, now - Span::hours(3)),
        (fresh, now - Span::hours(1)),
        (moved, now - Span::hours(3)),
    ] {
        run_at(
            store,
            "UPDATE clients SET created_at = $2 WHERE id = $1",
            client.0,
            at,
        )
        .await;
    }
    sub(store, trial, "trial", now + Span::days(2)).await;
    paid_order(store, buyer, None, now - Span::hours(1)).await;
    run(
        store,
        "UPDATE clients SET registration_source = 'import' WHERE id = $1",
        moved.0,
    )
    .await;
    let mut draft = rule_draft(Trigger::NoSubscription, 2, Channel::Bot);
    draft.unit = Unit::Hours;
    let rule = create_rule(&ctx, &draft, None).await.unwrap();
    assert_eq!(rule.timing, "через 2 часа после регистрации");
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[quiet]));
    let restarted = context_with(store, Some(FakeSender::new()), None);
    assert_eq!(auto::check(&restarted, now).await.unwrap().cases, 0);
    assert_eq!(
        auto::check(&restarted, now + Span::hours(1) + Span::minutes(5))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[quiet, fresh]));
    let mut wide = rule_draft(Trigger::NoPurchase, 1, Channel::Bot);
    wide.unit = Unit::Hours;
    let other = create_rule(&ctx, &wide, None).await.unwrap();
    auto::check(&ctx, now + Span::hours(1) + Span::minutes(5))
        .await
        .unwrap();
    assert!(fired(store, other.id).await.contains(&trial.0));
    db.cleanup().await;
}

#[tokio::test]
async fn пробный_скоро_закончится_а_подписка_не_трогает_пробные() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let soon = tg_client(store, 9821).await;
    let later = tg_client(store, 9822).await;
    let bought = tg_client(store, 9823).await;
    let paid = tg_client(store, 9824).await;
    let long = tg_client(store, 9825).await;
    sub(store, soon, "trial", now + Span::hours(5)).await;
    sub(store, later, "trial", now + Span::hours(7)).await;
    sub(store, bought, "trial", now + Span::hours(5)).await;
    paid_order(store, bought, None, now - Span::hours(1)).await;
    sub(store, paid, "paid", now + Span::days(3) - Span::hours(2)).await;
    sub(store, long, "trial", now + Span::days(3) - Span::hours(1)).await;
    let mut draft = rule_draft(Trigger::TrialExpiring, 6, Channel::Bot);
    draft.unit = Unit::Hours;
    let rule = create_rule(&ctx, &draft, None).await.unwrap();
    let expiring = create_rule(&ctx, &rule_draft(Trigger::Expiring, 3, Channel::Bot), None)
        .await
        .unwrap();
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[soon]));
    assert_eq!(fired(store, expiring.id).await, ids(&[paid]));
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 0);
    auto::check(&ctx, now + Span::hours(2)).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[soon, later]));
    assert_eq!(fired(store, expiring.id).await, ids(&[paid]));
    db.cleanup().await;
}

#[tokio::test]
async fn заканчивается_трафик_один_раз_до_сброса() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context_with(store, Some(sender.clone()), None);
    let now = now_of(store).await;
    let heavy = tg_client(store, 9831).await;
    let light = tg_client(store, 9832).await;
    let unlimited = tg_client(store, 9833).await;
    let lapsed = tg_client(store, 9834).await;
    let heavy_sub = sub(store, heavy, "paid", now + Span::days(20)).await;
    traffic(store, heavy_sub, 85, Some(100)).await;
    let light_sub = sub(store, light, "paid", now + Span::days(20)).await;
    traffic(store, light_sub, 50, Some(100)).await;
    let free_sub = sub(store, unlimited, "paid", now + Span::days(20)).await;
    traffic(store, free_sub, 500, None).await;
    let old_sub = sub(store, lapsed, "paid", now - Span::hours(2)).await;
    traffic(store, old_sub, 99, Some(100)).await;
    let mut draft = rule_draft(Trigger::TrafficLow, 0, Channel::Bot);
    draft.percent = Some(80);
    let rule = create_rule(&ctx, &draft, None).await.unwrap();
    assert_eq!(rule.timing, "когда израсходовано 80% трафика");
    let probe = rule_reach(
        &ctx,
        &ReachQuery {
            trigger: Trigger::TrafficLow,
            amount: 0,
            unit: Unit::Days,
            percent: Some(80),
            with_autorenew: false,
            id: None,
            audience: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(probe.cases, 1);
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[heavy]));
    traffic(store, heavy_sub, 95, Some(100)).await;
    let restarted = context_with(store, Some(sender.clone()), None);
    assert_eq!(auto::check(&restarted, now).await.unwrap().cases, 0);
    let next = now + Span::days(1);
    traffic(store, heavy_sub, 3, Some(100)).await;
    assert_eq!(auto::check(&restarted, next).await.unwrap().cases, 0);
    traffic(store, heavy_sub, 82, Some(100)).await;
    assert_eq!(auto::check(&restarted, next).await.unwrap().cases, 1);
    let mut conn = store.conn().await.unwrap();
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT case_key FROM auto_broadcast_cases WHERE rule_id = $1 ORDER BY case_key",
    )
    .bind(rule.id)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        keys,
        vec![
            format!("traffic:{heavy_sub}:1"),
            format!("traffic:{heavy_sub}:2")
        ]
    );
    traffic(store, heavy_sub, 120, Some(150)).await;
    assert_eq!(auto::check(&restarted, next).await.unwrap().cases, 0);
    traffic(store, light_sub, 90, Some(100)).await;
    assert_eq!(auto::check(&restarted, next).await.unwrap().cases, 1);
    let engine = spawn_engine(&ctx, &sender, fast());
    settle(store, rule.id).await;
    engine.stop().await;
    assert_eq!(sender.chats(), vec![9831, 9831, 9832]);
    let mut wrong = rule_draft(Trigger::TrafficLow, 0, Channel::Bot);
    wrong.percent = Some(5);
    wrong.enabled = false;
    assert_eq!(
        code(create_rule(&ctx, &wrong, None).await.unwrap_err()),
        "broadcast.invalid"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn английский_текст_уходит_клиентам_на_английском() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context_with(store, Some(sender.clone()), None);
    let now = now_of(store).await;
    let russian = tg_client(store, 9841).await;
    let english = tg_client(store, 9842).await;
    let later = tg_client(store, 9843).await;
    for client in [english, later] {
        run(
            store,
            "UPDATE clients SET locale = 'en' WHERE id = $1",
            client.0,
        )
        .await;
    }
    sub(store, russian, "paid", now + Span::days(3) - Span::hours(2)).await;
    sub(store, english, "paid", now + Span::days(3) - Span::hours(1)).await;
    sub(store, later, "paid", now + Span::days(3) + Span::hours(1)).await;
    let mut draft = rule_draft(Trigger::Expiring, 3, Channel::Bot);
    draft.text = "Подписка скоро закончится".into();
    draft.en = Some(Localized {
        subject: None,
        text: "Your subscription ends soon".into(),
        buttons: vec![ButtonSpec {
            text: "Renew".into(),
            url: None,
            section: Some(BotSection::Buy),
            style: None,
            emoji_id: None,
        }],
    });
    let rule = create_rule(&ctx, &draft, None).await.unwrap();
    assert_eq!(
        rule.en.as_ref().map(|en| en.text.as_str()),
        Some("Your subscription ends soon")
    );
    english_clients(store, true).await;
    auto::check(&ctx, now).await.unwrap();
    english_clients(store, false).await;
    auto::check(&ctx, now + Span::hours(2)).await.unwrap();
    let engine = spawn_engine(&ctx, &sender, fast());
    settle(store, rule.id).await;
    engine.stop().await;
    let mut sent: Vec<(i64, String, Vec<String>)> = sender
        .sent()
        .into_iter()
        .map(|s| {
            (
                s.chat,
                s.html,
                s.buttons.into_iter().map(|b| b.text).collect(),
            )
        })
        .collect();
    sent.sort();
    assert_eq!(
        sent,
        vec![
            (
                9841,
                "Подписка скоро закончится".to_string(),
                vec!["Продлить".to_string()]
            ),
            (
                9842,
                "Your subscription ends soon".to_string(),
                vec!["Renew".to_string()]
            ),
            (
                9843,
                "Подписка скоро закончится".to_string(),
                vec!["Продлить".to_string()]
            ),
        ]
    );
    let mut bad = draft.clone();
    bad.en = Some(Localized {
        subject: None,
        text: "<b>broken".into(),
        buttons: Vec::new(),
    });
    let err = create_rule(&ctx, &bad, None).await.unwrap_err();
    assert!(err.to_string().starts_with("Текст на английском"), "{err}");
    db.cleanup().await;
}

async fn settle_contest(store: &Store, contest: i64) {
    let started = Instant::now();
    loop {
        let mut conn = store.conn().await.unwrap();
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM broadcast_recipients r JOIN broadcasts b ON b.id = r.broadcast_id \
             JOIN contest_messages m ON m.id = b.contest_message_id \
             WHERE m.contest_id = $1 AND (r.state IN ('pending', 'sending') OR b.status = 'sending')",
        )
        .bind(contest)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        drop(conn);
        if left == 0 {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "сообщения конкурса {contest} не разошлись"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn сообщение_конкурса_приходит_на_языке_клиента() {
    use crate::catalog::contests::{self, Audience, ConditionsDraft, ContestDraft, MessageDraft};
    use crate::catalog::prize::PrizeDraft;
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context_with(store, Some(sender.clone()), None);
    let now = now_of(store).await;
    tg_client(store, 9851).await;
    let english = tg_client(store, 9852).await;
    run(
        store,
        "UPDATE clients SET locale = 'en' WHERE id = $1",
        english.0,
    )
    .await;
    english_clients(store, true).await;
    let message = |text: &str, text_en: Option<&str>| MessageDraft {
        id: None,
        audience: Audience::Outsiders,
        only_eligible: false,
        send_at: Some(now),
        text: text.into(),
        text_en: text_en.map(str::to_string),
    };
    let draft = ContestDraft {
        title: "Зимний".into(),
        description: None,
        prize: PrizeDraft::Days { days: 7 }.into(),
        winners: 1,
        starts_at: None,
        ends_at: now + Span::hours(2),
        conditions: ConditionsDraft::default(),
        messages: Some(vec![
            message("Конкурс «{title}»", Some("The “{title}” giveaway")),
            message("Только по-русски", None),
        ]),
    };
    let created = contests::create(&ctx, &draft, None).await.unwrap();
    assert_eq!(contests::run(&ctx).await.unwrap().messages, 2);
    let engine = spawn_engine(&ctx, &sender, fast());
    settle_contest(store, created.id).await;
    engine.stop().await;
    let mut sent: Vec<(i64, String, Vec<String>)> = sender
        .sent()
        .into_iter()
        .map(|s| {
            (
                s.chat,
                s.html,
                s.buttons.into_iter().map(|b| b.text).collect(),
            )
        })
        .collect();
    sent.sort();
    let join = |text: &str| vec![text.to_string()];
    assert_eq!(
        sent,
        vec![
            (9851, "Конкурс «Зимний»".to_string(), join("Участвовать")),
            (9851, "Только по-русски".to_string(), join("Участвовать")),
            (9852, "The “Зимний” giveaway".to_string(), join("Join")),
            (9852, "Только по-русски".to_string(), join("Join")),
        ]
    );
    let view = contests::contest(&ctx, created.id).await.unwrap();
    assert!(
        view.messages
            .iter()
            .all(|m| m.progress.is_some_and(|p| p.sent == 2 && p.total == 2))
    );
    db.cleanup().await;
}

async fn preset_rows(store: &Store) -> Vec<(String, String, bool, bool)> {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_as(
        "SELECT preset, trigger, enabled, deleted_at IS NULL FROM auto_broadcasts \
         WHERE preset IS NOT NULL ORDER BY preset",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

#[tokio::test]
async fn стандартные_правила_включены_на_новой_установке() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let seeded = seed_presets(&ctx).await.unwrap();
    assert!(seeded.enabled);
    assert_eq!(seeded.created.len(), 4);
    assert_eq!(
        preset_rows(store).await,
        vec![
            ("expiring".to_string(), "expiring".to_string(), true, true),
            (
                "no_subscription".to_string(),
                "no_subscription".to_string(),
                true,
                true
            ),
            (
                "traffic_low".to_string(),
                "traffic_low".to_string(),
                true,
                true
            ),
            (
                "trial_expiring".to_string(),
                "trial_expiring".to_string(),
                true,
                true
            ),
        ]
    );
    let items = rules(&ctx).await.unwrap();
    let timing: Vec<(Trigger, String, bool)> = items
        .iter()
        .map(|i| (i.trigger, i.timing.clone(), i.english))
        .collect();
    assert!(timing.contains(&(
        Trigger::TrialExpiring,
        "за 6 часов до окончания".to_string(),
        true
    )));
    assert!(timing.contains(&(Trigger::Expiring, "за 3 дня до окончания".to_string(), true)));
    assert!(timing.contains(&(
        Trigger::NoSubscription,
        "через 1 день после регистрации".to_string(),
        true
    )));
    assert!(timing.contains(&(
        Trigger::TrafficLow,
        "когда израсходовано 80% трафика".to_string(),
        true
    )));
    let welcome = items
        .iter()
        .find(|i| i.trigger == Trigger::NoSubscription)
        .unwrap();
    let full = auto::rule(&ctx, welcome.id).await.unwrap();
    assert_eq!(full.buttons.len(), 1);
    assert_eq!(full.buttons[0].section, Some(BotSection::Buy));
    assert_eq!(
        full.en.as_ref().map(|en| en.buttons[0].text.as_str()),
        Some("Choose a plan")
    );
    delete_rule(&ctx, welcome.id, None).await.unwrap();
    assert_eq!(seed_presets(&ctx).await.unwrap(), Seeded::default());
    assert!(preset_rows(store).await.contains(&(
        "no_subscription".to_string(),
        "no_subscription".to_string(),
        false,
        false
    )));
    db.cleanup().await;
}

#[tokio::test]
async fn стандартные_правила_выключены_на_рабочей_установке_и_не_дублируют_свои() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let client = tg_client(store, 9851).await;
    let s = sub(store, client, "trial", now + Span::hours(3)).await;
    traffic(store, s, 1, Some(10)).await;
    let mut own = rule_draft(Trigger::Expiring, 5, Channel::Bot);
    own.enabled = false;
    create_rule(&ctx, &own, None).await.unwrap();
    let mut wide = rule_draft(Trigger::NoPurchase, 2, Channel::Bot);
    wide.enabled = false;
    create_rule(&ctx, &wide, None).await.unwrap();
    let seeded = seed_presets(&ctx).await.unwrap();
    assert!(!seeded.enabled);
    assert_eq!(seeded.created, vec!["trial_expiring", "traffic_low"]);
    assert_eq!(seeded.covered, vec!["no_subscription", "expiring"]);
    assert!(
        preset_rows(store)
            .await
            .iter()
            .all(|(_, _, enabled, live)| !enabled && *live)
    );
    assert_eq!(seed_presets(&ctx).await.unwrap(), Seeded::default());
    let enabled = enable_presets(&ctx, None).await.unwrap();
    assert_eq!(enabled.len(), 2);
    assert!(
        preset_rows(store)
            .await
            .iter()
            .all(|(_, _, enabled, _)| *enabled)
    );
    auto::check(&ctx, now).await.unwrap();
    let mut conn = store.conn().await.unwrap();
    let fired: Vec<String> = sqlx::query_scalar(
        "SELECT a.preset FROM auto_broadcast_cases k JOIN auto_broadcasts a ON a.id = k.rule_id \
         WHERE a.preset IS NOT NULL ORDER BY a.preset",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(fired, vec!["trial_expiring"]);
    db.cleanup().await;
}

#[test]
fn тексты_стандартных_правил_проходят_проверку() {
    for preset in super::presets::PRESETS {
        for shop in [
            super::presets::Shop::default(),
            super::presets::Shop {
                trial: true,
                traffic_packs: true,
            },
        ] {
            let content = super::presets::content(&preset, shop);
            assert!(clean_text(&content.text, false).is_ok(), "{}", preset.key);
            assert!(clean_buttons(&content.buttons).is_ok(), "{}", preset.key);
            let en = content.en.unwrap();
            assert!(clean_text(&en.text, false).is_ok(), "{}", preset.key);
            assert_eq!(en.buttons.len(), content.buttons.len(), "{}", preset.key);
            assert!(
                en.buttons
                    .iter()
                    .zip(&content.buttons)
                    .all(|(e, r)| e.section == r.section),
                "{}",
                preset.key
            );
        }
    }
    let shop = super::presets::Shop {
        trial: true,
        traffic_packs: true,
    };
    let welcome = super::presets::content(&super::presets::PRESETS[0], shop);
    assert_eq!(welcome.buttons[0].section, Some(BotSection::Trial));
    let traffic = super::presets::content(&super::presets::PRESETS[3], shop);
    assert_eq!(traffic.buttons[0].section, Some(BotSection::Options));
}

#[tokio::test]
async fn админка_включает_стандартные_правила_и_принимает_часы_и_проценты() {
    let Some(db) = test_store().await else {
        return;
    };
    owner(&db).await;
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    tg_client(store, 9861).await;
    seed_presets(&ctx).await.unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx.clone());
    let call = |method: &str, uri: &str, body: Option<serde_json::Value>| {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/broadcast{uri}"))
            .extension(admin(AdminRole::Admin));
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        request
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap()
    };
    let options = json_of(
        router
            .clone()
            .oneshot(call("GET", "/options", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(options["auto"]["triggers"][0]["key"], "no_subscription");
    assert_eq!(options["auto"]["triggers"][3]["percent"], true);
    assert_eq!(options["auto"]["hours_max"], 168);
    let listed = json_of(
        router
            .clone()
            .oneshot(call("GET", "/auto", None))
            .await
            .unwrap(),
    )
    .await;
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    assert!(
        items
            .iter()
            .all(|i| i["enabled"] == false && i["preset"].is_string())
    );
    let on = router
        .clone()
        .oneshot(call("POST", "/auto/presets/enable", None))
        .await
        .unwrap();
    assert_eq!(on.status(), StatusCode::OK);
    assert_eq!(json_of(on).await["enabled"].as_array().unwrap().len(), 4);
    let created = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto",
            Some(serde_json::json!({
                "trigger": "trial_expiring",
                "amount": 12,
                "unit": "hours",
                "text": "Скоро конец пробного",
                "en": { "text": "Trial ends soon", "buttons": [] },
                "enabled": false
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json_of(created).await;
    assert_eq!(created["timing"], "за 12 часов до окончания");
    assert_eq!(created["unit"], "hours");
    assert_eq!(created["en"]["text"], "Trial ends soon");
    let too_long = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto",
            Some(serde_json::json!({
                "trigger": "trial_expiring",
                "amount": 200,
                "unit": "hours",
                "text": "x"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(too_long.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let reach = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto/reach",
            Some(serde_json::json!({ "trigger": "traffic_low", "percent": 90 })),
        ))
        .await
        .unwrap();
    assert_eq!(reach.status(), StatusCode::OK);
    assert_eq!(json_of(reach).await["cases"], 0);
    db.cleanup().await;
}

async fn tariff(store: &Store, extra_max: i32) -> i64 {
    let mut conn = store.conn().await.unwrap();
    let category: i64 = sqlx::query_scalar(
        "INSERT INTO tariff_categories (name) VALUES ('Категория') RETURNING id",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO tariffs (category_id, name, extra_devices_enabled, extra_devices_max) \
         VALUES ($1, 'Тариф', $2 > 0, $2) RETURNING id",
    )
    .bind(category)
    .bind(extra_max)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn order(store: &Store, client: ClientId, sql: &str, at: DateTime<Utc>) -> i64 {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_scalar(&format!(
        "INSERT INTO orders (client_id, kind, status, intent, quote, currency, total_minor, created_at, \
         paid_at, channel) VALUES ({sql}) RETURNING id"
    ))
    .bind(client.0)
    .bind(at)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

fn with_audience(mut draft: RuleDraft, audience: serde_json::Value) -> RuleDraft {
    draft.audience = Some(serde_json::from_value(audience).unwrap());
    draft
}

#[test]
fn условия_кому_в_правиле_проверяются() {
    let of = |value: serde_json::Value| {
        serde_json::from_value::<sn_store::ClientConditions>(value).unwrap()
    };
    let bad = |trigger: Trigger, value: serde_json::Value| {
        code(auto::check_audience(trigger, Some(&of(value))).unwrap_err())
    };
    assert_eq!(
        bad(Trigger::Expired, serde_json::json!({"status": "active"})),
        "broadcast.invalid"
    );
    assert_eq!(
        bad(Trigger::Expired, serde_json::json!({"ends": 3})),
        "broadcast.invalid"
    );
    assert_eq!(
        bad(
            Trigger::Expired,
            serde_json::json!({"from": "2026-01-01T00:00:00Z"})
        ),
        "broadcast.invalid"
    );
    assert_eq!(
        bad(Trigger::NoPurchase, serde_json::json!({"tariff": [1]})),
        "broadcast.invalid"
    );
    assert_eq!(
        bad(
            Trigger::NoSubscription,
            serde_json::json!({"category": [1]})
        ),
        "broadcast.invalid"
    );
    assert_eq!(
        bad(Trigger::UnpaidOrder, serde_json::json!({"autorenew": true})),
        "broadcast.invalid"
    );
    assert_eq!(
        bad(Trigger::Expired, serde_json::json!({"utm": "a b"})),
        "broadcast.invalid"
    );
    assert!(
        auto::check_audience(
            Trigger::UnpaidOrder,
            Some(&of(serde_json::json!({"tariff": [1]})))
        )
        .unwrap()
        .is_some()
    );
    assert_eq!(
        auto::check_audience(Trigger::Expired, Some(&of(serde_json::json!({})))).unwrap(),
        None
    );
    assert_eq!(auto::check_audience(Trigger::Expired, None).unwrap(), None);
    assert!(Trigger::DevicesFull.fixed());
    assert_eq!(
        Trigger::Idle.timing(Delay::days(7), None),
        "после 7 дней без подключений"
    );
    assert_eq!(
        Trigger::UnpaidOrder.timing(Delay::hours(1), None),
        "через 1 час после создания счёта"
    );
    assert_eq!(
        Trigger::FirstPurchase.timing(Delay::hours(24), None),
        "через 24 часа после первой покупки"
    );
    assert_eq!(
        Trigger::DevicesFull.timing(Delay::days(0), None),
        "когда заняты все устройства"
    );
    assert_eq!(
        Trigger::TrafficLow.timing(Delay::days(0), Some(100)),
        "когда израсходовано 100% трафика"
    );
}

#[tokio::test]
async fn кому_правила_берёт_тариф_подписки_события_и_свойства_клиента() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let standard = tariff(store, 0).await;
    let other = tariff(store, 0).await;
    let on_standard = tg_client(store, 9801).await;
    let on_other = tg_client(store, 9802).await;
    let from_site = tg_client(store, 9803).await;
    let mixed = tg_client(store, 9804).await;
    for (client, tariff_id) in [
        (on_standard, standard),
        (on_other, other),
        (from_site, standard),
        (mixed, other),
    ] {
        let id = sub(store, client, "paid", now - Span::days(1) - Span::hours(1)).await;
        let mut conn = store.conn().await.unwrap();
        sqlx::query("UPDATE subscriptions SET tariff_id = $2 WHERE id = $1")
            .bind(id)
            .bind(tariff_id)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let live = sub(store, mixed, "paid", now + Span::days(20)).await;
    {
        let mut conn = store.conn().await.unwrap();
        sqlx::query("UPDATE subscriptions SET status = 'disabled', tariff_id = $2 WHERE id = $1")
            .bind(live)
            .bind(standard)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    run(
        store,
        "UPDATE clients SET registration_source = 'web' WHERE id = $1",
        from_site.0,
    )
    .await;
    let by_tariff = create_rule(
        &ctx,
        &with_audience(
            rule_draft(Trigger::Expired, 1, Channel::Bot),
            serde_json::json!({"tariff": [standard]}),
        ),
        None,
    )
    .await
    .unwrap();
    assert!(
        by_tariff
            .audience_label
            .as_deref()
            .unwrap_or_default()
            .starts_with("Тариф")
    );
    let by_source = create_rule(
        &ctx,
        &with_audience(
            rule_draft(Trigger::Expired, 1, Channel::Bot),
            serde_json::json!({"source": ["web"]}),
        ),
        None,
    )
    .await
    .unwrap();
    let everyone = create_rule(&ctx, &rule_draft(Trigger::Expired, 1, Channel::Bot), None)
        .await
        .unwrap();
    assert_eq!(everyone.audience, None);
    let reach = auto::rule_reach(
        &ctx,
        &auto::ReachQuery {
            trigger: Trigger::Expired,
            amount: 1,
            unit: Unit::Days,
            percent: None,
            with_autorenew: false,
            id: None,
            audience: Some(
                serde_json::from_value(serde_json::json!({"tariff": [standard]})).unwrap(),
            ),
        },
    )
    .await
    .unwrap();
    assert_eq!(reach.cases, 2);
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(
        fired(store, by_tariff.id).await,
        ids(&[on_standard, from_site])
    );
    assert_eq!(fired(store, by_source.id).await, ids(&[from_site]));
    assert_eq!(
        fired(store, everyone.id).await,
        ids(&[on_standard, on_other, from_site, mixed])
    );
    db.cleanup().await;
}

#[tokio::test]
async fn неоплаченный_счёт_пишет_по_последней_попытке() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let plan = tariff(store, 0).await;
    let unpaid = format!(
        "$1, 'new', 'pending', '{{\"type\": \"subscription\", \"tariff_id\": {plan}}}', '{{}}', 'USD', 500, $2, NULL, 'bot'"
    );
    let left = tg_client(store, 9811).await;
    let retried = tg_client(store, 9812).await;
    let paid_later = tg_client(store, 9813).await;
    let renewing = tg_client(store, 9814).await;
    let early = tg_client(store, 9815).await;
    order(store, left, &unpaid, now - Span::minutes(90)).await;
    order(store, retried, &unpaid, now - Span::minutes(90)).await;
    order(store, retried, &unpaid, now - Span::minutes(20)).await;
    order(store, paid_later, &unpaid, now - Span::minutes(90)).await;
    order(
        store,
        paid_later,
        "$1, 'topup', 'fulfilled', '{}', '{}', 'USD', 500, $2, $2, 'bot'",
        now - Span::minutes(30),
    )
    .await;
    order(
        store,
        renewing,
        "$1, 'renewal', 'canceled', '{}', '{}', 'USD', 500, $2, NULL, 'autorenew'",
        now - Span::minutes(90),
    )
    .await;
    order(store, early, &unpaid, now - Span::minutes(30)).await;
    let rule = create_rule(
        &ctx,
        &RuleDraft {
            unit: Unit::Hours,
            ..rule_draft(Trigger::UnpaidOrder, 1, Channel::Bot)
        },
        None,
    )
    .await
    .unwrap();
    let narrow = create_rule(
        &ctx,
        &with_audience(
            RuleDraft {
                unit: Unit::Hours,
                ..rule_draft(Trigger::UnpaidOrder, 1, Channel::Bot)
            },
            serde_json::json!({"tariff": [plan + 1000]}),
        ),
        None,
    )
    .await
    .unwrap();
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[left]));
    assert!(fired(store, narrow.id).await.is_empty());
    auto::check(&ctx, now + Span::minutes(45)).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[left, retried, early]));
    db.cleanup().await;
}

#[tokio::test]
async fn первая_покупка_один_раз_и_не_перенесённым() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let bought = "$1, 'new', 'fulfilled', '{}', '{}', 'USD', 500, $2, $2, 'bot'";
    let fresh = tg_client(store, 9821).await;
    let returning = tg_client(store, 9822).await;
    let moved = tg_client(store, 9823).await;
    order(store, fresh, bought, now - Span::hours(25)).await;
    order(store, returning, bought, now - Span::days(40)).await;
    order(store, returning, bought, now - Span::hours(25)).await;
    run(
        store,
        "UPDATE clients SET registration_source = 'import' WHERE id = $1",
        moved.0,
    )
    .await;
    order(store, moved, bought, now - Span::hours(25)).await;
    let rule = create_rule(
        &ctx,
        &RuleDraft {
            unit: Unit::Hours,
            ..rule_draft(Trigger::FirstPurchase, 24, Channel::Bot)
        },
        None,
    )
    .await
    .unwrap();
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[fresh]));
    order(store, fresh, bought, now - Span::hours(1)).await;
    auto::check(&ctx, now + Span::hours(24)).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[fresh]));
    db.cleanup().await;
}

#[tokio::test]
async fn давно_нет_подключений_только_после_сверки() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let quiet = tg_client(store, 9831).await;
    let unchecked = tg_client(store, 9832).await;
    let limited = tg_client(store, 9833).await;
    let active = tg_client(store, 9834).await;
    for (client, synced, status) in [
        (quiet, now, None),
        (unchecked, now - Span::days(2), None),
        (limited, now, Some("limited")),
        (active, now, None),
    ] {
        let id = sub(store, client, "paid", now + Span::days(20)).await;
        let online = if client == active {
            now - Span::hours(3)
        } else {
            now - Span::days(7) - Span::hours(2)
        };
        let mut conn = store.conn().await.unwrap();
        sqlx::query(
            "UPDATE subscriptions SET online_at = $2, synced_at = $3, status = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(online)
        .bind(synced)
        .bind(status)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let short = auto::create_rule(
        &ctx,
        &RuleDraft {
            unit: Unit::Hours,
            ..rule_draft(Trigger::Idle, 12, Channel::Bot)
        },
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(code(short), "broadcast.invalid");
    let rule = create_rule(&ctx, &rule_draft(Trigger::Idle, 7, Channel::Bot), None)
        .await
        .unwrap();
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[quiet]));
    db.cleanup().await;
}

#[tokio::test]
async fn заняты_все_устройства_только_когда_можно_докупить() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let sells = tariff(store, 2).await;
    let closed = tariff(store, 0).await;
    let full = tg_client(store, 9841).await;
    let no_extra = tg_client(store, 9842).await;
    let maxed = tg_client(store, 9843).await;
    let free = tg_client(store, 9844).await;
    let mut subs = Vec::new();
    for (client, tariff_id, extra, count) in [
        (full, sells, 0, 3),
        (no_extra, closed, 0, 3),
        (maxed, sells, 2, 3),
        (free, sells, 0, 2),
    ] {
        let id = sub(store, client, "paid", now + Span::days(20)).await;
        let mut conn = store.conn().await.unwrap();
        sqlx::query(
            "UPDATE subscriptions SET tariff_id = $2, extra_devices = $3, hwid_limit = 3, devices_count = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(tariff_id)
        .bind(extra)
        .bind(count)
        .execute(conn.conn())
        .await
        .unwrap();
        subs.push(id);
    }
    let rule = create_rule(
        &ctx,
        &rule_draft(Trigger::DevicesFull, 0, Channel::Bot),
        None,
    )
    .await
    .unwrap();
    assert_eq!(rule.amount, 0);
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[full]));
    auto::check(&ctx, now + Span::minutes(10)).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[full]));
    run(
        store,
        "UPDATE subscriptions SET extra_devices = 1, hwid_limit = 4, devices_count = 4 WHERE id = $1",
        subs[0],
    )
    .await;
    auto::check(&ctx, now + Span::minutes(20)).await.unwrap();
    assert_eq!(case_clients(store, rule.id).await.len(), 2);
    db.cleanup().await;
}

#[tokio::test]
async fn трафик_закончился_полностью_при_пороге_сто() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let out = tg_client(store, 9851).await;
    let almost = tg_client(store, 9852).await;
    let first = sub(store, out, "paid", now + Span::days(20)).await;
    let second = sub(store, almost, "paid", now + Span::days(20)).await;
    traffic(store, first, 100, Some(100)).await;
    traffic(store, second, 99, Some(100)).await;
    let rule = create_rule(
        &ctx,
        &RuleDraft {
            percent: Some(100),
            ..rule_draft(Trigger::TrafficLow, 0, Channel::Bot)
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(rule.percent, Some(100));
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(fired(store, rule.id).await, ids(&[out]));
    db.cleanup().await;
}

#[tokio::test]
async fn во_время_технических_работ_уходят_только_рассылки_запущенные_вручную() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let held = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = held_context(store, Some(sender.clone()), None, held.clone());
    let now = now_of(store).await;
    let due = tg_client(store, 9701).await;
    let later = tg_client(store, 9702).await;
    sub(store, due, "paid", now + Span::days(3) - Span::hours(1)).await;
    let rule = create_rule(&ctx, &rule_draft(Trigger::Expiring, 3, Channel::Bot), None)
        .await
        .unwrap();
    assert_eq!(auto::check(&ctx, now).await.unwrap().queued, 1);

    held.store(true, std::sync::atomic::Ordering::SeqCst);
    sub(store, later, "paid", now + Span::days(3) - Span::hours(2)).await;
    BroadcastModule.run_task(&ctx, AUTO_TASK).await.unwrap();
    assert_eq!(
        fired(store, rule.id).await,
        ids(&[due]),
        "правила не срабатывают во время работ"
    );
    let manual = create(&ctx, &draft("Идут технические работы", Audience::All), None)
        .await
        .unwrap();
    start(&ctx, manual.id, None, None).await.unwrap();
    let planned = create(&ctx, &draft("Скидка до пятницы", Audience::All), None)
        .await
        .unwrap();
    start(&ctx, planned.id, Some(now + Span::hours(2)), None)
        .await
        .unwrap();
    run(
        store,
        "UPDATE broadcasts SET send_at = now() - interval '1 minute' WHERE id = $1",
        planned.id,
    )
    .await;

    let engine = spawn_engine(&ctx, &sender, fast());
    wait_status(store, manual.id, Status::Done).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let texts: Vec<String> = sender.sent().into_iter().map(|s| s.html).collect();
    assert_eq!(
        texts,
        vec!["Идут технические работы".to_string(); 2],
        "во время работ ушло что-то кроме ручной рассылки"
    );
    let mut conn = store.conn().await.unwrap();
    assert_eq!(
        store::view(&mut conn, planned.id).await.unwrap().status,
        Status::Scheduled
    );
    drop(conn);

    held.store(false, std::sync::atomic::Ordering::SeqCst);
    settle(store, rule.id).await;
    wait_status(store, planned.id, Status::Done).await;
    engine.stop().await;
    let sent = sender.sent();
    assert_eq!(sent.len(), 5, "{sent:?}");
    assert_eq!(
        sent.iter()
            .filter(|s| s.html.contains("Правило") && s.chat == 9701)
            .count(),
        1
    );
    assert_eq!(
        sent.iter()
            .filter(|s| s.html == "Скидка до пятницы")
            .count(),
        2
    );
    db.cleanup().await;
}
