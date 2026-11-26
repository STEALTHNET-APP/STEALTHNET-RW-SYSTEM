use super::*;
use chrono::{Duration, Utc};
use sn_domain::client::Locale;
use sn_domain::format::money_in;
use sn_domain::ids::SquadId;
use sn_domain::money::{Currency, Money};
use sn_domain::promotion::PromotionDraft;
use sn_domain::tariff::Tariff;

async fn standard(db: &TestDb) -> Tariff {
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: "Обычный".into(),
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
            name: "Стандарт".into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![SquadId(uuid::Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(19_900),
                },
                stars: Some(Stars(150)),
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    tx.promotions()
        .create(&PromotionDraft {
            name: "Чёрная пятница".into(),
            percent: 25,
            tariff_ids: vec![tariff.id],
            starts_at: Utc::now() - Duration::hours(1),
            ends_at: Utc::now() + Duration::days(3),
            enabled: true,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tariff
}

fn usd(minor: i64) -> String {
    money_in(Money::new(minor, Currency::Usd), Locale::Ru)
}

#[tokio::test]
async fn бот_показывает_цену_акции_и_зачёркивает_старую() {
    let Some((db, _services, server, ctx)) = setup().await else {
        return;
    };
    let tariff = standard(&db).await;
    handle_update(&ctx, text_update(1, 731, "/start")).await;
    handle_update(&ctx, callback_update(2, 731, "buy")).await;
    let catalog = last_text(&server).await;
    assert!(
        catalog.contains(&format!(
            "Стандарт — от <s>{}</s> <b>{}</b> в месяц, −25% · Чёрная пятница до ",
            usd(299),
            usd(224)
        )),
        "{catalog}"
    );
    handle_update(&ctx, callback_update(3, 731, &format!("t:{}", tariff.id))).await;
    let screen = last_text(&server).await;
    assert!(screen.contains("<b>Чёрная пятница</b> до "), "{screen}");
    assert!(
        screen.contains(&format!("— <s>{}</s> <b>{}</b>", usd(299), usd(224))),
        "{screen}"
    );
    let labels = last_labels(&server).await;
    assert!(
        labels
            .iter()
            .any(|l| l.contains(&usd(224)) && l.contains("−25%")),
        "{labels:?}"
    );
    handle_update(
        &ctx,
        callback_update(4, 731, &format!("p:{}", tariff.periods[0].id)),
    )
    .await;
    let summary = last_text(&server).await;
    assert!(summary.contains("Чёрная пятница, −25%"), "{summary}");
    assert!(
        summary.contains(&format!("К оплате: <b>{}</b>", usd(224))),
        "{summary}"
    );
    db.cleanup().await;
}
