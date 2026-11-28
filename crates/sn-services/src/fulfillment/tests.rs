use super::*;
use crate::Actor;
use crate::checkout::testkit::*;
use crate::checkout::{CartRequest, Channel, CheckoutRequest, CheckoutResult};
use chrono::{Duration as Days, Utc};
use sn_domain::ids::{AdminId, CategoryId, ClientId};
use sn_domain::payment::PaymentStatus;
use sn_domain::plan::whole_seconds;
use sn_domain::subscription::SubscriptionStatus;
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices, Tariff};
use sn_domain::traffic::{GIB, TIB, TrafficPolicy, TrafficReset, TrafficStrategy};
use sn_payments::{PaymentRef, ProviderStatus, WebhookOutcome, WebhookReply};
use sn_remnawave::fake::{Fault, panel_time, sample_device};
use sn_store::{NewPeriod, NewTariff, PlanStage};
use uuid::Uuid;

const ADMIN: Actor = Actor::Admin(AdminId(1));

fn term(paid_at: Option<chrono::DateTime<Utc>>, days: i64) -> chrono::DateTime<Utc> {
    whole_seconds(paid_at.unwrap() + Days::days(days))
}

async fn invoice(kit: &Kit, client_id: ClientId, cart: CartRequest) -> (Order, Uuid) {
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart,
                provider: TEST_PAY.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect {
        order_id,
        payment_id,
        ..
    } = result
    else {
        panic!("ожидался счёт: {result:?}");
    };
    (
        order_by_public_id(&kit.services, order_id).await,
        payment_id,
    )
}

async fn payment(kit: &Kit, public_id: Uuid) -> sn_domain::payment::Payment {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().by_public_id(public_id).await.unwrap()
}

async fn pay(kit: &Kit, public_id: Uuid) {
    let payment = payment(kit, public_id).await;
    let outcome = WebhookOutcome {
        event_key: format!("paid:{}", payment.id),
        payment: Some(PaymentRef::Payment(payment.id)),
        provider_txid: payment
            .provider_txid
            .clone()
            .or_else(|| Some(format!("tx-{}", payment.id))),
        status: ProviderStatus::Paid,
        amount: Some(payment.amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    };
    kit.services
        .inbox()
        .apply_outcome(TEST_PAY, &outcome)
        .await
        .unwrap();
}

fn new_tariff(
    category_id: CategoryId,
    name: &str,
    traffic: TrafficPolicy,
    squads: Vec<SquadId>,
    included: u32,
) -> NewTariff {
    NewTariff {
        traffic_addon: None,
        category_id,
        name: name.into(),
        description: None,
        on_sale: true,
        sort: 0,
        devices: DevicePolicy {
            included,
            extra: Some(ExtraDevices {
                max: 2,
                monthly: CurrencyPrices {
                    usd: Some(99),
                    rub: Some(4_900),
                },
                monthly_stars: None,
            }),
        },
        traffic,
        squads,
        external_squad: None,
        periods: [(30, 299, 19_900), (90, 699, 49_900)]
            .into_iter()
            .enumerate()
            .map(|(sort, (days, usd, rub))| NewPeriod {
                days,
                prices: CurrencyPrices {
                    usd: Some(usd),
                    rub: Some(rub),
                },
                stars: None,
                sort: sort as i32,
                active: true,
            })
            .collect(),
    }
}

async fn update_tariff(kit: &Kit, tariff: &Tariff, squads: Vec<SquadId>, included: u32) -> Tariff {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.tariffs()
        .update(
            tariff.id,
            &new_tariff(
                tariff.category_id,
                &tariff.name,
                tariff.traffic,
                squads,
                included,
            ),
        )
        .await
        .unwrap()
}

async fn give_extra_devices(kit: &Kit, subscription: &Subscription, extra: u32) -> Subscription {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.subscriptions()
        .apply_purchase(
            subscription.id,
            &sn_store::PurchaseApplied {
                tariff_id: subscription.tariff_id,
                kind: SubscriptionKind::Paid,
                extra_devices: extra,
                period_started_at: Utc::now(),
                period_days: 30,
            },
        )
        .await
        .unwrap();
    drop(conn);
    set_user(kit, subscription, |user| {
        user.hwid_device_limit = Some(3 + i64::from(extra));
    })
    .await
}

async fn subscription_by_id(kit: &Kit, id: SubscriptionId) -> Subscription {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.subscriptions().get(id).await.unwrap()
}

async fn audit_actions(kit: &Kit, kind: &str, id: i64) -> Vec<sn_store::AuditRecord> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.audit().for_entity(kind, id, 20).await.unwrap()
}

#[tokio::test]
async fn покупка_с_баланса_доходит_до_пользователя_в_remnawave() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 201).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 90, None)).await;
    assert_eq!(balance(&kit.services, client_id).await, 301);
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    let FulfillmentOutcome::Done {
        subscription_id: Some(subscription_id),
    } = outcome
    else {
        panic!("выдача не завершилась: {outcome:?}");
    };
    let order = self::order(&kit.services, order.id).await;
    assert_eq!(order.status, OrderStatus::Fulfilled);
    assert_eq!(order.subscription_id, Some(subscription_id));
    let subscription = subscription(&kit.services, &order).await;
    assert_eq!(subscription.kind, SubscriptionKind::Paid);
    assert_eq!(subscription.tariff_id, Some(tariff.id));
    assert_eq!(subscription.period_days, Some(90));
    assert_eq!(subscription.mirror.status, Some(SubscriptionStatus::Active));
    assert!(subscription.autorenew.enabled);
    let link = subscription.remna.clone().unwrap();
    assert_eq!(link.username, "user201");
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.username, link.username);
    assert_eq!(user.expire_at, term(order.paid_at, 90));
    assert_eq!(user.expire_at.timestamp_subsec_nanos(), 0);
    assert_eq!(user.squad_ids(), vec![squad(1), squad(2)]);
    assert_eq!(user.hwid_device_limit, Some(3));
    assert_eq!(user.telegram_id, Some(201));
    assert_eq!(user.tag, None);
    let client = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.clients().get(client_id).await.unwrap()
    };
    assert_eq!(user.description, Some(remna_description(client.public_id)));
    assert_eq!(subscription.mirror.expire_at, Some(user.expire_at));
    let events = events(&kit.services, client_id).await;
    assert!(events.iter().any(|e| matches!(
        e,
        DomainEvent::SubscriptionCreated { subscription_id: id, .. } if *id == subscription_id
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        DomainEvent::OrderFulfilled { order_id, days: 90, devices: 0, .. } if *order_id == order.id
    )));
    assert!(jobs(&kit.services).await.iter().any(|job| matches!(
        job,
        JobPayload::Notify { template, .. } if template == "order.fulfilled"
    )));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn продление_прибавляет_дни_к_дате_окончания() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 202).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(10));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 90, Some(&subscription)),
    )
    .await;
    assert_eq!(renewal.kind, OrderKind::Renewal);
    let done = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.expire_at, expire + Days::days(90));
    let renewed = self::subscription(&kit.services, &renewal).await;
    assert_eq!(renewed.mirror.expire_at, Some(expire + Days::days(90)));
    assert!(events(&kit.services, client_id).await.iter().any(|e| matches!(
        e,
        DomainEvent::SubscriptionRenewed { expire_at, .. } if *expire_at == expire + Days::days(90)
    )));
    assert_eq!(kit.remna.users().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn истёкшая_подписка_продлевается_от_даты_оплаты_и_включается() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 203).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let subscription = expire_in(
        &kit,
        &subscription,
        moment(Days::days(-5)),
        SubscriptionStatus::Expired,
    )
    .await;
    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    kit.services.fulfillment().run(renewal.id).await.unwrap();
    let renewal = order(&kit.services, renewal.id).await;
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.expire_at, term(renewal.paid_at, 30));
    assert_eq!(user.status, sn_remnawave::UserStatus::Active);
    assert!(
        events(&kit.services, client_id)
            .await
            .iter()
            .any(|e| matches!(
                e,
                DomainEvent::SubscriptionStatusChanged {
                    from: Some(SubscriptionStatus::Expired),
                    to: SubscriptionStatus::Active,
                    ..
                }
            ))
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn смена_тарифа_сбрасывает_трафик_и_меняет_сквады() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 204).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let subscription = set_user(&kit, &subscription, |user| {
        user.user_traffic.used_traffic_bytes = 50 * GIB as i64;
        user.active_internal_squads.push(sn_remnawave::SquadRef {
            uuid: squad(9).0,
            name: "Купленный отдельно".into(),
        });
    })
    .await;
    let unblock_category = category(&kit.services, "Unblock").await;
    let unblock = tariff(
        &kit.services,
        unblock_category,
        "Unblock",
        TrafficPolicy {
            limit_bytes: Some(300 * GIB),
            reset: TrafficReset::Month,
        },
        vec![squad(3), squad(1)],
    )
    .await;
    let change = buy_with_balance(
        &kit.services,
        client_id,
        cart(&unblock, 90, Some(&subscription)),
    )
    .await;
    assert_eq!(change.kind, OrderKind::Change);
    kit.services.fulfillment().run(change.id).await.unwrap();
    let change = order(&kit.services, change.id).await;
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.squad_ids(), vec![squad(3), squad(1)]);
    assert_eq!(user.traffic_limit_bytes, (300 * GIB) as i64);
    assert_eq!(user.traffic_limit_strategy, TrafficStrategy::Month);
    assert_eq!(user.user_traffic.used_traffic_bytes, 0);
    assert_eq!(user.expire_at, term(change.paid_at, 90));
    assert!(
        kit.remna
            .calls()
            .contains(&"reset_user_traffic".to_string())
    );
    let changed = self::subscription(&kit.services, &change).await;
    assert_eq!(changed.tariff_id, Some(unblock.id));
    assert!(events(&kit.services, client_id).await.iter().any(|e| matches!(
        e,
        DomainEvent::TariffChanged { from: Some(from), to, .. } if *from == standard.id && *to == unblock.id
    )));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn продление_сохраняет_сквады_купленные_отдельно() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 205).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let subscription = set_user(&kit, &subscription, |user| {
        user.active_internal_squads.push(sn_remnawave::SquadRef {
            uuid: squad(9).0,
            name: "Купленный отдельно".into(),
        });
    })
    .await;
    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    kit.services.fulfillment().run(renewal.id).await.unwrap();
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.squad_ids(), vec![squad(1), squad(2), squad(9)]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn таймаут_после_применения_patch_не_продлевает_дважды() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 206).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(20));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 90, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    let remna = kit.services.remna().unwrap();
    let Ok(context) = fulfillment.context(renewal.clone()).await else {
        panic!("контекст выдачи не собран");
    };
    let Ok(plan) = fulfillment.prepare(&*remna, &context).await else {
        panic!("план не сохранён");
    };
    assert_eq!(plan.target.expire_at, expire + Days::days(90));
    kit.remna.fail_next(Fault::TimeoutAfterApply);
    let first = fulfillment.run(renewal.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + Days::days(90)
    );
    let second = fulfillment.run(renewal.id).await.unwrap();
    assert!(
        matches!(second, FulfillmentOutcome::Done { .. }),
        "{second:?}"
    );
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + Days::days(90)
    );
    let updates = kit
        .remna
        .calls()
        .iter()
        .filter(|c| c.as_str() == "update_user")
        .count();
    assert_eq!(updates, 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn таймаут_при_создании_не_создаёт_второго_пользователя() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 207).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 90, None)).await;
    let fulfillment = kit.services.fulfillment();
    let remna = kit.services.remna().unwrap();
    let Ok(context) = fulfillment.context(order.clone()).await else {
        panic!("контекст выдачи не собран");
    };
    let Ok(_) = fulfillment.prepare(&*remna, &context).await else {
        panic!("план не сохранён");
    };
    kit.remna.fail_next(Fault::TimeoutAfterApply);
    let first = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    assert_eq!(kit.remna.users().len(), 1);
    let second = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(second, FulfillmentOutcome::Done { .. }),
        "{second:?}"
    );
    assert_eq!(kit.remna.users().len(), 1);
    let order = self::order(&kit.services, order.id).await;
    let subscription = subscription(&kit.services, &order).await;
    assert_eq!(
        subscription.remna.map(|l| l.user_id),
        Some(kit.remna.users()[0].id)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn сквада_нет_в_панели_заказ_уходит_на_ручную_проверку() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 208).await;
    let category = category(&kit.services, "Азия").await;
    let tariff = tariff(
        &kit.services,
        category,
        "Азия",
        TrafficPolicy::UNLIMITED,
        vec![squad(1), squad(77)],
    )
    .await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    let FulfillmentOutcome::Review { reason } = outcome else {
        panic!("ожидалась ручная проверка: {outcome:?}");
    };
    assert!(reason.contains(&squad(77).to_string()), "{reason}");
    let order = self::order(&kit.services, order.id).await;
    assert_eq!(order.status, OrderStatus::Review);
    assert_eq!(order.plan, None);
    assert!(kit.remna.users().is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn своё_имя_после_потерянного_ответа_привязывается() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 210).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let fulfillment = kit.services.fulfillment();
    let Ok(context) = fulfillment.context(order.clone()).await else {
        panic!("контекст выдачи не собран");
    };
    let mut ours = sn_remnawave::fake::sample_user(600, &context.profile.username);
    ours.description = Some(context.profile.description.clone());
    kit.remna.insert_user(ours);
    let outcome = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = self::order(&kit.services, order.id).await;
    let subscription = subscription(&kit.services, &order).await;
    assert_eq!(
        subscription.remna.map(|l| l.user_id),
        Some(sn_domain::ids::RemnaUserId(600))
    );
    assert_eq!(kit.remna.users().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказы_одной_подписки_выдаются_по_очереди() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 211).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(15));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let first = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let second = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 90, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    let waiting = fulfillment.run(second.id).await.unwrap();
    assert!(
        matches!(&waiting, FulfillmentOutcome::Retry { reason, .. } if reason.contains(&first.public_id.to_string())),
        "{waiting:?}"
    );
    assert!(matches!(
        fulfillment.run(first.id).await.unwrap(),
        FulfillmentOutcome::Done { .. }
    ));
    assert!(matches!(
        fulfillment.run(second.id).await.unwrap(),
        FulfillmentOutcome::Done { .. }
    ));
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + Days::days(120)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn неоплаченный_заказ_не_выдаётся_а_выданный_не_выдаётся_повторно() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 212).await;
    let tariff = standard(&kit.services).await;
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &crate::checkout::CheckoutRequest {
                cart: cart(&tariff, 30, None),
                provider: TEST_PAY.into(),
                method: None,
                channel: crate::checkout::Channel::Bot,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let crate::checkout::CheckoutResult::Redirect { order_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let pending = order_by_public_id(&kit.services, order_id).await;
    assert!(matches!(
        kit.services.fulfillment().run(pending.id).await.unwrap(),
        FulfillmentOutcome::Review { .. }
    ));
    assert_eq!(
        self::order(&kit.services, pending.id).await.status,
        OrderStatus::Pending
    );
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let calls = kit.remna.calls().len();
    let done = kit
        .services
        .fulfillment()
        .run(order_id_of(&kit, &subscription).await)
        .await
        .unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }));
    assert_eq!(kit.remna.calls().len(), calls);
    kit.db.cleanup().await;
}

async fn order_id_of(kit: &Kit, subscription: &Subscription) -> OrderId {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.orders()
        .for_client(subscription.client_id, Default::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|o| o.subscription_id == Some(subscription.id))
        .unwrap()
        .id
}

#[tokio::test]
async fn недоступная_remnawave_откладывает_выдачу() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 213).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    kit.remna.fail_next(Fault::Unavailable);
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Retry { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        self::order(&kit.services, order.id).await.status,
        OrderStatus::Paid
    );
    let done = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказ_на_проверке_без_плана_не_держит_следующие() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 214).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let broken = tariff(
        &kit.services,
        standard.category_id,
        "Стандарт плюс",
        TrafficPolicy::UNLIMITED,
        vec![squad(1), squad(77)],
    )
    .await;
    let first = buy_with_balance(
        &kit.services,
        client_id,
        cart(&broken, 30, Some(&subscription)),
    )
    .await;
    let second = buy_with_balance(
        &kit.services,
        client_id,
        cart(&standard, 30, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    assert!(matches!(
        fulfillment.run(first.id).await.unwrap(),
        FulfillmentOutcome::Review { .. }
    ));
    let done = fulfillment.run(second.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказ_ждёт_сохранённый_план_заказа_с_большим_номером() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 221).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(10));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let (opened, payment_id) =
        invoice(&kit, client_id, cart(&tariff, 30, Some(&subscription))).await;
    let paid = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    assert!(opened.id < paid.id);
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("update_user", Fault::Timeout);
    let first = fulfillment.run(paid.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    assert_eq!(remna_user(&kit, &subscription).await.expire_at, expire);
    pay(&kit, payment_id).await;
    assert_eq!(
        order(&kit.services, opened.id).await.status,
        OrderStatus::Paid
    );
    let waiting = fulfillment.run(opened.id).await.unwrap();
    assert!(
        matches!(&waiting, FulfillmentOutcome::Retry { reason, .. } if reason.contains(&paid.public_id.to_string())),
        "{waiting:?}"
    );
    assert_eq!(remna_user(&kit, &subscription).await.expire_at, expire);
    let done = fulfillment.run(paid.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let done = fulfillment.run(opened.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + Days::days(60)
    );
    for id in [opened.id, paid.id] {
        assert_eq!(
            order(&kit.services, id).await.status,
            OrderStatus::Fulfilled
        );
    }
    kit.db.cleanup().await;
}

#[tokio::test]
async fn дни_добавленные_после_сохранения_плана_не_затираются() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 222).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(10));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("update_user", Fault::Timeout);
    let first = fulfillment.run(renewal.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    kit.services
        .subscriptions()
        .add_days(subscription.id, 7, ADMIN)
        .await
        .unwrap();
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + Days::days(7)
    );
    let second = fulfillment.run(renewal.id).await.unwrap();
    assert!(
        matches!(&second, FulfillmentOutcome::Review { reason } if reason.contains("срок")),
        "{second:?}"
    );
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + Days::days(7)
    );
    assert_eq!(
        order(&kit.services, renewal.id).await.status,
        OrderStatus::Review
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn применённый_план_не_отправляется_повторно_и_не_затирает_дни() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 223).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let subscription = set_user(&kit, &subscription, |user| {
        user.user_traffic.used_traffic_bytes = 50 * GIB as i64;
    })
    .await;
    let unblock_category = category(&kit.services, "Unblock").await;
    let unblock = tariff(
        &kit.services,
        unblock_category,
        "Unblock",
        TrafficPolicy {
            limit_bytes: Some(300 * GIB),
            reset: TrafficReset::Month,
        },
        vec![squad(3), squad(1)],
    )
    .await;
    let change = buy_with_balance(
        &kit.services,
        client_id,
        cart(&unblock, 90, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("reset_user_traffic", Fault::Timeout);
    let first = fulfillment.run(change.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    let change = order(&kit.services, change.id).await;
    let target = term(change.paid_at, 90);
    assert_eq!(remna_user(&kit, &subscription).await.expire_at, target);
    kit.services
        .subscriptions()
        .add_days(subscription.id, 7, ADMIN)
        .await
        .unwrap();
    let second = fulfillment.run(change.id).await.unwrap();
    assert!(
        matches!(second, FulfillmentOutcome::Done { .. }),
        "{second:?}"
    );
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.expire_at, target + Days::days(7));
    assert_eq!(user.user_traffic.used_traffic_bytes, 0);
    assert_eq!(kit.remna.calls_of("update_user").len(), 1);
    assert_eq!(kit.remna.calls_of("reset_user_traffic").len(), 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn продление_в_тариф_с_лимитом_той_же_категории_сбрасывает_трафик() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 224).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let subscription = set_user(&kit, &subscription, |user| {
        user.user_traffic.used_traffic_bytes = TIB as i64;
    })
    .await;
    let limited = tariff(
        &kit.services,
        standard.category_id,
        "Стандарт 300",
        TrafficPolicy {
            limit_bytes: Some(300 * GIB),
            reset: TrafficReset::Month,
        },
        vec![squad(1), squad(2)],
    )
    .await;
    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&limited, 30, Some(&subscription)),
    )
    .await;
    assert_eq!(renewal.kind, OrderKind::Renewal);
    let done = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.traffic_limit_bytes, (300 * GIB) as i64);
    assert_eq!(user.traffic_limit_strategy, TrafficStrategy::Month);
    assert_eq!(user.user_traffic.used_traffic_bytes, 0);
    assert_eq!(user.status, sn_remnawave::UserStatus::Active);
    assert_eq!(kit.remna.calls_of("reset_user_traffic").len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn разблокированную_истёкшую_подписку_можно_продлить() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 225).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    kit.services
        .clients()
        .block(client_id, Some("проверка"), ADMIN)
        .await
        .unwrap();
    let subscription = expire_in(
        &kit,
        &subscription,
        moment(Days::days(-3)),
        SubscriptionStatus::Disabled,
    )
    .await;
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.status, sn_remnawave::UserStatus::Expired);
    let cart = cart(&tariff, 30, Some(&subscription));
    let quote = kit
        .services
        .checkout()
        .quote(client_id, &cart)
        .await
        .unwrap();
    assert_eq!(quote.kind, PlanKind::Renewal);
    let renewal = buy_with_balance(&kit.services, client_id, cart).await;
    let done = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let renewal = order(&kit.services, renewal.id).await;
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.status, sn_remnawave::UserStatus::Active);
    assert_eq!(user.expire_at, term(renewal.paid_at, 30));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оплата_заблокированного_клиента_уходит_на_проверку() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 226).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let fresh = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let other = client(&kit.services, 227).await;
    let subscription = provision(&kit, other, &tariff, 30).await;
    let renewal =
        buy_with_balance(&kit.services, other, cart(&tariff, 30, Some(&subscription))).await;
    for id in [client_id, other] {
        kit.services
            .clients()
            .block(id, Some("нарушение"), ADMIN)
            .await
            .unwrap();
    }
    let users_before = kit.remna.users().len();
    for order_id in [fresh.id, renewal.id] {
        let outcome = kit.services.fulfillment().run(order_id).await.unwrap();
        assert!(
            matches!(&outcome, FulfillmentOutcome::Review { reason } if reason.contains("заблокирован")),
            "{outcome:?}"
        );
        assert_eq!(
            order(&kit.services, order_id).await.status,
            OrderStatus::Review
        );
    }
    assert_eq!(kit.remna.users().len(), users_before);
    assert_eq!(
        remna_user(&kit, &subscription).await.status,
        sn_remnawave::UserStatus::Disabled
    );
    assert!(kit.remna.calls_of("update_user").is_empty());
    assert!(kit.remna.calls_of("create_user").len() == 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn блокировка_отменяет_открытые_счета_а_поздняя_оплата_возвращается() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 228).await;
    let tariff = standard(&kit.services).await;
    let (opened, payment_id) = invoice(&kit, client_id, cart(&tariff, 30, None)).await;
    assert_eq!(opened.status, OrderStatus::Pending);
    kit.services
        .clients()
        .block(client_id, Some("мошенничество"), ADMIN)
        .await
        .unwrap();
    assert_eq!(
        payment(&kit, payment_id).await.status,
        PaymentStatus::Canceled
    );
    assert_eq!(
        order(&kit.services, opened.id).await.status,
        OrderStatus::Canceled
    );
    let audit = audit_actions(&kit, "client", client_id.0).await;
    let block = audit.iter().find(|a| a.action == "client.block").unwrap();
    assert_eq!(
        block.details["canceled_orders"],
        serde_json::json!([opened.id])
    );
    pay(&kit, payment_id).await;
    assert_eq!(
        order(&kit.services, opened.id).await.status,
        OrderStatus::Paid
    );
    let outcome = kit.services.fulfillment().run(opened.id).await.unwrap();
    assert!(
        matches!(&outcome, FulfillmentOutcome::Refunded { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        order(&kit.services, opened.id).await.status,
        OrderStatus::Refunded
    );
    let refunded = payment(&kit, payment_id).await;
    assert_eq!(refunded.status, PaymentStatus::Refunded);
    let sent = kit.pay.refunds.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].amount, refunded.amount);
    assert!(kit.remna.users().is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn продление_снимает_сквад_убранный_из_тарифа() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 229).await;
    let family_category = category(&kit.services, "Семейный").await;
    let family = tariff(
        &kit.services,
        family_category,
        "Семейный",
        TrafficPolicy::UNLIMITED,
        vec![squad(1), squad(2), squad(3)],
    )
    .await;
    let subscription = provision(&kit, client_id, &family, 30).await;
    let subscription = set_user(&kit, &subscription, |user| {
        user.active_internal_squads.push(sn_remnawave::SquadRef {
            uuid: squad(9).0,
            name: "Купленный отдельно".into(),
        });
    })
    .await;
    let family = update_tariff(&kit, &family, vec![squad(1), squad(2)], 3).await;
    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&family, 30, Some(&subscription)),
    )
    .await;
    let done = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        remna_user(&kit, &subscription).await.squad_ids(),
        vec![squad(1), squad(2), squad(9)]
    );
    kit.db.cleanup().await;
}

struct PaidDevices {
    client_id: ClientId,
    tariff: Tariff,
    subscription: Subscription,
    expire: chrono::DateTime<Utc>,
}

async fn with_paid_devices(kit: &Kit, telegram_id: i64) -> PaidDevices {
    let client_id = client(&kit.services, telegram_id).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(kit, client_id, &tariff, 30).await;
    let subscription = give_extra_devices(kit, &subscription, 2).await;
    let expire = moment(Days::days(20));
    let subscription = expire_in(kit, &subscription, expire, SubscriptionStatus::Active).await;
    let user_id = subscription.remna.as_ref().unwrap().user_id;
    for (index, age) in [5_i64, 1, 4, 2, 3].into_iter().enumerate() {
        let mut device =
            sample_device(user_id, &format!("hw-{index}"), Utc::now() - Days::days(30));
        device.updated_at = Utc::now() - Days::days(age);
        kit.remna.add_device(device);
    }
    PaidDevices {
        client_id,
        tariff,
        subscription,
        expire,
    }
}

async fn renew(kit: &Kit, paid: &PaidDevices) -> Order {
    let renewal = buy_with_balance(
        &kit.services,
        paid.client_id,
        cart(&paid.tariff, 30, Some(&paid.subscription)),
    )
    .await;
    let done = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    renewal
}

async fn device_hold(kit: &Kit, subscription: &Subscription) -> Option<(i32, i32)> {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT held_limit, limit_after FROM subscription_device_holds WHERE subscription_id = $1",
    )
    .bind(subscription.id.0)
    .fetch_optional(conn.conn())
    .await
    .unwrap()
}

#[tokio::test]
async fn продление_без_доп_устройств_не_уменьшает_оплаченный_лимит() {
    let Some(kit) = kit().await else { return };
    let paid = with_paid_devices(&kit, 230).await;
    let link = paid.subscription.remna.clone().unwrap();
    let renewal = renew(&kit, &paid).await;
    let user = remna_user(&kit, &paid.subscription).await;
    assert_eq!(user.expire_at, paid.expire + Days::days(30));
    assert_eq!(user.hwid_device_limit, Some(5));
    assert_eq!(kit.remna.devices(link.user_id).len(), 5);
    let renewed = self::subscription(&kit.services, &renewal).await;
    assert_eq!(renewed.extra_devices, 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оплаченные_устройства_снимаются_после_старой_даты_окончания() {
    let Some(kit) = kit().await else { return };
    let paid = with_paid_devices(&kit, 231).await;
    let user_id = paid.subscription.remna.as_ref().unwrap().user_id;
    renew(&kit, &paid).await;
    assert_eq!(device_hold(&kit, &paid.subscription).await, Some((5, 3)));
    renew(&kit, &paid).await;
    let user = remna_user(&kit, &paid.subscription).await;
    assert_eq!(user.expire_at, paid.expire + Days::days(60));
    assert_eq!(user.hwid_device_limit, Some(5));
    assert_eq!(device_hold(&kit, &paid.subscription).await, Some((5, 3)));
    let ops = kit.services.subscriptions();
    assert_eq!(
        ops.release_held_devices(paid.expire - Days::minutes(1))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        remna_user(&kit, &paid.subscription).await.hwid_device_limit,
        Some(5)
    );
    assert_eq!(
        ops.release_held_devices(paid.expire + Days::seconds(1))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        remna_user(&kit, &paid.subscription).await.hwid_device_limit,
        Some(3)
    );
    let left: Vec<String> = kit
        .remna
        .devices(user_id)
        .into_iter()
        .map(|d| d.hwid)
        .collect();
    assert_eq!(left, vec!["hw-1", "hw-3", "hw-4"]);
    let stored = self::subscription_by_id(&kit, paid.subscription.id).await;
    assert_eq!(stored.mirror.hwid_limit, Some(3));
    assert_eq!(stored.mirror.devices_count, Some(3));
    let audit = audit_actions(&kit, "subscription", paid.subscription.id.0).await;
    assert_eq!(
        audit[0].action,
        crate::subscriptions::actions::RELEASE_DEVICES
    );
    assert_eq!(
        audit[0].details["removed_devices"],
        serde_json::json!(["hw-0", "hw-2"])
    );
    assert_eq!(device_hold(&kit, &paid.subscription).await, None);
    assert_eq!(
        ops.release_held_devices(paid.expire + Days::days(1))
            .await
            .unwrap(),
        0
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn просроченное_удержание_не_продлевается_следующим_продлением() {
    let Some(kit) = kit().await else { return };
    let paid = with_paid_devices(&kit, 235).await;
    renew(&kit, &paid).await;
    assert_eq!(device_hold(&kit, &paid.subscription).await, Some((5, 3)));
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE subscription_device_holds SET held_until = $2 WHERE subscription_id = $1",
        )
        .bind(paid.subscription.id.0)
        .bind(Utc::now() - Days::minutes(1))
        .execute(conn.conn())
        .await
        .unwrap();
    }
    renew(&kit, &paid).await;
    assert_eq!(
        remna_user(&kit, &paid.subscription).await.hwid_device_limit,
        Some(3)
    );
    assert_eq!(device_hold(&kit, &paid.subscription).await, None);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn уменьшение_устройств_в_тарифе_действует_с_нового_срока() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 234).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(12));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let tariff = update_tariff(&kit, &tariff, vec![squad(1), squad(2)], 2).await;
    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let done = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        remna_user(&kit, &subscription).await.hwid_device_limit,
        Some(3)
    );
    assert_eq!(device_hold(&kit, &subscription).await, Some((3, 2)));
    kit.services
        .subscriptions()
        .release_held_devices(expire + Days::seconds(1))
        .await
        .unwrap();
    assert_eq!(
        remna_user(&kit, &subscription).await.hwid_device_limit,
        Some(2)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ручная_правка_доп_устройств_отменяет_отложенное_снятие() {
    let Some(kit) = kit().await else { return };
    let paid = with_paid_devices(&kit, 232).await;
    renew(&kit, &paid).await;
    assert!(device_hold(&kit, &paid.subscription).await.is_some());
    kit.services
        .subscriptions()
        .change_limits(
            paid.subscription.id,
            &crate::subscriptions::LimitsChange {
                traffic_limit_bytes: None,
                extra_devices: Some(1),
                devices: None,
            },
            ADMIN,
        )
        .await
        .unwrap();
    assert_eq!(device_hold(&kit, &paid.subscription).await, None);
    assert_eq!(
        kit.services
            .subscriptions()
            .release_held_devices(paid.expire + Days::seconds(1))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        remna_user(&kit, &paid.subscription).await.hwid_device_limit,
        Some(4)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn смена_тарифа_снимает_отложенные_устройства_сразу() {
    let Some(kit) = kit().await else { return };
    let paid = with_paid_devices(&kit, 233).await;
    renew(&kit, &paid).await;
    assert!(device_hold(&kit, &paid.subscription).await.is_some());
    let unblock_category = category(&kit.services, "Unblock").await;
    let unblock = tariff(
        &kit.services,
        unblock_category,
        "Unblock",
        TrafficPolicy::UNLIMITED,
        vec![squad(3)],
    )
    .await;
    let current = self::subscription_by_id(&kit, paid.subscription.id).await;
    let change = buy_with_balance(
        &kit.services,
        paid.client_id,
        cart(&unblock, 30, Some(&current)),
    )
    .await;
    let done = kit.services.fulfillment().run(change.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        remna_user(&kit, &paid.subscription).await.hwid_device_limit,
        Some(3)
    );
    assert_eq!(device_hold(&kit, &paid.subscription).await, None);
    kit.db.cleanup().await;
}

async fn paid_with_microseconds(kit: &Kit, order_id: OrderId) {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query(
        "UPDATE orders SET paid_at = date_trunc('milliseconds', paid_at) + interval '7 microseconds' \
         WHERE id = $1",
    )
    .bind(order_id.0)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn request_refund(kit: &Kit, order_id: OrderId) -> i64 {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_scalar(
        "INSERT INTO provider_refunds (payment_id, amount_minor, currency, subscription_action, reason) \
         SELECT id, amount_minor, currency, 'keep', 'клиент передумал' FROM payments \
         WHERE order_id = $1 ORDER BY id LIMIT 1 RETURNING id",
    )
    .bind(order_id.0)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn resolve_refund(kit: &Kit, request_id: i64, status: &str) {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query("UPDATE provider_refunds SET status = $2, resolved_at = now() WHERE id = $1")
        .bind(request_id)
        .bind(status)
        .execute(conn.conn())
        .await
        .unwrap();
}

#[tokio::test]
async fn повтор_после_применённого_patch_без_ответа_завершает_заказ() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 291).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let unblock_category = category(&kit.services, "Unblock").await;
    let unblock = tariff(
        &kit.services,
        unblock_category,
        "Unblock",
        TrafficPolicy::UNLIMITED,
        vec![squad(3), squad(1)],
    )
    .await;
    let change = buy_with_balance(
        &kit.services,
        client_id,
        cart(&unblock, 30, Some(&subscription)),
    )
    .await;
    paid_with_microseconds(&kit, change.id).await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("update_user", Fault::TimeoutAfterApply);
    let first = fulfillment.run(change.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    let applied = remna_user(&kit, &subscription).await.expire_at;
    set_user(&kit, &subscription, |user| {
        user.expire_at = panel_time(applied);
    })
    .await;
    let second = fulfillment.run(change.id).await.unwrap();
    assert!(
        matches!(second, FulfillmentOutcome::Done { .. }),
        "{second:?}"
    );
    assert_eq!(
        order(&kit.services, change.id).await.status,
        OrderStatus::Fulfilled
    );
    let current = subscription_by_id(&kit, subscription.id).await;
    let renewal =
        buy_with_balance(&kit.services, client_id, cart(&unblock, 30, Some(&current))).await;
    let next = fulfillment.run(renewal.id).await.unwrap();
    assert!(matches!(next, FulfillmentOutcome::Done { .. }), "{next:?}");
    kit.db.cleanup().await;
}

struct Stuck {
    client_id: ClientId,
    tariff: Tariff,
    subscription: Subscription,
    expire: chrono::DateTime<Utc>,
    order: Order,
}

async fn stuck_renewal(kit: &Kit, telegram_id: i64) -> Stuck {
    let client_id = client(&kit.services, telegram_id).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(10));
    let subscription = expire_in(kit, &subscription, expire, SubscriptionStatus::Active).await;
    let stuck = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("update_user", Fault::Timeout);
    let first = fulfillment.run(stuck.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    kit.services
        .subscriptions()
        .add_days(subscription.id, 7, ADMIN)
        .await
        .unwrap();
    let second = fulfillment.run(stuck.id).await.unwrap();
    assert!(
        matches!(&second, FulfillmentOutcome::Review { reason } if reason.contains("срок")),
        "{second:?}"
    );
    Stuck {
        client_id,
        tariff,
        subscription,
        expire,
        order: order(&kit.services, stuck.id).await,
    }
}

#[tokio::test]
async fn заказ_на_проверке_с_неприменённым_планом_не_держит_следующие() {
    let Some(kit) = kit().await else { return };
    let stuck = stuck_renewal(&kit, 292).await;
    assert_eq!(stuck.order.status, OrderStatus::Review);
    let next = buy_with_balance(
        &kit.services,
        stuck.client_id,
        cart(&stuck.tariff, 30, Some(&stuck.subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    let outcome = fulfillment.run(next.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        order(&kit.services, next.id).await.status,
        OrderStatus::Fulfilled
    );
    assert_eq!(
        remna_user(&kit, &stuck.subscription).await.expire_at,
        stuck.expire + Days::days(37)
    );
    assert_eq!(
        order(&kit.services, stuck.order.id).await.status,
        OrderStatus::Review
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn выдача_ждёт_решения_кассы_по_возврату() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 293).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let request = request_refund(&kit, order.id).await;
    let fulfillment = kit.services.fulfillment();
    let waiting = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(&waiting, FulfillmentOutcome::Retry { reason, .. } if reason.contains("возврат")),
        "{waiting:?}"
    );
    assert!(kit.remna.users().is_empty());
    assert_eq!(
        self::order(&kit.services, order.id).await.status,
        OrderStatus::Paid
    );
    resolve_refund(&kit, request, "declined").await;
    let done = fulfillment.run(order.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(kit.remna.users().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn блокировка_во_время_создания_пользователя_отключает_новую_подписку() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 294).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let services = kit.services.clone();
    kit.remna.before_create_response(move |_| {
        let services = services.clone();
        Box::pin(async move {
            services
                .clients()
                .block(client_id, Some("мошенничество"), ADMIN)
                .await
                .unwrap();
        })
    });
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = self::order(&kit.services, order.id).await;
    assert_eq!(order.status, OrderStatus::Fulfilled);
    let subscription = subscription(&kit.services, &order).await;
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.status, sn_remnawave::UserStatus::Disabled);
    assert_eq!(
        subscription.mirror.status,
        Some(SubscriptionStatus::Disabled)
    );
    assert_eq!(kit.remna.calls_of("disable_user").len(), 1);
    kit.db.cleanup().await;
}

async fn plan_stage(kit: &Kit, order_id: OrderId) -> PlanStage {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.orders().plan_state(order_id).await.unwrap().stage()
}

async fn job_given_up(kit: &Kit, order_id: OrderId) {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query(
        "UPDATE jobs SET status = 'review', last_error = 'ручная проверка' WHERE dedupe_key = $1",
    )
    .bind(format!("fulfill:{order_id}"))
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn expire_at(kit: &Kit, subscription: &Subscription) -> chrono::DateTime<Utc> {
    remna_user(kit, subscription).await.expire_at
}

#[tokio::test]
async fn пересчитать_и_выдать_считает_план_от_текущего_срока() {
    let Some(kit) = kit().await else { return };
    let stuck = stuck_renewal(&kit, 295).await;
    job_given_up(&kit, stuck.order.id).await;
    assert_eq!(plan_stage(&kit, stuck.order.id).await, PlanStage::Stale);
    let fulfillment = kit.services.fulfillment();
    let refused = fulfillment
        .redeliver(stuck.order.id, ADMIN)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "order.plan_stale");
    let updates = kit.remna.calls_of("update_user").len();
    let again = fulfillment.run(stuck.order.id).await.unwrap();
    assert!(
        matches!(&again, FulfillmentOutcome::Review { reason }
            if reason.contains("Пересчитать и выдать") && reason.contains("срок")),
        "{again:?}"
    );
    assert_eq!(kit.remna.calls_of("update_user").len(), updates);
    assert_eq!(
        expire_at(&kit, &stuck.subscription).await,
        stuck.expire + Days::days(7)
    );
    let queued = fulfillment
        .recalculate(stuck.order.id, ADMIN)
        .await
        .unwrap();
    assert_eq!(queued.plan, PlanStage::Stale);
    assert_eq!(queued.state, "requeued");
    assert_eq!(plan_stage(&kit, stuck.order.id).await, PlanStage::None);
    let reset = order(&kit.services, stuck.order.id).await;
    assert_eq!(reset.status, OrderStatus::Review);
    assert_eq!(reset.plan, None);
    assert!(
        jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder {
                order_id: stuck.order.id
            })
    );
    let done = fulfillment.run(stuck.order.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        expire_at(&kit, &stuck.subscription).await,
        stuck.expire + Days::days(37)
    );
    let delivered = order(&kit.services, stuck.order.id).await;
    assert_eq!(delivered.status, OrderStatus::Fulfilled);
    assert_eq!(
        delivered.plan.as_ref().and_then(|p| p.base_expire_at),
        Some(stuck.expire + Days::days(7))
    );
    assert_eq!(plan_stage(&kit, stuck.order.id).await, PlanStage::Applied);
    let audit = audit_actions(&kit, "order", stuck.order.id.0).await;
    let record = audit
        .iter()
        .find(|a| a.action == "order.recalculate")
        .unwrap();
    assert_eq!(record.actor_kind, "admin");
    assert_eq!(record.details["plan"], "stale");
    let previous: chrono::DateTime<Utc> =
        serde_json::from_value(record.details["previous_plan"]["target_expire_at"].clone())
            .unwrap();
    assert_eq!(previous, stuck.expire + Days::days(30));
    let closed = fulfillment
        .recalculate(stuck.order.id, ADMIN)
        .await
        .unwrap_err();
    assert_eq!(closed.code(), "order.review_status");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отметить_выданным_завершает_заказ_без_записи_в_remnawave() {
    let Some(kit) = kit().await else { return };
    let stuck = stuck_renewal(&kit, 296).await;
    job_given_up(&kit, stuck.order.id).await;
    let fulfillment = kit.services.fulfillment();
    let writes = |kit: &Kit| {
        kit.remna
            .calls()
            .iter()
            .filter(|call| !call.starts_with("get_"))
            .count()
    };
    let calls = writes(&kit);
    for note in ["", "   ", &"я".repeat(MAX_FULFILL_NOTE + 1)] {
        let err = fulfillment
            .mark_fulfilled(stuck.order.id, note, ADMIN)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "order.mark_fulfilled_note");
    }
    assert_eq!(
        order(&kit.services, stuck.order.id).await.status,
        OrderStatus::Review
    );
    let closed = fulfillment
        .mark_fulfilled(
            stuck.order.id,
            "  Срок в панели уже учитывает эту оплату  ",
            ADMIN,
        )
        .await
        .unwrap();
    assert_eq!(closed.status, OrderStatus::Fulfilled);
    assert!(closed.fulfilled_at.is_some());
    assert_eq!(writes(&kit), calls);
    assert_eq!(
        expire_at(&kit, &stuck.subscription).await,
        stuck.expire + Days::days(7)
    );
    assert!(
        events(&kit.services, stuck.client_id)
            .await
            .iter()
            .any(|e| matches!(
                e,
                DomainEvent::OrderFulfilled { order_id, .. } if *order_id == stuck.order.id
            ))
    );
    let audit = audit_actions(&kit, "order", stuck.order.id.0).await;
    assert_eq!(audit[0].action, "order.mark_fulfilled");
    assert_eq!(audit[0].actor_kind, "admin");
    assert_eq!(
        audit[0].details["note"],
        "Срок в панели уже учитывает эту оплату"
    );
    assert_eq!(audit[0].details["plan"], "stale");
    assert!(
        audit[0].details["review_reason"]
            .as_str()
            .unwrap()
            .contains("срок")
    );
    for err in [
        fulfillment
            .mark_fulfilled(stuck.order.id, "ещё раз", ADMIN)
            .await
            .unwrap_err(),
        fulfillment
            .recalculate(stuck.order.id, ADMIN)
            .await
            .unwrap_err(),
    ] {
        assert_eq!(err.code(), "order.review_status");
    }
    let next = buy_with_balance(
        &kit.services,
        stuck.client_id,
        cart(&stuck.tariff, 30, Some(&stuck.subscription)),
    )
    .await;
    let done = fulfillment.run(next.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        expire_at(&kit, &stuck.subscription).await,
        stuck.expire + Days::days(37)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn устаревший_план_не_применяется_когда_срок_совпал_с_его_целью() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 297).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(10));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let held = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("update_user", Fault::Timeout);
    let first = fulfillment.run(held.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.orders()
            .mark_review(held.id, "выдача не завершилась")
            .await
            .unwrap();
    }
    assert_eq!(plan_stage(&kit, held.id).await, PlanStage::Stale);
    let next = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let done = fulfillment.run(next.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        expire_at(&kit, &subscription).await,
        expire + Days::days(30)
    );
    let stale = fulfillment.run(held.id).await.unwrap();
    assert!(
        matches!(&stale, FulfillmentOutcome::Review { reason } if reason.contains("Отметить выданным")),
        "{stale:?}"
    );
    assert_eq!(
        order(&kit.services, held.id).await.status,
        OrderStatus::Review
    );
    assert_eq!(
        expire_at(&kit, &subscription).await,
        expire + Days::days(30)
    );
    fulfillment.recalculate(held.id, ADMIN).await.unwrap();
    let recalculated = fulfillment.run(held.id).await.unwrap();
    assert!(
        matches!(recalculated, FulfillmentOutcome::Done { .. }),
        "{recalculated:?}"
    );
    assert_eq!(
        expire_at(&kit, &subscription).await,
        expire + Days::days(60)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пересчёт_отклоняется_пока_выдача_заказа_идёт() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 298).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let expire = moment(Days::days(10));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let wide = tariff(
        &kit.services,
        standard.category_id,
        "Стандарт плюс",
        TrafficPolicy::UNLIMITED,
        vec![squad(1), squad(77)],
    )
    .await;
    let order = buy_with_balance(
        &kit.services,
        client_id,
        cart(&wide, 30, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    let review = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(review, FulfillmentOutcome::Review { .. }),
        "{review:?}"
    );
    job_given_up(&kit, order.id).await;
    assert_eq!(plan_stage(&kit, order.id).await, PlanStage::None);
    let mut squads = kit.remna.squads();
    squads.push(sn_remnawave::InternalSquad {
        uuid: squad(77).0,
        name: "Сквад 77".into(),
        view_position: 0,
        info: sn_remnawave::SquadInfo::default(),
        inbounds: Vec::new(),
    });
    kit.remna.set_squads(squads);
    let queued = fulfillment.redeliver(order.id, ADMIN).await.unwrap();
    assert_eq!(queued.state, "requeued");
    kit.remna.fail_on("update_user", Fault::Timeout);
    let retry = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(retry, FulfillmentOutcome::Retry { .. }),
        "{retry:?}"
    );
    assert_eq!(plan_stage(&kit, order.id).await, PlanStage::Saved);
    let busy = fulfillment.recalculate(order.id, ADMIN).await.unwrap_err();
    assert_eq!(busy.code(), "order.delivery_running");
    let waiting = buy_with_balance(
        &kit.services,
        client_id,
        cart(&standard, 30, Some(&subscription)),
    )
    .await;
    let held = fulfillment.run(waiting.id).await.unwrap();
    assert!(
        matches!(&held, FulfillmentOutcome::Retry { reason, .. } if reason.contains(&order.public_id.to_string())),
        "{held:?}"
    );
    let done = fulfillment.run(order.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let next = fulfillment.run(waiting.id).await.unwrap();
    assert!(matches!(next, FulfillmentOutcome::Done { .. }), "{next:?}");
    assert_eq!(
        expire_at(&kit, &subscription).await,
        expire + Days::days(60)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказ_на_проверке_с_применённым_планом_держит_следующие_пока_его_не_закроют() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 299).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let unblock_category = category(&kit.services, "Unblock").await;
    let unblock = tariff(
        &kit.services,
        unblock_category,
        "Unblock",
        TrafficPolicy {
            limit_bytes: Some(300 * GIB),
            reset: TrafficReset::Month,
        },
        vec![squad(3), squad(1)],
    )
    .await;
    let change = buy_with_balance(
        &kit.services,
        client_id,
        cart(&unblock, 30, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("reset_user_traffic", Fault::Timeout);
    let first = fulfillment.run(change.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    assert_eq!(plan_stage(&kit, change.id).await, PlanStage::Applied);
    let user_id = subscription.remna.as_ref().unwrap().user_id;
    kit.remna.delete_user(user_id).await.unwrap();
    let lost = fulfillment.run(change.id).await.unwrap();
    assert!(
        matches!(&lost, FulfillmentOutcome::Review { reason } if reason.contains("удалён")),
        "{lost:?}"
    );
    job_given_up(&kit, change.id).await;
    assert_eq!(plan_stage(&kit, change.id).await, PlanStage::Applied);
    let listed = fulfillment.review_orders(10).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].order.order_id, change.id);
    assert_eq!(listed[0].order.plan, PlanStage::Applied);
    assert_eq!(
        listed[0].actions,
        vec![
            ReviewAction::Redeliver,
            ReviewAction::Recalculate,
            ReviewAction::MarkFulfilled
        ]
    );
    let current = subscription_by_id(&kit, subscription.id).await;
    let renewal =
        buy_with_balance(&kit.services, client_id, cart(&unblock, 30, Some(&current))).await;
    let held = fulfillment.run(renewal.id).await.unwrap();
    assert!(
        matches!(&held, FulfillmentOutcome::Retry { reason, .. } if reason.contains(&change.public_id.to_string())),
        "{held:?}"
    );
    fulfillment
        .mark_fulfilled(change.id, "Пользователя удалили в панели вручную", ADMIN)
        .await
        .unwrap();
    assert_eq!(
        events(&kit.services, client_id)
            .await
            .iter()
            .filter(|e| matches!(
                e,
                DomainEvent::OrderFulfilled { order_id, .. } if *order_id == change.id
            ))
            .count(),
        1
    );
    let done = fulfillment.run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(kit.remna.users().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn список_заказов_на_проверке_показывает_причину_план_и_действия() {
    let Some(kit) = kit().await else { return };
    let stuck = stuck_renewal(&kit, 300).await;
    let fulfillment = kit.services.fulfillment();
    let listed = fulfillment.review_orders(10).await.unwrap();
    assert_eq!(listed.len(), 1);
    let item = &listed[0];
    assert_eq!(item.order.order_id, stuck.order.id);
    assert_eq!(item.order.public_id, stuck.order.public_id);
    assert_eq!(item.order.client_id, stuck.client_id);
    assert_eq!(item.order.subscription_id, Some(stuck.subscription.id));
    assert_eq!(item.order.kind, OrderKind::Renewal);
    assert_eq!(item.order.tariff.as_deref(), Some("Стандарт"));
    assert_eq!(item.order.total, stuck.order.quote.total);
    assert!(item.order.reason.as_deref().unwrap().contains("срок"));
    assert_eq!(item.order.plan, PlanStage::Stale);
    assert_eq!(item.order.plan_base_expire_at, Some(stuck.expire));
    assert_eq!(
        item.order.plan_target_expire_at,
        Some(stuck.expire + Days::days(30))
    );
    assert!(!item.order.refund_pending);
    assert_eq!(
        item.order.job.as_ref().map(|j| j.status),
        Some(sn_domain::jobs::JobStatus::Queued)
    );
    assert_eq!(
        item.actions,
        vec![ReviewAction::Recalculate, ReviewAction::MarkFulfilled]
    );
    let json = serde_json::to_value(item).unwrap();
    assert_eq!(json["order_id"], stuck.order.id.0);
    assert_eq!(json["plan"], "stale");
    assert_eq!(
        json["actions"],
        serde_json::json!(["recalculate", "mark_fulfilled"])
    );
    let request = request_refund(&kit, stuck.order.id).await;
    let listed = fulfillment.review_orders(10).await.unwrap();
    assert!(listed[0].order.refund_pending);
    assert_eq!(listed[0].actions, vec![ReviewAction::MarkFulfilled]);
    for err in [
        fulfillment
            .recalculate(stuck.order.id, ADMIN)
            .await
            .unwrap_err(),
        fulfillment
            .redeliver(stuck.order.id, ADMIN)
            .await
            .unwrap_err(),
    ] {
        assert_eq!(err.code(), "order.refund_pending");
    }
    resolve_refund(&kit, request, "declined").await;
    fulfillment
        .mark_fulfilled(stuck.order.id, "Клиенту продлили вручную", ADMIN)
        .await
        .unwrap();
    assert!(fulfillment.review_orders(10).await.unwrap().is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ручной_повтор_выдачи_с_ожидающим_возвратом_отклоняется() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 301).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    job_given_up(&kit, order.id).await;
    let request = request_refund(&kit, order.id).await;
    let fulfillment = kit.services.fulfillment();
    let err = fulfillment.redeliver(order.id, ADMIN).await.unwrap_err();
    assert_eq!(err.code(), "order.refund_pending");
    assert!(err.to_string().contains("возврат"), "{err}");
    assert!(jobs(&kit.services).await.is_empty());
    resolve_refund(&kit, request, "declined").await;
    let queued = fulfillment.redeliver(order.id, ADMIN).await.unwrap();
    assert_eq!(queued.state, "requeued");
    let done = fulfillment.run(order.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказ_на_проверке_держит_автопродление_только_пока_его_план_в_работе() {
    let Some(kit) = kit().await else { return };
    let stuck = stuck_renewal(&kit, 302).await;
    let paid = buy_with_balance(
        &kit.services,
        stuck.client_id,
        cart(&stuck.tariff, 30, Some(&stuck.subscription)),
    )
    .await;
    let mut conn = kit.services.store().conn().await.unwrap();
    assert!(!holds_autorenew(&mut conn, stuck.order.id).await.unwrap());
    assert!(holds_autorenew(&mut conn, paid.id).await.unwrap());
    sqlx::query("UPDATE payments SET method = $2 WHERE order_id = $1")
        .bind(stuck.order.id.0)
        .bind(crate::autorenew::AUTORENEW_METHOD)
        .execute(conn.conn())
        .await
        .unwrap();
    assert!(holds_autorenew(&mut conn, stuck.order.id).await.unwrap());
    sqlx::query("UPDATE payments SET method = NULL WHERE order_id = $1")
        .bind(stuck.order.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    conn.orders().reset_plan(stuck.order.id).await.unwrap();
    assert!(!holds_autorenew(&mut conn, stuck.order.id).await.unwrap());
    let plan = paid_plan(&stuck);
    conn.orders()
        .save_plan(stuck.order.id, &plan)
        .await
        .unwrap();
    assert!(holds_autorenew(&mut conn, stuck.order.id).await.unwrap());
    conn.orders()
        .mark_review(stuck.order.id, "снова на проверке")
        .await
        .unwrap();
    assert!(!holds_autorenew(&mut conn, stuck.order.id).await.unwrap());
    conn.orders()
        .mark_plan_applied(stuck.order.id, Utc::now())
        .await
        .unwrap();
    assert!(holds_autorenew(&mut conn, stuck.order.id).await.unwrap());
    drop(conn);
    kit.db.cleanup().await;
}

fn paid_plan(stuck: &Stuck) -> FulfillmentPlan {
    let mut plan = stuck.order.plan.clone().unwrap();
    plan.base_expire_at = Some(stuck.expire + Days::days(7));
    plan.target.expire_at = stuck.expire + Days::days(37);
    plan
}

#[tokio::test]
async fn пользователь_созданный_до_блокировки_отключается_при_повторе() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 303).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("create_user", Fault::TimeoutAfterApply);
    let first = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    assert_eq!(kit.remna.users().len(), 1);
    assert_eq!(
        kit.remna.users()[0].status,
        sn_remnawave::UserStatus::Active
    );
    kit.services
        .clients()
        .block(client_id, Some("мошенничество"), ADMIN)
        .await
        .unwrap();
    let second = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(second, FulfillmentOutcome::Done { .. }),
        "{second:?}"
    );
    let order = self::order(&kit.services, order.id).await;
    assert_eq!(order.status, OrderStatus::Fulfilled);
    let subscription = subscription(&kit.services, &order).await;
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(kit.remna.users().len(), 1);
    assert_eq!(user.status, sn_remnawave::UserStatus::Disabled);
    assert_eq!(
        subscription.mirror.status,
        Some(SubscriptionStatus::Disabled)
    );
    let audit = audit_actions(&kit, "subscription", subscription.id.0).await;
    assert_eq!(
        audit[0].action,
        crate::subscriptions::actions::BLOCKED_DELIVERY
    );
    assert!(!jobs(&kit.services).await.iter().any(|job| matches!(
        job,
        JobPayload::Notify { template, .. } if template == "order.fulfilled"
    )));
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    assert_eq!(
        remna_user(&kit, &subscription).await.status,
        sn_remnawave::UserStatus::Active
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn блокировка_после_применения_плана_отключает_подписку_при_повторе() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 304).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("get_user", Fault::Timeout);
    let first = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    assert_eq!(plan_stage(&kit, order.id).await, PlanStage::Applied);
    kit.services
        .clients()
        .block(client_id, Some("мошенничество"), ADMIN)
        .await
        .unwrap();
    let second = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(second, FulfillmentOutcome::Done { .. }),
        "{second:?}"
    );
    let order = self::order(&kit.services, order.id).await;
    let subscription = subscription(&kit.services, &order).await;
    assert_eq!(
        remna_user(&kit, &subscription).await.status,
        sn_remnawave::UserStatus::Disabled
    );
    assert_eq!(
        subscription.mirror.status,
        Some(SubscriptionStatus::Disabled)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пересчёт_применённого_плана_восстанавливает_удалённого_пользователя() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 305).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let unblock_category = category(&kit.services, "Unblock").await;
    let unblock = tariff(
        &kit.services,
        unblock_category,
        "Unblock",
        TrafficPolicy {
            limit_bytes: Some(300 * GIB),
            reset: TrafficReset::Month,
        },
        vec![squad(3), squad(1)],
    )
    .await;
    let change = buy_with_balance(
        &kit.services,
        client_id,
        cart(&unblock, 30, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("reset_user_traffic", Fault::Timeout);
    fulfillment.run(change.id).await.unwrap();
    let old_user = subscription.remna.as_ref().unwrap().user_id;
    kit.remna.delete_user(old_user).await.unwrap();
    let lost = fulfillment.run(change.id).await.unwrap();
    assert!(
        matches!(lost, FulfillmentOutcome::Review { .. }),
        "{lost:?}"
    );
    job_given_up(&kit, change.id).await;
    assert_eq!(plan_stage(&kit, change.id).await, PlanStage::Applied);
    let queued = fulfillment.recalculate(change.id, ADMIN).await.unwrap();
    assert_eq!(queued.plan, PlanStage::Applied);
    assert_eq!(plan_stage(&kit, change.id).await, PlanStage::None);
    let done = fulfillment.run(change.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let change = order(&kit.services, change.id).await;
    assert_eq!(change.status, OrderStatus::Fulfilled);
    let current = subscription_by_id(&kit, subscription.id).await;
    let user = remna_user(&kit, &current).await;
    assert_ne!(user.id, old_user);
    assert_eq!(kit.remna.users().len(), 1);
    assert_eq!(user.expire_at, term(change.paid_at, 30));
    assert_eq!(user.squad_ids(), vec![squad(3), squad(1)]);
    let audit = audit_actions(&kit, "order", change.id.0).await;
    let record = audit
        .iter()
        .find(|a| a.action == "order.recalculate")
        .unwrap();
    assert_eq!(record.details["plan"], "applied");
    assert_eq!(record.details["panel_user_missing"], true);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказ_с_ожидающим_возвратом_не_держит_следующие_заказы_подписки() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 306).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(10));
    let subscription = expire_in(&kit, &subscription, expire, SubscriptionStatus::Active).await;
    let refunded = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let request = request_refund(&kit, refunded.id).await;
    let fulfillment = kit.services.fulfillment();
    let waiting = fulfillment.run(refunded.id).await.unwrap();
    assert!(
        matches!(&waiting, FulfillmentOutcome::Retry { after_seconds, reason }
            if *after_seconds >= 300 && reason.contains("возврат")),
        "{waiting:?}"
    );
    let next = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    let done = fulfillment.run(next.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        expire_at(&kit, &subscription).await,
        expire + Days::days(30)
    );
    resolve_refund(&kit, request, "declined").await;
    let late = fulfillment.run(refunded.id).await.unwrap();
    assert!(matches!(late, FulfillmentOutcome::Done { .. }), "{late:?}");
    assert_eq!(
        expire_at(&kit, &subscription).await,
        expire + Days::days(60)
    );
    kit.db.cleanup().await;
}

fn listed(plan: PlanStage, job: Option<sn_domain::jobs::JobStatus>) -> sn_store::ReviewOrder {
    sn_store::ReviewOrder {
        order_id: OrderId(1),
        public_id: Uuid::nil(),
        client_id: ClientId(1),
        subscription_id: Some(SubscriptionId(1)),
        kind: OrderKind::Renewal,
        tariff: Some("Стандарт".into()),
        total: sn_domain::money::Money::new(299, sn_domain::money::Currency::Usd),
        stars: None,
        reason: None,
        plan,
        plan_base_expire_at: None,
        plan_target_expire_at: None,
        refund_pending: false,
        job: job.map(|status| sn_store::ReviewJob {
            id: sn_domain::ids::JobId(1),
            status,
            attempts: 1,
            error: None,
            updated_at: Utc::now(),
        }),
        paid_at: None,
        updated_at: Utc::now(),
    }
}

#[test]
fn действия_над_заказом_на_проверке_зависят_от_плана_задачи_и_возврата() {
    use ReviewAction::{MarkFulfilled, Recalculate, Redeliver};
    use sn_domain::jobs::JobStatus;
    assert_eq!(
        review_actions(&listed(PlanStage::None, None)),
        vec![Redeliver, MarkFulfilled]
    );
    assert_eq!(
        review_actions(&listed(PlanStage::Stale, Some(JobStatus::Review))),
        vec![Recalculate, MarkFulfilled]
    );
    assert_eq!(
        review_actions(&listed(PlanStage::Applied, Some(JobStatus::Review))),
        vec![Redeliver, Recalculate, MarkFulfilled]
    );
    for stage in [PlanStage::Saved, PlanStage::Applied] {
        for status in [JobStatus::Queued, JobStatus::Running] {
            assert_eq!(
                review_actions(&listed(stage, Some(status))),
                vec![Redeliver, MarkFulfilled]
            );
        }
    }
    let mut topup = listed(PlanStage::None, None);
    topup.kind = OrderKind::TopUp;
    assert_eq!(review_actions(&topup), vec![Redeliver, MarkFulfilled]);
    let mut refund = listed(PlanStage::Stale, None);
    refund.refund_pending = true;
    assert_eq!(review_actions(&refund), vec![MarkFulfilled]);
    assert!(abandoned_reason(12, "Remnawave недоступна").contains("за 12 попыток"));
}

#[tokio::test]
async fn ручной_лимит_устройств_переживает_продление_и_правку_тарифа_а_смена_тарифа_снимает() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 240).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let ops = kit.services.subscriptions();
    let limits = |devices| crate::subscriptions::LimitsChange {
        devices: Some(devices),
        ..Default::default()
    };
    let view = ops
        .change_limits(subscription.id, &limits(4), ADMIN)
        .await
        .unwrap();
    assert_eq!(view.subscription.extra_devices, 1);
    assert_eq!(view.subscription.device_limit, None);
    let view = ops
        .change_limits(subscription.id, &limits(3), ADMIN)
        .await
        .unwrap();
    assert_eq!(view.subscription.extra_devices, 0);
    let view = ops
        .change_limits(subscription.id, &limits(9), ADMIN)
        .await
        .unwrap();
    assert_eq!(view.subscription.device_limit, Some(9));
    assert_eq!(view.subscription.extra_devices, 0);
    assert_eq!(
        remna_user(&kit, &subscription).await.hwid_device_limit,
        Some(9)
    );
    let audit = audit_actions(&kit, "subscription", subscription.id.0).await;
    assert_eq!(audit[0].details["hwid_limit_before"], 3);
    assert_eq!(audit[0].details["hwid_limit"], 9);
    assert_eq!(
        audit[0].details["device_limit_before"],
        serde_json::Value::Null
    );
    assert_eq!(audit[0].details["device_limit"], 9);

    let renewal = buy_with_balance(
        &kit.services,
        client_id,
        cart(&standard, 30, Some(&subscription)),
    )
    .await;
    let done = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        remna_user(&kit, &subscription).await.hwid_device_limit,
        Some(9)
    );
    assert_eq!(
        subscription_by_id(&kit, subscription.id).await.device_limit,
        Some(9)
    );
    assert_eq!(device_hold(&kit, &subscription).await, None);

    let fewer = update_tariff(&kit, &standard, standard.squads.clone(), 2).await;
    let report = kit
        .services
        .tariffs()
        .apply_to_subscribers(&standard, &fewer)
        .await
        .unwrap();
    assert!(report.devices);
    assert_eq!(report.users, 1);
    assert_eq!(
        remna_user(&kit, &subscription).await.hwid_device_limit,
        Some(9)
    );

    let other_category = category(&kit.services, "Другие").await;
    let other = tariff(
        &kit.services,
        other_category,
        "Другой",
        TrafficPolicy::UNLIMITED,
        vec![squad(1), squad(2)],
    )
    .await;
    let change = buy_with_balance(
        &kit.services,
        client_id,
        cart(&other, 90, Some(&subscription)),
    )
    .await;
    let done = kit.services.fulfillment().run(change.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        subscription_by_id(&kit, subscription.id).await.device_limit,
        None
    );
    assert_eq!(
        remna_user(&kit, &subscription).await.hwid_device_limit,
        Some(3)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn лимит_устройств_без_тарифа_и_вне_пределов_становится_ручным() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 241).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE subscriptions SET tariff_id = NULL WHERE id = $1")
            .bind(subscription.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let user_id = subscription.remna.as_ref().unwrap().user_id;
    for index in 0..4 {
        kit.remna.add_device(sample_device(
            user_id,
            &format!("hw-{index}"),
            Utc::now() - Days::days(10 - index),
        ));
    }
    let ops = kit.services.subscriptions();
    let limits = |devices| crate::subscriptions::LimitsChange {
        devices: Some(devices),
        ..Default::default()
    };
    let view = ops
        .change_limits(subscription.id, &limits(2), ADMIN)
        .await
        .unwrap();
    assert_eq!(view.subscription.device_limit, Some(2));
    assert_eq!(
        remna_user(&kit, &subscription).await.hwid_device_limit,
        Some(2)
    );
    assert_eq!(kit.remna.devices(user_id).len(), 2);
    for wrong in [0, 101] {
        let err = ops
            .change_limits(subscription.id, &limits(wrong), ADMIN)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "subscription.devices_range");
    }
    let err = ops
        .change_limits(
            subscription.id,
            &crate::subscriptions::LimitsChange {
                devices: Some(5),
                extra_devices: Some(1),
                ..Default::default()
            },
            ADMIN,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "subscription.devices_twice");
    let err = ops
        .change_limits(subscription.id, &limits(5), Actor::Client(client_id))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "auth.forbidden");
    kit.db.cleanup().await;
}
