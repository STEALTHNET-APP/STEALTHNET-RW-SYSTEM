use super::*;
use sn_domain::bot_content::ContentConfig;
use sn_domain::client::{Languages, Locale};
use sn_services::bot_content::ContentUpdate;
use std::collections::BTreeMap;

fn english_user(id: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id, "is_bot": false, "first_name": "Ann", "username": "ann_w", "language_code": "en-GB"
    })
}

fn english_text(update_id: i64, from: i64, text: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id, "from": english_user(from),
            "chat": {"id": from, "type": "private"}, "date": 1, "text": text
        }
    }))
    .unwrap()
}

fn english_callback(update_id: i64, from: i64, data: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "callback_query": {
            "id": format!("cb{update_id}"), "from": english_user(from), "data": data,
            "message": {"message_id": 55, "chat": {"id": from, "type": "private"}, "date": 1}
        }
    }))
    .unwrap()
}

async fn owner(services: &Services) -> sn_domain::ids::AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

async fn english_on(services: &Services, default: Locale) {
    let admin = owner(services).await;
    services
        .settings_ops()
        .update_languages(
            &Languages {
                english: true,
                default,
            },
            admin,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn без_английского_клиент_из_англоязычного_telegram_видит_русский() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, english_text(1, 951, "/start")).await;
    assert!(last_text(&server).await.contains("Подписки пока нет"));
    assert!(!last_buttons(&server).await.contains(&"lang".to_string()));
    assert_eq!(client_of(&services, 951).await.unwrap().locale, Locale::Ru);
    handle_update(&ctx, english_text(2, 951, "/language")).await;
    assert!(last_text(&server).await.contains("Подписки пока нет"));
    db.cleanup().await;
}

#[tokio::test]
async fn с_английским_новый_клиент_получает_язык_из_telegram_и_может_сменить_его() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    english_on(&services, Locale::Ru).await;
    handle_update(&ctx, english_text(1, 952, "/start")).await;
    assert_eq!(client_of(&services, 952).await.unwrap().locale, Locale::En);
    let menu = last_text(&server).await;
    assert!(menu.contains("No subscription yet."), "{menu}");
    let labels = last_labels(&server).await;
    assert!(
        labels.contains(&"Buy a subscription".to_string()),
        "{labels:?}"
    );
    assert!(labels.contains(&"Language".to_string()), "{labels:?}");
    assert!(last_buttons(&server).await.contains(&"lang".to_string()));

    handle_update(&ctx, english_callback(2, 952, "lang")).await;
    assert_eq!(last_text(&server).await, "Choose your language:");
    let labels = last_labels(&server).await;
    assert!(labels.contains(&"Русский".to_string()), "{labels:?}");
    assert!(labels.contains(&"✓ English".to_string()), "{labels:?}");

    handle_update(&ctx, english_callback(3, 952, "lang:ru")).await;
    assert_eq!(client_of(&services, 952).await.unwrap().locale, Locale::Ru);
    assert!(last_text(&server).await.contains("Подписки пока нет"));
    assert!(last_labels(&server).await.contains(&"Язык".to_string()));

    handle_update(&ctx, english_text(4, 952, "/start")).await;
    assert!(last_text(&server).await.contains("Подписки пока нет"));

    handle_update(&ctx, english_text(5, 952, "/language")).await;
    assert_eq!(last_text(&server).await, "Выберите язык:");
    handle_update(&ctx, english_callback(6, 952, "lang:en")).await;
    assert_eq!(client_of(&services, 952).await.unwrap().locale, Locale::En);
    assert!(last_text(&server).await.contains("No subscription yet."));
    db.cleanup().await;
}

#[tokio::test]
async fn английский_текст_оператора_важнее_стандартного_а_без_него_стандартный() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    english_on(&services, Locale::Ru).await;
    let admin = owner(&services).await;
    services
        .bot_content_ops()
        .update_content(
            &ContentUpdate {
                texts: BTreeMap::from([(
                    "menu.greeting".to_string(),
                    Some("Привет, {name}!".to_string()),
                )]),
                ..ContentUpdate::default()
            },
            admin,
        )
        .await
        .unwrap();
    handle_update(&ctx, english_text(1, 953, "/start")).await;
    let menu = last_text(&server).await;
    assert!(!menu.contains("Привет"), "{menu}");
    assert!(menu.contains("No subscription yet."), "{menu}");

    services
        .bot_content_ops()
        .update_content(
            &ContentUpdate {
                en: Some(sn_services::bot_content::TranslationUpdate {
                    texts: BTreeMap::from([(
                        "menu.greeting".to_string(),
                        Some("Hi, {name}!".to_string()),
                    )]),
                    buttons: BTreeMap::from([(
                        "menu.btn.buy".to_string(),
                        Some("Get VPN".to_string()),
                    )]),
                }),
                ..ContentUpdate::default()
            },
            admin,
        )
        .await
        .unwrap();
    handle_update(&ctx, english_text(2, 953, "/start")).await;
    assert!(last_text(&server).await.contains("Hi, Ann!"));
    assert!(last_labels(&server).await.contains(&"Get VPN".to_string()));
    let stored: ContentConfig = services.bot_content().content.config().clone();
    assert_eq!(stored.en.texts["menu.greeting"], "Hi, {name}!");
    assert_eq!(stored.texts["menu.greeting"], "Привет, {name}!");
    db.cleanup().await;
}

#[tokio::test]
async fn после_выключения_английского_клиент_снова_видит_русский() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    english_on(&services, Locale::En).await;
    handle_update(&ctx, english_text(1, 954, "/start")).await;
    assert!(last_text(&server).await.contains("No subscription yet."));
    let admin = owner(&services).await;
    services
        .settings_ops()
        .update_languages(&Languages::default(), admin)
        .await
        .unwrap();
    handle_update(&ctx, english_text(2, 954, "/start")).await;
    assert!(last_text(&server).await.contains("Подписки пока нет"));
    assert_eq!(client_of(&services, 954).await.unwrap().locale, Locale::En);
    db.cleanup().await;
}

#[test]
fn уведомления_и_их_параметры_по_английски() {
    use sn_domain::money::{Currency, Money};
    use sn_domain::traffic::GIB;
    let content = sn_services::bot_content::BotContent::default().in_locale(Locale::En);
    let offset = chrono::FixedOffset::east_opt(0).unwrap();
    let render = |template: &str, params: serde_json::Value| {
        sn_domain::lang::sync_scope(Locale::En, || {
            crate::notify::render(&content.content, template, &params, offset)
        })
        .unwrap()
        .text
    };
    assert_eq!(
        render(
            "payment_paid",
            serde_json::json!({ "amount": Money::new(1500, Currency::Usd) })
        ),
        "Payment of $15.00 received. The subscription will update within a minute."
    );
    let restricted = render(
        "traffic_restricted",
        serde_json::json!({
            "used": "52 ГБ", "limit": "50 ГБ", "used_bytes": 52 * GIB, "limit_bytes": 50 * GIB,
            "period": "day", "when": "Сегодня", "per": "в сутки",
            "until": "19.11 в 00:00", "until_at": "2026-11-19T00:00:00Z"
        }),
    );
    assert_eq!(
        restricted,
        "Today you've used 52\u{a0}GB with a limit of 50\u{a0}GB per day. Access is restricted until 19\u{a0}November, 00:00 and comes back on its own."
    );
    assert_eq!(
        render(
            "payment.credited",
            serde_json::json!({ "text": "Оплата пришла с излишком", "text_en": "You paid more than needed" })
        ),
        "You paid more than needed"
    );
    let gift = render(
        "gift.redeemed",
        serde_json::json!({ "code": "ABCD-EFGH-JKLM", "tariff": "Стандарт", "term": "1 месяц", "days": 30 }),
    );
    assert!(gift.contains("“Стандарт” for 1\u{a0}month"), "{gift}");
    let won = render(
        "contest_won",
        serde_json::json!({
            "title": "Зимний", "place": 1, "prize": "+30 дней к подписке",
            "prize_data": { "kind": "days", "days": 30 }, "note": "Дни уже добавлены к подписке."
        }),
    );
    assert!(
        won.contains("You took place 1 in the “Зимний” giveaway."),
        "{won}"
    );
    assert!(
        won.contains("Your prize: +30\u{a0}days of subscription."),
        "{won}"
    );
    assert!(
        won.contains("The days are already added to your subscription."),
        "{won}"
    );
    let russian = sn_services::bot_content::BotContent::default();
    let plain = crate::notify::render(
        &russian.content,
        "traffic_warning",
        &serde_json::json!({
            "used": "52\u{a0}ГБ", "limit": "50\u{a0}ГБ", "used_bytes": 52 * GIB, "limit_bytes": 50 * GIB,
            "period": "day", "when": "Сегодня", "per": "в сутки"
        }),
        offset,
    )
    .unwrap()
    .text;
    assert!(
        plain.starts_with("Сегодня через VPN прошло 52\u{a0}ГБ"),
        "{plain}"
    );
}

#[tokio::test]
async fn конкурс_в_боте_по_английски_с_условиями_и_призом() {
    use chrono::{Duration, Utc};
    use sn_domain::module::ModuleId;
    use sn_modules::catalog::contests::{self, ConditionsDraft, ContestDraft, PrizeConfigDraft};
    use sn_modules::catalog::prize::PrizeDraft;
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    english_on(&services, Locale::Ru).await;
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Contests, true, None)
        .await
        .unwrap();
    let created = contests::create(
        &services.module_context(),
        &ContestDraft {
            title: "Зимний розыгрыш".into(),
            description: None,
            prize: PrizeConfigDraft::Single(PrizeDraft::Days { days: 7 }),
            winners: 2,
            starts_at: None,
            ends_at: Utc::now() + Duration::days(3),
            conditions: ConditionsDraft {
                subscription: true,
                ..ConditionsDraft::default()
            },
            messages: None,
        },
        None,
    )
    .await
    .unwrap();
    handle_update(&ctx, english_text(1, 955, "/start")).await;
    assert!(
        last_labels(&server)
            .await
            .contains(&"Giveaways".to_string())
    );
    handle_update(
        &ctx,
        english_callback(2, 955, &format!("ct:{}", created.id)),
    )
    .await;
    let card = last_text(&server).await;
    assert!(card.contains("<b>Зимний розыгрыш</b>"), "{card}");
    assert!(
        card.contains("Prize: +7\u{a0}days of subscription. Winners: 2."),
        "{card}"
    );
    assert!(card.contains("An active subscription"), "{card}");
    assert!(card.contains("Results on "), "{card}");
    assert!(!card.contains("Итоги"), "{card}");
    assert!(last_labels(&server).await.contains(&"Join".to_string()));
    db.cleanup().await;
}
