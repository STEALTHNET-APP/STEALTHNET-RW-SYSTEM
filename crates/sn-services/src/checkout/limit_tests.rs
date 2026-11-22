use super::testkit::*;
use super::{CATEGORY_FULL, CHOOSE_SUBSCRIPTION};
use crate::Actor;
use crate::clients::GrantRequest;
use crate::fulfillment::naming_tests::set_multi;
use crate::fulfillment::{FulfillmentOutcome, ORDER_ATTACHED};
use chrono::Duration;
use sn_domain::ids::{AdminId, CategoryId, ClientId, OrderId};
use sn_domain::order::OrderKind;
use sn_domain::plan::PlanKind;
use sn_domain::subscription::Subscription;
use sn_domain::tariff::Tariff;
use sn_domain::traffic::TrafficPolicy;
use sn_store::{Db, audit_entity};

async fn set_limit(kit: &Kit, category: CategoryId, limit: Option<u32>) {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.tariffs()
        .set_subscription_limit(category, limit)
        .await
        .unwrap();
}

async fn live(kit: &Kit, client_id: ClientId) -> Vec<Subscription> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.subscriptions()
        .by_client(client_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|s| s.ended_at.is_none())
        .collect()
}

async fn deliver(kit: &Kit, order_id: OrderId) -> Subscription {
    let outcome = kit.services.fulfillment().run(order_id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = order(&kit.services, order_id).await;
    subscription(&kit.services, &order).await
}

struct Catalog {
    bypass: CategoryId,
    bypass_one: Tariff,
    bypass_two: Tariff,
    plain: Tariff,
}

async fn catalog(kit: &Kit) -> Catalog {
    let bypass = category(&kit.services, "С обходом").await;
    let bypass_one = tariff(
        &kit.services,
        bypass,
        "Обход",
        TrafficPolicy::UNLIMITED,
        vec![squad(1)],
    )
    .await;
    let bypass_two = tariff(
        &kit.services,
        bypass,
        "Обход плюс",
        TrafficPolicy::UNLIMITED,
        vec![squad(1), squad(2)],
    )
    .await;
    let plain_category = category(&kit.services, "Без обхода").await;
    let plain = tariff(
        &kit.services,
        plain_category,
        "Обычный",
        TrafficPolicy::UNLIMITED,
        vec![squad(3)],
    )
    .await;
    Catalog {
        bypass,
        bypass_one,
        bypass_two,
        plain,
    }
}

#[tokio::test]
async fn при_лимите_покупка_тарифа_категории_продлевает_её_подписку() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 901).await;
    let catalog = catalog(&kit).await;
    set_limit(&kit, catalog.bypass, Some(1)).await;
    let first = provision(&kit, client_id, &catalog.bypass_one, 30).await;
    let checkout = kit.services.checkout();
    let view = checkout
        .quote(client_id, &cart(&catalog.bypass_one, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Renewal);
    assert_eq!(view.current.map(|c| c.subscription_id), Some(first.id));
    let other = checkout
        .quote(client_id, &cart(&catalog.bypass_two, 30, None))
        .await
        .unwrap();
    assert_eq!(other.kind, PlanKind::Renewal);
    assert_eq!(other.current.map(|c| c.subscription_id), Some(first.id));
    credit(&kit.services, client_id, 10_000).await;
    let order = buy_with_balance(
        &kit.services,
        client_id,
        cart(&catalog.bypass_one, 30, None),
    )
    .await;
    assert_eq!(order.kind, OrderKind::Renewal);
    assert_eq!(order.subscription_id, Some(first.id));
    let renewed = deliver(&kit, order.id).await;
    assert_eq!(renewed.id, first.id);
    assert_eq!(
        renewed.mirror.expire_at,
        first.mirror.expire_at.map(|at| at + Duration::days(30))
    );
    let plain = checkout
        .quote(client_id, &cart(&catalog.plain, 30, None))
        .await
        .unwrap();
    assert_eq!(plain.kind, PlanKind::New);
    let separate = provision(&kit, client_id, &catalog.plain, 30).await;
    assert_ne!(separate.id, first.id);
    assert_eq!(live(&kit, client_id).await.len(), 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn старые_подписки_сверх_лимита_остаются_и_клиент_выбирает_какую_продлить() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 902).await;
    let catalog = catalog(&kit).await;
    let first = provision(&kit, client_id, &catalog.bypass_one, 30).await;
    let second = provision(&kit, client_id, &catalog.bypass_one, 30).await;
    set_limit(&kit, catalog.bypass, Some(1)).await;
    let checkout = kit.services.checkout();
    let err = checkout
        .quote(client_id, &cart(&catalog.bypass_one, 30, None))
        .await
        .unwrap_err();
    assert_eq!(err.code(), CHOOSE_SUBSCRIPTION);
    assert!(err.to_string().contains("С обходом"), "{err}");
    credit(&kit.services, client_id, 10_000).await;
    let refused = checkout
        .start(
            client_id,
            &super::CheckoutRequest {
                cart: cart(&catalog.bypass_one, 30, None),
                provider: sn_domain::payment::providers::BALANCE.into(),
                method: None,
                channel: super::Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code(), CHOOSE_SUBSCRIPTION);
    for chosen in [&first, &second] {
        let view = checkout
            .quote(client_id, &cart(&catalog.bypass_two, 30, Some(chosen)))
            .await
            .unwrap();
        assert_eq!(view.kind, PlanKind::Renewal);
        assert_eq!(view.current.map(|c| c.subscription_id), Some(chosen.id));
    }
    let order = buy_with_balance(
        &kit.services,
        client_id,
        cart(&catalog.bypass_one, 30, Some(&second)),
    )
    .await;
    assert_eq!(deliver(&kit, order.id).await.id, second.id);
    assert_eq!(live(&kit, client_id).await.len(), 2);
    let room = checkout
        .category_room(client_id, catalog.bypass_one.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        room.held.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    assert!(
        checkout
            .category_room(client_id, catalog.plain.id)
            .await
            .unwrap()
            .is_none()
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn подписку_другой_категории_и_пробную_нельзя_перевести_в_заполненную_категорию() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 903).await;
    let trial_order = kit
        .services
        .checkout()
        .start_trial(client_id, None)
        .await
        .unwrap();
    let trial = deliver(&kit, trial_order).await;
    let catalog = catalog(&kit).await;
    set_limit(&kit, catalog.bypass, Some(1)).await;
    let checkout = kit.services.checkout();
    let converted = checkout
        .quote(client_id, &cart(&catalog.bypass_one, 30, None))
        .await
        .unwrap();
    assert_eq!(converted.kind, PlanKind::Change);
    assert_eq!(converted.current.map(|c| c.subscription_id), Some(trial.id));
    let bypass = provision(&kit, client_id, &catalog.bypass_one, 30).await;
    assert_eq!(bypass.id, trial.id);
    let plain = provision(&kit, client_id, &catalog.plain, 30).await;
    let err = checkout
        .quote(client_id, &cart(&catalog.bypass_two, 30, Some(&plain)))
        .await
        .unwrap_err();
    assert_eq!(err.code(), CATEGORY_FULL);
    let into_plain = checkout
        .quote(client_id, &cart(&catalog.plain, 30, Some(&bypass)))
        .await
        .unwrap();
    assert_eq!(into_plain.kind, PlanKind::Change);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn без_режима_нескольких_подписок_лимит_ничего_не_меняет() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 904).await;
    let catalog = catalog(&kit).await;
    provision(&kit, client_id, &catalog.bypass_one, 30).await;
    let newer = provision(&kit, client_id, &catalog.bypass_one, 30).await;
    set_limit(&kit, catalog.bypass, Some(1)).await;
    set_multi(&kit, false).await;
    let checkout = kit.services.checkout();
    let view = checkout
        .quote(client_id, &cart(&catalog.bypass_one, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Renewal);
    assert_eq!(view.current.map(|c| c.subscription_id), Some(newer.id));
    let change = checkout
        .quote(client_id, &cart(&catalog.plain, 30, None))
        .await
        .unwrap();
    assert_eq!(change.kind, PlanKind::Change);
    assert!(
        checkout
            .category_room(client_id, catalog.bypass_one.id)
            .await
            .unwrap()
            .is_none()
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn две_одновременные_покупки_не_дают_второй_подписки_в_категории() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 905).await;
    let catalog = catalog(&kit).await;
    set_limit(&kit, catalog.bypass, Some(1)).await;
    credit(&kit.services, client_id, 10_000).await;
    let checkout = kit.services.checkout();
    let request = |tariff: &Tariff| super::CheckoutRequest {
        cart: cart(tariff, 30, None),
        provider: sn_domain::payment::providers::BALANCE.into(),
        method: None,
        channel: super::Channel::Cabinet,
        return_url: None,
    };
    let one = request(&catalog.bypass_one);
    let two = request(&catalog.bypass_two);
    let (first, second) = tokio::join!(
        checkout.start(client_id, &one),
        checkout.start(client_id, &two),
    );
    let mut orders = Vec::new();
    for result in [first.unwrap(), second.unwrap()] {
        let super::CheckoutResult::Completed { order_id } = result else {
            panic!("оплата с баланса проходит сразу");
        };
        orders.push(order_by_public_id(&kit.services, order_id).await);
    }
    for order in &orders {
        assert_eq!(order.kind, OrderKind::New);
        assert_eq!(order.subscription_id, None);
    }
    let fulfillment = kit.services.fulfillment();
    let (a, b) = tokio::join!(fulfillment.run(orders[0].id), fulfillment.run(orders[1].id));
    for (order, outcome) in orders.iter().zip([a.unwrap(), b.unwrap()]) {
        let outcome = match outcome {
            FulfillmentOutcome::Retry { .. } => fulfillment.run(order.id).await.unwrap(),
            done => done,
        };
        assert!(
            matches!(outcome, FulfillmentOutcome::Done { .. }),
            "{outcome:?}"
        );
    }
    let held = live(&kit, client_id).await;
    assert_eq!(held.len(), 1);
    assert_eq!(kit.remna.users().len(), 1);
    let first = order(&kit.services, orders[0].id).await;
    let second = order(&kit.services, orders[1].id).await;
    assert_eq!(first.subscription_id, Some(held[0].id));
    assert_eq!(second.subscription_id, Some(held[0].id));
    let attached = if first.kind == OrderKind::Renewal {
        &first
    } else {
        &second
    };
    assert_eq!(attached.kind, OrderKind::Renewal);
    let user = remna_user(&kit, &held[0]).await;
    let paid = first.paid_at.unwrap().max(second.paid_at.unwrap());
    assert!(user.expire_at >= paid + Duration::days(59), "{user:?}");
    let mut conn = kit.services.store().conn().await.unwrap();
    let audit = conn
        .audit()
        .for_entity(audit_entity::ORDER, attached.id.0, 10)
        .await
        .unwrap();
    let record = audit
        .iter()
        .find(|a| a.action == ORDER_ATTACHED)
        .expect("заказ привязан к подписке категории");
    assert!(
        record.details.to_string().contains("не больше 1"),
        "{:?}",
        record.details
    );
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn перевод_в_категорию_одновременно_с_новой_подпиской_уходит_на_проверку() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 906).await;
    let catalog = catalog(&kit).await;
    set_limit(&kit, catalog.bypass, Some(1)).await;
    let plain = provision(&kit, client_id, &catalog.plain, 30).await;
    credit(&kit.services, client_id, 10_000).await;
    let fresh = buy_with_balance(
        &kit.services,
        client_id,
        cart(&catalog.bypass_one, 30, None),
    )
    .await;
    let moved = buy_with_balance(
        &kit.services,
        client_id,
        cart(&catalog.bypass_two, 30, Some(&plain)),
    )
    .await;
    assert_eq!(fresh.kind, OrderKind::New);
    assert_eq!(moved.kind, OrderKind::Change);
    let created = deliver(&kit, fresh.id).await;
    assert_ne!(created.id, plain.id);
    let outcome = kit.services.fulfillment().run(moved.id).await.unwrap();
    let FulfillmentOutcome::Review { reason } = outcome else {
        panic!("перевод сверх лимита ждёт решения администратора: {outcome:?}");
    };
    assert!(reason.contains("С обходом"), "{reason}");
    let mut conn = kit.services.store().conn().await.unwrap();
    let kept = conn.subscriptions().get(plain.id).await.unwrap();
    assert_eq!(kept.tariff_id, Some(catalog.plain.id));
    drop(conn);
    assert_eq!(remna_user(&kit, &plain).await.squad_ids(), vec![squad(3)]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оператор_выдаёт_подписку_сверх_лимита() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 907).await;
    let catalog = catalog(&kit).await;
    set_limit(&kit, catalog.bypass, Some(1)).await;
    let first = provision(&kit, client_id, &catalog.bypass_one, 30).await;
    let granted = kit
        .services
        .clients()
        .grant(
            client_id,
            &GrantRequest {
                tariff_id: catalog.bypass_one.id,
                period_id: Some(catalog.bypass_one.periods[0].id),
                days: None,
                subscription_id: None,
                note: None,
            },
            Actor::Admin(AdminId(1)),
        )
        .await
        .unwrap();
    assert_eq!(granted.plan, PlanKind::New);
    let order = order_by_public_id(&kit.services, granted.order_id).await;
    let extra = deliver(&kit, order.id).await;
    assert_ne!(extra.id, first.id);
    assert_eq!(live(&kit, client_id).await.len(), 2);
    let options = kit.services.clients().grant_options().await.unwrap();
    let limit = options
        .iter()
        .find(|t| t.id == catalog.bypass_one.id)
        .and_then(|t| t.category_limit);
    assert_eq!(limit, Some(1));
    kit.db.cleanup().await;
}
