use super::*;
use crate::alerts::{Alert, AlertsUpdate, BATCH_LINES, RouteInput};
use sn_domain::alerts::AlertKind;
use sn_domain::payment::Payment;

async fn settle(services: &Services) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query("UPDATE domain_events SET occurred_at = occurred_at - interval '1 minute'")
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn collected(services: &Services, kind: AlertKind) -> Vec<Alert> {
    settle(services).await;
    services
        .alerts()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .filter(|alert| alert.kind == kind)
        .collect()
}

async fn joined(
    services: &Services,
    id: i64,
    source: RegistrationSource,
    start: Option<&str>,
) -> ClientId {
    services
        .clients()
        .ensure_telegram(&profile(id), source, start)
        .await
        .unwrap()
        .0
        .id
}

async fn route(services: &Services, kind: AlertKind, on: bool) {
    let admin = owner(services).await;
    services
        .alerts()
        .update(
            &AlertsUpdate {
                routes: vec![RouteInput {
                    kind,
                    on,
                    topic: None,
                }],
            },
            admin,
        )
        .await
        .unwrap();
}

fn with_public_url(services: &Services) {
    let mut settings = (*services.settings()).clone();
    settings.public_url = Some("https://panel.example.com".into());
    services.replace_settings(settings);
}

async fn paid(services: &Services, payment: &Payment) {
    let admin = owner(services).await;
    services
        .admin()
        .mark_paid(payment.id, &MarkPaidRequest::default(), admin)
        .await
        .unwrap();
}

async fn exec(services: &Services, sql: &str, id: i64) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(sql)
        .bind(id)
        .execute(conn.conn())
        .await
        .unwrap();
}

#[tokio::test]
async fn новые_клиенты_приходят_одним_сообщением_со_списком_без_повторов() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    with_public_url(services);
    joined(services, 8_000, RegistrationSource::Bot, None).await;
    assert!(collected(services, AlertKind::Clients).await.is_empty());

    let first = joined(
        services,
        8_001,
        RegistrationSource::Bot,
        Some("ref_FRIEND1__s_tiktok__k_winter"),
    )
    .await;
    joined(
        services,
        8_002,
        RegistrationSource::Miniapp,
        Some("pl_SPRING25"),
    )
    .await;
    joined(
        services,
        8_003,
        RegistrationSource::Web,
        Some("auth_0123456789abcdef0123456789abcdef"),
    )
    .await;
    joined(services, 8_004, RegistrationSource::Admin, None).await;
    for id in 8_005..8_014 {
        joined(services, id, RegistrationSource::Bot, None).await;
    }
    let alerts = collected(services, AlertKind::Clients).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    let lines: Vec<&str> = alerts[0].html.lines().collect();
    assert_eq!(lines[0], "<b>Новые клиенты: 12</b>");
    assert_eq!(lines.len(), 1 + BATCH_LINES + 1, "{lines:?}");
    assert_eq!(lines.last(), Some(&"и ещё 2"));
    assert_eq!(
        lines[1],
        format!(
            "Алексей (@morozov_a) · <a href=\"https://panel.example.com/admin/clients/{0}\">№{0}</a> · бот · реферальная ссылка FRIEND1 · UTM: tiktok / winter",
            first.0
        )
    );
    assert!(
        lines[2].ends_with("· мини-приложение · промо-ссылка SPRING25"),
        "{}",
        lines[2]
    );
    assert!(lines[3].ends_with("· кабинет"), "{}", lines[3]);
    assert!(!alerts[0].html.contains("auth_"));
    assert_eq!(alerts[0].path, "/admin/clients");
    assert!(collected(services, AlertKind::Clients).await.is_empty());

    let single = joined(services, 8_100, RegistrationSource::Bot, None).await;
    let alerts = collected(services, AlertKind::Clients).await;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].html.lines().count(), 2);
    assert_eq!(alerts[0].path, format!("/admin/clients/{}", single.0));

    route(services, AlertKind::Clients, false).await;
    joined(services, 8_200, RegistrationSource::Bot, None).await;
    assert!(collected(services, AlertKind::Clients).await.is_empty());
    route(services, AlertKind::Clients, true).await;
    assert!(collected(services, AlertKind::Clients).await.is_empty());
    env.db.cleanup().await;
}

#[tokio::test]
async fn новые_оплаты_приходят_пачкой_со_способом_и_пометками_без_повторов() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let client = telegram_client(services, 9_001).await;
    let tariff = tariff_with(
        services,
        CurrencyPrices {
            usd: Some(799),
            rub: None,
        },
    )
    .await;
    let early = pending_payment(services, client, &tariff, providers::CRYPTOPAY, false).await;
    paid(services, &early).await;
    assert!(collected(services, AlertKind::Sales).await.is_empty());

    let mut created = Vec::new();
    for n in 0..12 {
        let provider = if n == 0 {
            providers::LAVA
        } else {
            providers::CRYPTOPAY
        };
        created.push(pending_payment(services, client, &tariff, provider, false).await);
    }
    for payment in &created {
        paid(services, payment).await;
    }
    exec(
        services,
        "UPDATE payments SET pay_method = 'paypal' WHERE id = $1",
        created[0].id.0,
    )
    .await;
    exec(
        services,
        "UPDATE payments SET method = 'autorenew' WHERE id = $1",
        created[1].id.0,
    )
    .await;
    exec(
        services,
        "INSERT INTO payment_repeats (payment_id, original_payment_id, action) VALUES ($1, NULL, 'balance')",
        created[2].id.0,
    )
    .await;
    exec(
        services,
        "UPDATE payments SET status = 'refunded', refunded_minor = amount_minor WHERE id = $1",
        created[3].id.0,
    )
    .await;

    let alerts = collected(services, AlertKind::Sales).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    let lines: Vec<&str> = alerts[0].html.lines().collect();
    assert_eq!(lines[0], "<b>Новые оплаты: 12</b>");
    assert_eq!(lines.len(), 1 + BATCH_LINES + 1, "{lines:?}");
    assert_eq!(lines.last(), Some(&"и ещё 2"));
    let name = format!("«{}»", tariff.name);
    assert_eq!(
        lines[1],
        format!("$7.99 · {name} на 3\u{a0}месяца · Алексей (@morozov_a) · Lava.top, PayPal")
    );
    assert!(
        lines[2].ends_with("· Crypto Pay · автопродление"),
        "{}",
        lines[2]
    );
    assert!(
        lines[3].ends_with("· повторная оплата, зачислена на баланс"),
        "{}",
        lines[3]
    );
    assert!(lines[4].ends_with("· уже возвращена"), "{}", lines[4]);
    assert_eq!(alerts[0].path, "/admin/money");
    assert!(collected(services, AlertKind::Sales).await.is_empty());

    let last = pending_payment(services, client, &tariff, providers::CRYPTOPAY, false).await;
    paid(services, &last).await;
    let alerts = collected(services, AlertKind::Sales).await;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].html.lines().count(), 2);
    assert_eq!(alerts[0].path, format!("/admin/clients/{}", client.0));
    assert!(collected(services, AlertKind::Sales).await.is_empty());
    env.db.cleanup().await;
}
