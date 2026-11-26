use super::testkit::*;
use super::*;
use crate::FixedClock;
use crate::autorenew::AutoRenewReport;
use crate::fulfillment::FulfillmentOutcome;
use crate::promotions::PromotionInput;
use sn_domain::ids::AdminId;
use sn_domain::money::Stars;
use sn_domain::pricing::{QuoteLineKind, promotion_line};
use sn_domain::promotion::PromotionState;
use sn_domain::traffic::TrafficPolicy;
use sn_modules::catalog::promo::{self, ListFilter, NewPromo, NewPromoValue, PromoModule};
use sn_payments::{PaymentRef, ProviderStatus, WebhookOutcome, WebhookReply};

const ADMIN: AdminId = AdminId(1);

fn input(
    percent: u8,
    tariffs: &[&Tariff],
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
) -> PromotionInput {
    PromotionInput {
        name: "Чёрная пятница".into(),
        percent,
        tariff_ids: tariffs.iter().map(|t| t.id).collect(),
        starts_at,
        ends_at,
        enabled: true,
    }
}

async fn promote(
    kit: &Kit,
    percent: u8,
    tariffs: &[&Tariff],
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
) -> i64 {
    kit.services
        .promotions()
        .create(&input(percent, tariffs, starts_at, ends_at), ADMIN)
        .await
        .unwrap()
        .promotion
        .id
}

async fn premium(kit: &Kit) -> Tariff {
    let category_id = category(&kit.services, "Премиум").await;
    tariff(
        &kit.services,
        category_id,
        "Премиум",
        TrafficPolicy::UNLIMITED,
        vec![squad(3)],
    )
    .await
}

async fn offer(kit: &Kit, tariff: &Tariff, days: u32) -> PeriodOffer {
    kit.services
        .checkout()
        .catalog()
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.tariff.id == tariff.id)
        .unwrap()
        .periods
        .into_iter()
        .find(|p| p.days == days)
        .unwrap()
}

async fn priced(kit: &Kit, client_id: ClientId, cart: &CartRequest) -> QuoteView {
    kit.services
        .checkout()
        .quote(client_id, cart)
        .await
        .unwrap()
}

fn has_code(quote: &Quote) -> bool {
    quote
        .lines
        .iter()
        .any(|line| matches!(line.kind, QuoteLineKind::Adjustment { .. }))
}

async fn renewal_orders(kit: &Kit, client_id: ClientId) -> Vec<Order> {
    let mut conn = kit.services.store().conn().await.unwrap();
    let mut orders: Vec<Order> = conn
        .orders()
        .for_client(client_id, Default::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .filter(|o| o.kind == OrderKind::Renewal)
        .collect();
    orders.sort_by_key(|o| o.id.0);
    orders
}

#[tokio::test]
async fn акция_действует_с_начала_и_до_конца_в_каталоге_и_расчёте() {
    let start = moment(Duration::zero());
    let clock = Arc::new(FixedClock::new(start));
    let Some(kit) = kit_clocked(Vec::new(), clock.clone()).await else {
        return;
    };
    let client_id = client(&kit.services, 7_101).await;
    let tariff = standard(&kit.services).await;
    let other = premium(&kit).await;
    let starts = start + Duration::hours(1);
    let ends = start + Duration::days(3);
    let id = promote(&kit, 25, &[&tariff], starts, ends).await;
    let month = cart(&tariff, 30, None);

    let before = priced(&kit, client_id, &month).await;
    assert_eq!(before.quote.total, Money::new(299, Currency::Usd));
    assert_eq!(promotion_line(&before.quote), None);
    assert!(before.promotion.is_none());
    assert!(offer(&kit, &tariff, 30).await.promotion.is_none());

    clock.set(starts);
    let shown = offer(&kit, &tariff, 30).await;
    assert_eq!(shown.price, Money::new(224, Currency::Usd));
    assert_eq!(shown.stars, Some(Stars(113)));
    let mark = shown.promotion.expect("акция в каталоге");
    assert_eq!(mark.id, id);
    assert_eq!(mark.percent, 25);
    assert_eq!(mark.full_price, Money::new(299, Currency::Usd));
    assert_eq!(mark.full_stars, Some(Stars(150)));
    assert_eq!(mark.ends_at, ends);
    assert!(mark.until.starts_with("до "), "{}", mark.until);
    let quarter = offer(&kit, &tariff, 90).await;
    assert_eq!(quarter.price, Money::new(524, Currency::Usd));
    assert_eq!(quarter.stars, Some(Stars(263)));
    assert!(offer(&kit, &other, 30).await.promotion.is_none());

    let during = priced(&kit, client_id, &month).await;
    assert_eq!(during.quote.total, Money::new(224, Currency::Usd));
    assert_eq!(during.quote.subtotal, Money::new(299, Currency::Usd));
    assert_eq!(during.quote.stars.unwrap().total, Stars(113));
    assert_eq!(promotion_line(&during.quote), Some((id, 25)));
    let note = during.promotion.expect("пометка акции в расчёте");
    assert!(note.applied);
    assert!(!note.code_skipped);
    assert_eq!(note.name, "Чёрная пятница");
    let plain = priced(&kit, client_id, &cart(&other, 30, None)).await;
    assert_eq!(plain.quote.total, Money::new(299, Currency::Usd));
    assert!(plain.promotion.is_none());

    clock.set(ends - Duration::seconds(1));
    assert_eq!(
        priced(&kit, client_id, &month).await.quote.total,
        Money::new(224, Currency::Usd)
    );

    clock.set(ends);
    let after = priced(&kit, client_id, &month).await;
    assert_eq!(after.quote.total, Money::new(299, Currency::Usd));
    assert_eq!(promotion_line(&after.quote), None);
    assert!(after.promotion.is_none());
    assert!(offer(&kit, &tariff, 30).await.promotion.is_none());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn выключенная_и_удалённая_акция_не_действуют() {
    let Some(kit) = kit().await else {
        return;
    };
    let client_id = client(&kit.services, 7_102).await;
    let tariff = standard(&kit.services).await;
    let now = kit.services.now();
    let id = promote(
        &kit,
        25,
        &[&tariff],
        now - Duration::hours(1),
        now + Duration::days(1),
    )
    .await;
    let month = cart(&tariff, 30, None);
    let total = |view: QuoteView| view.quote.total.minor;
    assert_eq!(total(priced(&kit, client_id, &month).await), 224);
    let promotions = kit.services.promotions();
    let off = promotions.set_enabled(id, false, ADMIN).await.unwrap();
    assert_eq!(off.state, PromotionState::Off);
    assert_eq!(total(priced(&kit, client_id, &month).await), 299);
    assert!(offer(&kit, &tariff, 30).await.promotion.is_none());
    let on = promotions.set_enabled(id, true, ADMIN).await.unwrap();
    assert_eq!(on.state, PromotionState::Running);
    assert_eq!(total(priced(&kit, client_id, &month).await), 224);
    promotions.delete(id, ADMIN).await.unwrap();
    assert_eq!(total(priced(&kit, client_id, &month).await), 299);
    assert!(promotions.list().await.unwrap().promotions.is_empty());
    let mut conn = kit.services.store().conn().await.unwrap();
    let actions: Vec<String> = conn
        .audit()
        .for_entity("promotion", id, 20)
        .await
        .unwrap()
        .into_iter()
        .map(|record| record.action)
        .collect();
    drop(conn);
    assert_eq!(
        actions,
        [
            "tariff.promotion_deleted",
            "tariff.promotion_enabled",
            "tariff.promotion_disabled",
            "tariff.promotion_created",
        ]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn поля_акции_проверяются_и_правка_пишется_в_журнал() {
    let Some(kit) = kit().await else {
        return;
    };
    let tariff = standard(&kit.services).await;
    let now = kit.services.now();
    let good = input(25, &[&tariff], now, now + Duration::days(3));
    let promotions = kit.services.promotions();
    for broken in [
        PromotionInput {
            name: "  ".into(),
            ..good.clone()
        },
        PromotionInput {
            percent: 0,
            ..good.clone()
        },
        PromotionInput {
            percent: 91,
            ..good.clone()
        },
        PromotionInput {
            tariff_ids: Vec::new(),
            ..good.clone()
        },
        PromotionInput {
            tariff_ids: vec![TariffId(999_999)],
            ..good.clone()
        },
        PromotionInput {
            ends_at: now,
            ..good.clone()
        },
        PromotionInput {
            starts_at: now - Duration::days(3),
            ends_at: now - Duration::days(1),
            ..good.clone()
        },
    ] {
        match promotions.create(&broken, ADMIN).await {
            Err(ServiceError::Rejected { code, .. }) => assert_eq!(code, "promotion.invalid"),
            other => panic!("{broken:?}: {other:?}"),
        }
    }
    let created = promotions.create(&good, ADMIN).await.unwrap();
    assert_eq!(created.state, PromotionState::Running);
    let scheduled = promotions
        .update(
            created.promotion.id,
            &PromotionInput {
                percent: 30,
                starts_at: now + Duration::days(1),
                ..good.clone()
            },
            ADMIN,
        )
        .await
        .unwrap();
    assert_eq!(scheduled.state, PromotionState::Scheduled);
    assert_eq!(scheduled.promotion.percent, 30);
    let view = promotions.list().await.unwrap();
    assert_eq!(view.promotions.len(), 1);
    assert_eq!(view.offset_minutes, 0);
    let mut conn = kit.services.store().conn().await.unwrap();
    let records = conn
        .audit()
        .for_entity("promotion", created.promotion.id, 20)
        .await
        .unwrap();
    drop(conn);
    let updated = records
        .iter()
        .find(|r| r.action == "tariff.promotion_updated")
        .expect("правка в журнале");
    assert_eq!(updated.details["from"]["percent"], 25);
    assert_eq!(updated.details["to"]["percent"], 30);
    assert_eq!(updated.details["to"]["tariffs"][0], "Стандарт");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn промокод_меньше_акции_не_применяется_и_не_тратится_а_больший_побеждает() {
    let Some(kit) = kit_with(vec![Arc::new(PromoModule)]).await else {
        return;
    };
    let ctx = kit.services.module_context();
    for (code, percent) in [("SMALL10", 10), ("BIG50", 50)] {
        promo::create(
            &ctx,
            &NewPromo {
                code: code.into(),
                value: NewPromoValue::Percent { percent },
                max_uses: None,
                per_client: 5,
                valid_from: None,
                valid_until: None,
                first_purchase_only: false,
                tariff_ids: Vec::new(),
                note: None,
            },
            None,
        )
        .await
        .unwrap();
    }
    let client_id = client(&kit.services, 7_103).await;
    let tariff = standard(&kit.services).await;
    let now = kit.services.now();
    let id = promote(
        &kit,
        25,
        &[&tariff],
        now - Duration::hours(1),
        now + Duration::days(1),
    )
    .await;
    let with_code = |code: &str| CartRequest {
        promo_code: Some(code.into()),
        ..cart(&tariff, 30, None)
    };

    let small = priced(&kit, client_id, &with_code("SMALL10")).await;
    assert_eq!(small.quote.total, Money::new(224, Currency::Usd));
    assert_eq!(promotion_line(&small.quote), Some((id, 25)));
    assert!(!has_code(&small.quote));
    let note = small.promotion.expect("пометка акции");
    assert!(note.applied && note.code_skipped);

    let big = priced(&kit, client_id, &with_code("BIG50")).await;
    assert_eq!(big.quote.total, Money::new(149, Currency::Usd));
    assert_eq!(promotion_line(&big.quote), None);
    assert!(has_code(&big.quote));
    let note = big.promotion.expect("пометка акции");
    assert!(!note.applied && !note.code_skipped);

    credit(&kit.services, client_id, 10_000).await;
    let order = buy_with_balance(&kit.services, client_id, with_code("SMALL10")).await;
    assert_eq!(order.quote.total, Money::new(224, Currency::Usd));
    assert_eq!(promo::promo_in_quote(&order.quote), None);
    assert_eq!(balance(&kit.services, client_id).await, 10_000 - 224);
    let page = promo::list(&ctx, ListFilter::All, Default::default())
        .await
        .unwrap();
    let small = page
        .items
        .iter()
        .find(|p| p.code.code == "SMALL10")
        .unwrap();
    assert_eq!((small.used, small.reserved), (0, 0));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn счёт_выставленный_в_акцию_оплачивается_по_цене_счёта() {
    let start = moment(Duration::zero());
    let clock = Arc::new(FixedClock::new(start));
    let Some(kit) = kit_clocked(Vec::new(), clock.clone()).await else {
        return;
    };
    let client_id = client(&kit.services, 7_104).await;
    let tariff = standard(&kit.services).await;
    let ends = start + Duration::hours(1);
    let id = promote(&kit, 25, &[&tariff], start - Duration::hours(1), ends).await;
    let started = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(&tariff, 30, None),
                provider: TEST_PAY.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = started else {
        panic!("ожидалась ссылка на оплату: {started:?}");
    };
    let payment = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    assert_eq!(payment.amount, Money::new(224, Currency::Usd).into());
    assert_eq!(
        kit.pay.invoices.lock().unwrap()[0].amount,
        Money::new(224, Currency::Usd).into()
    );

    clock.set(ends + Duration::minutes(5));
    let now = priced(&kit, client_id, &cart(&tariff, 30, None)).await;
    assert_eq!(now.quote.total, Money::new(299, Currency::Usd));
    kit.services
        .inbox()
        .apply_outcome(
            TEST_PAY,
            &WebhookOutcome {
                event_key: format!("paid:{}", payment.id),
                payment: Some(PaymentRef::Payment(payment.id)),
                provider_txid: payment.provider_txid.clone(),
                status: ProviderStatus::Paid,
                amount: Some(payment.amount),
                saved_method: None,
                reply: WebhookReply::ok(),
                signal: None,
            },
        )
        .await
        .unwrap();
    let order = order(&kit.services, payment.order_id).await;
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(order.quote.total, Money::new(224, Currency::Usd));
    assert_eq!(promotion_line(&order.quote), Some((id, 25)));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn автопродление_в_акцию_по_цене_акции_а_после_по_полной() {
    let Some(kit) = kit().await else {
        return;
    };
    let client_id = client(&kit.services, 7_105).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 10_000).await;
    let first = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    assert_eq!(first.quote.total, Money::new(299, Currency::Usd));
    let outcome = kit.services.fulfillment().run(first.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let subscription = subscription(&kit.services, &order(&kit.services, first.id).await).await;
    assert!(subscription.autorenew.enabled);
    let now = kit.services.now();
    let id = promote(
        &kit,
        25,
        &[&tariff],
        now - Duration::hours(1),
        now + Duration::days(1),
    )
    .await;

    let subscription = expire_in(
        &kit,
        &subscription,
        moment(Duration::hours(12)),
        SubscriptionStatus::Active,
    )
    .await;
    let before = balance(&kit.services, client_id).await;
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!(
        report,
        AutoRenewReport {
            due: 1,
            charged: 1,
            reminded: 0,
            failed: 0
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, before - 224);
    let renewals = renewal_orders(&kit, client_id).await;
    assert_eq!(renewals.len(), 1);
    assert_eq!(renewals[0].quote.total, Money::new(224, Currency::Usd));
    assert_eq!(promotion_line(&renewals[0].quote), Some((id, 25)));
    let outcome = kit
        .services
        .fulfillment()
        .run(renewals[0].id)
        .await
        .unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );

    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE tariff_promotions SET starts_at = now() - interval '2 days', \
             ends_at = now() - interval '1 minute' WHERE id = $1",
        )
        .bind(id)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    expire_in(
        &kit,
        &subscription,
        moment(Duration::hours(13)),
        SubscriptionStatus::Active,
    )
    .await;
    let before = balance(&kit.services, client_id).await;
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!(report.charged, 1, "{report:?}");
    assert_eq!(balance(&kit.services, client_id).await, before - 299);
    let renewals = renewal_orders(&kit, client_id).await;
    assert_eq!(renewals.len(), 2);
    assert_eq!(renewals[1].quote.total, Money::new(299, Currency::Usd));
    assert_eq!(promotion_line(&renewals[1].quote), None);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оплата_через_tribute_по_подписке_идёт_без_скидки_акции() {
    let Some(kit) = kit().await else {
        return;
    };
    let client_id = client(&kit.services, 7_106).await;
    let tariff = standard(&kit.services).await;
    let now = kit.services.now();
    promote(
        &kit,
        25,
        &[&tariff],
        now - Duration::hours(1),
        now + Duration::days(1),
    )
    .await;
    let planned = kit
        .services
        .checkout()
        .plan_order(client_id, &cart(&tariff, 30, None))
        .await
        .unwrap();
    assert_eq!(planned.quote.total, Money::new(299, Currency::Usd));
    assert_eq!(promotion_line(&planned.quote), None);
    kit.db.cleanup().await;
}
