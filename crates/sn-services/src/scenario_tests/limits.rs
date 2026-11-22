use crate::Services;
use crate::checkout::testkit::{Kit, client, kit_with, squad};
use chrono::{DateTime, TimeZone, Utc};
use sn_domain::alerts::AlertKind;
use sn_domain::ids::{ClientId, RemnaUserId, SubscriptionId};
use sn_domain::module::ModuleId;
use sn_domain::subscription::Subscription;
use sn_domain::traffic::GIB;
use sn_modules::Module;
use sn_modules::catalog::limits::{
    self, Audience, LIFT_TASK, LimitsModule, Period, Restriction, RuleInput, TrafficScope,
};
use sn_remnawave::UserStatus;
use sn_store::Db;
use std::sync::Arc;

async fn limits_kit() -> Option<Kit> {
    let kit = kit_with(vec![Arc::new(LimitsModule)]).await?;
    kit.services
        .modules()
        .set_enabled(kit.services.store(), ModuleId::Limits, true, None)
        .await
        .unwrap();
    Some(kit)
}

async fn trial_subscription(kit: &Kit, telegram_id: i64) -> (ClientId, Subscription) {
    let services = &kit.services;
    let client_id = client(services, telegram_id).await;
    let order = services
        .checkout()
        .start_trial(client_id, None)
        .await
        .unwrap();
    services.fulfillment().run(order).await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    let subscription = conn
        .subscriptions()
        .by_client(client_id)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.ended_at.is_none())
        .unwrap();
    (client_id, subscription)
}

fn user_of(subscription: &Subscription) -> RemnaUserId {
    subscription.remna.as_ref().unwrap().user_id
}

fn set_lifetime(kit: &Kit, user: RemnaUserId, bytes: u64) {
    let mut found = kit.remna.user(user).unwrap();
    found.user_traffic.lifetime_used_traffic_bytes = i64::try_from(bytes).unwrap();
    kit.remna.insert_user(found);
}

fn rule(period: Period, gb: f64, restriction: Restriction) -> RuleInput {
    RuleInput {
        enabled: true,
        audience: Audience::All,
        traffic: TrafficScope::Any,
        devices_min: None,
        period,
        limit_gb: gb,
        per_device: false,
        warn: true,
        alert: true,
        restriction,
        squad: match restriction {
            Restriction::Squad => Some(squad(3).0),
            _ => None,
        },
        note: None,
    }
}

async fn add_rule(services: &Services, input: RuleInput) -> i64 {
    let rule = limits::validate(&input, Utc::now()).unwrap();
    let mut conn = services.store().conn().await.unwrap();
    limits::store::save_rule(&mut conn, None, &rule, None)
        .await
        .unwrap()
        .unwrap()
        .id
}

async fn notices(services: &Services, client_id: ClientId) -> Vec<(String, serde_json::Value)> {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT payload ->> 'template', payload -> 'params' FROM jobs \
         WHERE kind = 'notify' AND (payload ->> 'client_id')::bigint = $1 ORDER BY id",
    )
    .bind(client_id.0)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

#[derive(sqlx::FromRow)]
struct ExceedRow {
    id: i64,
    used_bytes: i64,
    restricted_at: Option<DateTime<Utc>>,
    lift_due_at: Option<DateTime<Utc>>,
    lifted_at: Option<DateTime<Utc>>,
    lift_reason: Option<String>,
}

async fn exceeds(services: &Services, subscription: SubscriptionId) -> Vec<ExceedRow> {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT id, used_bytes, restricted_at, lift_due_at, lifted_at, lift_reason FROM traffic_exceeds \
         WHERE subscription_id = $1 ORDER BY id",
    )
    .bind(subscription.0)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, day, hour, minute, 0)
        .unwrap()
}

#[tokio::test]
async fn превышение_за_сутки_отключает_до_полуночи_и_включает_само() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (client_id, subscription) = trial_subscription(&kit, 7_001).await;
    let user = user_of(&subscription);
    add_rule(services, rule(Period::Day, 10.0, Restriction::Disable)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, GIB);
    let first = limits::scan_at(&ctx, at(10, 10, 0)).await.unwrap();
    assert_eq!(first.subscriptions, 1);
    assert_eq!(first.exceeded, 0);
    set_lifetime(&kit, user, 12 * GIB);
    let second = limits::scan_at(&ctx, at(10, 10, 10)).await.unwrap();
    assert_eq!(second.exceeded, 1);
    assert_eq!(second.restricted, 1);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Disabled);
    assert_eq!(kit.remna.calls_of("drop_connections").len(), 1);
    let rows = exceeds(services, subscription.id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].used_bytes, i64::try_from(11 * GIB).unwrap());
    assert!(rows[0].restricted_at.is_some());
    assert_eq!(rows[0].lift_due_at, Some(at(11, 0, 0)));
    let sent = notices(services, client_id).await;
    let restricted: Vec<_> = sent
        .iter()
        .filter(|(t, _)| t == limits::TEMPLATE_RESTRICTED)
        .collect();
    assert_eq!(restricted.len(), 1);
    let params = &restricted[0].1;
    assert_eq!(params["used"], "11\u{a0}ГБ");
    assert_eq!(params["limit"], "10\u{a0}ГБ");
    assert_eq!(params["until"], "11.11 в 00:00");
    assert_eq!(params["also_mail"], true);
    set_lifetime(&kit, user, 13 * GIB);
    let third = limits::scan_at(&ctx, at(10, 10, 20)).await.unwrap();
    assert_eq!(third.exceeded, 0);
    assert_eq!(
        exceeds(services, subscription.id).await[0].used_bytes,
        i64::try_from(12 * GIB).unwrap()
    );
    let early = limits::lift_due(&ctx, at(10, 23, 50), false).await.unwrap();
    assert_eq!(early.lifted, 0);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Disabled);
    let late = limits::lift_due(&ctx, at(11, 0, 2), false).await.unwrap();
    assert_eq!(late.lifted, 1);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Active);
    let row = &exceeds(services, subscription.id).await[0];
    assert_eq!(row.lift_reason.as_deref(), Some("window"));
    assert!(row.lifted_at.is_some());
    assert!(
        notices(services, client_id)
            .await
            .iter()
            .any(|(t, _)| t == limits::TEMPLATE_LIFTED)
    );
    let mirror = {
        let mut conn = services.store().conn().await.unwrap();
        conn.subscriptions()
            .get(subscription.id)
            .await
            .unwrap()
            .mirror
    };
    assert_eq!(
        mirror.status,
        Some(sn_domain::subscription::SubscriptionStatus::Active)
    );
    set_lifetime(&kit, user, 14 * GIB);
    let next_day = limits::scan_at(&ctx, at(11, 0, 5)).await.unwrap();
    assert_eq!(next_day.exceeded, 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn недельный_порог_только_предупреждает_и_шлёт_уведомление_админам() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (client_id, subscription) = trial_subscription(&kit, 7_002).await;
    let user = user_of(&subscription);
    let mut input = rule(Period::Week, 30.0, Restriction::None);
    input.audience = Audience::Trial;
    add_rule(services, input).await;
    let mut paid_only = rule(Period::Week, 1.0, Restriction::Disable);
    paid_only.audience = Audience::Paid;
    add_rule(services, paid_only).await;
    assert!(services.alerts().collect().await.unwrap().is_empty());
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(9, 9, 0)).await.unwrap();
    set_lifetime(&kit, user, 31 * GIB);
    let report = limits::scan_at(&ctx, at(12, 18, 0)).await.unwrap();
    assert_eq!(report.exceeded, 1);
    assert_eq!(report.restricted, 0);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Active);
    let sent = notices(services, client_id).await;
    let warning = sent
        .iter()
        .find(|(t, _)| t == limits::TEMPLATE_WARNING)
        .unwrap();
    assert_eq!(warning.1["when"], "На этой неделе");
    assert_eq!(warning.1["per"], "в неделю");
    let alerts = services.alerts().collect().await.unwrap();
    let traffic: Vec<_> = alerts
        .iter()
        .filter(|a| a.kind == AlertKind::Traffic)
        .collect();
    assert_eq!(traffic.len(), 1, "{alerts:?}");
    assert!(
        traffic[0]
            .html
            .contains("За неделю: 31\u{a0}ГБ при пороге 30\u{a0}ГБ")
    );
    assert!(traffic[0].html.contains("Клиент предупреждён"));
    assert_eq!(traffic[0].path, "/admin/clients/limits");
    assert!(services.alerts().collect().await.unwrap().is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заблокированного_клиента_не_включают_а_выключенный_модуль_снимает_всё() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (first_client, first) = trial_subscription(&kit, 7_003).await;
    let (_, second) = trial_subscription(&kit, 7_004).await;
    add_rule(services, rule(Period::Day, 5.0, Restriction::Disable)).await;
    let ctx = services.module_context();
    for sub in [&first, &second] {
        set_lifetime(&kit, user_of(sub), 0);
    }
    limits::scan_at(&ctx, at(10, 8, 0)).await.unwrap();
    for sub in [&first, &second] {
        set_lifetime(&kit, user_of(sub), 6 * GIB);
    }
    let report = limits::scan_at(&ctx, at(10, 8, 10)).await.unwrap();
    assert_eq!(report.restricted, 2);
    limits::scan_at(&ctx, at(10, 23, 50)).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE clients SET status = 'blocked', blocked_at = now() WHERE id = $1")
            .bind(first_client.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let lifted = limits::lift_due(&ctx, at(11, 0, 1), false).await.unwrap();
    assert_eq!(lifted.lifted, 2);
    assert_eq!(
        kit.remna.user(user_of(&first)).unwrap().status,
        UserStatus::Disabled
    );
    assert_eq!(
        exceeds(services, first.id).await[0].lift_reason.as_deref(),
        Some("blocked")
    );
    assert_eq!(
        kit.remna.user(user_of(&second)).unwrap().status,
        UserStatus::Active
    );
    set_lifetime(&kit, user_of(&second), 20 * GIB);
    limits::scan_at(&ctx, at(11, 9, 0)).await.unwrap();
    assert_eq!(
        kit.remna.user(user_of(&second)).unwrap().status,
        UserStatus::Disabled
    );
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Limits, false, None)
        .await
        .unwrap();
    LimitsModule.run_task(&ctx, LIFT_TASK).await.unwrap();
    assert_eq!(
        kit.remna.user(user_of(&second)).unwrap().status,
        UserStatus::Active
    );
    let rows = exceeds(services, second.id).await;
    assert_eq!(rows.last().unwrap().lift_reason.as_deref(), Some("off"));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn перевод_в_сквад_и_возврат_прежних_сквадов() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (_, subscription) = trial_subscription(&kit, 7_005).await;
    let user = user_of(&subscription);
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(9)]);
    let id = add_rule(services, rule(Period::Day, 2.0, Restriction::Squad)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(10, 12, 0)).await.unwrap();
    set_lifetime(&kit, user, 3 * GIB);
    let report = limits::scan_at(&ctx, at(10, 12, 10)).await.unwrap();
    assert_eq!(report.restricted, 1);
    let restricted = kit.remna.user(user).unwrap();
    assert_eq!(restricted.status, UserStatus::Active);
    assert_eq!(restricted.squad_ids(), vec![squad(3)]);
    let exceed = exceeds(services, subscription.id).await.remove(0);
    let admin = crate::scenario_tests::owner(services).await;
    let lifted = limits::lift_by_admin(&ctx, exceed.id, admin).await.unwrap();
    assert_eq!(lifted.lift_reason.as_deref(), Some("admin"));
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(9)]);
    assert!(limits::lift_by_admin(&ctx, exceed.id, admin).await.is_err());
    let mut conn = services.store().conn().await.unwrap();
    assert!(limits::store::delete_rule(&mut conn, id).await.unwrap());
    assert!(
        limits::store::rules(&mut conn, false)
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn сбой_панели_при_ограничении_повторяется_в_следующую_проверку() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (_, subscription) = trial_subscription(&kit, 7_006).await;
    let user = user_of(&subscription);
    add_rule(services, rule(Period::Day, 1.0, Restriction::Disable)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(10, 15, 0)).await.unwrap();
    set_lifetime(&kit, user, 2 * GIB);
    kit.remna
        .fail_on("disable_user", sn_remnawave::fake::Fault::ServerError);
    let report = limits::scan_at(&ctx, at(10, 15, 10)).await.unwrap();
    assert_eq!(report.exceeded, 1);
    assert_eq!(report.restricted, 0);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Active);
    let retry = limits::scan_at(&ctx, at(10, 15, 20)).await.unwrap();
    assert_eq!(retry.restricted, 1);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Disabled);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn общее_ограничение_двух_правил_держится_до_последнего_окна() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (_, subscription) = trial_subscription(&kit, 7_007).await;
    let user = user_of(&subscription);
    add_rule(services, rule(Period::Day, 5.0, Restriction::Disable)).await;
    add_rule(services, rule(Period::Week, 8.0, Restriction::Disable)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(10, 9, 0)).await.unwrap();
    set_lifetime(&kit, user, 9 * GIB);
    let report = limits::scan_at(&ctx, at(10, 9, 10)).await.unwrap();
    assert_eq!(report.exceeded, 2);
    assert_eq!(report.restricted, 2);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Disabled);
    let rows = exceeds(services, subscription.id).await;
    assert_eq!(kit.remna.calls_of("disable_user").len(), 1);
    let day = limits::lift_due(&ctx, at(11, 0, 1), false).await.unwrap();
    assert_eq!(day.lifted, 1);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Disabled);
    let admin = crate::scenario_tests::owner(services).await;
    let week = rows
        .iter()
        .find(|r| r.lifted_at.is_none() && r.lift_due_at == Some(at(16, 0, 0)))
        .unwrap();
    limits::lift_by_admin(&ctx, week.id, admin).await.unwrap();
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Active);
    assert!(
        exceeds(services, subscription.id)
            .await
            .iter()
            .all(|r| r.lifted_at.is_some())
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn кнопка_админа_снимает_все_ограничения_подписки() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (_, subscription) = trial_subscription(&kit, 7_008).await;
    let user = user_of(&subscription);
    add_rule(services, rule(Period::Day, 5.0, Restriction::Disable)).await;
    add_rule(services, rule(Period::Week, 8.0, Restriction::Disable)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(10, 9, 0)).await.unwrap();
    set_lifetime(&kit, user, 9 * GIB);
    limits::scan_at(&ctx, at(10, 9, 10)).await.unwrap();
    let rows = exceeds(services, subscription.id).await;
    assert_eq!(rows.len(), 2);
    let admin = crate::scenario_tests::owner(services).await;
    limits::lift_by_admin(&ctx, rows[0].id, admin)
        .await
        .unwrap();
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Active);
    let after = exceeds(services, subscription.id).await;
    assert!(
        after
            .iter()
            .all(|r| r.lift_reason.as_deref() == Some("admin"))
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn два_правила_со_сквадом_в_одном_проходе_держат_ограничение_до_последнего_окна() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (_, subscription) = trial_subscription(&kit, 7_020).await;
    let user = user_of(&subscription);
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(9)]);
    add_rule(services, rule(Period::Day, 5.0, Restriction::Squad)).await;
    add_rule(services, rule(Period::Week, 8.0, Restriction::Squad)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(10, 9, 0)).await.unwrap();
    set_lifetime(&kit, user, 9 * GIB);
    let report = limits::scan_at(&ctx, at(10, 9, 10)).await.unwrap();
    assert_eq!(report.restricted, 2);
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(3)]);
    limits::lift_due(&ctx, at(11, 0, 1), false).await.unwrap();
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(3)]);
    limits::lift_due(&ctx, at(16, 0, 1), false).await.unwrap();
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(9)]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn суточный_сквад_поверх_недельного_не_оставляет_подписку_без_сквадов() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (_, subscription) = trial_subscription(&kit, 7_021).await;
    let user = user_of(&subscription);
    add_rule(services, rule(Period::Day, 5.0, Restriction::Squad)).await;
    add_rule(services, rule(Period::Week, 8.0, Restriction::Squad)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(10, 9, 0)).await.unwrap();
    set_lifetime(&kit, user, 4 * GIB);
    limits::scan_at(&ctx, at(10, 23, 58)).await.unwrap();
    set_lifetime(&kit, user, 17 * GIB / 2);
    let week = limits::scan_at(&ctx, at(11, 23, 58)).await.unwrap();
    assert_eq!(week.restricted, 1);
    set_lifetime(&kit, user, 14 * GIB);
    let day = limits::scan_at(&ctx, at(12, 23, 58)).await.unwrap();
    assert_eq!(day.restricted, 1);
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(3)]);
    limits::lift_due(&ctx, at(13, 0, 1), false).await.unwrap();
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(3)]);
    limits::lift_due(&ctx, at(16, 0, 1), false).await.unwrap();
    assert_eq!(kit.remna.user(user).unwrap().squad_ids(), vec![squad(9)]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отключение_срабатывает_и_поверх_ограничения_сквадом() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (_, subscription) = trial_subscription(&kit, 7_022).await;
    let user = user_of(&subscription);
    add_rule(services, rule(Period::Day, 5.0, Restriction::Squad)).await;
    add_rule(services, rule(Period::Week, 8.0, Restriction::Disable)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(10, 9, 0)).await.unwrap();
    set_lifetime(&kit, user, 9 * GIB);
    let report = limits::scan_at(&ctx, at(10, 9, 10)).await.unwrap();
    assert_eq!(report.restricted, 2);
    let restricted = kit.remna.user(user).unwrap();
    assert_eq!(restricted.status, UserStatus::Disabled);
    assert_eq!(restricted.squad_ids(), vec![squad(3)]);
    limits::lift_due(&ctx, at(11, 0, 1), false).await.unwrap();
    let night = kit.remna.user(user).unwrap();
    assert_eq!(night.status, UserStatus::Disabled);
    assert_eq!(night.squad_ids(), vec![squad(9)]);
    limits::lift_due(&ctx, at(16, 0, 1), false).await.unwrap();
    let after = kit.remna.user(user).unwrap();
    assert_eq!(after.status, UserStatus::Active);
    assert_eq!(after.squad_ids(), vec![squad(9)]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn превышение_без_ограничения_после_перезапуска_ограничивается_следующей_проверкой() {
    let Some(kit) = limits_kit().await else {
        return;
    };
    let services = &kit.services;
    let (client_id, subscription) = trial_subscription(&kit, 7_023).await;
    let user = user_of(&subscription);
    let rule_id = add_rule(services, rule(Period::Day, 5.0, Restriction::Disable)).await;
    let ctx = services.module_context();
    set_lifetime(&kit, user, 0);
    limits::scan_at(&ctx, at(10, 9, 0)).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        limits::store::upsert_exceed(
            &mut conn,
            &limits::store::NewExceed {
                rule: rule_id,
                subscription_id: subscription.id,
                client_id,
                period: Period::Day,
                window_start: chrono::NaiveDate::from_ymd_opt(2026, 11, 10).unwrap(),
                window_end: at(11, 0, 0),
                limit_bytes: 5 * GIB,
                used_bytes: 6 * GIB,
                warn: true,
                alert: true,
                restriction: Restriction::Disable,
                squad: None,
                at: at(10, 9, 5),
            },
        )
        .await
        .unwrap();
    }
    set_lifetime(&kit, user, 6 * GIB);
    let report = limits::scan_at(&ctx, at(10, 9, 10)).await.unwrap();
    assert_eq!(report.restricted, 1);
    assert_eq!(kit.remna.user(user).unwrap().status, UserStatus::Disabled);
    kit.db.cleanup().await;
}
