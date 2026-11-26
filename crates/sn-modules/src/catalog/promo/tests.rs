use super::*;
use crate::catalog::testing::{
    TestCore, client, context, offline, order, paid_payment, pooled, usd,
};
use crate::catalog::web::{ClientAddr, address_network, is_internal};
use crate::registry::ModuleRegistry;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::TimeZone;
use sn_domain::access::{AdminPrincipal, AdminRole, ClientChannel, ClientPrincipal};
use sn_domain::ids::{CategoryId, PeriodId, RemnaUserId};
use sn_domain::money::{Amount, Stars};
use sn_domain::order::OrderKind;
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::subscription::{AutoRenewState, RemnaLink, SubscriptionKind};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, Tariff, TariffPeriod};
use sn_domain::traffic::TrafficPolicy;
use sn_store::NewSubscription;
use sn_store::testing::{test_store, wait_for_window_room};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tower::ServiceExt;

fn code(kind: PromoKind) -> PromoCode {
    PromoCode {
        id: 1,
        code: "ZIMA".into(),
        kind,
        max_uses: Some(10),
        per_client: 1,
        valid_from: None,
        valid_until: None,
        first_purchase_only: false,
        tariff_ids: Vec::new(),
        active: true,
        note: None,
        created_at: now(),
        owner: None,
    }
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap()
}

fn checkout() -> Purpose {
    Purpose::Checkout {
        tariff: TariffId(1),
        currency: Currency::Usd,
    }
}

fn tariff() -> Tariff {
    Tariff {
        traffic_addon: None,
        id: TariffId(1),
        category_id: CategoryId(1),
        name: "Стандарт".into(),
        description: None,
        on_sale: true,
        sort: 0,
        periods: vec![TariffPeriod {
            id: PeriodId(12),
            days: 90,
            prices: CurrencyPrices {
                usd: Some(699),
                rub: Some(49_900),
            },
            stars: Some(Stars(350)),
            sort: 0,
            active: true,
        }],
        devices: DevicePolicy {
            included: 3,
            extra: None,
        },
        traffic: TrafficPolicy::UNLIMITED,
        squads: Vec::new(),
        external_squad: None,
        device_discounts: Vec::new(),
    }
}

fn priced(adjustments: &[PriceAdjustment]) -> Quote {
    quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff: &tariff(),
        period_id: PeriodId(12),
        extra_devices: 0,
        sell_extra_devices: false,
        adjustments,
        promotion: None,
    })
    .unwrap()
}

fn intent() -> PurchaseIntent {
    PurchaseIntent::Subscription {
        tariff_id: TariffId(1),
        period_id: PeriodId(12),
        extra_devices: 0,
        target: None,
    }
}

fn percent(code: &str, percent: u8, max_uses: Option<u32>) -> NewPromo {
    NewPromo {
        code: code.into(),
        value: NewPromoValue::Percent { percent },
        max_uses,
        per_client: 1,
        valid_from: None,
        valid_until: None,
        first_purchase_only: false,
        tariff_ids: Vec::new(),
        note: None,
    }
}

fn hook(client_id: ClientId, code: &str) -> QuoteHook {
    QuoteHook {
        client_id,
        tariff_id: TariffId(1),
        period_id: PeriodId(12),
        order_kind: OrderKind::New,
        currency: Currency::Usd,
        promo_code: Some(code.into()),
    }
}

#[test]
fn код_приводится_к_верхнему_регистру_без_лишнего() {
    assert_eq!(normalize_code(" zima ").as_deref(), Some("ZIMA"));
    assert_eq!(
        normalize_code("Winter-2026_x").as_deref(),
        Some("WINTER-2026_X")
    );
    assert_eq!(normalize_code("ЗИМА"), None);
    assert_eq!(normalize_code("AB"), None);
    assert_eq!(normalize_code("%ZIMA%"), None);
    assert_eq!(normalize_code(&"A".repeat(33)), None);
}

#[test]
fn проверка_промокода_по_правилам() {
    let usage = Usage::default();
    let base = code(PromoKind::Percent { percent: 10 });
    assert_eq!(evaluate(&base, &usage, now(), checkout()), Ok(()));
    let mut disabled = base.clone();
    disabled.active = false;
    assert_eq!(
        evaluate(&disabled, &usage, now(), checkout()),
        Err(PromoRejection::Disabled)
    );
    let mut later = base.clone();
    later.valid_from = Some(now() + Duration::days(1));
    assert_eq!(
        evaluate(&later, &usage, now(), checkout()),
        Err(PromoRejection::NotStarted)
    );
    let mut expired = base.clone();
    expired.valid_until = Some(now());
    assert_eq!(
        evaluate(&expired, &usage, now(), checkout()),
        Err(PromoRejection::Expired)
    );
    let exhausted = Usage {
        active_total: 10,
        ..Usage::default()
    };
    assert_eq!(
        evaluate(&base, &exhausted, now(), checkout()),
        Err(PromoRejection::Exhausted)
    );
    let mine = Usage {
        active_total: 1,
        client_active: 1,
        client_has_purchases: false,
        ..Usage::default()
    };
    assert_eq!(
        evaluate(&base, &mine, now(), checkout()),
        Err(PromoRejection::AlreadyUsed)
    );
    let unpaid = Usage {
        active_total: 1,
        client_active: 1,
        client_held: 1,
        client_has_purchases: false,
    };
    assert_eq!(
        evaluate(&base, &unpaid, now(), checkout()),
        Err(PromoRejection::Reserved)
    );
    let mut first = base.clone();
    first.first_purchase_only = true;
    let buyer = Usage {
        client_has_purchases: true,
        ..Usage::default()
    };
    assert_eq!(
        evaluate(&first, &buyer, now(), checkout()),
        Err(PromoRejection::FirstPurchaseOnly)
    );
    let mut scoped = base.clone();
    scoped.tariff_ids = vec![TariffId(7)];
    assert_eq!(
        evaluate(&scoped, &usage, now(), checkout()),
        Err(PromoRejection::WrongTariff)
    );
    assert_eq!(
        evaluate(
            &scoped,
            &usage,
            now(),
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        ),
        Ok(())
    );
}

#[test]
fn фиксированная_скидка_только_в_своей_валюте_а_дни_отдельно() {
    let usage = Usage::default();
    let fixed = code(PromoKind::Fixed {
        amount: Money::new(10_000, Currency::Rub),
    });
    assert_eq!(
        evaluate(&fixed, &usage, now(), checkout()),
        Err(PromoRejection::WrongCurrency)
    );
    let days = code(PromoKind::Days { days: 7 });
    assert_eq!(
        evaluate(&days, &usage, now(), checkout()),
        Err(PromoRejection::DaysCode)
    );
    assert_eq!(evaluate(&days, &usage, now(), Purpose::Activate), Ok(()));
    let discount = code(PromoKind::Percent { percent: 5 });
    assert_eq!(
        evaluate(&discount, &usage, now(), Purpose::Activate),
        Err(PromoRejection::DiscountCode)
    );
    assert_eq!(days.adjustment(), None);
    assert_eq!(discount.label(), "Промокод ZIMA, −5%");
}

#[test]
fn новый_промокод_проверяется_до_записи() {
    let mut new = percent("zima", 10, Some(100));
    assert_eq!(
        validate_new(&new, Currency::Usd).unwrap(),
        ("ZIMA".to_string(), PromoKind::Percent { percent: 10 })
    );
    new.value = NewPromoValue::Percent { percent: 0 };
    assert!(validate_new(&new, Currency::Usd).is_err());
    new.value = NewPromoValue::Fixed {
        amount: "1.50".into(),
    };
    assert_eq!(
        validate_new(&new, Currency::Usd).unwrap().1,
        PromoKind::Fixed { amount: usd(150) }
    );
    assert!(validate_new(&new, Currency::Rub).is_err());
    new.value = NewPromoValue::Fixed {
        amount: "1.505".into(),
    };
    assert!(validate_new(&new, Currency::Usd).is_err());
    new.value = NewPromoValue::Days { days: 7 };
    new.tariff_ids = vec![TariffId(1)];
    assert!(validate_new(&new, Currency::Usd).is_err());
    new.tariff_ids.clear();
    new.max_uses = Some(0);
    assert!(validate_new(&new, Currency::Usd).is_err());
}

#[test]
fn тело_запроса_на_создание_разбирается() {
    let parsed: NewPromo = serde_json::from_value(serde_json::json!({
        "code": "ZIMA", "kind": "percent", "percent": 10, "max_uses": 100
    }))
    .unwrap();
    assert_eq!(parsed.value, NewPromoValue::Percent { percent: 10 });
    assert_eq!(parsed.per_client, 1);
    assert_eq!(parsed.max_uses, Some(100));
}

#[tokio::test]
async fn скидка_по_точному_коду_попадает_в_расчёт() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("ZIMA2026", 10, None), None)
        .await
        .unwrap();
    let buyer = client(&db.store).await;
    let adjustments = PromoModule
        .quote_adjustments(&ctx, &hook(buyer, " zima2026 "))
        .await
        .unwrap();
    assert_eq!(adjustments.len(), 1);
    let q = priced(&adjustments);
    assert_eq!(q.total, usd(629));
    assert_eq!(q.stars.unwrap().total, Stars(315));
    assert_eq!(
        promo_in_quote(&q).map(|(_, c)| c).as_deref(),
        Some("ZIMA2026")
    );
    for wrong in ["ZIMA", "ZIMA202", "ZIMA20266", "%ZIMA%"] {
        assert!(
            matches!(
                PromoModule
                    .quote_adjustments(&ctx, &hook(buyer, wrong))
                    .await,
                Err(ModuleError::Invalid(_))
            ),
            "{wrong}"
        );
    }
    let none = PromoModule
        .quote_adjustments(
            &ctx,
            &QuoteHook {
                promo_code: Some("  ".into()),
                ..hook(buyer, "")
            },
        )
        .await
        .unwrap();
    assert!(none.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn последний_промокод_резервирует_только_один_счёт() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("LAST", 10, Some(1)), None)
        .await
        .unwrap();
    let adjustment = promo.adjustment().unwrap();
    let q = priced(std::slice::from_ref(&adjustment));
    let first = client(&db.store).await;
    let second = client(&db.store).await;
    let a = order(&db.store, first, OrderKind::New, intent(), q.clone()).await;
    let b = order(&db.store, second, OrderKind::New, intent(), q.clone()).await;
    let until = Utc::now() + Duration::minutes(60);
    let (ra, rb) = tokio::join!(reserve(&ctx, &a, until), reserve(&ctx, &b, until));
    let results = [ra, rb];
    let reserved = results.iter().filter(|r| matches!(r, Ok(Some(_)))).count();
    let rejected = results
        .iter()
        .filter(|r| matches!(r, Err(ModuleError::Invalid(_))))
        .count();
    assert_eq!((reserved, rejected), (1, 1));
    let winner = if matches!(results[0], Ok(Some(_))) {
        &a
    } else {
        &b
    };
    assert!(matches!(reserve(&ctx, winner, until).await, Ok(Some(_))));
    let third = client(&db.store).await;
    assert!(matches!(
        check(
            &ctx,
            third,
            "last",
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        )
        .await,
        Err(PromoError::Rejected(PromoRejection::Exhausted))
    ));
    assert!(release(&ctx, winner.id).await.unwrap());
    assert!(
        check(
            &ctx,
            third,
            "last",
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        )
        .await
        .is_ok()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn просроченный_резерв_не_держит_промокод() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("ONCE", 15, Some(1)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let buyer = client(&db.store).await;
    let late = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    reserve(&ctx, &late, Utc::now() - Duration::minutes(1))
        .await
        .unwrap();
    let other = client(&db.store).await;
    let fresh = order(&db.store, other, OrderKind::New, intent(), q).await;
    assert!(matches!(
        reserve(&ctx, &fresh, Utc::now() + Duration::minutes(30)).await,
        Ok(Some(_))
    ));
    db.cleanup().await;
}

#[tokio::test]
async fn оплата_закрепляет_использование_а_возврат_освобождает() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("PAIDONE", 10, Some(1)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let buyer = client(&db.store).await;
    let paid = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    let payment = paid_payment(
        &db.store,
        paid.id,
        buyer,
        "cryptopay",
        Amount::from(q.total),
    )
    .await;
    let event = DomainEvent::PaymentPaid {
        payment_id: payment.id,
        order_id: paid.id,
        client_id: buyer,
        order_kind: OrderKind::New,
        amount: payment.amount,
        provider: payment.provider.clone(),
    };
    PromoModule.on_event(&ctx, &event).await.unwrap();
    PromoModule.on_event(&ctx, &event).await.unwrap();
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!((page.items[0].used, page.items[0].reserved), (1, 0));
    let other = client(&db.store).await;
    assert!(matches!(
        check(
            &ctx,
            other,
            "PAIDONE",
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        )
        .await,
        Err(PromoError::Rejected(PromoRejection::Exhausted))
    ));
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.payments()
            .transition(payment.id, PaymentStatus::Refunded, None, None)
            .await
            .unwrap();
    }
    PromoModule
        .on_event(
            &ctx,
            &DomainEvent::PaymentRefunded {
                payment_id: payment.id,
                order_id: paid.id,
                client_id: buyer,
                amount: payment.amount,
            },
        )
        .await
        .unwrap();
    assert!(
        check(
            &ctx,
            other,
            "PAIDONE",
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        )
        .await
        .is_ok()
    );
    db.cleanup().await;
}

async fn subscription(db: &sn_store::testing::TestDb, owner: ClientId) -> SubscriptionId {
    let mut conn = db.store.conn().await.unwrap();
    let sub = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: owner,
            tariff_id: None,
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    conn.subscriptions()
        .link_remna(
            sub.id,
            &RemnaLink {
                user_id: RemnaUserId(sub.id.0 + 1000),
                username: format!("sn_test_{}", sub.id),
                short_uuid: None,
            },
        )
        .await
        .unwrap();
    sub.id
}

#[tokio::test]
async fn промокод_на_дни_добавляет_дни_через_ядро_один_раз() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut days = percent("WEEK", 1, None);
    days.value = NewPromoValue::Days { days: 7 };
    create(&ctx, &days, None).await.unwrap();
    create(&ctx, &percent("SALE", 10, None), None)
        .await
        .unwrap();
    let owner = client(&db.store).await;
    let stranger = client(&db.store).await;
    let sub = subscription(&db, owner).await;
    assert!(matches!(
        activate(&ctx, stranger, "WEEK", sub).await,
        Err(PromoError::Rejected(PromoRejection::NoSubscription))
    ));
    assert!(matches!(
        activate(&ctx, owner, "SALE", sub).await,
        Err(PromoError::Rejected(PromoRejection::DiscountCode))
    ));
    core.fail_days.store(true, Ordering::SeqCst);
    assert!(matches!(
        activate(&ctx, owner, "week", sub).await,
        Err(PromoError::Module(ModuleError::Core(_)))
    ));
    core.fail_days.store(false, Ordering::SeqCst);
    let done = activate(&ctx, owner, "week", sub).await.unwrap();
    assert_eq!((done.days, done.subscription_id), (7, sub));
    assert_eq!(core.days(), vec![(sub, 7, "промокод WEEK".to_string())]);
    assert!(matches!(
        activate(&ctx, owner, "WEEK", sub).await,
        Err(PromoError::Rejected(PromoRejection::AlreadyUsed))
    ));
    db.cleanup().await;
}

fn admin_as(admin_id: sn_domain::ids::AdminId, role: AdminRole) -> AdminPrincipal {
    AdminPrincipal {
        admin_id,
        role,
        permissions: Vec::new(),
        session_id: 1,
        api_key: None,
    }
}

async fn admin_id(store: &sn_store::Store) -> sn_domain::ids::AdminId {
    let mut conn = store.conn().await.unwrap();
    conn.admins()
        .create(&sn_store::NewAdmin {
            email: "owner@example.com".into(),
            password_hash: "x".into(),
            role: AdminRole::Owner,
            must_change_password: false,
        })
        .await
        .unwrap()
        .id
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn админка_создаёт_и_выключает_промокод() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx.clone());
    let owner = admin_id(&db.store).await;
    let admin = |role: AdminRole| admin_as(owner, role);
    let create_request = |role: AdminRole| {
        Request::post("/promo/codes")
            .header("content-type", "application/json")
            .extension(admin(role))
            .body(Body::from(
                serde_json::json!({"code": "spring", "kind": "fixed", "amount": "1.00", "max_uses": 50})
                    .to_string(),
            ))
            .unwrap()
    };
    let denied = router
        .clone()
        .oneshot(create_request(AdminRole::Support))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let created = router
        .clone()
        .oneshot(create_request(AdminRole::Admin))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = body_json(created).await;
    assert_eq!(created["code"], "SPRING");
    assert_eq!(created["kind"]["type"], "fixed");
    let duplicate = router
        .clone()
        .oneshot(create_request(AdminRole::Admin))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let id = created["id"].as_i64().unwrap();
    let disabled = router
        .clone()
        .oneshot(
            Request::post(format!("/promo/codes/{id}/disable"))
                .extension(admin(AdminRole::Owner))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::OK);
    let listed = router
        .oneshot(
            Request::get("/promo/codes?status=disabled")
                .extension(admin(AdminRole::Owner))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed = body_json(listed).await;
    assert_eq!(listed["items"][0]["code"], "SPRING");
    assert_eq!(listed["items"][0]["active"], false);
    db.cleanup().await;
}

#[tokio::test]
async fn клиент_проверяет_код_и_видит_причину_отказа() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("FRIEND", 20, None), None)
        .await
        .unwrap();
    let buyer = client(&db.store).await;
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx);
    let principal = ClientPrincipal {
        client_id: buyer,
        channel: ClientChannel::Cabinet,
        full_access: true,
        session_id: Some(1),
    };
    let ask = |code: &str| {
        Request::post("/promo/check")
            .header("content-type", "application/json")
            .extension(principal.clone())
            .body(Body::from(serde_json::json!({ "code": code }).to_string()))
            .unwrap()
    };
    let ok = router.clone().oneshot(ask("friend")).await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let ok = body_json(ok).await;
    assert_eq!(ok["label"], "Промокод FRIEND, −20%");
    assert_eq!(ok["discount"], true);
    let missing = router.oneshot(ask("FRIENDS")).await.unwrap();
    assert_eq!(missing.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let missing = body_json(missing).await;
    assert_eq!(missing["error"]["code"], "promo.not_found");
    db.cleanup().await;
}

#[tokio::test]
async fn код_приз_применяет_только_победитель() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let winner = client(&db.store).await;
    let stranger = client(&db.store).await;
    let prize = {
        let mut tx = db.store.begin().await.unwrap();
        let prize = issue_personal(
            &mut tx,
            PromoKind::Percent { percent: 30 },
            None,
            "Приз рулетки",
            winner,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        prize
    };
    assert_eq!(prize.owner, Some(winner));
    let refused = check(&ctx, stranger, &prize.code, checkout())
        .await
        .unwrap_err();
    assert!(
        matches!(refused, PromoError::Rejected(PromoRejection::NotYours)),
        "{refused:?}"
    );
    assert!(refused.to_string().contains("выдан другому клиенту"));
    assert_eq!(PromoRejection::NotYours.code(), "promo.not_yours");
    let accepted = check(&ctx, winner, &prize.code, checkout()).await.unwrap();
    assert_eq!(accepted.id, prize.id);
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx);
    let principal = ClientPrincipal {
        client_id: stranger,
        channel: ClientChannel::Cabinet,
        full_access: true,
        session_id: Some(1),
    };
    let answer = router
        .oneshot(
            Request::post("/promo/check")
                .header("content-type", "application/json")
                .extension(principal)
                .body(Body::from(
                    serde_json::json!({ "code": prize.code }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(answer.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body_json(answer).await["error"]["code"], "promo.not_yours");
    db.cleanup().await;
}

#[tokio::test]
async fn брошенный_счёт_не_мешает_оплатить_с_тем_же_промокодом() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("AGAIN", 10, Some(5)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let buyer = client(&db.store).await;
    let abandoned = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    let until = Utc::now() + Duration::minutes(60);
    reserve(&ctx, &abandoned, until).await.unwrap();
    assert!(
        check(&ctx, buyer, "again", checkout()).await.is_ok(),
        "промокод с неоплаченного счёта переносится в новый счёт"
    );
    let retry = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    {
        let mut tx = db.store.begin().await.unwrap();
        assert_eq!(
            superseded(&mut tx, &retry).await.unwrap(),
            vec![abandoned.id]
        );
        tx.rollback().await.unwrap();
    }
    assert!(matches!(
        reserve(&ctx, &retry, until).await,
        Err(ModuleError::Invalid(_))
    ));
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.orders()
            .transition(abandoned.id, sn_domain::order::OrderStatus::Canceled)
            .await
            .unwrap();
    }
    assert!(
        check(&ctx, buyer, "again", checkout()).await.is_ok(),
        "отменённый счёт не держит промокод"
    );
    assert!(matches!(reserve(&ctx, &retry, until).await, Ok(Some(_))));
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(page.items[0].reserved, 1);
    let payment = paid_payment(
        &db.store,
        retry.id,
        buyer,
        "cryptopay",
        Amount::from(q.total),
    )
    .await;
    PromoModule
        .on_event(
            &ctx,
            &DomainEvent::PaymentPaid {
                payment_id: payment.id,
                order_id: retry.id,
                client_id: buyer,
                order_kind: OrderKind::New,
                amount: payment.amount,
                provider: payment.provider.clone(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        check(&ctx, buyer, "again", checkout()).await,
        Err(PromoError::Rejected(PromoRejection::AlreadyUsed))
    ));
    db.cleanup().await;
}

#[tokio::test]
async fn два_одновременных_нажатия_не_добавляют_дни_дважды() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut days = percent("TWICE", 1, None);
    days.value = NewPromoValue::Days { days: 3 };
    create(&ctx, &days, None).await.unwrap();
    let owner = client(&db.store).await;
    let sub = subscription(&db, owner).await;
    let (first, second) = tokio::join!(
        activate(&ctx, owner, "TWICE", sub),
        activate(&ctx, owner, "TWICE", sub)
    );
    let applied = [first.is_ok(), second.is_ok()]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    assert_eq!(applied, 1);
    assert_eq!(core.days().len(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn потерянное_событие_оплаты_досводится_задачей() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("LOST", 10, Some(1)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let buyer = client(&db.store).await;
    let paid = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    paid_payment(
        &db.store,
        paid.id,
        buyer,
        "cryptopay",
        Amount::from(q.total),
    )
    .await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.orders()
            .transition(paid.id, sn_domain::order::OrderStatus::Paid)
            .await
            .unwrap();
    }
    assert_eq!(settle(&ctx, 10).await.unwrap(), 1);
    assert_eq!(settle(&ctx, 10).await.unwrap(), 0);
    let personal = priced(&[sn_domain::pricing::PriceAdjustment {
        source: sn_domain::pricing::AdjustmentSource::PersonalDiscount,
        kind: sn_domain::pricing::AdjustmentKind::Percent { percent: 10 },
        label: "Личная скидка".into(),
    }]);
    let discounted = order(
        &db.store,
        buyer,
        OrderKind::Renewal,
        intent(),
        personal.clone(),
    )
    .await;
    paid_payment(
        &db.store,
        discounted.id,
        buyer,
        "cryptopay",
        Amount::from(personal.total),
    )
    .await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.orders()
            .transition(discounted.id, sn_domain::order::OrderStatus::Paid)
            .await
            .unwrap();
    }
    assert_eq!(settle(&ctx, 10).await.unwrap(), 0);
    let other = client(&db.store).await;
    assert!(matches!(
        check(&ctx, other, "LOST", checkout()).await,
        Err(PromoError::Rejected(PromoRejection::Exhausted))
    ));
    db.cleanup().await;
}

fn customer(client_id: ClientId) -> ClientPrincipal {
    ClientPrincipal {
        client_id,
        channel: ClientChannel::Cabinet,
        full_access: true,
        session_id: Some(1),
    }
}

async fn post(
    router: &axum::Router,
    who: ClientId,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .extension(customer(who))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn use_up(ctx: &ModuleContext, store: &sn_store::Store, code: &PromoCode) {
    let user = client(store).await;
    let q = priced(&[code.adjustment().unwrap()]);
    let done = order(store, user, OrderKind::New, intent(), q.clone()).await;
    let payment = paid_payment(store, done.id, user, "cryptopay", Amount::from(q.total)).await;
    PromoModule
        .on_event(
            ctx,
            &DomainEvent::PaymentPaid {
                payment_id: payment.id,
                order_id: done.id,
                client_id: user,
                order_kind: OrderKind::New,
                amount: payment.amount,
                provider: payment.provider.clone(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn несуществующий_и_неподходящий_код_неотличимы_для_клиента() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut expired = percent("OLD10", 10, None);
    expired.valid_until = Some(Utc::now() - Duration::days(1));
    create(&ctx, &expired, None).await.unwrap();
    let mut later = percent("SOON10", 10, None);
    later.valid_from = Some(Utc::now() + Duration::days(1));
    create(&ctx, &later, None).await.unwrap();
    let off = create(&ctx, &percent("OFF10", 10, None), None)
        .await
        .unwrap();
    set_active(&ctx, off.id, false).await.unwrap();
    let full = create(&ctx, &percent("FULL10", 10, Some(1)), None)
        .await
        .unwrap();
    use_up(&ctx, &db.store, &full).await;
    let mut days_old = percent("WEEKOLD", 1, None);
    days_old.value = NewPromoValue::Days { days: 7 };
    days_old.valid_until = Some(Utc::now() - Duration::days(1));
    create(&ctx, &days_old, None).await.unwrap();
    let mut days_full = percent("WEEKFULL", 1, Some(1));
    days_full.value = NewPromoValue::Days { days: 7 };
    create(&ctx, &days_full, None).await.unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let filler = client(&db.store).await;
    let filler_sub = subscription(&db, filler).await;
    activate(&ctx, filler, "WEEKFULL", filler_sub)
        .await
        .unwrap();
    let buyer = client(&db.store).await;
    let sub = subscription(&db, buyer).await;
    let missing = post(
        &router,
        buyer,
        "/promo/check",
        serde_json::json!({"code": "NOSUCH10"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(missing.1["error"]["code"], "promo.not_found");
    for code in ["OLD10", "SOON10", "OFF10", "FULL10"] {
        let answer = post(
            &router,
            buyer,
            "/promo/check",
            serde_json::json!({ "code": code }),
        )
        .await;
        assert_eq!(answer, missing, "{code}");
    }
    let missing_days = post(
        &router,
        buyer,
        "/promo/activate",
        serde_json::json!({"code": "NOSUCH7", "subscription_id": sub}),
    )
    .await;
    assert_eq!(missing_days, missing);
    for code in ["WEEKOLD", "WEEKFULL", "OFF10"] {
        let answer = post(
            &router,
            buyer,
            "/promo/activate",
            serde_json::json!({ "code": code, "subscription_id": sub }),
        )
        .await;
        assert_eq!(answer, missing, "{code}");
    }
    db.cleanup().await;
}

async fn window_room(store: &sn_store::Store) {
    let window = std::time::Duration::from_secs(FAILURE_WINDOW_MINUTES.unsigned_abs() * 60);
    wait_for_window_room(store, window).await;
}

#[tokio::test]
async fn перебор_промокодов_останавливается_после_серии_ошибок() {
    let Some(db) = test_store().await else {
        return;
    };
    window_room(&db.store).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("REAL20", 20, None), None)
        .await
        .unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let guesser = client(&db.store).await;
    let honest = client(&db.store).await;
    for n in 0..10 {
        let (status, _) = post(
            &router,
            guesser,
            "/promo/check",
            serde_json::json!({ "code": format!("GUESS{n}") }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "попытка {n}");
    }
    let (status, body) = post(
        &router,
        guesser,
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "promo.rate_limited");
    let sub = subscription(&db, guesser).await;
    let (status, _) = post(
        &router,
        guesser,
        "/promo/activate",
        serde_json::json!({"code": "REAL20", "subscription_id": sub}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        PromoModule
            .quote_adjustments(&ctx, &hook(guesser, "REAL20"))
            .await
            .is_err()
    );
    assert!(matches!(
        check(&ctx, guesser, "REAL20", checkout()).await,
        Err(PromoError::Rejected(PromoRejection::TooManyAttempts))
    ));
    let (status, body) = post(
        &router,
        honest,
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    db.cleanup().await;
}

async fn invoice(
    store: &sn_store::Store,
    buyer: ClientId,
    q: &Quote,
    expires_at: Option<DateTime<Utc>>,
) -> (Order, sn_domain::payment::Payment) {
    let created = order(store, buyer, OrderKind::New, intent(), q.clone()).await;
    let mut conn = store.conn().await.unwrap();
    let payment = conn
        .payments()
        .create(&sn_domain::payment::NewPayment {
            order_id: created.id,
            client_id: buyer,
            provider: sn_domain::payment::ProviderId::from("cryptopay"),
            method: None,
            amount: Amount::from(q.total),
            expires_at,
        })
        .await
        .unwrap();
    (created, payment)
}

async fn created(
    ctx: &ModuleContext,
    store: &sn_store::Store,
    order: &Order,
    payment: &sn_domain::payment::Payment,
) -> Result<(), ModuleError> {
    let mut tx = store.begin().await.unwrap();
    PromoModule
        .on_order_created(ctx, &mut tx, order, payment)
        .await?;
    tx.commit().await.unwrap();
    Ok(())
}

async fn pay(
    ctx: &ModuleContext,
    store: &sn_store::Store,
    order: &Order,
    payment: &sn_domain::payment::Payment,
) -> Result<(), ModuleError> {
    let mut tx = store.begin().await.unwrap();
    let (order, _) = tx
        .orders()
        .transition(order.id, sn_domain::order::OrderStatus::Paid)
        .await
        .unwrap();
    PromoModule
        .on_order_paid(ctx, &mut tx, &order, payment)
        .await?;
    tx.commit().await.unwrap();
    Ok(())
}

#[tokio::test]
async fn резерв_создаётся_в_транзакции_заказа_и_снимается_при_отмене() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("TXONE", 10, Some(1)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let first = client(&db.store).await;
    let second = client(&db.store).await;
    let (dropped, dropped_payment) = invoice(
        &db.store,
        first,
        &q,
        Some(Utc::now() + Duration::minutes(60)),
    )
    .await;
    {
        let mut tx = db.store.begin().await.unwrap();
        PromoModule
            .on_order_created(&ctx, &mut tx, &dropped, &dropped_payment)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
    }
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!((page.items[0].used, page.items[0].reserved), (0, 0));
    let (held, held_payment) = invoice(
        &db.store,
        first,
        &q,
        Some(Utc::now() + Duration::minutes(60)),
    )
    .await;
    created(&ctx, &db.store, &held, &held_payment)
        .await
        .unwrap();
    let (other, other_payment) = invoice(
        &db.store,
        second,
        &q,
        Some(Utc::now() + Duration::minutes(60)),
    )
    .await;
    assert!(matches!(
        created(&ctx, &db.store, &other, &other_payment).await,
        Err(ModuleError::Invalid(_))
    ));
    {
        let mut tx = db.store.begin().await.unwrap();
        let (canceled, _) = tx
            .orders()
            .transition(held.id, sn_domain::order::OrderStatus::Canceled)
            .await
            .unwrap();
        PromoModule
            .on_order_canceled(&ctx, &mut tx, &canceled)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    created(&ctx, &db.store, &other, &other_payment)
        .await
        .unwrap();
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!((page.items[0].used, page.items[0].reserved), (0, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn поздняя_оплата_истёкшего_счёта_выдаётся_по_цене_счёта() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let single = create(&ctx, &percent("LATE1", 10, Some(1)), None)
        .await
        .unwrap();
    let double = create(&ctx, &percent("LATE2", 10, Some(2)), None)
        .await
        .unwrap();
    for promo in [single, double] {
        let q = priced(&[promo.adjustment().unwrap()]);
        let slow = client(&db.store).await;
        let quick = client(&db.store).await;
        let (stale, stale_payment) =
            invoice(&db.store, slow, &q, Some(Utc::now() - Duration::minutes(1))).await;
        created(&ctx, &db.store, &stale, &stale_payment)
            .await
            .unwrap();
        let (fresh, fresh_payment) = invoice(
            &db.store,
            quick,
            &q,
            Some(Utc::now() + Duration::minutes(60)),
        )
        .await;
        created(&ctx, &db.store, &fresh, &fresh_payment)
            .await
            .unwrap();
        pay(&ctx, &db.store, &fresh, &fresh_payment).await.unwrap();
        let late = pay(&ctx, &db.store, &stale, &stale_payment).await;
        assert!(late.is_ok(), "{}: {late:?}", promo.code);
        let mut conn = db.store.conn().await.unwrap();
        let used: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM promo_redemptions WHERE promo_id = $1 AND status = 'used'",
        )
        .bind(promo.id)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        assert_eq!(
            used, 2,
            "{}: оба счёта оплачены по цене со скидкой",
            promo.code
        );
    }
    db.cleanup().await;
}

#[tokio::test]
async fn брошенная_активация_кода_на_дни_считается_использованной() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut week = percent("CRASH7", 1, None);
    week.value = NewPromoValue::Days { days: 7 };
    create(&ctx, &week, None).await.unwrap();
    let owner = client(&db.store).await;
    let sub = subscription(&db, owner).await;
    hold_days(&ctx, owner, "CRASH7", sub).await.unwrap();
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("UPDATE promo_redemptions SET reserved_until = now() - interval '1 minute'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    assert!(matches!(
        activate(&ctx, owner, "CRASH7", sub).await,
        Err(PromoError::Rejected(PromoRejection::AlreadyUsed))
    ));
    assert!(
        core.days().is_empty(),
        "дни по брошенной брони не добавляются повторно"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn скидки_на_первую_покупку_не_складываются_в_разных_счетах() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut welcome = percent("WELCOME", 30, None);
    welcome.first_purchase_only = true;
    let welcome = create(&ctx, &welcome, None).await.unwrap();
    let mut hello = percent("HELLO", 20, None);
    hello.first_purchase_only = true;
    let hello = create(&ctx, &hello, None).await.unwrap();
    let buyer = client(&db.store).await;
    let until = Some(Utc::now() + Duration::minutes(60));
    let (first, first_payment) = invoice(
        &db.store,
        buyer,
        &priced(&[welcome.adjustment().unwrap()]),
        until,
    )
    .await;
    created(&ctx, &db.store, &first, &first_payment)
        .await
        .unwrap();
    let (second, second_payment) = invoice(
        &db.store,
        buyer,
        &priced(&[hello.adjustment().unwrap()]),
        until,
    )
    .await;
    assert!(matches!(
        created(&ctx, &db.store, &second, &second_payment).await,
        Err(ModuleError::Invalid(_))
    ));
    assert!(matches!(
        check(&ctx, buyer, "HELLO", checkout()).await,
        Err(PromoError::Rejected(PromoRejection::FirstPurchaseOnly))
    ));
    let plain = order(&db.store, buyer, OrderKind::New, intent(), priced(&[])).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.orders()
            .transition(plain.id, sn_domain::order::OrderStatus::Paid)
            .await
            .unwrap();
    }
    pay(&ctx, &db.store, &first, &first_payment).await.unwrap();
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    let welcome_row = page
        .items
        .iter()
        .find(|i| i.code.code == "WELCOME")
        .unwrap();
    assert_eq!((welcome_row.used, welcome_row.reserved), (1, 0));
    db.cleanup().await;
}

fn any_check() -> Purpose {
    Purpose::Check {
        tariff: None,
        currency: Currency::Usd,
    }
}

async fn failures_of(store: &sn_store::Store, client_id: ClientId) -> i64 {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_scalar(
        "SELECT COALESCE(sum(failures), 0)::bigint FROM promo_attempts WHERE client_id = $1",
    )
    .bind(client_id.0)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

#[tokio::test]
async fn код_неверного_формата_отклоняется_без_обращения_к_базе() {
    let store = offline();
    let core = TestCore::new(&store);
    let ctx = context(&store, &core, Currency::Usd);
    let started = Instant::now();
    for raw in ["", "AB", "%ZIMA%", "ЗИМА", "ZIMA 2026"] {
        assert!(
            matches!(
                check(&ctx, ClientId(1), raw, checkout()).await,
                Err(PromoError::Rejected(PromoRejection::NotFound))
            ),
            "проверка «{raw}»"
        );
        assert!(
            matches!(
                activate(&ctx, ClientId(1), raw, SubscriptionId(1)).await,
                Err(PromoError::Rejected(PromoRejection::NotFound))
            ),
            "активация «{raw}»"
        );
        assert!(
            matches!(
                PromoModule
                    .quote_adjustments(&ctx, &hook(ClientId(1), raw))
                    .await,
                Ok(_) | Err(ModuleError::Invalid(_))
            ),
            "расчёт «{raw}»"
        );
    }
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
}

#[tokio::test]
async fn маршруты_отклоняют_код_неверного_формата_без_обращения_к_базе() {
    let store = offline();
    let core = TestCore::new(&store);
    let ctx = context(&store, &core, Currency::Usd);
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx);
    let started = Instant::now();
    for (path, body) in [
        ("/promo/check", serde_json::json!({"code": "%ZIMA%"})),
        (
            "/promo/activate",
            serde_json::json!({"code": "ЗИМА", "subscription_id": 1}),
        ),
    ] {
        let (status, answer) =
            post_from(&router, ClientId(1), Some("203.0.113.7"), path, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {answer}");
        assert_eq!(answer["error"]["code"], "promo.not_found", "{path}");
    }
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
}

#[tokio::test]
async fn параллельный_перебор_проверяет_не_больше_кодов_чем_разрешено() {
    let Some(db) = test_store().await else {
        return;
    };
    window_room(&db.store).await;
    let wide = pooled(&db, 16, std::time::Duration::from_secs(8)).await;
    let core = TestCore::new(&wide);
    let ctx = context(&wide, &core, Currency::Usd);
    let guesser = client(&db.store).await;
    let mut handles = Vec::new();
    for n in 0..80 {
        let ctx = ctx.clone();
        handles.push(tokio::spawn(async move {
            check(&ctx, guesser, &format!("WRONG{n:03}"), any_check()).await
        }));
    }
    let (mut evaluated, mut limited) = (0, 0);
    for handle in handles {
        match handle.await.unwrap() {
            Err(PromoError::Rejected(PromoRejection::NotFound)) => evaluated += 1,
            Err(PromoError::Rejected(PromoRejection::TooManyAttempts)) => limited += 1,
            other => panic!("неожиданный ответ: {other:?}"),
        }
    }
    assert_eq!((evaluated, limited), (FAILURE_LIMIT, 80 - FAILURE_LIMIT));
    assert_eq!(
        failures_of(&db.store, guesser).await,
        i64::from(FAILURE_LIMIT)
    );
    wide.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn удачная_проверка_и_отказ_без_перебора_не_расходуют_попытки() {
    let Some(db) = test_store().await else {
        return;
    };
    window_room(&db.store).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("GOOD20", 20, None), None)
        .await
        .unwrap();
    let mut week = percent("WEEK7", 1, None);
    week.value = NewPromoValue::Days { days: 7 };
    create(&ctx, &week, None).await.unwrap();
    let buyer = client(&db.store).await;
    for n in 0..FAILURE_LIMIT - 1 {
        assert!(matches!(
            check(&ctx, buyer, &format!("MISS{n}"), any_check()).await,
            Err(PromoError::Rejected(PromoRejection::NotFound))
        ));
    }
    for _ in 0..FAILURE_LIMIT * 2 {
        assert!(check(&ctx, buyer, "good20", any_check()).await.is_ok());
        assert!(matches!(
            check(&ctx, buyer, "WEEK7", checkout()).await,
            Err(PromoError::Rejected(PromoRejection::DaysCode))
        ));
    }
    assert_eq!(
        failures_of(&db.store, buyer).await,
        i64::from(FAILURE_LIMIT - 1)
    );
    assert!(matches!(
        check(&ctx, buyer, "MISSLAST", any_check()).await,
        Err(PromoError::Rejected(PromoRejection::NotFound))
    ));
    assert!(matches!(
        check(&ctx, buyer, "GOOD20", any_check()).await,
        Err(PromoError::Rejected(PromoRejection::TooManyAttempts))
    ));
    assert_eq!(
        failures_of(&db.store, buyer).await,
        i64::from(FAILURE_LIMIT),
        "отказ по лимиту не увеличивает счётчик"
    );
    db.cleanup().await;
}

async fn post_from(
    router: &axum::Router,
    who: ClientId,
    address: Option<&str>,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::post(path)
        .header("content-type", "application/json")
        .extension(customer(who));
    if let Some(address) = address {
        let ip: IpAddr = address.parse().unwrap();
        request = request.extension(ClientAddr(Some(ip)));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[test]
fn адрес_ipv6_сводится_к_сети_64_а_ipv4_остаётся_адресом() {
    let network = |raw: &str| address_network(raw.parse().unwrap());
    assert_eq!(network("203.0.113.7"), "203.0.113.7");
    assert_eq!(network("::ffff:203.0.113.7"), "203.0.113.7");
    assert_eq!(
        network("2001:db8:1:2:aaaa:bbbb:cccc:dddd"),
        "2001:db8:1:2::/64"
    );
    assert_eq!(network("2001:db8:1:2::1"), "2001:db8:1:2::/64");
    assert_ne!(network("2001:db8:1:3::1"), network("2001:db8:1:2::1"));
    let limit = |raw: &str| ClientAddr(Some(raw.parse().unwrap())).limit_network();
    assert_eq!(ClientAddr(None).limit_network(), None);
    assert_eq!(limit("198.51.100.9").as_deref(), Some("198.51.100.9"));
    assert_eq!(
        limit("2001:db8:1:2::9").as_deref(),
        Some("2001:db8:1:2::/64")
    );
    for internal in [
        "127.0.0.1",
        "10.1.2.3",
        "172.18.0.1",
        "192.168.1.1",
        "169.254.1.1",
        "100.64.0.1",
        "0.0.0.0",
        "::1",
        "::",
        "fd00::1",
        "fe80::1",
        "::ffff:10.0.0.1",
    ] {
        assert!(is_internal(internal.parse().unwrap()), "{internal}");
        assert_eq!(limit(internal), None, "{internal}");
    }
    for public in ["203.0.113.7", "8.8.8.8", "100.128.0.1", "2001:db8::1"] {
        assert!(!is_internal(public.parse().unwrap()), "{public}");
    }
}

#[tokio::test]
async fn внутренний_адрес_прокси_не_становится_общим_лимитом_для_всех() {
    let Some(db) = test_store().await else {
        return;
    };
    window_room(&db.store).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("REAL20", 20, None), None)
        .await
        .unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    for gateway in ["172.18.0.1", "127.0.0.1"] {
        for account in 0..ADDRESS_FAILURE_LIMIT / FAILURE_LIMIT + 1 {
            let who = client(&db.store).await;
            for n in 0..FAILURE_LIMIT {
                let (status, _) = post_from(
                    &router,
                    who,
                    Some(gateway),
                    "/promo/check",
                    serde_json::json!({ "code": format!("GUESS{account}X{n}") }),
                )
                .await;
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            }
            let (status, _) = post_from(
                &router,
                who,
                Some(gateway),
                "/promo/check",
                serde_json::json!({"code": "REAL20"}),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::TOO_MANY_REQUESTS,
                "лимит клиента действует"
            );
        }
        let fresh = client(&db.store).await;
        let (status, body) = post_from(
            &router,
            fresh,
            Some(gateway),
            "/promo/check",
            serde_json::json!({"code": "REAL20"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{gateway}: {body}");
    }
    db.cleanup().await;
}

#[tokio::test]
async fn перебор_с_одного_адреса_останавливается_и_для_новых_аккаунтов() {
    let Some(db) = test_store().await else {
        return;
    };
    window_room(&db.store).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("REAL20", 20, None), None)
        .await
        .unwrap();
    let mut week = percent("WEEK7", 1, None);
    week.value = NewPromoValue::Days { days: 7 };
    create(&ctx, &week, None).await.unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let office = Some("203.0.113.7");
    let accounts = ADDRESS_FAILURE_LIMIT / FAILURE_LIMIT;
    for account in 0..accounts {
        let who = client(&db.store).await;
        for n in 0..FAILURE_LIMIT {
            let (status, _) = post_from(
                &router,
                who,
                office,
                "/promo/check",
                serde_json::json!({ "code": format!("GUESS{account}X{n}") }),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "аккаунт {account}, попытка {n}"
            );
        }
    }
    let fresh = client(&db.store).await;
    let (status, body) = post_from(
        &router,
        fresh,
        office,
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "promo.rate_limited");
    let sub = subscription(&db, fresh).await;
    let (status, _) = post_from(
        &router,
        fresh,
        office,
        "/promo/activate",
        serde_json::json!({"code": "WEEK7", "subscription_id": sub}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(core.days().is_empty());
    assert_eq!(
        failures_of(&db.store, fresh).await,
        0,
        "отказ по адресу не тратит попытки клиента"
    );
    let (status, body) = post_from(
        &router,
        fresh,
        Some("203.0.113.8"),
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post_from(
        &router,
        fresh,
        None,
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        check(&ctx, fresh, "REAL20", any_check()).await.is_ok(),
        "бот проверяет код без адреса: действует только лимит клиента"
    );
    db.cleanup().await;
}

async fn address_failures(store: &sn_store::Store) -> i64 {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_scalar("SELECT COALESCE(sum(failures), 0)::bigint FROM promo_address_attempts")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn payer(store: &sn_store::Store, provider: &str) -> ClientId {
    let who = client(store).await;
    let bought = order(store, who, OrderKind::New, intent(), priced(&[])).await;
    paid_payment(
        store,
        bought.id,
        who,
        provider,
        Amount::from(bought.quote.total),
    )
    .await;
    who
}

#[tokio::test]
async fn лимит_по_адресу_не_действует_на_клиентов_с_оплатой() {
    let Some(db) = test_store().await else {
        return;
    };
    window_room(&db.store).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("REAL20", 20, None), None)
        .await
        .unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let office = Some("203.0.113.9");
    for account in 0..ADDRESS_FAILURE_LIMIT / FAILURE_LIMIT {
        let who = client(&db.store).await;
        for n in 0..FAILURE_LIMIT {
            let (status, _) = post_from(
                &router,
                who,
                office,
                "/promo/check",
                serde_json::json!({ "code": format!("GUESS{account}X{n}") }),
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        }
    }
    let fresh = client(&db.store).await;
    let from_balance = payer(&db.store, "balance").await;
    for blocked in [fresh, from_balance] {
        let (status, body) = post_from(
            &router,
            blocked,
            office,
            "/promo/check",
            serde_json::json!({"code": "REAL20"}),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    }
    let paying = payer(&db.store, "cryptopay").await;
    let (status, body) = post_from(
        &router,
        paying,
        office,
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = address_failures(&db.store).await;
    for n in 0..FAILURE_LIMIT {
        let (status, _) = post_from(
            &router,
            paying,
            office,
            "/promo/check",
            serde_json::json!({ "code": format!("PAYER{n}") }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "попытка {n}");
    }
    assert_eq!(
        address_failures(&db.store).await,
        before,
        "ошибки клиента с оплатой не идут в счётчик адреса"
    );
    let (status, body) = post_from(
        &router,
        paying,
        office,
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "лимит на клиента остаётся: {body}"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn адреса_одной_сети_ipv6_делят_общий_лимит_перебора() {
    let Some(db) = test_store().await else {
        return;
    };
    window_room(&db.store).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("REAL20", 20, None), None)
        .await
        .unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let mut host = 0;
    for account in 0..ADDRESS_FAILURE_LIMIT / FAILURE_LIMIT {
        let who = client(&db.store).await;
        for n in 0..FAILURE_LIMIT {
            host += 1;
            let address = format!("2001:db8:1:2:{host:x}::{n:x}");
            let (status, _) = post_from(
                &router,
                who,
                Some(&address),
                "/promo/check",
                serde_json::json!({ "code": format!("GUESS{account}X{n}") }),
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{address}");
        }
    }
    let fresh = client(&db.store).await;
    let (status, body) = post_from(
        &router,
        fresh,
        Some("2001:db8:1:2:ffff::1"),
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    let (status, body) = post_from(
        &router,
        fresh,
        Some("2001:db8:1:3::1"),
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    db.cleanup().await;
}

#[tokio::test]
async fn удачные_проверки_не_расходуют_лимит_адреса() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("REAL20", 20, None), None)
        .await
        .unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let office = Some("198.51.100.20");
    for _ in 0..ADDRESS_FAILURE_LIMIT + 5 {
        let who = client(&db.store).await;
        let (status, body) = post_from(
            &router,
            who,
            office,
            "/promo/check",
            serde_json::json!({"code": "REAL20"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let failures: i64 = {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query_scalar("SELECT COALESCE(sum(failures), 0)::bigint FROM promo_address_attempts")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    assert_eq!(failures, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn счётчики_старше_суток_удаляются_при_неудаче_в_новом_окне() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let old = client(&db.store).await;
    let recent = client(&db.store).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        for (who, age) in [(old, "2 days"), (recent, "2 hours")] {
            sqlx::query(&format!(
                "INSERT INTO promo_attempts (client_id, window_start, failures) \
                 VALUES ($1, date_trunc('hour', now() - interval '{age}'), 7)"
            ))
            .bind(who.0)
            .execute(conn.conn())
            .await
            .unwrap();
            sqlx::query(&format!(
                "INSERT INTO promo_address_attempts (address, window_start, failures) \
                 VALUES ($1, date_trunc('hour', now() - interval '{age}'), 7)"
            ))
            .bind(format!("198.51.100.{}", who.0 % 200))
            .execute(conn.conn())
            .await
            .unwrap();
        }
    }
    let guesser = client(&db.store).await;
    let office: IpAddr = "203.0.113.90".parse().unwrap();
    assert!(
        check_from(&ctx, guesser, Some(office), "GOOD20", any_check())
            .await
            .is_err()
    );
    let mut conn = db.store.conn().await.unwrap();
    let clients: Vec<i64> =
        sqlx::query_scalar("SELECT client_id FROM promo_attempts ORDER BY client_id")
            .fetch_all(conn.conn())
            .await
            .unwrap();
    assert_eq!(clients, vec![recent.0, guesser.0]);
    let addresses: i64 = sqlx::query_scalar("SELECT count(*) FROM promo_address_attempts")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(addresses, 2);
    drop(conn);
    db.cleanup().await;
}

async fn failures_until(store: &sn_store::Store, client_id: ClientId, expected: i64) -> i64 {
    let mut seen = failures_of(store, client_id).await;
    for _ in 0..150 {
        if seen == expected {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        seen = failures_of(store, client_id).await;
    }
    seen
}

#[tokio::test]
async fn оборванный_запрос_не_оставляет_засчитанной_попытки() {
    let Some(db) = test_store().await else {
        return;
    };
    window_room(&db.store).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("GOOD20", 20, None), None)
        .await
        .unwrap();
    let buyer = client(&db.store).await;
    let office: IpAddr = "203.0.113.77".parse().unwrap();
    let mut blocker = db.store.begin().await.unwrap();
    let window = attempt_window(db_now(&mut blocker).await.unwrap());
    sqlx::query(
        "INSERT INTO promo_address_attempts (address, window_start, failures) VALUES ($1, $2, 0)",
    )
    .bind(address_network(office))
    .bind(window)
    .execute(blocker.conn())
    .await
    .unwrap();
    let dropped = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        check_from(&ctx, buyer, Some(office), "GOOD20", any_check()),
    )
    .await;
    assert!(dropped.is_err(), "запрос ждал счётчик адреса и был оборван");
    assert_eq!(
        failures_until(&db.store, buyer, 1).await,
        1,
        "попытка засчитана до проверки кода"
    );
    blocker.rollback().await.unwrap();
    assert_eq!(
        failures_until(&db.store, buyer, 0).await,
        0,
        "верный код прощён, хотя клиент не дождался ответа"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn оборванная_активация_доводится_до_конца_и_не_повторяется() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut week = percent("WEEK7", 1, None);
    week.value = NewPromoValue::Days { days: 7 };
    create(&ctx, &week, None).await.unwrap();
    let owner = client(&db.store).await;
    let sub = subscription(&db, owner).await;
    core.slow_days_ms.store(400, Ordering::SeqCst);
    let dropped = tokio::time::timeout(
        std::time::Duration::from_millis(150),
        activate(&ctx, owner, "WEEK7", sub),
    )
    .await;
    assert!(
        dropped.is_err(),
        "клиент оборвал запрос, пока ядро добавляло дни"
    );
    core.slow_days_ms.store(0, Ordering::SeqCst);
    let mut status = String::new();
    for _ in 0..150 {
        let mut conn = db.store.conn().await.unwrap();
        status = sqlx::query_scalar("SELECT status FROM promo_redemptions")
            .fetch_optional(conn.conn())
            .await
            .unwrap()
            .unwrap_or_default();
        drop(conn);
        if status == "used" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(status, "used", "активация записана без участия клиента");
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("UPDATE promo_redemptions SET reserved_until = now() - interval '1 minute'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    assert!(matches!(
        activate(&ctx, owner, "WEEK7", sub).await,
        Err(PromoError::Rejected(PromoRejection::AlreadyUsed))
    ));
    assert_eq!(core.days().len(), 1, "дни добавлены один раз");
    db.cleanup().await;
}
