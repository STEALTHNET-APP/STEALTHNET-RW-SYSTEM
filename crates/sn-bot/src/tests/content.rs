use super::*;
use crate::handler::{emoji_codes, emoji_line};
use crate::telegram::Message;
use sn_services::bot_content::{
    CONSENT_KEY, CONTENT_KEY, DOCUMENTS_KEY, EMOJI_REJECTED_KEY, IMAGES_KEY,
};
use sn_store::BotImage;
use wiremock::matchers::body_string_contains;

const JPEG: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01,
];

async fn put_setting(services: &Services, key: &str, value: serde_json::Value) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings().set(key, &value, None).await.unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

async fn put_image(services: &Services, slot: &str) {
    let mut conn = services.store().conn().await.unwrap();
    conn.bot_images()
        .put(
            slot,
            &BotImage {
                content_type: "image/jpeg".into(),
                bytes: JPEG.to_vec(),
                sha256: format!("{slot}0123456789abcdef0123456789abcdef"),
            },
            None,
        )
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

fn photo_message(chat: i64) -> serde_json::Value {
    serde_json::json!({
        "message_id": 200, "chat": {"id": chat, "type": "private"}, "date": 0,
        "photo": [
            {"file_id": "small-id", "file_unique_id": "u1", "width": 90, "height": 90},
            {"file_id": "big-id", "file_unique_id": "u2", "width": 800, "height": 800}
        ]
    })
}

async fn mount_media(server: &MockServer, chat: i64) {
    Mock::given(method("POST"))
        .and(path_regex(r"/sendPhoto$"))
        .respond_with(ok(photo_message(chat)))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/editMessageMedia$"))
        .respond_with(ok(photo_message(chat)))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/deleteMessage$"))
        .respond_with(ok(serde_json::json!(true)))
        .mount(server)
        .await;
}

async fn requests(server: &MockServer, name: &str) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with(&format!("/{name}")))
        .collect()
}

fn photo_callback(update_id: i64, from: i64, data: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "callback_query": {
            "id": format!("cb{update_id}"), "from": user(from), "data": data,
            "message": {
                "message_id": 77, "chat": {"id": from, "type": "private"}, "date": 1,
                "photo": [{"file_id": "big-id", "file_unique_id": "u2", "width": 800, "height": 800}]
            }
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn свои_тексты_кнопки_и_две_колонки_в_главном_меню() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    put_setting(
        &services,
        CONTENT_KEY,
        serde_json::json!({
            "texts": {
                "menu.greeting": "Привет, {name}! 👋",
                "menu.no_subscription": ""
            },
            "buttons": {
                "menu.btn.buy": {"text": "🛒 Купить VPN", "emoji_id": "5368324170671202286", "style": "success"},
                "menu.btn.balance": {"hidden": true}
            },
            "menu_columns": 2
        }),
    )
    .await;
    handle_update(&ctx, text_update(1, 600, "/start")).await;
    let body = sent(&server).await.last().cloned().unwrap();
    let text = body["text"].as_str().unwrap();
    assert!(text.contains("Привет, Алексей! 👋"), "{text}");
    assert!(!text.contains("Подписки пока нет"), "{text}");
    let rows = body["reply_markup"]["inline_keyboard"].as_array().unwrap();
    assert!(rows.iter().any(|row| row.as_array().unwrap().len() == 2));
    let buttons: Vec<&serde_json::Value> = rows
        .iter()
        .flat_map(|row| row.as_array().unwrap())
        .collect();
    let buy = buttons
        .iter()
        .find(|b| b["callback_data"] == "buy")
        .unwrap();
    assert_eq!(buy["text"], "🛒 Купить VPN");
    assert_eq!(buy["icon_custom_emoji_id"], "5368324170671202286");
    assert_eq!(buy["style"], "success");
    assert!(buttons.iter().all(|b| b["callback_data"] != "bal"));
    assert!(buttons.iter().any(|b| b["callback_data"] == "sup"));
    db.cleanup().await;
}

#[tokio::test]
async fn картинка_уходит_файлом_один_раз_дальше_по_коду() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    mount_media(&server, 601).await;
    put_image(&services, "common").await;
    handle_update(&ctx, text_update(1, 601, "/start")).await;
    let first = requests(&server, "sendPhoto").await;
    assert_eq!(first.len(), 1);
    let kind = first[0]
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(kind.starts_with("multipart/form-data"), "{kind}");
    let body = String::from_utf8_lossy(&first[0].body).to_string();
    assert!(body.contains("name=\"photo\""));
    assert!(body.contains("name=\"caption\""));
    handle_update(&ctx, text_update(2, 601, "/start")).await;
    let second = requests(&server, "sendPhoto").await;
    assert_eq!(second.len(), 2);
    let json: serde_json::Value = serde_json::from_slice(&second[1].body).unwrap();
    assert_eq!(json["photo"], "big-id");
    assert_eq!(json["parse_mode"], "HTML");
    handle_update(&ctx, photo_callback(3, 601, "bal")).await;
    let edits = requests(&server, "editMessageMedia").await;
    assert_eq!(edits.len(), 1);
    let edit: serde_json::Value = serde_json::from_slice(&edits[0].body).unwrap();
    assert_eq!(edit["message_id"], 77);
    assert_eq!(edit["media"]["type"], "photo");
    assert_eq!(edit["media"]["media"], "big-id");
    assert!(
        edit["media"]["caption"]
            .as_str()
            .unwrap()
            .contains("На балансе")
    );
    assert!(requests(&server, "sendMessage").await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn с_картинки_на_экран_без_неё_новое_сообщение_и_удаление_старого() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    mount_media(&server, 602).await;
    put_image(&services, "common").await;
    put_setting(
        &services,
        IMAGES_KEY,
        serde_json::json!({ "sections": { "balance": "off" } }),
    )
    .await;
    handle_update(&ctx, text_update(1, 602, "/start")).await;
    handle_update(&ctx, photo_callback(2, 602, "bal")).await;
    assert!(requests(&server, "editMessageMedia").await.is_empty());
    assert!(last_text(&server).await.contains("На балансе"));
    let deleted = requests(&server, "deleteMessage").await;
    assert_eq!(deleted.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&deleted[0].body).unwrap();
    assert_eq!(body["message_id"], 77);
    db.cleanup().await;
}

#[tokio::test]
async fn премиум_эмодзи_убираются_если_telegram_их_не_принял() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_string_contains("tg-emoji"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: custom emoji are not allowed"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    put_setting(
        &services,
        CONTENT_KEY,
        serde_json::json!({
            "texts": {"menu.title": "<tg-emoji emoji-id=\"5368324170671202286\">🔥</tg-emoji> <b>{brand}</b>"}
        }),
    )
    .await;
    handle_update(&ctx, text_update(1, 603, "/start")).await;
    let all = sent(&server).await;
    let last = all.last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(last.starts_with("🔥 <b>"), "{last}");
    assert!(!last.contains("tg-emoji"));
    let mut conn = db.store.conn().await.unwrap();
    let flag = conn
        .bot_state()
        .get(EMOJI_REJECTED_KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(flag.get("at").is_some());
    drop(conn);
    let view = services.bot_content_ops().content().await.unwrap();
    assert!(view.emoji_rejected_at.is_some());
    db.cleanup().await;
}

#[test]
fn команда_emoji_отвечает_кодами_для_текстов_и_кнопок() {
    let message: Message = serde_json::from_value(serde_json::json!({
        "message_id": 5, "chat": {"id": 7, "type": "private"}, "date": 1,
        "text": "/emoji 🔥🚀🔥",
        "entities": [
            {"type": "bot_command", "offset": 0, "length": 6},
            {"type": "custom_emoji", "offset": 7, "length": 2, "custom_emoji_id": "5368324170671202286"},
            {"type": "custom_emoji", "offset": 9, "length": 2, "custom_emoji_id": "5377498341074542641"},
            {"type": "custom_emoji", "offset": 11, "length": 2, "custom_emoji_id": "5368324170671202286"}
        ]
    }))
    .unwrap();
    let screen = emoji_codes(&message);
    assert!(screen.menu);
    assert!(
        screen
            .text
            .contains("🔥 — <code>5368324170671202286</code>"),
        "{}",
        screen.text
    );
    assert!(
        screen
            .text
            .contains("🚀 — <code>5377498341074542641</code>")
    );
    assert_eq!(screen.text.matches("5368324170671202286").count(), 3);
    assert!(
        screen.text.contains(
            "&lt;tg-emoji emoji-id=&quot;5368324170671202286&quot;&gt;🔥&lt;/tg-emoji&gt;"
        )
    );
    let empty: Message = serde_json::from_value(serde_json::json!({
        "message_id": 6, "chat": {"id": 7, "type": "private"}, "date": 1, "text": "/emoji"
    }))
    .unwrap();
    assert!(emoji_codes(&empty).text.contains("Пришлите /emoji"));
}

#[test]
fn команда_emoji_собирает_фразу_одной_строкой_для_вставки() {
    let message: Message = serde_json::from_value(serde_json::json!({
        "message_id": 5, "chat": {"id": 7, "type": "private"}, "date": 1,
        "text": "/emoji@demo_bot Скидка 🔥 до <пятницы> & 😎",
        "entities": [
            {"type": "bot_command", "offset": 0, "length": 15},
            {"type": "custom_emoji", "offset": 23, "length": 2, "custom_emoji_id": "111"},
            {"type": "custom_emoji", "offset": 41, "length": 2, "custom_emoji_id": "222"}
        ]
    }))
    .unwrap();
    assert_eq!(
        emoji_line(&message),
        "Скидка <tg-emoji emoji-id=\"111\">🔥</tg-emoji> до &lt;пятницы&gt; &amp; <tg-emoji emoji-id=\"222\">😎</tg-emoji>"
    );
    let screen = emoji_codes(&message);
    assert!(
        screen.text.contains("🔥 — <code>111</code>"),
        "{}",
        screen.text
    );
    assert!(screen.text.contains("😎 — <code>222</code>"));
    assert!(screen.text.contains(
        "&lt;tg-emoji emoji-id=&quot;111&quot;&gt;🔥&lt;/tg-emoji&gt; до &amp;lt;пятницы&amp;gt;"
    ));
}

#[tokio::test]
async fn документы_в_меню_и_согласие_только_если_включено() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    put_setting(
        &services,
        DOCUMENTS_KEY,
        serde_json::json!([
            {"title": "Оферта", "url": "https://example.com/offer"},
            {"title": "Политика", "url": "https://example.com/privacy?a=1&b=2"}
        ]),
    )
    .await;
    put_setting(&services, CONSENT_KEY, serde_json::json!(false)).await;
    handle_update(&ctx, text_update(1, 604, "/start")).await;
    assert!(client_of(&services, 604).await.is_some());
    assert!(last_buttons(&server).await.contains(&"docs".to_string()));
    handle_update(&ctx, callback_update(2, 604, "docs")).await;
    let text = last_text(&server).await;
    assert!(text.contains("<b>Документы</b>"), "{text}");
    assert!(text.contains("<a href=\"https://example.com/offer\">Оферта</a>"));
    assert!(text.contains("privacy?a=1&amp;b=2"));
    put_setting(&services, CONSENT_KEY, serde_json::json!(true)).await;
    handle_update(&ctx, text_update(3, 605, "/start")).await;
    assert!(client_of(&services, 605).await.is_none());
    assert_eq!(last_buttons(&server).await, vec!["terms:ok".to_string()]);
    db.cleanup().await;
}
