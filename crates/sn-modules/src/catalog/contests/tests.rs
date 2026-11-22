use super::*;
use crate::catalog::testing::{
    TestCore, age_challenge, antibot, antibot_actions, client, context, suspect,
};
use sn_domain::client::{IdentityKind, NewIdentity};
use sn_domain::ids::{RemnaUserId, SubscriptionId};
use sn_domain::subscription::{AutoRenewState, RemnaLink, SubscriptionKind};
use sn_store::testing::test_store;
use sn_store::{NewSubscription, Store};

#[test]
fn состояние_конкурса_по_датам() {
    let start = DateTime::from_timestamp(1_000_000, 0).unwrap();
    let end = start + Duration::days(1);
    assert_eq!(
        Status::of(start, end, None, None, start - Duration::seconds(1)),
        Status::Scheduled
    );
    assert_eq!(Status::of(start, end, None, None, start), Status::Running);
    assert_eq!(Status::of(start, end, None, None, end), Status::Drawing);
    assert_eq!(
        Status::of(start, end, Some(end), None, end),
        Status::Finished
    );
    assert_eq!(
        Status::of(start, end, None, Some(start), start),
        Status::Canceled
    );
}

#[test]
fn призы_по_местам_читаются_и_находятся_по_месту() {
    let legacy: PrizeConfig =
        serde_json::from_value(serde_json::json!({ "kind": "days", "days": 7 })).unwrap();
    assert_eq!(legacy, PrizeConfig::Single(Prize::Days { days: 7 }));
    assert_eq!(prize_for_place(&legacy, 4), Some(Prize::Days { days: 7 }));

    let raw = serde_json::json!({
        "kind": "tiered",
        "tiers": [
            { "from_place": 1, "to_place": 1, "prize": { "kind": "custom", "title": "Telegram Premium" } },
            { "from_place": 2, "to_place": 2, "prize": { "kind": "days", "days": 30 } },
            { "from_place": 3, "to_place": 5, "prize": { "kind": "promo", "percent": 20, "valid_days": 14 } }
        ]
    });
    let tiered: PrizeConfig = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(tiered.winners(), Some(5));
    assert_eq!(tiered.tiers().len(), 3);
    assert_eq!(
        prize_for_place(&tiered, 1),
        Some(Prize::Custom {
            title: "Telegram Premium".into(),
            description: None,
        })
    );
    assert_eq!(prize_for_place(&tiered, 2), Some(Prize::Days { days: 30 }));
    assert_eq!(
        prize_for_place(&tiered, 4),
        Some(Prize::Promo {
            percent: 20,
            valid_days: 14,
        })
    );
    assert_eq!(prize_for_place(&tiered, 6), None);
    assert_eq!(serde_json::to_value(&tiered).unwrap(), raw);
    assert_eq!(place_title(1, 1), "🥇 1 место");
    assert_eq!(place_title(2, 3), "2–3 места");
    assert_eq!(place_title(4, 10), "4–10 мест");
}

#[test]
fn призовые_места_проверяются_на_пересечения_и_пропуски() {
    let tier = |from: u32, to: u32| PrizeTierDraft {
        from_place: from,
        to_place: to,
        prize: PrizeDraft::Days { days: 3 },
    };
    let ok = clean_prize_config(
        &PrizeConfigDraft::Tiered {
            _kind: TieredKind::Tiered,
            tiers: vec![tier(1, 1), tier(2, 2), tier(3, 10)],
        },
        Currency::Rub,
    )
    .unwrap();
    assert_eq!(ok.winners(), Some(10));

    assert!(
        clean_prize_config(
            &PrizeConfigDraft::Tiered {
                _kind: TieredKind::Tiered,
                tiers: vec![tier(2, 3)],
            },
            Currency::Rub
        )
        .is_err()
    );
    assert!(
        clean_prize_config(
            &PrizeConfigDraft::Tiered {
                _kind: TieredKind::Tiered,
                tiers: vec![tier(1, 1), tier(3, 4)],
            },
            Currency::Rub
        )
        .is_err()
    );
    assert!(
        clean_prize_config(
            &PrizeConfigDraft::Tiered {
                _kind: TieredKind::Tiered,
                tiers: vec![tier(1, 2), tier(2, 3)],
            },
            Currency::Rub
        )
        .is_err()
    );
    assert!(
        clean_prize_config(
            &PrizeConfigDraft::Tiered {
                _kind: TieredKind::Tiered,
                tiers: vec![tier(1, 101)],
            },
            Currency::Rub
        )
        .is_err()
    );
    assert!(
        clean_prize_config(
            &PrizeConfigDraft::Tiered {
                _kind: TieredKind::Tiered,
                tiers: Vec::new(),
            },
            Currency::Rub
        )
        .is_err()
    );
}

#[test]
fn тексты_условий_и_подстановки() {
    let offset = offset_of(180);
    let start = DateTime::parse_from_rfc3339("2026-12-01T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let end = DateTime::parse_from_rfc3339("2026-12-31T17:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(when(end, offset), "31\u{a0}декабря в 20:00");
    let lines = condition_lines(
        &Conditions {
            subscription: Some(SubscriptionRule { paid_only: true }),
            purchase: Some(PurchaseRule {
                from: None,
                until: None,
                min: Some(Money::new(30_000, Currency::Rub)),
            }),
            friends: Some(FriendsRule {
                count: 2,
                paid: true,
            }),
        },
        start,
        end,
        offset,
    );
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Действующая купленная или подаренная подписка",
            "Оплата подписки или опций во время конкурса от 300\u{a0}₽",
            "Пригласить 2 друзей за время конкурса, друг должен оплатить подписку",
        ]
    );
    let rendered = render(
        "<b>{title}</b>: {prize}, {winners}, {date}",
        "Новый <год>",
        &PrizeConfig::Single(Prize::Days { days: 7 }),
        3,
        end,
        offset,
    );
    assert_eq!(
        rendered,
        "<b>Новый &lt;год&gt;</b>: +7\u{a0}дней к подписке, 3, 31\u{a0}декабря в 20:00"
    );
    let messages = default_messages(start, end);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].send_at, Some(start));
    assert_eq!(messages[3].audience, Audience::Results);
    assert!(
        default_messages(start, start + Duration::hours(1))
            .iter()
            .all(|m| m.audience != Audience::Participants)
    );
}

async fn tg_client(store: &Store, chat: i64) -> ClientId {
    let id = client(store).await;
    let mut conn = store.conn().await.unwrap();
    conn.clients()
        .add_identity(
            id,
            &NewIdentity {
                kind: IdentityKind::Telegram,
                value: chat.to_string(),
                verified: true,
                meta: serde_json::json!({ "first_name": format!("Клиент {chat}") }),
            },
        )
        .await
        .unwrap();
    id
}

async fn live_sub(store: &Store, owner: ClientId, kind: SubscriptionKind) -> SubscriptionId {
    let mut conn = store.conn().await.unwrap();
    let sub = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: owner,
            tariff_id: None,
            kind,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    conn.subscriptions()
        .link_remna(
            sub.id,
            &RemnaLink {
                user_id: RemnaUserId(sub.id.0 + 5000),
                username: format!("sn_contest_{}", sub.id),
                short_uuid: None,
            },
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE subscriptions SET expire_at = now() + interval '30 days', status = 'active', \
         created_at = now() - interval '1 day' WHERE id = $1",
    )
    .bind(sub.id.0)
    .execute(conn.conn())
    .await
    .unwrap();
    sub.id
}

async fn paid_order(store: &Store, owner: ClientId, total_minor: i64, ago_minutes: i64) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO orders (client_id, kind, status, intent, quote, currency, total_minor, paid_at) \
         VALUES ($1, 'new', 'fulfilled', '{}'::jsonb, '{}'::jsonb, 'USD', $2, now() - make_interval(mins => $3::int))",
    )
    .bind(owner.0)
    .bind(total_minor)
    .bind(i32::try_from(ago_minutes).unwrap())
    .execute(conn.conn())
    .await
    .unwrap();
}

fn draft(prize: PrizeDraft, winners: u32) -> ContestDraft {
    ContestDraft {
        title: "Новогодний розыгрыш".into(),
        description: Some("Разыгрываем призы среди клиентов".into()),
        prize: PrizeConfigDraft::Single(prize),
        winners,
        starts_at: None,
        ends_at: Utc::now() + Duration::hours(1),
        conditions: ConditionsDraft::default(),
        messages: None,
    }
}

async fn finish_now(store: &Store, id: i64) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "UPDATE contests SET starts_at = now() - interval '2 hours', ends_at = now() - interval '1 minute' \
         WHERE id = $1",
    )
    .bind(id)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn balance(store: &Store, owner: ClientId) -> i64 {
    let mut conn = store.conn().await.unwrap();
    conn.ledger()
        .balance(owner, sn_domain::ledger::Wallet::Main, Currency::Usd)
        .await
        .unwrap()
        .minor
}

#[tokio::test]
async fn конкурс_от_создания_до_выдачи_призов_с_честной_жеребьёвкой() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let a = tg_client(&db.store, 7001).await;
    let b = tg_client(&db.store, 7002).await;
    let c = tg_client(&db.store, 7003).await;
    let d = tg_client(&db.store, 7004).await;
    for owner in [a, b, c] {
        live_sub(&db.store, owner, SubscriptionKind::Paid).await;
    }
    let mut new = draft(PrizeDraft::Balance { amount: "5".into() }, 2);
    new.conditions.subscription = true;
    let created = create(&ctx, &new, None).await.unwrap();
    assert_eq!(created.status, Status::Running);
    assert_eq!(created.seed, None);
    assert_eq!(created.messages.len(), 2);
    assert_eq!(created.condition_lines.len(), 1);
    let id = created.id;

    assert_eq!(join(&ctx, id, a).await.unwrap(), JoinOutcome::Joined);
    assert_eq!(join(&ctx, id, a).await.unwrap(), JoinOutcome::Already);
    assert_eq!(join(&ctx, id, b).await.unwrap(), JoinOutcome::Joined);
    assert_eq!(join(&ctx, id, c).await.unwrap(), JoinOutcome::Joined);
    assert_eq!(join(&ctx, id, d).await.unwrap(), JoinOutcome::NotEligible);
    let seen = client_view(&ctx, id, d).await.unwrap().unwrap();
    assert!(!seen.joined);
    assert!(!seen.eligible());
    assert_eq!(seen.entries, 3);

    finish_now(&db.store, id).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query(
            "UPDATE subscriptions SET expire_at = now() - interval '30 minutes' WHERE client_id = $1",
        )
        .bind(c.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    assert_eq!(join(&ctx, id, d).await.unwrap(), JoinOutcome::Closed);

    let report = run(&ctx).await.unwrap();
    assert_eq!(report.drawn, 1);
    assert_eq!(report.prizes, 2);
    let done = contest(&ctx, id).await.unwrap();
    assert_eq!(done.status, Status::Finished);
    assert_eq!(done.entries_drawn, Some(2));
    assert_eq!(done.counts.excluded, 1);
    assert_eq!(done.counts.issued, 2);
    let seed = done.seed.clone().unwrap();
    let raw = hex::decode(&seed).unwrap();
    assert_eq!(draw::seed_hash(&raw), done.seed_hash);
    assert_eq!(done.list_hash, Some(draw::list_hash(&[a.0, b.0])));
    let expected: Vec<i64> = draw::tickets(&raw, &[a.0, b.0])
        .into_iter()
        .map(|t| t.client_id)
        .collect();
    let winners: Vec<i64> = done.winners_list.iter().map(|w| w.client_id.0).collect();
    assert_eq!(winners, expected);
    assert!(
        done.winners_list
            .iter()
            .all(|w| w.prize_state.as_deref() == Some("issued"))
    );
    assert_eq!(balance(&db.store, a).await, 500);
    assert_eq!(balance(&db.store, b).await, 500);
    assert_eq!(balance(&db.store, c).await, 0);
    let excluded = entries(&ctx, id, EntryFilter::Excluded, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(excluded.items.len(), 1);
    assert_eq!(excluded.items[0].client_id, c);
    assert!(
        excluded.items[0]
            .excluded
            .as_deref()
            .unwrap()
            .starts_with("Не выполнено")
    );
    let won: Vec<ClientId> = core
        .notifications()
        .into_iter()
        .filter(|(_, n)| n.template == TEMPLATE_WON)
        .map(|(c, _)| c)
        .collect();
    assert_eq!(won.len(), 2);
    assert!(won.contains(&a) && won.contains(&b));
    assert!(done.log.iter().any(|l| l.kind == "draw"));

    let results = done
        .messages
        .iter()
        .find(|m| m.audience == Audience::Results)
        .unwrap();
    assert_eq!(results.state, MessageState::Queued);
    assert_eq!(results.recipients, Some(1));
    let announce = done
        .messages
        .iter()
        .find(|m| m.audience == Audience::Outsiders)
        .unwrap();
    assert_eq!(announce.state, MessageState::Skipped);

    let again = run(&ctx).await.unwrap();
    assert_eq!(again, RunReport::default());
    assert_eq!(balance(&db.store, a).await, 500);
    let csv = entries_csv(&ctx, id).await.unwrap();
    assert_eq!(csv.lines().count(), 4);
    assert!(csv.contains(&format!("{},", c.0)));
    db.cleanup().await;
}

#[tokio::test]
async fn напоминания_уходят_скрытыми_пачками_рассылок() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let joined = tg_client(&db.store, 8001).await;
    let ready = tg_client(&db.store, 8002).await;
    let idle = tg_client(&db.store, 8003).await;
    let stopped = tg_client(&db.store, 8004).await;
    live_sub(&db.store, joined, SubscriptionKind::Paid).await;
    live_sub(&db.store, ready, SubscriptionKind::Paid).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.clients()
            .set_bot_unreachable(stopped, true)
            .await
            .unwrap();
    }
    let now = Utc::now();
    let mut new = draft(PrizeDraft::Days { days: 7 }, 1);
    new.conditions.subscription = true;
    new.messages = Some(vec![
        MessageDraft {
            id: None,
            audience: Audience::Outsiders,
            only_eligible: true,
            send_at: Some(now),
            text: "Вы подходите: {title}".into(),
        },
        MessageDraft {
            id: None,
            audience: Audience::Outsiders,
            only_eligible: false,
            send_at: Some(now),
            text: "Всем: {title}, итоги {date}".into(),
        },
        MessageDraft {
            id: None,
            audience: Audience::Participants,
            only_eligible: false,
            send_at: Some(now + Duration::minutes(30)),
            text: "Скоро итоги".into(),
        },
    ]);
    let created = create(&ctx, &new, None).await.unwrap();
    assert_eq!(
        join(&ctx, created.id, joined).await.unwrap(),
        JoinOutcome::Joined
    );
    let report = run(&ctx).await.unwrap();
    assert_eq!(report.messages, 2);
    let view = contest(&ctx, created.id).await.unwrap();
    let eligible = view.messages.iter().find(|m| m.only_eligible).unwrap();
    assert_eq!(eligible.recipients, Some(1));
    let all = view
        .messages
        .iter()
        .find(|m| m.audience == Audience::Outsiders && !m.only_eligible)
        .unwrap();
    assert_eq!(all.recipients, Some(2));
    assert_eq!(all.progress.unwrap().pending, 2);
    let participants = view
        .messages
        .iter()
        .find(|m| m.audience == Audience::Participants)
        .unwrap();
    assert_eq!(participants.state, MessageState::Planned);
    let mut conn = db.store.conn().await.unwrap();
    let hidden: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT b.id, b.status, b.text FROM broadcasts b WHERE b.contest_message_id IS NOT NULL ORDER BY b.id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(hidden.len(), 2);
    assert!(hidden.iter().all(|(_, status, _)| status == "sending"));
    assert!(hidden[1].2.starts_with("Всем: Новогодний розыгрыш, итоги"));
    let chats: Vec<i64> = sqlx::query_scalar(
        "SELECT r.chat_id FROM broadcast_recipients r WHERE r.broadcast_id = $1 ORDER BY r.chat_id",
    )
    .bind(hidden[1].0)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(chats, [8002, 8003]);
    drop(conn);
    let once = crate::catalog::broadcast::list(&ctx, PageRequest::default())
        .await
        .unwrap();
    assert!(once.items.is_empty());
    assert!(chats.contains(&8003) && !chats.contains(&8001));
    let _ = idle;

    let canceled = cancel(&ctx, created.id, None).await.unwrap();
    assert_eq!(canceled.status, Status::Canceled);
    let left = canceled
        .messages
        .iter()
        .find(|m| m.audience == Audience::Participants)
        .unwrap();
    assert_eq!(left.state, MessageState::Skipped);
    assert_eq!(left.skipped.as_deref(), Some("Конкурс отменён"));
    let stopped_rows: i64 = {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query_scalar("SELECT count(*) FROM broadcast_recipients WHERE state = 'skipped'")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    assert_eq!(stopped_rows, 3);
    assert!(delete(&ctx, created.id, None).await.is_err());
    db.cleanup().await;
}

#[tokio::test]
async fn приз_днями_ждёт_подписку_а_сбой_выдаётся_снова() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let waiting = tg_client(&db.store, 9001).await;
    let unlucky = tg_client(&db.store, 9002).await;
    let unlucky_sub = live_sub(&db.store, unlucky, SubscriptionKind::Trial).await;
    let created = create(&ctx, &draft(PrizeDraft::Days { days: 10 }, 2), None)
        .await
        .unwrap();
    join(&ctx, created.id, waiting).await.unwrap();
    join(&ctx, created.id, unlucky).await.unwrap();
    finish_now(&db.store, created.id).await;
    core.fail_days
        .store(true, std::sync::atomic::Ordering::SeqCst);
    run(&ctx).await.unwrap();
    let view = contest(&ctx, created.id).await.unwrap();
    let state = |client: ClientId| {
        view.winners_list
            .iter()
            .find(|w| w.client_id == client)
            .and_then(|w| w.prize_state.clone())
            .unwrap()
    };
    assert_eq!(state(waiting), "waiting");
    assert_eq!(state(unlucky), "failed");
    assert_eq!(
        core.notifications()
            .iter()
            .filter(|(c, n)| *c == waiting && n.template == TEMPLATE_WAITING)
            .count(),
        1
    );
    run(&ctx).await.unwrap();
    assert_eq!(
        core.notifications()
            .iter()
            .filter(|(_, n)| n.template == TEMPLATE_WAITING)
            .count(),
        1
    );
    core.fail_days
        .store(false, std::sync::atomic::Ordering::SeqCst);
    retry_prize(&ctx, created.id, unlucky, None).await.unwrap();
    assert!(retry_prize(&ctx, created.id, unlucky, None).await.is_err());
    let waiting_sub = live_sub(&db.store, waiting, SubscriptionKind::Paid).await;
    run(&ctx).await.unwrap();
    let view = contest(&ctx, created.id).await.unwrap();
    assert!(
        view.winners_list
            .iter()
            .all(|w| w.prize_state.as_deref() == Some("issued"))
    );
    let days = core.days();
    assert!(days.iter().any(|(s, d, _)| *s == waiting_sub && *d == 10));
    assert!(days.iter().any(|(s, d, _)| *s == unlucky_sub && *d == 10));
    db.cleanup().await;
}

#[tokio::test]
async fn промокод_в_призе_личный_одноразовый_и_только_с_модулем() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let winner = tg_client(&db.store, 9101).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.modules()
            .ensure_defaults(&[("promo", false)])
            .await
            .unwrap();
        sqlx::query("UPDATE module_states SET enabled = false WHERE module = 'promo'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let promo = PrizeDraft::Promo {
        percent: 25,
        valid_days: 10,
    };
    let refused = create(&ctx, &draft(promo.clone(), 1), None)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("Промокоды"));
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("UPDATE module_states SET enabled = true WHERE module = 'promo'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let created = create(&ctx, &draft(promo, 1), None).await.unwrap();
    join(&ctx, created.id, winner).await.unwrap();
    finish_now(&db.store, created.id).await;
    run(&ctx).await.unwrap();
    let view = contest(&ctx, created.id).await.unwrap();
    let code = view.winners_list[0].prize_code.clone().unwrap();
    assert!(code.starts_with("WIN-"));
    let mut conn = db.store.conn().await.unwrap();
    let (percent, max_uses, per_client, until): (i32, i32, i32, Option<DateTime<Utc>>) =
        sqlx::query_as(
            "SELECT percent, max_uses, per_client, valid_until FROM promo_codes WHERE code = $1",
        )
        .bind(&code)
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!((percent, max_uses, per_client), (25, 1, 1));
    assert!(until.unwrap() > Utc::now() + Duration::days(9));
    let notified = core
        .notifications()
        .into_iter()
        .find(|(c, _)| *c == winner)
        .unwrap();
    assert_eq!(notified.1.params["code"], code);
    db.cleanup().await;
}

#[tokio::test]
async fn разные_призы_по_местам_выдаются_каждому_победителю() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let first = tg_client(&db.store, 9301).await;
    let second = tg_client(&db.store, 9302).await;
    let third = tg_client(&db.store, 9303).await;
    for owner in [first, second, third] {
        live_sub(&db.store, owner, SubscriptionKind::Paid).await;
    }
    let mut new = draft(PrizeDraft::Days { days: 1 }, 0);
    new.prize = PrizeConfigDraft::Tiered {
        _kind: TieredKind::Tiered,
        tiers: vec![
            PrizeTierDraft {
                from_place: 1,
                to_place: 1,
                prize: PrizeDraft::Custom {
                    title: "Telegram Premium на год".into(),
                    description: Some("Напишите в поддержку".into()),
                },
            },
            PrizeTierDraft {
                from_place: 2,
                to_place: 2,
                prize: PrizeDraft::Days { days: 30 },
            },
            PrizeTierDraft {
                from_place: 3,
                to_place: 3,
                prize: PrizeDraft::Balance { amount: "5".into() },
            },
        ],
    };
    let created = create(&ctx, &new, None).await.unwrap();
    assert_eq!(created.winners, 3);
    assert!(created.prize.is_tiered());
    assert!(created.prize_label.contains("Telegram Premium на год"));
    assert!(created.prize_label.contains("+30\u{a0}дней к подписке"));
    for owner in [first, second, third] {
        assert_eq!(
            join(&ctx, created.id, owner).await.unwrap(),
            JoinOutcome::Joined
        );
    }
    finish_now(&db.store, created.id).await;
    run(&ctx).await.unwrap();
    let view = contest(&ctx, created.id).await.unwrap();
    assert_eq!(view.winners_list.len(), 3);
    let seed = hex::decode(view.seed.clone().unwrap()).unwrap();
    let order: Vec<ClientId> = draw::tickets(&seed, &[first.0, second.0, third.0])
        .into_iter()
        .map(|drawn| ClientId(drawn.client_id))
        .collect();
    let custom = order[0];
    let days = order[1];
    let balance_winner = order[2];

    let winner = |client: ClientId| {
        view.winners_list
            .iter()
            .find(|w| w.client_id == client)
            .unwrap()
    };
    assert_eq!(winner(custom).place, 1);
    assert_eq!(winner(custom).prize_state.as_deref(), Some("issued"));
    assert_eq!(
        winner(custom).prize_label.as_deref(),
        Some("Telegram Premium на год")
    );
    assert_eq!(
        winner(custom).prize_note.as_deref(),
        Some("Напишите в поддержку")
    );
    assert_eq!(winner(days).place, 2);
    assert_eq!(winner(days).prize_state.as_deref(), Some("issued"));
    assert_eq!(
        winner(days).prize_label.as_deref(),
        Some("+30\u{a0}дней к подписке")
    );
    assert!(core.days().iter().any(|(_, days, _)| *days == 30));
    assert_eq!(winner(balance_winner).place, 3);
    assert_eq!(
        winner(balance_winner).prize_state.as_deref(),
        Some("issued")
    );
    assert_eq!(balance(&db.store, balance_winner).await, 500);

    let notified = core
        .notifications()
        .into_iter()
        .find(|(client, _)| *client == custom)
        .unwrap()
        .1;
    assert_eq!(notified.template, TEMPLATE_WON);
    assert_eq!(notified.params["place"], 1);
    assert_eq!(notified.params["prize_data"]["kind"], "custom");
    assert_eq!(
        notified.params["note"].as_str(),
        Some("Напишите в поддержку")
    );
    db.cleanup().await;
}

#[tokio::test]
async fn правки_конкурса_зависят_от_этапа() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut later = draft(PrizeDraft::Days { days: 3 }, 1);
    later.starts_at = Some(Utc::now() + Duration::days(1));
    later.ends_at = Utc::now() + Duration::days(3);
    let planned = create(&ctx, &later, None).await.unwrap();
    assert_eq!(planned.status, Status::Scheduled);
    assert!(planned.editable.all);
    assert_eq!(planned.messages.len(), 4);
    let mut changed = later.clone();
    changed.prize = PrizeConfigDraft::Single(PrizeDraft::Days { days: 5 });
    changed.messages = Some(vec![MessageDraft {
        id: None,
        audience: Audience::Results,
        only_eligible: false,
        send_at: None,
        text: "Итоги".into(),
    }]);
    let updated = update(&ctx, planned.id, &changed, None).await.unwrap();
    assert_eq!(updated.prize, PrizeConfig::Single(Prize::Days { days: 5 }));
    assert_eq!(updated.messages.len(), 1);
    delete(&ctx, planned.id, None).await.unwrap();

    let running = create(&ctx, &draft(PrizeDraft::Days { days: 3 }, 1), None)
        .await
        .unwrap();
    assert!(!running.editable.all && running.editable.texts);
    let mut wrong = draft(PrizeDraft::Days { days: 4 }, 1);
    wrong.ends_at = running.ends_at;
    assert!(update(&ctx, running.id, &wrong, None).await.is_err());
    let mut shorter = draft(PrizeDraft::Days { days: 3 }, 1);
    shorter.ends_at = running.ends_at - Duration::minutes(20);
    assert!(update(&ctx, running.id, &shorter, None).await.is_err());
    let mut longer = draft(PrizeDraft::Days { days: 3 }, 1);
    longer.title = "Розыгрыш дольше".into();
    longer.ends_at = running.ends_at + Duration::days(1);
    let extended = update(&ctx, running.id, &longer, None).await.unwrap();
    assert_eq!(extended.title, "Розыгрыш дольше");
    assert!(delete(&ctx, running.id, None).await.is_err());
    let mut too_many = draft(PrizeDraft::Days { days: 3 }, 0);
    too_many.winners = 0;
    assert!(create(&ctx, &too_many, None).await.is_err());
    let mut short = draft(PrizeDraft::Days { days: 3 }, 1);
    short.ends_at = Utc::now() + Duration::minutes(5);
    assert!(create(&ctx, &short, None).await.is_err());
    db.cleanup().await;
}

#[tokio::test]
async fn условия_покупки_и_друзей_считаются_по_данным_проекта() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let buyer = tg_client(&db.store, 9201).await;
    let cheap = tg_client(&db.store, 9202).await;
    let host = tg_client(&db.store, 9203).await;
    let friend_paid = tg_client(&db.store, 9204).await;
    let friend_free = tg_client(&db.store, 9205).await;
    let mut new = draft(PrizeDraft::Balance { amount: "1".into() }, 3);
    new.conditions.purchase = true;
    new.conditions.purchase_min = Some("3".into());
    let shop = create(&ctx, &new, None).await.unwrap();
    paid_order(&db.store, buyer, 500, 0).await;
    paid_order(&db.store, cheap, 100, 0).await;
    paid_order(&db.store, cheap, 900, 24 * 60).await;
    assert_eq!(
        join(&ctx, shop.id, buyer).await.unwrap(),
        JoinOutcome::Joined
    );
    assert_eq!(
        join(&ctx, shop.id, cheap).await.unwrap(),
        JoinOutcome::NotEligible
    );

    let mut friends = draft(PrizeDraft::Balance { amount: "1".into() }, 1);
    friends.conditions.friends = 1;
    friends.conditions.friends_paid = true;
    let invite = create(&ctx, &friends, None).await.unwrap();
    {
        let mut conn = db.store.conn().await.unwrap();
        for friend in [friend_paid, friend_free] {
            sqlx::query("INSERT INTO referral_links (client_id, referrer_id) VALUES ($1, $2)")
                .bind(friend.0)
                .bind(host.0)
                .execute(conn.conn())
                .await
                .unwrap();
        }
    }
    let before = client_view(&ctx, invite.id, host).await.unwrap().unwrap();
    assert_eq!(
        before.checks[0].text,
        "Пригласить 1 друга за время конкурса: 0 из 1, друг должен оплатить подписку"
    );
    assert_eq!(
        join(&ctx, invite.id, host).await.unwrap(),
        JoinOutcome::NotEligible
    );
    paid_order(&db.store, friend_paid, 300, 0).await;
    assert_eq!(
        join(&ctx, invite.id, host).await.unwrap(),
        JoinOutcome::Joined
    );
    let listed = list(&ctx, ListFilter::Live, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 2);
    assert_eq!(listed.items[0].id, invite.id);
    assert_eq!(listed.items[0].counts.entries, 1);
    let mine = visible_for(&ctx, host).await.unwrap();
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().any(|c| c.id == invite.id && c.joined));
    assert!(has_running(&ctx).await.unwrap());
    db.cleanup().await;
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn маршруты_конкурсов_под_модулем_и_правом_маркетинга() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sn_domain::access::{AdminPrincipal, AdminRole};
    use std::sync::Arc;
    use tower::ServiceExt;
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let registry = Arc::new(crate::registry::ModuleRegistry::new(crate::catalog::all()).unwrap());
    let admin_id = {
        let mut conn = db.store.conn().await.unwrap();
        conn.admins()
            .create(&sn_store::NewAdmin {
                email: "owner@example.com".into(),
                password_hash: "x".into(),
                role: AdminRole::Owner,
                must_change_password: false,
            })
            .await
            .unwrap()
            .id
    };
    let as_role = |role: AdminRole| AdminPrincipal {
        admin_id,
        role,
        permissions: Vec::new(),
        session_id: 1,
        api_key: None,
    };
    let body = serde_json::json!({
        "title": "Розыгрыш через API",
        "prize": {"kind": "balance", "amount": "2.50"},
        "winners": 2,
        "ends_at": (Utc::now() + Duration::days(2)).to_rfc3339(),
        "conditions": {"subscription": true, "paid_only": true}
    });
    let post = |role: AdminRole| {
        Request::post("/contests/contests")
            .header("content-type", "application/json")
            .extension(as_role(role))
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let router = registry.admin_router(ctx.clone());
    let off = router
        .clone()
        .oneshot(post(AdminRole::Owner))
        .await
        .unwrap();
    assert_eq!(off.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_of(off).await["error"]["code"], "module.disabled");
    registry
        .set_enabled(&db.store, ModuleId::Contests, true, None)
        .await
        .unwrap();
    let denied = router
        .clone()
        .oneshot(post(AdminRole::Support))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let created = router
        .clone()
        .oneshot(post(AdminRole::Admin))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json_of(created).await;
    assert_eq!(created["prize"]["amount"]["minor"], 250);
    assert_eq!(created["status"], "running");
    assert!(created["seed"].is_null());
    assert_eq!(created["seed_hash"].as_str().unwrap().len(), 64);
    assert_eq!(
        created["condition_lines"][0]["text"],
        "Действующая купленная или подаренная подписка"
    );
    let id = created["id"].as_i64().unwrap();
    let get = |path: String| {
        Request::get(path)
            .extension(as_role(AdminRole::Owner))
            .body(Body::empty())
            .unwrap()
    };
    let listed = json_of(
        router
            .clone()
            .oneshot(get("/contests/contests?status=live".into()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["items"][0]["id"], id);
    let bad = router
        .clone()
        .oneshot(get("/contests/contests?status=nope".into()))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let options = json_of(
        router
            .clone()
            .oneshot(get("/contests/options".into()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(options["winners_max"], WINNERS_MAX);
    let canceled = router
        .clone()
        .oneshot(
            Request::post(format!("/contests/contests/{id}/cancel"))
                .extension(as_role(AdminRole::Admin))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(canceled.status(), StatusCode::OK);
    assert_eq!(json_of(canceled).await["status"], "canceled");
    let csv = router
        .clone()
        .oneshot(get(format!("/contests/contests/{id}/entries.csv")))
        .await
        .unwrap();
    assert_eq!(csv.status(), StatusCode::OK);
    assert!(
        csv.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/csv")
    );
    let removed = router
        .oneshot(
            Request::delete(format!("/contests/contests/{id}"))
                .extension(as_role(AdminRole::Admin))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    db.cleanup().await;
}

fn guarded(mode: &str) -> serde_json::Value {
    serde_json::json!({ "mode": mode, "burst_count": 0, "check_username": false })
}

async fn invite(store: &Store, friend: ClientId, host: ClientId) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query("INSERT INTO referral_links (client_id, referrer_id) VALUES ($1, $2)")
        .bind(friend.0)
        .bind(host.0)
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn session(store: &Store, owner: ClientId, ip: &str) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO client_sessions (client_id, token_hash, channel, ip, expires_at) \
         VALUES ($1, $2, 'miniapp', $3, now() + interval '1 day')",
    )
    .bind(owner.0)
    .bind(format!("s-{}-{ip}", owner.0).into_bytes())
    .bind(ip)
    .execute(conn.conn())
    .await
    .unwrap();
}

#[tokio::test]
async fn антибот_не_пускает_в_конкурс_в_режиме_запрета_а_снятая_отметка_пускает_сразу() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    antibot(&db.store, guarded("block")).await;
    let bot = tg_client(&db.store, 9301).await;
    let honest = tg_client(&db.store, 9302).await;
    suspect(&db.store, bot).await;
    let created = create(
        &ctx,
        &draft(PrizeDraft::Balance { amount: "1".into() }, 1),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        join(&ctx, created.id, bot).await.unwrap(),
        JoinOutcome::Denied
    );
    assert_eq!(
        join(&ctx, created.id, bot).await.unwrap(),
        JoinOutcome::Denied
    );
    assert!(
        !client_view(&ctx, created.id, bot)
            .await
            .unwrap()
            .unwrap()
            .joined
    );
    assert_eq!(
        join(&ctx, created.id, honest).await.unwrap(),
        JoinOutcome::Joined
    );
    assert_eq!(antibot_actions(&db.store, bot).await, ["contest_denied"]);
    assert!(antibot_actions(&db.store, honest).await.is_empty());
    {
        let mut conn = db.store.conn().await.unwrap();
        let journal = crate::catalog::antibot::store::journal(
            &mut conn,
            crate::catalog::antibot::JournalFilter::Contest,
            None,
            PageRequest::default(),
        )
        .await
        .unwrap();
        assert_eq!(journal.items.len(), 1);
        assert_eq!(journal.items[0].title, "Участие в конкурсе не принято");
        assert_eq!(journal.items[0].details["contest_id"], created.id);
        assert_eq!(journal.items[0].details["title"], "Новогодний розыгрыш");
    }

    crate::catalog::antibot::clear(&ctx, bot, None)
        .await
        .unwrap();
    assert_eq!(
        join(&ctx, created.id, bot).await.unwrap(),
        JoinOutcome::Joined
    );

    let other = tg_client(&db.store, 9303).await;
    suspect(&db.store, other).await;
    assert_eq!(
        join(&ctx, created.id, other).await.unwrap(),
        JoinOutcome::Denied
    );
    let mut off = guarded("block");
    off["contest_guard"] = serde_json::json!(false);
    antibot(&db.store, off).await;
    assert_eq!(
        join(&ctx, created.id, other).await.unwrap(),
        JoinOutcome::Joined
    );

    antibot(&db.store, guarded("mark")).await;
    let watched = client(&db.store).await;
    suspect(&db.store, watched).await;
    assert_eq!(
        join(&ctx, created.id, watched).await.unwrap(),
        JoinOutcome::Joined
    );
    assert_eq!(antibot_actions(&db.store, watched).await, ["contest_noted"]);
    db.cleanup().await;
}

#[tokio::test]
async fn антибот_просит_подтвердить_участие_в_конкурсе_кнопкой() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    antibot(&db.store, guarded("confirm")).await;
    let player = tg_client(&db.store, 9311).await;
    suspect(&db.store, player).await;
    let created = create(
        &ctx,
        &draft(PrizeDraft::Balance { amount: "1".into() }, 1),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        join(&ctx, created.id, player).await.unwrap(),
        JoinOutcome::Confirm
    );
    let token = crate::catalog::antibot::challenge(&ctx, player)
        .await
        .unwrap();
    assert_eq!(
        join_confirmed(&ctx, created.id, player, &token)
            .await
            .unwrap(),
        JoinOutcome::Confirm
    );
    age_challenge(&db.store, player).await;
    assert_eq!(
        join_confirmed(&ctx, created.id, player, "не-тот-код")
            .await
            .unwrap(),
        JoinOutcome::Confirm
    );
    assert_eq!(
        join_confirmed(&ctx, created.id, player, &token)
            .await
            .unwrap(),
        JoinOutcome::Joined
    );
    assert_eq!(
        antibot_actions(&db.store, player).await,
        ["contest_asked", "confirmed"]
    );
    let second = create(
        &ctx,
        &draft(PrizeDraft::Balance { amount: "1".into() }, 1),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        join(&ctx, second.id, player).await.unwrap(),
        JoinOutcome::Joined
    );
    let picky = tg_client(&db.store, 9312).await;
    suspect(&db.store, picky).await;
    let mut strict = draft(PrizeDraft::Balance { amount: "1".into() }, 1);
    strict.conditions.subscription = true;
    let strict = create(&ctx, &strict, None).await.unwrap();
    assert_eq!(
        join(&ctx, strict.id, picky).await.unwrap(),
        JoinOutcome::NotEligible
    );
    assert!(antibot_actions(&db.store, picky).await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn отметка_посреди_конкурса_исключает_участника_на_жеребьёвке() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    antibot(&db.store, guarded("block")).await;
    let caught = tg_client(&db.store, 9321).await;
    let forgiven = tg_client(&db.store, 9322).await;
    let honest = tg_client(&db.store, 9323).await;
    let created = create(
        &ctx,
        &draft(PrizeDraft::Balance { amount: "2".into() }, 3),
        None,
    )
    .await
    .unwrap();
    for player in [caught, forgiven, honest] {
        assert_eq!(
            join(&ctx, created.id, player).await.unwrap(),
            JoinOutcome::Joined
        );
    }
    suspect(&db.store, caught).await;
    suspect(&db.store, forgiven).await;
    crate::catalog::antibot::clear(&ctx, forgiven, None)
        .await
        .unwrap();
    finish_now(&db.store, created.id).await;
    let report = run(&ctx).await.unwrap();
    assert_eq!(report.drawn, 1);
    let done = contest(&ctx, created.id).await.unwrap();
    assert_eq!(done.entries_drawn, Some(2));
    assert_eq!(done.counts.excluded, 1);
    let mut winners: Vec<ClientId> = done.winners_list.iter().map(|w| w.client_id).collect();
    winners.sort();
    assert_eq!(winners, [forgiven, honest]);
    assert_eq!(balance(&db.store, caught).await, 0);
    assert_eq!(balance(&db.store, forgiven).await, 200);
    let out = entries(
        &ctx,
        created.id,
        EntryFilter::Excluded,
        PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].client_id, caught);
    assert_eq!(out.items[0].excluded.as_deref(), Some(ANTIBOT_EXCLUDED));
    assert!(done.log.iter().any(|l| l.kind == "antibot"
        && l.text == format!("Не прошли проверку антибота — 1: клиент #{}", caught.0)));
    assert!(
        done.log
            .iter()
            .any(|l| l.kind == "draw" && l.text.contains("не прошли проверку — 1"))
    );
    let seen = client_view(&ctx, created.id, caught)
        .await
        .unwrap()
        .unwrap();
    let result = seen.result.unwrap();
    assert!(!result.won);
    assert_eq!(result.excluded.as_deref(), Some(UNAVAILABLE));
    assert_eq!(
        antibot_actions(&db.store, caught).await,
        ["contest_excluded"]
    );
    assert_eq!(antibot_actions(&db.store, forgiven).await, ["cleared"]);

    antibot(&db.store, guarded("confirm")).await;
    let late = tg_client(&db.store, 9324).await;
    let calm = create(
        &ctx,
        &draft(PrizeDraft::Balance { amount: "1".into() }, 1),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        join(&ctx, calm.id, late).await.unwrap(),
        JoinOutcome::Joined
    );
    suspect(&db.store, late).await;
    finish_now(&db.store, calm.id).await;
    run(&ctx).await.unwrap();
    let calm = contest(&ctx, calm.id).await.unwrap();
    assert_eq!(calm.entries_drawn, Some(1));
    assert_eq!(calm.counts.excluded, 0);
    assert_eq!(calm.winners_list[0].client_id, late);
    db.cleanup().await;
}

#[tokio::test]
async fn в_зачёт_друзей_не_идут_отмеченные_и_самоприглашённые() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    antibot(&db.store, guarded("block")).await;
    let host = tg_client(&db.store, 9331).await;
    let friend = tg_client(&db.store, 9332).await;
    let marked = tg_client(&db.store, 9333).await;
    let twin = tg_client(&db.store, 9334).await;
    let fresh = tg_client(&db.store, 9335).await;
    let mut new = draft(PrizeDraft::Balance { amount: "1".into() }, 1);
    new.conditions.friends = 4;
    let created = create(&ctx, &new, None).await.unwrap();
    for guest in [friend, marked, twin, fresh] {
        invite(&db.store, guest, host).await;
    }
    suspect(&db.store, marked).await;
    session(&db.store, host, "198.51.100.20").await;
    session(&db.store, twin, "198.51.100.20").await;
    session(&db.store, friend, "203.0.113.5").await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.clients()
            .add_identity(
                fresh,
                &NewIdentity::new(IdentityKind::Email, "fresh@mailinator.com", true).unwrap(),
            )
            .await
            .unwrap();
    }
    let line = |c: &ClientContest| c.checks[0].text.clone();
    let seen = async |ctx: &ModuleContext| {
        line(&client_view(ctx, created.id, host).await.unwrap().unwrap())
    };
    assert_eq!(
        seen(&ctx).await,
        "Пригласить 4 друзей за время конкурса: 1 из 4"
    );
    assert_eq!(
        join(&ctx, created.id, host).await.unwrap(),
        JoinOutcome::NotEligible
    );
    {
        let mut conn = db.store.conn().await.unwrap();
        let mark = crate::catalog::antibot::store::mark_of(&mut conn, twin)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            mark.reasons,
            [crate::catalog::antibot::signals::Signal::SelfInvite]
        );
        assert_eq!(mark.details["referrer_id"], host.0);
        let assessed = crate::catalog::antibot::store::mark_of(&mut conn, fresh)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            assessed.reasons,
            [crate::catalog::antibot::signals::Signal::DisposableEmail]
        );
        assert!(
            crate::catalog::antibot::store::mark_of(&mut conn, friend)
                .await
                .unwrap()
                .is_none()
        );
    }

    antibot(&db.store, guarded("confirm")).await;
    assert_eq!(
        seen(&ctx).await,
        "Пригласить 4 друзей за время конкурса: 1 из 4"
    );
    let confirm = async |client: ClientId| {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("UPDATE antibot_marks SET confirmed_at = now() WHERE client_id = $1")
            .bind(client.0)
            .execute(conn.conn())
            .await
            .unwrap();
    };
    confirm(marked).await;
    assert_eq!(
        seen(&ctx).await,
        "Пригласить 4 друзей за время конкурса: 2 из 4"
    );

    antibot(&db.store, guarded("mark")).await;
    assert_eq!(
        seen(&ctx).await,
        "Пригласить 4 друзей за время конкурса: 3 из 4"
    );

    let mut off = guarded("block");
    off["contest_guard"] = serde_json::json!(false);
    antibot(&db.store, off).await;
    assert_eq!(
        seen(&ctx).await,
        "Пригласить 4 друзей за время конкурса: 4 из 4"
    );

    antibot(&db.store, guarded("confirm")).await;
    confirm(fresh).await;
    crate::catalog::antibot::clear(&ctx, twin, None)
        .await
        .unwrap();
    assert_eq!(
        join(&ctx, created.id, host).await.unwrap(),
        JoinOutcome::Joined
    );
    finish_now(&db.store, created.id).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query(
            "UPDATE referral_links SET created_at = now() - interval '30 minutes' WHERE referrer_id = $1",
        )
        .bind(host.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    run(&ctx).await.unwrap();
    let done = contest(&ctx, created.id).await.unwrap();
    assert_eq!(done.entries_drawn, Some(1));
    assert_eq!(done.winners_list[0].client_id, host);
    db.cleanup().await;
}

#[tokio::test]
async fn призы_ждущие_подписку_не_задерживают_выдачу_остальных() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let days = create(&ctx, &draft(PrizeDraft::Days { days: 10 }, 50), None)
        .await
        .unwrap();
    for n in 0..50 {
        let waiting = tg_client(&db.store, 9300 + n).await;
        join(&ctx, days.id, waiting).await.unwrap();
    }
    let money = create(
        &ctx,
        &draft(PrizeDraft::Balance { amount: "5".into() }, 1),
        None,
    )
    .await
    .unwrap();
    let lucky = tg_client(&db.store, 9400).await;
    join(&ctx, money.id, lucky).await.unwrap();
    finish_now(&db.store, days.id).await;
    finish_now(&db.store, money.id).await;
    run(&ctx).await.unwrap();
    run(&ctx).await.unwrap();
    let view = contest(&ctx, money.id).await.unwrap();
    assert_eq!(
        view.winners_list
            .iter()
            .find(|w| w.client_id == lucky)
            .and_then(|w| w.prize_state.clone())
            .as_deref(),
        Some("issued")
    );
    assert_eq!(balance(&db.store, lucky).await, 500);
    db.cleanup().await;
}

#[tokio::test]
async fn оплаченной_считается_только_купленная_или_подаренная_подписка() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut paid_only = draft(PrizeDraft::Days { days: 10 }, 1);
    paid_only.conditions.subscription = true;
    paid_only.conditions.paid_only = true;
    let created = create(&ctx, &paid_only, None).await.unwrap();
    let mut outcomes = Vec::new();
    for (n, kind) in [
        SubscriptionKind::Paid,
        SubscriptionKind::Gift,
        SubscriptionKind::Promo,
        SubscriptionKind::Manual,
        SubscriptionKind::Custom,
        SubscriptionKind::Trial,
    ]
    .into_iter()
    .enumerate()
    {
        let member = tg_client(&db.store, 9_300 + n as i64).await;
        live_sub(&db.store, member, kind).await;
        outcomes.push(join(&ctx, created.id, member).await.unwrap());
    }
    assert_eq!(
        outcomes,
        [
            JoinOutcome::Joined,
            JoinOutcome::Joined,
            JoinOutcome::NotEligible,
            JoinOutcome::NotEligible,
            JoinOutcome::NotEligible,
            JoinOutcome::NotEligible,
        ]
    );
    db.cleanup().await;
}
