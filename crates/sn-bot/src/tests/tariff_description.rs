use super::*;
use sn_domain::bot_content::normalize_markup;
use sn_domain::ids::SquadId;
use sn_domain::tariff::Tariff;

async fn unblock(db: &TestDb, description: &str) -> Tariff {
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: "Unblock".into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    let tariff = tx
        .tariffs()
        .create(&NewTariff {
            traffic_addon: None,
            category_id: category.id,
            name: "Unblock".into(),
            description: Some(description.into()),
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 4,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![SquadId(uuid::Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(39_900),
                },
                stars: Some(Stars(150)),
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tariff
}

async fn tariff_screen(description: &str, chat: i64) -> Option<String> {
    let (db, _services, server, ctx) = setup().await?;
    let tariff = unblock(&db, description).await;
    handle_update(&ctx, text_update(1, chat, "/start")).await;
    handle_update(&ctx, callback_update(2, chat, "buy")).await;
    handle_update(&ctx, callback_update(3, chat, &format!("t:{}", tariff.id))).await;
    let screen = last_text(&server).await;
    db.cleanup().await;
    Some(screen)
}

#[tokio::test]
async fn экран_тарифа_показывает_описание_с_оформлением_и_переносами() {
    let description = "🎉 <b>Чёрная пятница!</b>\n⏳ Только 2 дня!\n\n💰 <b>Цена:</b> <s>от 419 ₽</s> <i>(доп. устройства можно докупить)</i>";
    let Some(screen) = tariff_screen(description, 9_101).await else {
        return;
    };
    assert!(
        screen.contains(&format!(
            "<b>Unblock</b>\n\n{description}\n\nУстройств в тарифе: 4\nТрафик: без лимита\n\nВыберите срок:"
        )),
        "{screen}"
    );
    assert!(normalize_markup(&screen).is_ok(), "{screen}");
}

#[tokio::test]
async fn старое_описание_в_одну_строку_выглядит_как_раньше() {
    let Some(screen) = tariff_screen("Для мобильного интернета", 9_102).await
    else {
        return;
    };
    assert!(
        screen.contains(
            "<b>Unblock</b>\nДля мобильного интернета\nУстройств в тарифе: 4\nТрафик: без лимита\n\nВыберите срок:"
        ),
        "{screen}"
    );
}

#[tokio::test]
async fn битая_разметка_описания_из_базы_не_ломает_сообщение() {
    let broken = "<b>Цена: <s>от 419 ₽</b>\n<div>чужой</div> & <a href=\"javascript:alert(1)\">ссылка</a> <i>курсив";
    let Some(screen) = tariff_screen(broken, 9_103).await else {
        return;
    };
    assert!(
        screen.contains(
            "<b>Unblock</b>\n\n<b>Цена: <s>от 419 ₽</s></b>\n&lt;div&gt;чужой&lt;/div&gt; &amp; ссылка <i>курсив</i>\n\nУстройств в тарифе: 4"
        ),
        "{screen}"
    );
    assert!(normalize_markup(&screen).is_ok(), "{screen}");
}
