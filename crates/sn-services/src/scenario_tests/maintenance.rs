use super::*;
use crate::admins::NewAdminRequest;
use crate::maintenance::{MaintenanceUpdate, TelegramUpdate};
use chrono::FixedOffset;
use sn_domain::access::AdminRole;
use sn_domain::client::Locale;
use sn_domain::maintenance::{MESSAGE_MAX, standard};

fn moscow(day: u32, hour: u32) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(3 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 11, day, hour, 0, 0)
        .unwrap()
}

async fn journal(env: &Env) -> Vec<(String, serde_json::Value)> {
    let mut conn = env.db.store.conn().await.unwrap();
    sqlx::query_as(
        "SELECT action, details FROM audit_log WHERE action LIKE 'settings.maintenance%' \
         OR action = 'admin.telegram' ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn notices(env: &Env) -> i64 {
    let mut conn = env.db.store.conn().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM bot_maintenance_notices")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn режим_работ_включается_с_текстом_и_временем_и_пишется_в_журнал() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.maintenance();
    assert!(!ops.active());
    assert_eq!(ops.public(), None);
    assert!(!services.settings().maintenance.enabled);

    let view = ops
        .update(
            &MaintenanceUpdate {
                enabled: true,
                message_ru: Some("  Обновляем серверы  \n".into()),
                message_en: Some("   ".into()),
                until: Some(moscow(16, 20)),
            },
            admin,
        )
        .await
        .unwrap();
    assert!(view.settings.enabled);
    assert_eq!(
        view.settings.message_ru.as_deref(),
        Some("Обновляем серверы")
    );
    assert_eq!(view.settings.message_en, None);
    assert_eq!(view.settings.offset_minutes, 180);
    assert_eq!(view.settings.since, Some(start()));
    assert!(ops.active());
    assert_eq!(
        ops.text(Locale::Ru),
        "Обновляем серверы\n\nЗакончим примерно 16\u{a0}ноября в\u{a0}20:00 (UTC+3)."
    );
    assert!(ops.text(Locale::En).starts_with(standard(Locale::En)));
    let public = ops.public().unwrap();
    assert_eq!(public["message"]["ru"], "Обновляем серверы");
    assert_eq!(public["message"]["en"], standard(Locale::En));
    assert_eq!(public["until"], "2026-11-16T17:00:00Z");

    let same = ops
        .update(
            &MaintenanceUpdate {
                enabled: true,
                message_ru: Some("Обновляем серверы".into()),
                message_en: None,
                until: Some(moscow(16, 20)),
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(same.settings, view.settings);

    env.clock.set(start() + Duration::minutes(30));
    let edited = ops
        .update(
            &MaintenanceUpdate {
                enabled: true,
                message_ru: Some("Обновляем серверы".into()),
                message_en: Some("Updating servers".into()),
                until: None,
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(edited.settings.since, Some(start()));
    assert_eq!(edited.settings.until, None);
    assert_eq!(ops.text(Locale::En), "Updating servers");

    sqlx::query("INSERT INTO bot_maintenance_notices (chat_id) VALUES (1), (2)")
        .execute(env.db.store.conn().await.unwrap().conn())
        .await
        .unwrap();
    let off = ops
        .update(
            &MaintenanceUpdate {
                enabled: false,
                message_ru: Some("Обновляем серверы".into()),
                message_en: Some("Updating servers".into()),
                until: None,
            },
            admin,
        )
        .await
        .unwrap();
    assert!(!off.settings.enabled);
    assert_eq!(off.settings.since, None);
    assert_eq!(
        off.settings.message_ru.as_deref(),
        Some("Обновляем серверы")
    );
    assert_eq!(ops.public(), None);
    assert_eq!(notices(&env).await, 0, "отметки прошлых работ не сброшены");

    let entries = journal(&env).await;
    let actions: Vec<&str> = entries.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(
        actions,
        [
            "settings.maintenance_on",
            "settings.maintenance",
            "settings.maintenance_off"
        ]
    );
    assert_eq!(entries[0].1["from"]["message_ru"], serde_json::Value::Null);
    assert_eq!(entries[0].1["to"]["message_ru"], "Обновляем серверы");
    assert_eq!(entries[0].1["to"]["until"], "16.11.2026 20:00 (UTC+3)");
    assert_eq!(entries[1].1["to"]["message_en"], "Updating servers");
    assert_eq!(entries[1].1["from"]["until"], "16.11.2026 20:00 (UTC+3)");
    assert_eq!(entries[1].1["to"]["until"], serde_json::Value::Null);
    let stored: serde_json::Value = {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.settings()
            .get(keys::MAINTENANCE)
            .await
            .unwrap()
            .unwrap()
    };
    assert_eq!(stored["enabled"], false);
}

#[tokio::test]
async fn режим_работ_проверяет_поля() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.maintenance();
    let past = ops
        .update(
            &MaintenanceUpdate {
                enabled: true,
                until: Some(moscow(16, 17)),
                ..MaintenanceUpdate::default()
            },
            admin,
        )
        .await;
    assert_eq!(code(past), "maintenance.invalid");
    let long = ops
        .update(
            &MaintenanceUpdate {
                enabled: true,
                message_en: Some("a".repeat(MESSAGE_MAX + 1)),
                ..MaintenanceUpdate::default()
            },
            admin,
        )
        .await;
    assert_eq!(code(long), "maintenance.invalid");
    assert!(!ops.active());
    assert!(journal(&env).await.is_empty());

    ops.update(
        &MaintenanceUpdate {
            enabled: true,
            until: Some(moscow(16, 20)),
            ..MaintenanceUpdate::default()
        },
        admin,
    )
    .await
    .unwrap();
    env.clock.set(start() + Duration::hours(6));
    assert_eq!(ops.text(Locale::Ru), standard(Locale::Ru));
    assert_eq!(ops.public().unwrap()["until"], serde_json::Value::Null);
    let kept = ops
        .update(
            &MaintenanceUpdate {
                enabled: true,
                message_ru: Some("Ещё немного".into()),
                until: Some(moscow(16, 20)),
                ..MaintenanceUpdate::default()
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(kept.settings.message_ru.as_deref(), Some("Ещё немного"));
}

#[tokio::test]
async fn бот_пишет_чату_о_работах_не_чаще_раза_в_десять_минут() {
    let Some(env) = env().await else { return };
    let ops = env.services.maintenance();
    assert!(ops.claim_notice(501).await.unwrap());
    assert!(!ops.claim_notice(501).await.unwrap());
    assert!(ops.claim_notice(502).await.unwrap());
    let mut conn = env.db.store.conn().await.unwrap();
    sqlx::query(
        "UPDATE bot_maintenance_notices SET sent_at = sent_at - interval '9 minutes' WHERE chat_id = 501",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    assert!(!ops.claim_notice(501).await.unwrap());
    sqlx::query(
        "UPDATE bot_maintenance_notices SET sent_at = sent_at - interval '2 minutes' WHERE chat_id = 501",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    assert!(ops.claim_notice(501).await.unwrap());
    assert!(!ops.claim_notice(501).await.unwrap());
    assert!(!ops.claim_notice(502).await.unwrap());
}

#[tokio::test]
async fn telegram_id_администратора_пускает_в_бот_во_время_работ() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.maintenance();
    for wrong in [0, -5, 1 << 52] {
        let refused = ops
            .set_telegram(
                admin,
                &TelegramUpdate {
                    telegram_id: Some(wrong),
                },
            )
            .await;
        assert_eq!(code(refused), "admins.telegram_invalid");
    }
    assert!(!ops.lets_through(700_100).await.unwrap());
    assert_eq!(
        ops.set_telegram(
            admin,
            &TelegramUpdate {
                telegram_id: Some(700_100)
            }
        )
        .await
        .unwrap(),
        Some(700_100)
    );
    assert!(ops.lets_through(700_100).await.unwrap());
    assert!(!ops.lets_through(700_101).await.unwrap());

    let issued = services
        .admins()
        .create(
            admin,
            &NewAdminRequest {
                email: "support@example.com".into(),
                role: AdminRole::Support,
            },
        )
        .await
        .unwrap();
    let helper = issued.admin.id;
    ops.set_telegram(
        helper,
        &TelegramUpdate {
            telegram_id: Some(700_200),
        },
    )
    .await
    .unwrap();
    let view = ops.view(admin).await.unwrap();
    assert_eq!(view.telegram_id, Some(700_100));
    assert_eq!(view.testers, 1);
    assert!(ops.lets_through(700_200).await.unwrap());
    services.admins().disable(admin, helper).await.unwrap();
    assert!(!ops.lets_through(700_200).await.unwrap());
    assert_eq!(ops.view(admin).await.unwrap().testers, 0);

    assert_eq!(
        ops.set_telegram(admin, &TelegramUpdate { telegram_id: None })
            .await
            .unwrap(),
        None
    );
    assert!(!ops.lets_through(700_100).await.unwrap());
    let entries = journal(&env).await;
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|(action, _)| action == "admin.telegram"));
    assert_eq!(entries[0].1["to"]["telegram_id"], 700_100);
    assert_eq!(entries[2].1["from"]["telegram_id"], 700_100);
    assert_eq!(entries[2].1["to"]["telegram_id"], serde_json::Value::Null);
}
