use super::money::scene;
use super::*;
use sn_store::testing::test_store;

#[tokio::test]
async fn регистрации_пробные_и_состояние_клиентов() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let seed = &s.seed;
    seed.buy(Buy {
        client: s.f,
        kind: "trial",
        at: utc(2026, 3, 3, 10),
        days: 3,
        provider: "yookassa",
        amount: 0,
        ..Buy::default()
    })
    .await;
    let sf = seed.subscription(s.f, "paid", Some(s.start)).await;
    seed.buy(Buy {
        client: s.f,
        subscription: Some(sf),
        kind: "new",
        at: utc(2026, 3, 5, 10),
        tariff: Some(s.start),
        total: 30_000,
        amount: 30_000,
        ..Buy::default()
    })
    .await;
    seed.exec(&format!(
        "UPDATE clients SET status = 'blocked', blocked_at = '2026-03-10 10:00+00' WHERE id = {b}; \
         UPDATE clients SET locale = 'en' WHERE id = {e}; \
         INSERT INTO client_balances (client_id, wallet, currency, amount_minor) VALUES ({c}, 'main', 'RUB', 20000); \
         UPDATE subscriptions SET devices_count = 3, hwid_limit = 3, first_connected_at = '2026-03-02 10:00+00', \
           online_at = '2026-04-05 10:00+00' WHERE client_id = {a}; \
         UPDATE subscriptions SET traffic_used_bytes = 95 * 1073741824::bigint, traffic_limit_bytes = 100 * 1073741824::bigint, \
           first_connected_at = '2026-02-10 10:00+00', online_at = '2026-04-15 11:00+00' WHERE client_id = {b}",
        b = s.b,
        e = s.e,
        c = s.c,
        a = s.a
    ))
    .await;
    let r = ctx_report(&db.store, "clients", month(3, Compare::Prev)).await;
    assert_eq!(value(&r, "registrations"), 4.0);
    assert_eq!(metric(&r, "registrations").prev, Some(2.0));
    assert_eq!(
        breakdown(&r, "registrations", "channel"),
        vec![("bot".to_string(), 3.0), ("miniapp".to_string(), 1.0)]
    );
    assert_eq!(
        breakdown(&r, "registrations", "source"),
        vec![("none".to_string(), 3.0), ("utm".to_string(), 1.0)]
    );
    assert_eq!(value(&r, "trials"), 1.0);
    assert_eq!(value(&r, "trial_bought"), 100.0);
    assert_eq!(value(&r, "trial_wait"), 172_800.0);
    assert_eq!(value(&r, "active_subs"), 5.0);
    assert_eq!(value(&r, "devices"), 1.0);
    assert_eq!(value(&r, "traffic"), 1.0);
    assert_eq!(value(&r, "never"), 3.0);
    assert_eq!(
        metric(&r, "never").note.as_deref(),
        Some("не заходили 7 дней: 1, 30 дней: 0")
    );
    assert_eq!(value(&r, "balances"), 20_000.0);
    assert_eq!(value(&r, "blocked"), 1.0);
    assert_eq!(metric(&r, "blocked").note.as_deref(), Some("+1 за период"));
    assert_eq!(value(&r, "english"), 1.0);
    let q = filtered(month(3, Compare::None), |q| {
        q.source = Some("utm:vk".into())
    });
    let r = ctx_report(&db.store, "clients", q).await;
    assert_eq!(value(&r, "registrations"), 1.0);
    assert_eq!(value(&r, "english"), 1.0);
}

#[tokio::test]
async fn бонусы_за_пополнения_видны_отдельно_и_не_попадают_в_выручку() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let before = ctx_report(&db.store, "clients", month(3, Compare::None)).await;
    assert_eq!(table(&before, "balances").len(), 2);
    let money_before = ctx_report(&db.store, "money", month(3, Compare::None)).await;
    s.seed
        .exec(&format!(
            "INSERT INTO balance_entries (client_id, wallet, currency, delta_minor, balance_after_minor, kind, created_at) VALUES \
               ({a}, 'main', 'RUB', 2500, 2500, 'topup_bonus', '2026-03-04 10:00+00'), \
               ({a}, 'main', 'RUB', -1000, 1500, 'topup_bonus_reversal', '2026-03-05 10:00+00'), \
               ({c}, 'main', 'RUB', 700, 700, 'topup_bonus', '2026-03-06 10:00+00'), \
               ({c}, 'main', 'RUB', 900, 1600, 'topup_bonus', '2026-02-06 10:00+00')",
            a = s.a,
            c = s.c
        ))
        .await;
    let report = ctx_report(&db.store, "clients", month(3, Compare::None)).await;
    assert_eq!(
        table(&report, "balances")[2],
        (
            "Бонусы за пополнения".to_string(),
            vec![Some(2_200.0), Some(2.0)]
        )
    );
    assert_eq!(value(&report, "balances"), value(&before, "balances"));
    let money = ctx_report(&db.store, "money", month(3, Compare::None)).await;
    for group in &money.groups {
        for m in &group.metrics {
            assert_eq!(m.value, metric(&money_before, m.key).value, "{}", m.key);
        }
    }
    let costs = ctx_report(&db.store, "economy", month(3, Compare::None)).await;
    let rows = table(&costs, "margin");
    let cell = |label: &str| {
        rows.iter()
            .find(|(name, _)| name == label)
            .and_then(|(_, cells)| cells[0])
    };
    assert_eq!(cell("Бонусы за пополнения"), Some(-2_200.0));
    let parts: f64 = rows
        .iter()
        .filter(|(name, _)| name != "Маржа")
        .filter_map(|(_, cells)| cells[0])
        .sum();
    assert_eq!(cell("Маржа"), Some(parts));
}
