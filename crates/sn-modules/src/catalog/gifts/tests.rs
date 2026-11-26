use super::*;
use crate::catalog::testing::{TestCore, client, context, order, usd};
use crate::context::GrantRefusal;
use crate::module::ItemHook;
use sn_domain::ids::SquadId;
use sn_domain::money::Currency;
use sn_domain::order::OrderKind;
use sn_domain::tariff::CurrencyPrices;
use sn_store::testing::test_store;
use sn_store::{NewCategory, NewPeriod, NewTariff, Store};

async fn tariff(store: &Store, name: &str) -> Tariff {
    let mut conn = store.conn().await.unwrap();
    let category = conn
        .tariffs()
        .create_category(&NewCategory {
            name: format!("Категория {name}"),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    conn.tariffs()
        .create(&NewTariff {
            traffic_addon: None,
            category_id: category.id,
            name: name.into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![SquadId(Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![
                NewPeriod {
                    days: 30,
                    prices: CurrencyPrices {
                        usd: Some(299),
                        rub: Some(19_900),
                    },
                    stars: Some(Stars(150)),
                    sort: 0,
                    active: true,
                },
                NewPeriod {
                    days: 90,
                    prices: CurrencyPrices {
                        usd: Some(699),
                        rub: None,
                    },
                    stars: None,
                    sort: 1,
                    active: true,
                },
            ],
        })
        .await
        .unwrap()
}

async fn links_settings(store: &Store) {
    let mut conn = store.conn().await.unwrap();
    conn.settings()
        .set(keys::BOT_USERNAME, &serde_json::json!("vpn_test_bot"), None)
        .await
        .unwrap();
    conn.settings()
        .set(
            keys::PUBLIC_URL,
            &serde_json::json!("https://panel.example.com/"),
            None,
        )
        .await
        .unwrap();
}

async fn telegram_client(store: &Store, id: i64, username: &str) -> ClientId {
    let client_id = client(store).await;
    let mut conn = store.conn().await.unwrap();
    conn.clients()
        .add_identity(
            client_id,
            &sn_domain::client::NewIdentity {
                kind: sn_domain::client::IdentityKind::Telegram,
                value: id.to_string(),
                verified: true,
                meta: serde_json::json!({"first_name": "Мария", "username": username}),
            },
        )
        .await
        .unwrap();
    client_id
}

fn period(tariff: &Tariff, days: u32) -> PeriodId {
    tariff.periods.iter().find(|p| p.days == days).unwrap().id
}

async fn bought(
    store: &Store,
    core: &std::sync::Arc<TestCore>,
    giver: ClientId,
    tariff: &Tariff,
    days: u32,
) -> (Order, Gift) {
    let ctx = context(store, core, Currency::Usd);
    let request = GiftRequest {
        tariff_id: tariff.id,
        period_id: period(tariff, days),
    };
    let quote = GiftsModule
        .quote_item(
            &ctx,
            &ItemHook {
                client_id: giver,
                currency: Currency::Usd,
                kind: ITEM_GIFT.into(),
                payload: request.payload(),
            },
        )
        .await
        .unwrap();
    let mut paid = order(
        store,
        giver,
        OrderKind::Module,
        PurchaseIntent::Module {
            module: ModuleId::Gifts,
            kind: ITEM_GIFT.into(),
            payload: request.payload(),
        },
        quote,
    )
    .await;
    {
        let mut conn = store.conn().await.unwrap();
        conn.orders()
            .transition(paid.id, OrderStatus::Paid)
            .await
            .unwrap();
    }
    paid.status = OrderStatus::Paid;
    GiftsModule.fulfill(&ctx, &paid).await.unwrap();
    let gift = issue(&ctx, &paid).await.unwrap();
    (paid, gift)
}

#[test]
fn код_подарка_приводится_к_виду_и_берётся_из_ссылки() {
    assert_eq!(
        normalize_code(" abcd efgh jkmn ").as_deref(),
        Some("ABCD-EFGH-JKMN")
    );
    assert_eq!(
        normalize_code("ABCDEFGHJKMN").as_deref(),
        Some("ABCD-EFGH-JKMN")
    );
    assert_eq!(
        normalize_code("https://t.me/vpn_bot?start=gift_ABCD-EFGH-JKMN").as_deref(),
        Some("ABCD-EFGH-JKMN")
    );
    assert_eq!(normalize_code("ABCD-EFGH-JKM"), None);
    assert_eq!(normalize_code("ABCD-EFGH-JKM0"), None);
    assert_eq!(normalize_code(""), None);
    assert_eq!(
        start_code("gift_ABCD-EFGH-JKMN__s_tiktok").as_deref(),
        Some("ABCD-EFGH-JKMN")
    );
    assert_eq!(start_code("ref_ABC"), None);
    let code = generate_code();
    assert_eq!(normalize_code(&code), Some(code.clone()));
    assert_eq!(
        bot_link("@vpn_bot", "ABCD-EFGH-JKMN"),
        "https://t.me/vpn_bot?start=gift_ABCD-EFGH-JKMN"
    );
    assert_eq!(
        cabinet_link("https://panel.example.com/", "ABCD-EFGH-JKMN"),
        "https://panel.example.com/cabinet/gifts?code=ABCD-EFGH-JKMN"
    );
    assert_eq!(term(30), "1\u{a0}месяц");
    assert_eq!(term(90), "3\u{a0}месяца");
    assert_eq!(term(7), "7\u{a0}дней");
}

#[test]
fn настройки_подарков_читаются_и_проверяются() {
    let parsed = GiftSettings::parse(&ModuleConfig::from_value(
        &serde_json::json!({"tariffs": [3, 5, 3], "code_days": "30"}),
    ))
    .unwrap();
    assert_eq!(parsed.tariffs, vec![TariffId(3), TariffId(5)]);
    assert_eq!(parsed.code_days, 30);
    assert!(parsed.allows(TariffId(5)));
    assert!(!parsed.allows(TariffId(4)));
    let empty = GiftSettings::parse(&ModuleConfig::default()).unwrap();
    assert_eq!(empty.code_days, DEFAULT_CODE_DAYS);
    assert!(empty.allows(TariffId(4)));
    assert!(
        GiftSettings::parse(&ModuleConfig::from_value(
            &serde_json::json!({"tariffs": "все"})
        ))
        .is_err()
    );
    assert!(
        GiftSettings::parse(&ModuleConfig::from_value(
            &serde_json::json!({"code_days": 5000})
        ))
        .is_err()
    );
    assert!(
        GiftSettings::parse(&ModuleConfig::from_value(
            &serde_json::json!({"code_days": "год"})
        ))
        .is_err()
    );
}

#[tokio::test]
async fn подарок_оплачивается_как_товар_и_выдаёт_один_код() {
    let Some(db) = test_store().await else {
        return;
    };
    links_settings(&db.store).await;
    let core = TestCore::new(&db.store);
    let giver = client(&db.store).await;
    let standard = tariff(&db.store, "Стандарт").await;
    let ctx = context(&db.store, &core, Currency::Usd);
    let quote = GiftsModule
        .quote_item(
            &ctx,
            &ItemHook {
                client_id: giver,
                currency: Currency::Usd,
                kind: ITEM_GIFT.into(),
                payload: GiftRequest {
                    tariff_id: standard.id,
                    period_id: period(&standard, 30),
                }
                .payload(),
            },
        )
        .await
        .unwrap();
    assert_eq!(quote.total, usd(299));
    assert_eq!(quote.stars.unwrap().total, Stars(150));
    assert_eq!(quote.lines[0].label, "Подарок: «Стандарт» на 1\u{a0}месяц");
    let (paid, gift) = bought(&db.store, &core, giver, &standard, 90).await;
    assert_eq!(gift.order_id, paid.id);
    assert_eq!(gift.giver_id, giver);
    assert_eq!(gift.days, 90);
    assert_eq!(gift.status, GiftStatus::Active);
    let expires = gift.expires_at.unwrap() - gift.created_at;
    assert!((expires.num_hours() - 90 * 24).abs() <= 1, "{expires:?}");
    GiftsModule.fulfill(&ctx, &paid).await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM gifts")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(total, 1);
    drop(conn);
    let ready: Vec<_> = core
        .notifications()
        .into_iter()
        .filter(|(_, n)| n.template == TEMPLATE_READY)
        .collect();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].0, giver);
    assert_eq!(ready[0].1.params["code"], gift.code);
    assert_eq!(
        ready[0].1.params["bot_link"],
        format!("https://t.me/vpn_test_bot?start=gift_{}", gift.code)
    );
    assert_eq!(
        ready[0].1.params["cabinet_link"],
        format!("https://panel.example.com/cabinet/gifts?code={}", gift.code)
    );
    let mine = mine(&ctx, giver).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].term, "3\u{a0}месяца");
    assert!(mine[0].links.bot.is_some());
    db.cleanup().await;
}

#[tokio::test]
async fn настройки_ограничивают_тарифы_и_срок_кода() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let giver = client(&db.store).await;
    let standard = tariff(&db.store, "Стандарт").await;
    let premium = tariff(&db.store, "Премиум").await;
    let saved = save_settings(
        &ctx,
        &SettingsUpdate {
            tariffs: vec![premium.id],
            code_days: 0,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(saved.tariffs, vec![premium.id]);
    assert_eq!(saved.code_days, 0);
    assert!(
        saved
            .available
            .iter()
            .any(|t| t.id == standard.id && t.giftable)
    );
    let mut conn = db.store.conn().await.unwrap();
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let offered = offers(&mut conn, Currency::Usd, Utc::now(), utc)
        .await
        .unwrap();
    assert_eq!(
        offered.iter().map(|o| o.tariff.id).collect::<Vec<_>>(),
        vec![premium.id]
    );
    let rub = offers(&mut conn, Currency::Rub, Utc::now(), utc)
        .await
        .unwrap();
    assert_eq!(rub[0].periods.len(), 1);
    let refused = quote(
        &mut conn,
        GiftRequest {
            tariff_id: standard.id,
            period_id: period(&standard, 30),
        },
        Currency::Usd,
        Utc::now(),
    )
    .await;
    assert!(matches!(
        refused,
        Err(GiftError::Rejected(GiftRejection::NotGiftable))
    ));
    let no_price = quote(
        &mut conn,
        GiftRequest {
            tariff_id: premium.id,
            period_id: period(&premium, 90),
        },
        Currency::Rub,
        Utc::now(),
    )
    .await;
    assert!(matches!(
        no_price,
        Err(GiftError::Rejected(GiftRejection::NoPeriod))
    ));
    drop(conn);
    let (_, gift) = bought(&db.store, &core, giver, &premium, 30).await;
    assert_eq!(gift.expires_at, None);
    assert!(
        save_settings(
            &ctx,
            &SettingsUpdate {
                tariffs: vec![TariffId(999_999)],
                code_days: 30,
            },
            None,
        )
        .await
        .is_err()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn подарок_активируется_один_раз_и_даритель_узнаёт_об_этом() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let giver = client(&db.store).await;
    let friend = client(&db.store).await;
    let stranger = client(&db.store).await;
    let standard = tariff(&db.store, "Стандарт").await;
    let (_, gift) = bought(&db.store, &core, giver, &standard, 90).await;
    let lower = gift.code.to_lowercase().replace('-', "");
    let shown = preview(&ctx, friend, &lower, GrantTarget::Auto)
        .await
        .unwrap();
    assert_eq!(shown.gift.code, gift.code);
    assert_eq!(shown.gift.tariff, "Стандарт");
    assert!(!shown.gift.own);
    let done = redeem(&ctx, friend, &lower, GrantTarget::Auto)
        .await
        .unwrap();
    assert_eq!(done.gift.code, gift.code);
    let grants = core.grants();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].0.client_id, friend);
    assert_eq!(
        grants[0].0.spec,
        GrantSpec::Tariff {
            tariff_id: standard.id,
            period_id: period(&standard, 90),
            days: 90,
        }
    );
    assert_eq!(grants[0].0.reference, format!("gift:{}", gift.id));
    let mut conn = db.store.conn().await.unwrap();
    let now = db_now(&mut conn).await.unwrap();
    let stored = store::by_code(&mut conn, &gift.code, now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, GiftStatus::Redeemed);
    assert_eq!(stored.redeemed_by, Some(friend));
    assert_eq!(stored.grant_order_id, Some(grants[0].1));
    drop(conn);
    assert!(matches!(
        redeem(&ctx, friend, &gift.code, GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::RedeemedByYou))
    ));
    assert!(matches!(
        redeem(&ctx, stranger, &gift.code, GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::Redeemed))
    ));
    assert!(matches!(
        preview(&ctx, stranger, &gift.code, GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::Redeemed))
    ));
    assert_eq!(core.grants().len(), 1);
    let redeemed: Vec<_> = core
        .notifications()
        .into_iter()
        .filter(|(_, n)| n.template == TEMPLATE_REDEEMED)
        .collect();
    assert_eq!(redeemed.len(), 1);
    assert_eq!(redeemed[0].0, giver);
    assert!(matches!(
        redeem(&ctx, stranger, "ABCD-EFGH-JKMN", GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::NotFound))
    ));
    assert!(matches!(
        redeem(&ctx, stranger, "не код", GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::NotFound))
    ));
    db.cleanup().await;
}

#[tokio::test]
async fn даритель_может_активировать_свой_подарок_без_уведомления_себе() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let giver = client(&db.store).await;
    let standard = tariff(&db.store, "Стандарт").await;
    let (_, gift) = bought(&db.store, &core, giver, &standard, 30).await;
    let own = preview(&ctx, giver, &gift.code, GrantTarget::Auto)
        .await
        .unwrap();
    assert!(own.gift.own);
    redeem(&ctx, giver, &gift.code, GrantTarget::Auto)
        .await
        .unwrap();
    assert!(
        core.notifications()
            .iter()
            .all(|(_, n)| n.template != TEMPLATE_REDEEMED)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn просрочка_возврат_и_отказ_ядра_не_выдают_подписку() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let giver = client(&db.store).await;
    let friend = client(&db.store).await;
    let standard = tariff(&db.store, "Стандарт").await;
    let (_, expired) = bought(&db.store, &core, giver, &standard, 30).await;
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query("UPDATE gifts SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(expired.id)
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    assert!(matches!(
        redeem(&ctx, friend, &expired.code, GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::Expired))
    ));
    let (refunded_order, refunded) = bought(&db.store, &core, giver, &standard, 30).await;
    let mut conn = db.store.conn().await.unwrap();
    conn.orders()
        .transition(refunded_order.id, OrderStatus::Fulfilled)
        .await
        .unwrap();
    conn.orders()
        .transition(refunded_order.id, OrderStatus::Refunded)
        .await
        .unwrap();
    drop(conn);
    assert!(matches!(
        preview(&ctx, friend, &refunded.code, GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::Canceled))
    ));
    GiftsModule
        .on_event(
            &ctx,
            &DomainEvent::PaymentRefunded {
                payment_id: sn_domain::ids::PaymentId(1),
                order_id: refunded_order.id,
                client_id: giver,
                amount: sn_domain::money::Amount::new(299, sn_domain::money::PayCurrency::Usd),
            },
        )
        .await
        .unwrap();
    let mut conn = db.store.conn().await.unwrap();
    let now = db_now(&mut conn).await.unwrap();
    let canceled = store::by_code(&mut conn, &refunded.code, now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(canceled.status, GiftStatus::Canceled);
    assert!(canceled.canceled_at.is_some());
    drop(conn);
    assert!(matches!(
        redeem(&ctx, friend, &refunded.code, GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::Canceled))
    ));
    let (_, refused) = bought(&db.store, &core, giver, &standard, 30).await;
    core.refuse(Some(GrantRefusal::new(
        "grant.has_subscription",
        "Подписку сейчас не выдать",
    )));
    match redeem(&ctx, friend, &refused.code, GrantTarget::Auto).await {
        Err(GiftError::Rejected(GiftRejection::Grant(refusal))) => {
            assert_eq!(refusal.code, "grant.has_subscription");
        }
        other => panic!("ожидали отказ ядра: {other:?}"),
    }
    core.refuse(None);
    let mut conn = db.store.conn().await.unwrap();
    let now = db_now(&mut conn).await.unwrap();
    let still = store::by_code(&mut conn, &refused.code, now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.status, GiftStatus::Active);
    drop(conn);
    redeem(&ctx, friend, &refused.code, GrantTarget::Auto)
        .await
        .unwrap();
    assert_eq!(core.grants().len(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn возврат_до_активации_гасит_код_даже_если_активируют_раньше_события() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let giver = client(&db.store).await;
    let friend = client(&db.store).await;
    let standard = tariff(&db.store, "Стандарт").await;
    let (paid, gift) = bought(&db.store, &core, giver, &standard, 30).await;
    let mut conn = db.store.conn().await.unwrap();
    conn.orders()
        .transition(paid.id, OrderStatus::Refunded)
        .await
        .unwrap();
    drop(conn);
    assert!(matches!(
        redeem(&ctx, friend, &gift.code, GrantTarget::Auto).await,
        Err(GiftError::Rejected(GiftRejection::Canceled))
    ));
    let mut conn = db.store.conn().await.unwrap();
    let now = db_now(&mut conn).await.unwrap();
    assert_eq!(
        store::by_code(&mut conn, &gift.code, now)
            .await
            .unwrap()
            .unwrap()
            .status,
        GiftStatus::Canceled
    );
    drop(conn);
    assert!(core.grants().is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn админка_видит_подарки_с_поиском_и_состояниями() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let giver = telegram_client(&db.store, 9_001, "maria_gift").await;
    let friend = telegram_client(&db.store, 9_002, "ivan_friend").await;
    let standard = tariff(&db.store, "Стандарт").await;
    let (_, first) = bought(&db.store, &core, giver, &standard, 30).await;
    let (_, second) = bought(&db.store, &core, giver, &standard, 90).await;
    redeem(&ctx, friend, &second.code, GrantTarget::Auto)
        .await
        .unwrap();
    let all = admin_list(&ctx, ListFilter::All, None, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(all.items.len(), 2);
    assert_eq!(all.items[0].gift.code, second.code);
    assert_eq!(
        all.items[0]
            .recipient
            .as_ref()
            .and_then(|p| p.telegram_username.as_deref()),
        Some("ivan_friend")
    );
    assert_eq!(
        all.items[1]
            .giver
            .as_ref()
            .and_then(|p| p.telegram_username.as_deref()),
        Some("maria_gift")
    );
    assert_eq!(all.items[1].paid.map(|a| a.minor), None);
    let active = admin_list(&ctx, ListFilter::Active, None, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(
        active
            .items
            .iter()
            .map(|i| i.gift.code.clone())
            .collect::<Vec<_>>(),
        vec![first.code.clone()]
    );
    let redeemed = admin_list(
        &ctx,
        ListFilter::Redeemed,
        Some("@ivan"),
        PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(redeemed.items.len(), 1);
    let by_code = admin_list(
        &ctx,
        ListFilter::All,
        Some(&first.code[..4]),
        PageRequest::default(),
    )
    .await
    .unwrap();
    assert!(by_code.items.iter().any(|i| i.gift.code == first.code));
    let nobody = admin_list(
        &ctx,
        ListFilter::All,
        Some("никого нет"),
        PageRequest::default(),
    )
    .await
    .unwrap();
    assert!(nobody.items.is_empty());
    let page = admin_list(&ctx, ListFilter::All, None, PageRequest::new(Some(1), None))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next.is_some());
    let counts = counts(&ctx).await.unwrap();
    assert_eq!(
        (
            counts.active,
            counts.redeemed,
            counts.expired,
            counts.canceled
        ),
        (1, 1, 0, 0)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn суммы_подарков_видны_только_с_правом_деньги() {
    use crate::catalog::testing::paid_payment;
    use sn_domain::access::{AdminPrincipal, AdminRole, Permission};
    use sn_domain::money::{Amount, PayCurrency};
    use tower::ServiceExt;
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let giver = telegram_client(&db.store, 9_101, "maria_gift").await;
    let standard = tariff(&db.store, "Стандарт").await;
    let (paid, gift) = bought(&db.store, &core, giver, &standard, 30).await;
    paid_payment(
        &db.store,
        paid.id,
        giver,
        "cryptopay",
        Amount::new(299, PayCurrency::Usd),
    )
    .await;
    let registry =
        std::sync::Arc::new(crate::registry::ModuleRegistry::new(crate::catalog::all()).unwrap());
    registry.apply_rows(&[sn_store::ModuleStateRow {
        module: ModuleId::Gifts.as_str().into(),
        enabled: true,
        config: serde_json::json!({}),
        updated_at: Utc::now(),
    }]);
    let router = registry.admin_router(ctx.clone());
    let list = |extra: &[Permission], api_key: Option<i64>| {
        let mut permissions = vec![Permission::Marketing];
        permissions.extend_from_slice(extra);
        let principal = AdminPrincipal {
            admin_id: sn_domain::ids::AdminId(1),
            role: AdminRole::Support,
            permissions,
            session_id: 1,
            api_key,
        };
        let router = router.clone();
        async move {
            let response = router
                .oneshot(
                    axum::http::Request::get("/gifts/gifts")
                        .extension(principal)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        }
    };
    for api_key in [None, Some(7)] {
        let page = list(&[], api_key).await;
        let item = &page["items"][0];
        assert_eq!(item["code"], gift.code.as_str(), "{page}");
        assert!(item.get("paid").is_none(), "{page}");
        assert!(item.get("refunded").is_some(), "{page}");
    }
    let page = list(&[Permission::Money], Some(7)).await;
    assert_eq!(page["items"][0]["paid"]["minor"], 299, "{page}");
    db.cleanup().await;
}

#[tokio::test]
async fn подарок_по_тарифу_акции_со_скидкой() {
    let Some(db) = test_store().await else {
        return;
    };
    let standard = tariff(&db.store, "Стандарт").await;
    let premium = tariff(&db.store, "Премиум").await;
    let now = Utc::now();
    let promotion = {
        let mut conn = db.store.conn().await.unwrap();
        conn.promotions()
            .create(&sn_domain::promotion::PromotionDraft {
                name: "Чёрная пятница".into(),
                percent: 25,
                tariff_ids: vec![standard.id],
                starts_at: now - Duration::hours(1),
                ends_at: now + Duration::days(1),
                enabled: true,
            })
            .await
            .unwrap()
    };
    let mut conn = db.store.conn().await.unwrap();
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let offered = offers(&mut conn, Currency::Usd, now, utc).await.unwrap();
    let month = offered
        .iter()
        .find(|o| o.tariff.id == standard.id)
        .unwrap()
        .periods
        .iter()
        .find(|p| p.days == 30)
        .unwrap();
    assert_eq!(month.price, Money::new(224, Currency::Usd));
    assert_eq!(month.stars, Some(Stars(113)));
    let shown = month.promotion.as_ref().unwrap();
    assert_eq!(shown.full_price, Money::new(299, Currency::Usd));
    assert_eq!(shown.percent, 25);
    assert!(shown.until.starts_with("до "), "{}", shown.until);
    assert!(
        offered
            .iter()
            .find(|o| o.tariff.id == premium.id)
            .unwrap()
            .periods
            .iter()
            .all(|p| p.promotion.is_none())
    );
    let request = |tariff: &Tariff| GiftRequest {
        tariff_id: tariff.id,
        period_id: period(tariff, 30),
    };
    let priced = quote(&mut conn, request(&standard), Currency::Usd, now)
        .await
        .unwrap();
    assert_eq!(priced.total, Money::new(224, Currency::Usd));
    assert_eq!(priced.subtotal, Money::new(299, Currency::Usd));
    assert_eq!(priced.discount, Money::new(75, Currency::Usd));
    assert_eq!(priced.stars.unwrap().total, Stars(113));
    assert_eq!(
        priced.lines[1].kind,
        QuoteLineKind::Promotion {
            promotion_id: promotion.id,
            percent: 25
        }
    );
    assert_eq!(
        priced.lines.iter().map(|l| l.amount.minor).sum::<i64>(),
        priced.total.minor
    );
    let plain = quote(&mut conn, request(&premium), Currency::Usd, now)
        .await
        .unwrap();
    assert_eq!(plain.total, Money::new(299, Currency::Usd));
    assert_eq!(plain.lines.len(), 1);
    let later = quote(
        &mut conn,
        request(&standard),
        Currency::Usd,
        now + Duration::days(2),
    )
    .await
    .unwrap();
    assert_eq!(later.total, Money::new(299, Currency::Usd));
    drop(conn);
    db.cleanup().await;
}
