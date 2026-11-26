use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use sn_domain::ids::{SquadId, TariffId};
use sn_domain::money::{Currency, Stars};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::subscription::{AutoRenewState, SubscriptionKind};
use sn_domain::tariff::CurrencyPrices;
use uuid::Uuid;

#[tokio::test]
async fn правка_тарифа_выключает_исчезнувший_срок_и_сохраняет_заказ_на_него() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let first = category(&mut conn, "Unblock").await;
    let second = category(&mut conn, "Семейный").await;
    let created = conn
        .tariffs()
        .create(&tariff(
            first,
            vec![period(30, 349, 24_900, 175), period(90, 899, 64_900, 450)],
            &[3, 1],
        ))
        .await
        .unwrap();
    let month = created
        .periods
        .iter()
        .find(|p| p.days == 30)
        .unwrap()
        .clone();
    let quarter = created
        .periods
        .iter()
        .find(|p| p.days == 90)
        .unwrap()
        .clone();
    let client = client(&mut conn, &Person::NOBODY).await;
    let sub = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: Some(created.id),
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState {
                enabled: true,
                period_id: Some(quarter.id),
            },
        })
        .await
        .unwrap();
    let priced = quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff: &created,
        period_id: quarter.id,
        extra_devices: 0,
        sell_extra_devices: true,
        adjustments: &[],
        promotion: None,
    })
    .unwrap();
    let order = conn
        .orders()
        .create(&NewOrder {
            client_id: client,
            subscription_id: Some(sub.id),
            kind: OrderKind::Renewal,
            intent: PurchaseIntent::Subscription {
                tariff_id: created.id,
                period_id: quarter.id,
                extra_devices: 0,
                target: Some(sub.id),
            },
            quote: priced.clone(),
            snapshot: Some(created.snapshot(&quarter)),
        })
        .await
        .unwrap();

    let mut changed = tariff(
        second,
        vec![
            period(30, 399, 29_900, 200),
            period(365, 2_999, 219_000, 1_500),
        ],
        &[2, 3, 2],
    );
    changed.name = "  Unblock Плюс ".into();
    changed.on_sale = false;
    let updated = conn.tariffs().update(created.id, &changed).await.unwrap();

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.category_id, second);
    assert_eq!(updated.name, "Unblock Плюс");
    assert!(!updated.on_sale);
    assert_eq!(
        updated.squads,
        [SquadId(Uuid::from_u128(2)), SquadId(Uuid::from_u128(3))]
    );
    let by_days = |days: u32| updated.periods.iter().find(|p| p.days == days).unwrap();
    assert_eq!(by_days(30).id, month.id);
    assert_eq!(by_days(30).prices.usd, Some(399));
    assert_eq!(by_days(30).stars, Some(Stars(200)));
    assert!(by_days(30).active);
    assert_eq!(by_days(90).id, quarter.id);
    assert!(!by_days(90).active);
    assert_eq!(by_days(90).prices.usd, Some(899));
    assert!(by_days(365).active);
    assert_eq!(updated.periods.len(), 3);
    assert!(
        updated
            .offered_periods(Currency::Usd)
            .iter()
            .all(|p| p.days != 90)
    );

    let stored = conn.orders().get(order.id).await.unwrap();
    assert_eq!(stored.quote, priced);
    assert_eq!(stored.snapshot.unwrap().period_id, quarter.id);
    assert_eq!(
        stored.intent,
        PurchaseIntent::Subscription {
            tariff_id: created.id,
            period_id: quarter.id,
            extra_devices: 0,
            target: Some(sub.id),
        }
    );
    let renewing = conn.subscriptions().get(sub.id).await.unwrap();
    assert_eq!(renewing.autorenew.period_id, Some(quarter.id));

    let mut back = changed.clone();
    back.periods.push(period(90, 949, 69_900, 470));
    let restored = conn.tariffs().update(created.id, &back).await.unwrap();
    let again = restored.periods.iter().find(|p| p.days == 90).unwrap();
    assert_eq!(again.id, quarter.id);
    assert!(again.active);
    assert_eq!(again.prices.usd, Some(949));
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn неудачная_правка_тарифа_не_меняет_ничего() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let first = category(&mut conn, "Unblock").await;
    let created = conn
        .tariffs()
        .create(&tariff(first, vec![period(30, 349, 24_900, 175)], &[1]))
        .await
        .unwrap();

    let mut doubled = tariff(first, vec![period(30, 1, 1, 1), period(30, 2, 2, 2)], &[1]);
    doubled.name = "Дубль".into();
    assert!(matches!(
        conn.tariffs().update(created.id, &doubled).await,
        Err(StoreError::Invalid(_))
    ));

    let mut broken = tariff(first, vec![period(30, 1, 1, 1), period(60, 2, 2, 2)], &[9]);
    broken.name = "Сломанный".into();
    broken.periods[1].stars = Some(Stars(0));
    assert!(matches!(
        conn.tariffs().update(created.id, &broken).await,
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(conn.tariffs().get(created.id).await.unwrap(), created);

    assert!(matches!(
        conn.tariffs()
            .update(TariffId(created.id.0 + 1_000), &broken)
            .await,
        Err(StoreError::NotFound(_))
    ));

    let mut tx = db.store.begin().await.unwrap();
    let mut renamed = tariff(first, vec![period(30, 349, 24_900, 175)], &[1, 1]);
    renamed.name = "Переименован".into();
    renamed.periods[0].prices = CurrencyPrices {
        usd: Some(359),
        rub: None,
    };
    let inside = tx.tariffs().update(created.id, &renamed).await.unwrap();
    assert_eq!(inside.squads.len(), 1);
    tx.rollback().await.unwrap();
    assert_eq!(conn.tariffs().get(created.id).await.unwrap(), created);

    let twice = conn
        .tariffs()
        .create(&tariff(
            first,
            vec![period(7, 1, 1, 1), period(7, 1, 1, 1)],
            &[1],
        ))
        .await;
    assert!(matches!(twice, Err(StoreError::Invalid(_))));
    assert_eq!(conn.tariffs().list(true).await.unwrap().len(), 1);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn лимит_подписок_категории_и_лесенка_скидок_хранятся_и_читаются() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let first = category(&mut conn, "С обходом").await;
    assert_eq!(
        conn.tariffs()
            .category(first)
            .await
            .unwrap()
            .subscription_limit,
        None
    );
    let limited = conn
        .tariffs()
        .set_subscription_limit(first, Some(1))
        .await
        .unwrap();
    assert_eq!(limited.subscription_limit, Some(1));
    let renamed = conn
        .tariffs()
        .update_category(
            first,
            &NewCategory {
                name: "Обход".into(),
                sort: 2,
                visible: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.subscription_limit, Some(1));
    assert_eq!(
        conn.tariffs().categories(true).await.unwrap()[0].subscription_limit,
        Some(1)
    );
    conn.tariffs()
        .set_subscription_limit(first, None)
        .await
        .unwrap();
    assert_eq!(
        conn.tariffs()
            .category(first)
            .await
            .unwrap()
            .subscription_limit,
        None
    );
    assert!(
        conn.tariffs()
            .set_subscription_limit(first, Some(0))
            .await
            .is_err()
    );
    let created = conn
        .tariffs()
        .create(&tariff(first, vec![period(30, 349, 24_900, 175)], &[1]))
        .await
        .unwrap();
    assert!(created.device_discounts.is_empty());
    let ladder = [
        sn_domain::tariff::DeviceDiscount {
            from: 1,
            percent: 5,
        },
        sn_domain::tariff::DeviceDiscount {
            from: 2,
            percent: 10,
        },
    ];
    conn.tariffs()
        .set_device_discounts(created.id, &ladder)
        .await
        .unwrap();
    let stored = conn.tariffs().get(created.id).await.unwrap();
    assert_eq!(stored.device_discounts, ladder.to_vec());
    assert!(
        conn.tariffs()
            .set_device_discounts(TariffId(9_999_999), &ladder)
            .await
            .is_err()
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn тариф_по_номеру_читается_так_же_как_в_списке() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let cat = category(&mut conn, "Основные").await;
    let created = conn
        .tariffs()
        .create(&tariff(
            cat,
            vec![
                period(365, 2_990, 199_000, 1_400),
                period(30, 349, 24_900, 175),
                period(90, 899, 64_900, 450),
            ],
            &[7, 3, 5],
        ))
        .await
        .unwrap();
    let bare = conn
        .tariffs()
        .create(&tariff(cat, Vec::new(), &[]))
        .await
        .unwrap();
    let listed = conn.tariffs().list(true).await.unwrap();
    for id in [created.id, bare.id] {
        let one = conn.tariffs().get(id).await.unwrap();
        let same = listed.iter().find(|t| t.id == id).unwrap();
        assert_eq!(&one, same);
    }
    assert_eq!(created.periods.len(), 3);
    conn.tariffs().soft_delete(bare.id).await.unwrap();
    assert!(matches!(
        conn.tariffs().get(bare.id).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        conn.tariffs().get(TariffId(987_654)).await,
        Err(StoreError::NotFound(_))
    ));
    drop(conn);
    db.cleanup().await;
}
