use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use chrono::{DateTime, Duration, FixedOffset, NaiveTime, Timelike, Utc};
use sn_domain::client::ClientStatus;
use sn_domain::ids::PaymentId;
use sn_domain::money::{Amount, Currency, PayCurrency, Stars};
use sn_domain::payment::{PaymentStatus, RefundMethod, RefundSubscriptionAction};
use sn_domain::subscription::{SubscriptionKind, SubscriptionStatus};

async fn paid_at(db: &mut impl Db, id: PaymentId, at: DateTime<Utc>) {
    sqlx::query("UPDATE payments SET paid_at = $2 WHERE id = $1")
        .bind(id.0)
        .bind(at)
        .execute(&mut *db.conn())
        .await
        .unwrap();
}

async fn refund(db: &mut impl Db, id: PaymentId, minor: i64, to: PaymentStatus) {
    db.refunds()
        .create(&NewRefund {
            bonus_minor: 0,
            payment_id: id,
            amount: usd(minor),
            method: RefundMethod::Provider,
            subscription_action: RefundSubscriptionAction::Keep,
            reason: None,
            admin_id: None,
            provider_refund_id: None,
        })
        .await
        .unwrap();
    db.payments().transition(id, to, None, None).await.unwrap();
}

fn sub(
    kind: SubscriptionKind,
    status: Option<SubscriptionStatus>,
    days: i64,
    connected: bool,
) -> Sub {
    Sub {
        kind,
        status,
        expire_at: Some(Utc::now() + Duration::days(days)),
        connected,
    }
}

#[tokio::test]
async fn главная_считает_подписки_и_выручку_в_валюте_установки() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let now = Utc::now();
    let noon_shift = 12 * 3_600 - i32::try_from(now.num_seconds_from_midnight()).unwrap();
    let offset = FixedOffset::east_opt(noon_shift).unwrap();
    let today_start = now
        .with_timezone(&offset)
        .date_naive()
        .and_time(NaiveTime::MIN)
        .and_local_timezone(offset)
        .unwrap()
        .with_timezone(&Utc);

    let payer = client(&mut conn, &Person::NOBODY).await;
    paid(&mut conn, payer, "cryptopay", usd(699)).await;
    let partial = paid(&mut conn, payer, "cryptopay", usd(1_000)).await;
    refund(&mut conn, partial, 300, PaymentStatus::PartiallyRefunded).await;
    let full = paid(&mut conn, payer, "cryptopay", usd(100)).await;
    refund(&mut conn, full, 100, PaymentStatus::Refunded).await;
    let yesterday = paid(&mut conn, payer, "cryptopay", usd(500)).await;
    paid_at(&mut conn, yesterday, today_start - Duration::hours(1)).await;
    let ten_days = paid(&mut conn, payer, "cryptopay", usd(2_000)).await;
    paid_at(&mut conn, ten_days, now - Duration::days(10)).await;
    let old = paid(&mut conn, payer, "cryptopay", usd(4_000)).await;
    paid_at(&mut conn, old, now - Duration::days(40)).await;
    paid(&mut conn, payer, "balance", usd(300)).await;
    paid(
        &mut conn,
        payer,
        "stars",
        Amount::new(350, PayCurrency::Xtr),
    )
    .await;
    let stars_old = paid(
        &mut conn,
        payer,
        "stars",
        Amount::new(175, PayCurrency::Xtr),
    )
    .await;
    paid_at(&mut conn, stars_old, now - Duration::days(8)).await;
    paid(
        &mut conn,
        payer,
        "yookassa",
        Amount::new(49_900, PayCurrency::Rub),
    )
    .await;
    payment(&mut conn, payer, "cryptopay", usd(999)).await;

    let a = client(&mut conn, &Person::NOBODY).await;
    subscription(
        &mut conn,
        a,
        sub(
            SubscriptionKind::Paid,
            Some(SubscriptionStatus::Active),
            30,
            true,
        ),
    )
    .await;
    let b = client(&mut conn, &Person::NOBODY).await;
    subscription(
        &mut conn,
        b,
        sub(
            SubscriptionKind::Paid,
            Some(SubscriptionStatus::Active),
            1,
            true,
        ),
    )
    .await;
    let c = client(&mut conn, &Person::NOBODY).await;
    subscription(
        &mut conn,
        c,
        sub(
            SubscriptionKind::Trial,
            Some(SubscriptionStatus::Active),
            2,
            false,
        ),
    )
    .await;
    let d = client(&mut conn, &Person::NOBODY).await;
    subscription(
        &mut conn,
        d,
        sub(
            SubscriptionKind::Paid,
            Some(SubscriptionStatus::Limited),
            30,
            true,
        ),
    )
    .await;
    let e = client(&mut conn, &Person::NOBODY).await;
    subscription(
        &mut conn,
        e,
        sub(
            SubscriptionKind::Paid,
            Some(SubscriptionStatus::Expired),
            -3,
            true,
        ),
    )
    .await;
    let f = client(&mut conn, &Person::NOBODY).await;
    subscription(
        &mut conn,
        f,
        sub(
            SubscriptionKind::Paid,
            Some(SubscriptionStatus::Active),
            30,
            true,
        ),
    )
    .await;
    conn.clients()
        .set_status(f, ClientStatus::Blocked, None)
        .await
        .unwrap();
    let g = client(&mut conn, &Person::NOBODY).await;
    subscription(
        &mut conn,
        g,
        sub(
            SubscriptionKind::Paid,
            Some(SubscriptionStatus::Active),
            30,
            true,
        ),
    )
    .await;
    conn.clients().soft_delete(g).await.unwrap();
    let h = client(&mut conn, &Person::NOBODY).await;
    let ended = subscription(
        &mut conn,
        h,
        sub(
            SubscriptionKind::Paid,
            Some(SubscriptionStatus::Active),
            30,
            true,
        ),
    )
    .await;
    conn.subscriptions().end(ended, now).await.unwrap();
    let i = client(&mut conn, &Person::NOBODY).await;
    let lost = subscription(
        &mut conn,
        i,
        sub(
            SubscriptionKind::Paid,
            Some(SubscriptionStatus::Active),
            30,
            false,
        ),
    )
    .await;
    conn.subscriptions().mark_missing(lost, true).await.unwrap();
    let j = client(&mut conn, &Person::NOBODY).await;
    subscription(&mut conn, j, sub(SubscriptionKind::Paid, None, 30, true)).await;

    let stats = conn
        .subscriptions()
        .dashboard_in(now, Currency::Usd, offset)
        .await
        .unwrap();
    assert_eq!(
        (
            stats.active_paid,
            stats.trials,
            stats.expiring_soon,
            stats.never_connected,
            stats.limited
        ),
        (3, 1, 2, 1, 1)
    );
    assert_eq!(stats.revenue_today_minor, 699 + 700);
    assert_eq!(stats.payments_today, 2);
    assert_eq!(stats.revenue_yesterday_minor, 500);
    assert_eq!(stats.revenue_7d_minor, 699 + 700 + 500);
    assert_eq!(stats.revenue_30d_minor, 699 + 700 + 500 + 2_000);
    assert_eq!(stats.revenue_today_stars, Stars(350));
    assert_eq!(stats.revenue_7d_stars, Stars(350));
    assert_eq!(stats.revenue_30d_stars, Stars(525));
    assert_eq!(
        (stats.new_clients_today, stats.new_clients_paid_today),
        (10, 1)
    );
    assert_eq!(stats.revenue_by_day.len(), REVENUE_DAYS as usize);
    let today = stats.revenue_by_day.last().unwrap();
    assert_eq!(today.day, now.with_timezone(&offset).date_naive());
    assert_eq!((today.minor, today.stars), (1_399, Stars(350)));
    let back = |days: usize| stats.revenue_by_day[stats.revenue_by_day.len() - 1 - days];
    assert_eq!(back(1).minor, 500);
    assert_eq!(back(8).stars, Stars(175));
    assert_eq!(back(10).minor, 2_000);
    assert_eq!(
        stats.revenue_by_day.iter().map(|d| d.minor).sum::<i64>(),
        1_399 + 500 + 2_000
    );

    let rub = conn
        .subscriptions()
        .dashboard_in(now, Currency::Rub, offset)
        .await
        .unwrap();
    assert_eq!(rub.revenue_today_minor, 49_900);
    assert_eq!(rub.revenue_today_stars, Stars(350));
    let utc = conn
        .subscriptions()
        .dashboard(now, Currency::Usd)
        .await
        .unwrap();
    assert_eq!(utc.active_paid, 3);
    assert_eq!(utc.revenue_30d_minor, stats.revenue_30d_minor);
    drop(conn);
    db.cleanup().await;
}
