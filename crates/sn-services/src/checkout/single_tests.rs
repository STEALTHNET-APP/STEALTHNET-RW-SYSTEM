use super::testkit::*;
use crate::Actor;
use crate::clients::GrantRequest;
use crate::fulfillment::FulfillmentOutcome;
use crate::fulfillment::naming_tests::set_multi;
use chrono::Duration;
use sn_domain::events::DomainEvent;
use sn_domain::ids::AdminId;
use sn_domain::order::OrderKind;
use sn_domain::plan::{PlanKind, whole_seconds};
use sn_domain::subscription::Subscription;
use sn_domain::traffic::TrafficPolicy;
use sn_store::{Db, audit_entity};

async fn live(kit: &Kit, client_id: sn_domain::ids::ClientId) -> Vec<Subscription> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.subscriptions()
        .by_client(client_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|s| s.ended_at.is_none())
        .collect()
}

async fn deliver(kit: &Kit, order_id: sn_domain::ids::OrderId) -> Subscription {
    let outcome = kit.services.fulfillment().run(order_id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = order(&kit.services, order_id).await;
    subscription(&kit.services, &order).await
}

#[tokio::test]
async fn одна_подписка_покупка_той_же_категории_продлевает() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 311).await;
    let tariff = standard(&kit.services).await;
    let first = provision(&kit, client_id, &tariff, 30).await;
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart(&tariff, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Renewal);
    let current = view.current.unwrap();
    assert_eq!(current.subscription_id, first.id);
    assert_eq!(current.tariff.as_deref(), Some("Стандарт"));
    assert!(!current.trial);
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    assert_eq!(order.kind, OrderKind::Renewal);
    assert_eq!(order.subscription_id, Some(first.id));
    let renewed = deliver(&kit, order.id).await;
    assert_eq!(renewed.id, first.id);
    assert_eq!(
        renewed.mirror.expire_at,
        first.mirror.expire_at.map(|at| at + Duration::days(30))
    );
    assert_eq!(live(&kit, client_id).await.len(), 1);
    assert_eq!(kit.remna.users().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn переход_на_другую_категорию_заменяет_подписку() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 312).await;
    let standard = standard(&kit.services).await;
    let first = provision(&kit, client_id, &standard, 90).await;
    let first_user = remna_user(&kit, &first).await;
    let premium_category = category(&kit.services, "Премиум").await;
    let premium = tariff(
        &kit.services,
        premium_category,
        "Премиум",
        TrafficPolicy::UNLIMITED,
        vec![squad(3)],
    )
    .await;
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart(&premium, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Change);
    let current = view.current.clone().unwrap();
    assert_eq!(current.subscription_id, first.id);
    assert_eq!(current.tariff.as_deref(), Some("Стандарт"));
    assert!(view.expire_preview < first.mirror.expire_at.unwrap());
    let order = buy_with_balance(&kit.services, client_id, cart(&premium, 30, None)).await;
    assert_eq!(order.kind, OrderKind::Change);
    assert_eq!(order.subscription_id, Some(first.id));
    let moved = deliver(&kit, order.id).await;
    let order = self::order(&kit.services, order.id).await;
    assert_eq!(moved.id, first.id);
    assert_eq!(moved.tariff_id, Some(premium.id));
    assert_eq!(
        moved.mirror.expire_at,
        Some(whole_seconds(order.paid_at.unwrap() + Duration::days(30)))
    );
    let user = remna_user(&kit, &moved).await;
    assert_eq!(user.id, first_user.id);
    assert_eq!(user.username, "user312");
    assert_eq!(user.squad_ids(), vec![squad(3)]);
    assert_eq!(live(&kit, client_id).await.len(), 1);
    assert_eq!(kit.remna.users().len(), 1);
    assert!(
        events(&kit.services, client_id)
            .await
            .iter()
            .any(|e| matches!(
                e,
                DomainEvent::TariffChanged { from, to, .. }
                    if *from == Some(standard.id) && *to == premium.id
            ))
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn вторая_оплата_до_выдачи_первой_не_создаёт_вторую_подписку() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 313).await;
    let standard = standard(&kit.services).await;
    let premium_category = category(&kit.services, "Премиум").await;
    let premium = tariff(
        &kit.services,
        premium_category,
        "Премиум",
        TrafficPolicy::UNLIMITED,
        vec![squad(3)],
    )
    .await;
    credit(&kit.services, client_id, 10_000).await;
    let first = buy_with_balance(&kit.services, client_id, cart(&standard, 30, None)).await;
    let second = buy_with_balance(&kit.services, client_id, cart(&standard, 90, None)).await;
    let third = buy_with_balance(&kit.services, client_id, cart(&premium, 30, None)).await;
    for order in [&first, &second, &third] {
        assert_eq!(order.kind, OrderKind::New);
        assert_eq!(order.subscription_id, None);
    }
    let created = deliver(&kit, first.id).await;
    let renewed = deliver(&kit, second.id).await;
    assert_eq!(renewed.id, created.id);
    assert_eq!(
        renewed.mirror.expire_at,
        created.mirror.expire_at.map(|at| at + Duration::days(90))
    );
    let second = order(&kit.services, second.id).await;
    assert_eq!(second.kind, OrderKind::Renewal);
    assert_eq!(second.subscription_id, Some(created.id));
    let moved = deliver(&kit, third.id).await;
    assert_eq!(moved.id, created.id);
    assert_eq!(moved.tariff_id, Some(premium.id));
    assert_eq!(order(&kit.services, third.id).await.kind, OrderKind::Change);
    assert_eq!(live(&kit, client_id).await.len(), 1);
    assert_eq!(kit.remna.users().len(), 1);
    let mut conn = kit.services.store().conn().await.unwrap();
    let audit = conn
        .audit()
        .for_entity(audit_entity::ORDER, second.id.0, 10)
        .await
        .unwrap();
    assert!(
        audit
            .iter()
            .any(|a| a.action == crate::fulfillment::ORDER_ATTACHED),
        "{audit:?}"
    );
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn с_мультиподписками_новая_покупка_даёт_новую_подписку() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 314).await;
    let tariff = standard(&kit.services).await;
    let first = provision(&kit, client_id, &tariff, 30).await;
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart(&tariff, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::New);
    assert_eq!(view.current, None);
    let second = provision(&kit, client_id, &tariff, 30).await;
    assert_ne!(second.id, first.id);
    assert_eq!(remna_user(&kit, &first).await.username, "user314");
    assert_eq!(remna_user(&kit, &second).await.username, "user314_2");
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart(&tariff, 30, Some(&first)))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Renewal);
    assert_eq!(view.current.map(|c| c.subscription_id), Some(first.id));
    assert_eq!(live(&kit, client_id).await.len(), 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn без_мультиподписок_при_нескольких_старых_берётся_та_же_категория() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 315).await;
    let standard = standard(&kit.services).await;
    let premium_category = category(&kit.services, "Премиум").await;
    let premium = tariff(
        &kit.services,
        premium_category,
        "Премиум",
        TrafficPolicy::UNLIMITED,
        vec![squad(3)],
    )
    .await;
    let older = provision(&kit, client_id, &standard, 30).await;
    let newer = provision(&kit, client_id, &premium, 30).await;
    set_multi(&kit, false).await;
    let checkout = kit.services.checkout();
    let view = checkout
        .quote(client_id, &cart(&standard, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Renewal);
    assert_eq!(view.current.map(|c| c.subscription_id), Some(older.id));
    let other = category(&kit.services, "Азия").await;
    let asia = tariff(
        &kit.services,
        other,
        "Азия",
        TrafficPolicy::UNLIMITED,
        vec![squad(2)],
    )
    .await;
    let view = checkout
        .quote(client_id, &cart(&asia, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Change);
    assert_eq!(view.current.map(|c| c.subscription_id), Some(newer.id));
    let order = buy_with_balance(&kit.services, client_id, cart(&asia, 30, None)).await;
    deliver(&kit, order.id).await;
    assert_eq!(live(&kit, client_id).await.len(), 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пробный_период_при_оплате_переводится_на_тариф() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 316).await;
    let tariff = standard(&kit.services).await;
    let trial_order = kit
        .services
        .checkout()
        .start_trial(client_id, None)
        .await
        .unwrap();
    let trial = deliver(&kit, trial_order).await;
    assert_eq!(remna_user(&kit, &trial).await.username, "user316");
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart(&tariff, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Change);
    let current = view.current.unwrap();
    assert!(current.trial);
    assert_eq!(current.subscription_id, trial.id);
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let paid = deliver(&kit, order.id).await;
    assert_eq!(paid.id, trial.id);
    assert_eq!(remna_user(&kit, &paid).await.username, "user316");
    assert_eq!(live(&kit, client_id).await.len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn две_выдачи_админом_до_выдачи_первой_дают_одну_подписку() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 317).await;
    let tariff = standard(&kit.services).await;
    let request = GrantRequest {
        tariff_id: tariff.id,
        period_id: Some(tariff.periods[0].id),
        days: None,
        subscription_id: None,
        note: None,
    };
    let admin = Actor::Admin(AdminId(1));
    let first = kit
        .services
        .clients()
        .grant(client_id, &request, admin)
        .await
        .unwrap();
    let second = kit
        .services
        .clients()
        .grant(client_id, &request, admin)
        .await
        .unwrap();
    let first = order_by_public_id(&kit.services, first.order_id).await;
    let second = order_by_public_id(&kit.services, second.order_id).await;
    let created = deliver(&kit, first.id).await;
    let renewed = deliver(&kit, second.id).await;
    assert_eq!(renewed.id, created.id);
    assert_eq!(
        renewed.mirror.expire_at,
        created.mirror.expire_at.map(|at| at + Duration::days(30))
    );
    let second = order(&kit.services, second.id).await;
    assert_eq!(second.kind, OrderKind::AdminGrant);
    assert_eq!(second.subscription_id, Some(created.id));
    assert_eq!(live(&kit, client_id).await.len(), 1);
    assert_eq!(kit.remna.users().len(), 1);
    kit.db.cleanup().await;
}
