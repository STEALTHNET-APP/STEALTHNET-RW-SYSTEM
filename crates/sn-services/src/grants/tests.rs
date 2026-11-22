use crate::checkout::testkit::*;
use crate::fulfillment::FulfillmentOutcome;
use crate::fulfillment::naming_tests::set_multi;
use chrono::Duration;
use sn_domain::ids::{ClientId, OrderId};
use sn_domain::module::ModuleId;
use sn_domain::order::{OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::plan::{PlanKind, whole_seconds};
use sn_domain::subscription::{Subscription, SubscriptionKind};
use sn_domain::tariff::Tariff;
use sn_domain::traffic::{GIB, TrafficPolicy, TrafficReset};
use sn_modules::{GrantRefusal, GrantSpec, GrantTarget, GrantedOrder, SubscriptionGrant};
use sn_store::Db;

fn gift(client_id: ClientId, tariff: &Tariff, days: u32, target: GrantTarget) -> SubscriptionGrant {
    let period = tariff.periods.iter().find(|p| p.days == days).unwrap();
    SubscriptionGrant {
        client_id,
        module: ModuleId::Gifts,
        reference: format!("gift:test:{}", client_id.0),
        label: format!("Подарок: «{}»", tariff.name),
        spec: GrantSpec::Tariff {
            tariff_id: tariff.id,
            period_id: period.id,
            days,
        },
        target,
    }
}

fn promo(client_id: ClientId) -> SubscriptionGrant {
    SubscriptionGrant {
        client_id,
        module: ModuleId::Promolinks,
        reference: format!("promo_link:test:{}", client_id.0),
        label: "Промо-ссылка «Блогер»".into(),
        spec: GrantSpec::Custom {
            name: "Промо-ссылка «Блогер»".into(),
            days: 7,
            devices: 2,
            traffic: TrafficPolicy {
                limit_bytes: Some(20 * GIB),
                reset: TrafficReset::NoReset,
            },
            squads: vec![squad(3)],
        },
        target: GrantTarget::Auto,
    }
}

async fn grant(kit: &Kit, request: &SubscriptionGrant) -> Result<GrantedOrder, GrantRefusal> {
    let mut tx = kit.services.store().begin().await.unwrap();
    let result = kit.services.grants().grant(&mut tx, request).await.unwrap();
    tx.commit().await.unwrap();
    result
}

async fn deliver(kit: &Kit, order_id: OrderId) -> Subscription {
    let outcome = kit.services.fulfillment().run(order_id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = order(&kit.services, order_id).await;
    assert_eq!(order.status, OrderStatus::Fulfilled);
    subscription(&kit.services, &order).await
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

#[tokio::test]
async fn подарок_без_подписки_выдаётся_новой_подпиской_вида_подарок() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 701).await;
    let tariff = standard(&kit.services).await;
    let request = gift(client_id, &tariff, 90, GrantTarget::Auto);
    let preview = kit.services.grants().preview(&request).await.unwrap();
    assert_eq!(preview.plan, PlanKind::New);
    assert!(preview.refusal.is_none());
    assert!(preview.current.is_none());
    let granted = grant(&kit, &request).await.unwrap();
    assert_eq!(granted.plan, PlanKind::New);
    let order = order(&kit.services, granted.order_id).await;
    assert_eq!(order.kind, OrderKind::ModuleGrant);
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(order.quote.total.minor, 0);
    assert!(matches!(
        order.intent,
        PurchaseIntent::ModuleGrant {
            module: ModuleId::Gifts,
            days: 90,
            ..
        }
    ));
    let subscription = deliver(&kit, granted.order_id).await;
    let order = self::order(&kit.services, granted.order_id).await;
    assert_eq!(subscription.kind, SubscriptionKind::Gift);
    assert_eq!(subscription.tariff_id, Some(tariff.id));
    assert!(!subscription.autorenew.enabled);
    assert_eq!(
        subscription.mirror.expire_at,
        Some(whole_seconds(order.paid_at.unwrap() + Duration::days(90)))
    );
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.squad_ids(), vec![squad(1), squad(2)]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn подарок_продлевает_единственную_подписку_той_же_категории() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 702).await;
    let tariff = standard(&kit.services).await;
    let first = provision(&kit, client_id, &tariff, 30).await;
    let request = gift(client_id, &tariff, 90, GrantTarget::New);
    let preview = kit.services.grants().preview(&request).await.unwrap();
    assert_eq!(preview.plan, PlanKind::Renewal);
    assert_eq!(
        preview.current.as_ref().map(|c| c.subscription_id),
        Some(first.id)
    );
    assert_eq!(
        preview.current.as_ref().and_then(|c| c.tariff.as_deref()),
        Some("Стандарт")
    );
    let granted = grant(&kit, &request).await.unwrap();
    assert_eq!(granted.subscription_id, Some(first.id));
    let renewed = deliver(&kit, granted.order_id).await;
    assert_eq!(renewed.id, first.id);
    assert_eq!(renewed.kind, SubscriptionKind::Paid);
    assert_eq!(
        renewed.mirror.expire_at,
        first.mirror.expire_at.map(|at| at + Duration::days(90))
    );
    assert_eq!(live(&kit, client_id).await.len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn при_нескольких_подписках_подарок_идёт_отдельно_или_в_выбранную() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 703).await;
    let tariff = standard(&kit.services).await;
    let first = provision(&kit, client_id, &tariff, 30).await;
    let auto = gift(client_id, &tariff, 30, GrantTarget::Auto);
    let preview = kit.services.grants().preview(&auto).await.unwrap();
    assert!(preview.multi);
    assert_eq!(preview.plan, PlanKind::New);
    assert_eq!(
        preview
            .choices
            .iter()
            .map(|c| c.subscription_id)
            .collect::<Vec<_>>(),
        vec![first.id]
    );
    let separate = grant(&kit, &auto).await.unwrap();
    let second = deliver(&kit, separate.order_id).await;
    assert_ne!(second.id, first.id);
    assert_eq!(second.kind, SubscriptionKind::Gift);
    let chosen = gift(client_id, &tariff, 30, GrantTarget::Subscription(first.id));
    let preview = kit.services.grants().preview(&chosen).await.unwrap();
    assert_eq!(preview.plan, PlanKind::Renewal);
    let renewal = grant(&kit, &chosen).await.unwrap();
    let renewed = deliver(&kit, renewal.order_id).await;
    assert_eq!(renewed.id, first.id);
    assert_eq!(
        renewed.mirror.expire_at,
        first.mirror.expire_at.map(|at| at + Duration::days(30))
    );
    assert_eq!(live(&kit, client_id).await.len(), 2);
    let missing = gift(
        client_id,
        &tariff,
        30,
        GrantTarget::Subscription(sn_domain::ids::SubscriptionId(9_999_999)),
    );
    assert_eq!(
        grant(&kit, &missing).await.unwrap_err().code,
        "grant.target_missing"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn промо_выдача_без_тарифа_со_своими_сквадами_и_лимитами() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 704).await;
    let request = promo(client_id);
    let granted = grant(&kit, &request).await.unwrap();
    assert_eq!(granted.plan, PlanKind::New);
    let subscription = deliver(&kit, granted.order_id).await;
    assert_eq!(subscription.kind, SubscriptionKind::Promo);
    assert_eq!(subscription.tariff_id, None);
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.squad_ids(), vec![squad(3)]);
    assert_eq!(user.traffic_limit_bytes, (20 * GIB) as i64);
    assert_eq!(user.hwid_device_limit, Some(2));
    let order = order(&kit.services, granted.order_id).await;
    assert_eq!(
        subscription.mirror.expire_at,
        Some(whole_seconds(order.paid_at.unwrap() + Duration::days(7)))
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn промо_выдача_не_заменяет_действующую_платную_подписку_а_пробную_заменяет() {
    let Some(kit) = kit().await else { return };
    let paid_client = client(&kit.services, 705).await;
    let tariff = standard(&kit.services).await;
    provision(&kit, paid_client, &tariff, 30).await;
    let preview = kit
        .services
        .grants()
        .preview(&promo(paid_client))
        .await
        .unwrap();
    assert_eq!(
        preview.refusal.map(|r| r.code),
        Some("grant.has_subscription")
    );
    assert_eq!(
        grant(&kit, &promo(paid_client)).await.unwrap_err().code,
        "grant.has_subscription"
    );
    let trial_client = client(&kit.services, 706).await;
    let trial_order = kit
        .services
        .checkout()
        .start_trial(trial_client, None)
        .await
        .unwrap();
    let trial = deliver(&kit, trial_order).await;
    assert_eq!(trial.kind, SubscriptionKind::Trial);
    let granted = grant(&kit, &promo(trial_client)).await.unwrap();
    assert_eq!(granted.plan, PlanKind::Change);
    assert_eq!(granted.subscription_id, Some(trial.id));
    let replaced = deliver(&kit, granted.order_id).await;
    assert_eq!(replaced.id, trial.id);
    assert_eq!(replaced.kind, SubscriptionKind::Promo);
    assert_eq!(replaced.tariff_id, None);
    assert_eq!(
        remna_user(&kit, &replaced).await.squad_ids(),
        vec![squad(3)]
    );
    set_multi(&kit, true).await;
    let separate = grant(&kit, &promo(paid_client)).await.unwrap();
    assert_eq!(separate.plan, PlanKind::New);
    let second = deliver(&kit, separate.order_id).await;
    assert_eq!(second.kind, SubscriptionKind::Promo);
    assert_eq!(live(&kit, paid_client).await.len(), 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn выдача_отказывает_заблокированному_и_без_remnawave() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 707).await;
    let tariff = standard(&kit.services).await;
    kit.services
        .clients()
        .block(client_id, Some("тест"), crate::Actor::System)
        .await
        .unwrap();
    let request = gift(client_id, &tariff, 30, GrantTarget::Auto);
    assert_eq!(
        grant(&kit, &request).await.unwrap_err().code,
        "grant.client_blocked"
    );
    let other = client(&kit.services, 708).await;
    kit.services.set_remna(None);
    assert_eq!(
        grant(&kit, &gift(other, &tariff, 30, GrantTarget::Auto))
            .await
            .unwrap_err()
            .code,
        "grant.unavailable"
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let orders: i64 = sqlx::query_scalar("SELECT count(*) FROM orders WHERE kind = 'module_grant'")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(orders, 0);
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn промо_выдача_не_заменяет_подписку_оплаченную_до_выдачи() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 709).await;
    let trial_order = kit
        .services
        .checkout()
        .start_trial(client_id, None)
        .await
        .unwrap();
    let trial = deliver(&kit, trial_order).await;
    let granted = grant(&kit, &promo(client_id)).await.unwrap();
    assert_eq!(granted.subscription_id, Some(trial.id));
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE subscriptions SET kind = 'paid' WHERE id = $1")
            .bind(trial.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let outcome = kit
        .services
        .fulfillment()
        .run(granted.order_id)
        .await
        .unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Review { .. }),
        "{outcome:?}"
    );
    assert_eq!(remna_user(&kit, &trial).await.squad_ids(), vec![squad(9)]);
    kit.db.cleanup().await;
}

async fn limited_category(kit: &Kit, tariff: &Tariff, limit: Option<u32>) {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.tariffs()
        .set_subscription_limit(tariff.category_id, limit)
        .await
        .unwrap();
}

#[tokio::test]
async fn при_лимите_категории_подарок_добавляет_дни_к_её_подписке() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 711).await;
    let tariff = standard(&kit.services).await;
    let first = provision(&kit, client_id, &tariff, 30).await;
    limited_category(&kit, &tariff, Some(1)).await;
    for target in [GrantTarget::Auto, GrantTarget::New] {
        let preview = kit
            .services
            .grants()
            .preview(&gift(client_id, &tariff, 30, target))
            .await
            .unwrap();
        assert_eq!(preview.plan, PlanKind::Renewal);
        assert!(!preview.new_allowed);
        assert_eq!(preview.current.map(|c| c.subscription_id), Some(first.id));
    }
    let granted = grant(&kit, &gift(client_id, &tariff, 30, GrantTarget::New))
        .await
        .unwrap();
    assert_eq!(granted.subscription_id, Some(first.id));
    let renewed = deliver(&kit, granted.order_id).await;
    assert_eq!(renewed.id, first.id);
    assert_eq!(
        renewed.mirror.expire_at,
        first.mirror.expire_at.map(|at| at + Duration::days(30))
    );
    assert_eq!(live(&kit, client_id).await.len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn при_нескольких_подписках_заполненной_категории_подарок_просит_выбрать() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 712).await;
    let tariff = standard(&kit.services).await;
    let first = provision(&kit, client_id, &tariff, 30).await;
    let second = provision(&kit, client_id, &tariff, 30).await;
    limited_category(&kit, &tariff, Some(2)).await;
    let preview = kit
        .services
        .grants()
        .preview(&gift(client_id, &tariff, 30, GrantTarget::Auto))
        .await
        .unwrap();
    assert!(!preview.new_allowed);
    assert!(preview.current.is_none());
    assert_eq!(
        preview
            .choices
            .iter()
            .map(|c| c.subscription_id)
            .collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    assert_eq!(
        grant(&kit, &gift(client_id, &tariff, 30, GrantTarget::New))
            .await
            .unwrap_err()
            .code,
        "grant.choose"
    );
    let premium_category = category(&kit.services, "Премиум").await;
    let premium = crate::checkout::testkit::tariff(
        &kit.services,
        premium_category,
        "Премиум",
        TrafficPolicy::UNLIMITED,
        vec![squad(3)],
    )
    .await;
    let other = provision(&kit, client_id, &premium, 30).await;
    assert_eq!(
        grant(
            &kit,
            &gift(client_id, &tariff, 30, GrantTarget::Subscription(other.id))
        )
        .await
        .unwrap_err()
        .code,
        "grant.category_full"
    );
    let chosen = grant(
        &kit,
        &gift(client_id, &tariff, 30, GrantTarget::Subscription(second.id)),
    )
    .await
    .unwrap();
    assert_eq!(deliver(&kit, chosen.order_id).await.id, second.id);
    assert_eq!(live(&kit, client_id).await.len(), 3);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn подарок_выданный_до_покупки_в_заполненной_категории_продлевает_её() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 713).await;
    let tariff = standard(&kit.services).await;
    limited_category(&kit, &tariff, Some(1)).await;
    let granted = grant(&kit, &gift(client_id, &tariff, 30, GrantTarget::Auto))
        .await
        .unwrap();
    assert_eq!(granted.plan, PlanKind::New);
    let bought = provision(&kit, client_id, &tariff, 30).await;
    let renewed = deliver(&kit, granted.order_id).await;
    assert_eq!(renewed.id, bought.id);
    assert_eq!(
        order(&kit.services, granted.order_id).await.kind,
        OrderKind::ModuleGrant
    );
    assert_eq!(live(&kit, client_id).await.len(), 1);
    kit.db.cleanup().await;
}
