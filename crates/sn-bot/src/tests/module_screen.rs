use super::*;
use sn_services::ext_modules::{secret_key, signing};
use sn_services::settings::write_secret;
use sn_services::updates::{BuildInfo, UpdateConfig};
use wiremock::matchers::path;

async fn install_module(services: &Services, port: u16) {
    let owner = services
        .auth()
        .upsert_owner("owner@example.com", "пароль-владельца-модуля")
        .await
        .unwrap()
        .0;
    let manifest = serde_json::json!({
        "format": 1,
        "id": "game",
        "name": "Бонус-игра",
        "version": "1.0.0",
        "author": "STEALTHNET",
        "description": null,
        "site": null,
        "webhook": null,
        "events": [],
        "permissions": ["clients_read"],
        "page": null,
        "bot": { "button": "🎮 Игра", "path": "/bot/screen" },
        "settings": [],
        "run": {
            "port": port,
            "health_path": "/healthz",
            "memory_mb": 128,
            "cpu_tenths": 5,
            "internet": true,
            "volumes": [],
            "images": []
        }
    });
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "INSERT INTO ext_modules (id, manifest, events, installed_by, updated_by, managed, package, job) \
         VALUES ('game', $1, '{}', $2, $2, true, 'null', 'null')",
    )
    .bind(&manifest)
    .bind(owner.0)
    .execute(conn.conn())
    .await
    .unwrap();
    write_secret(
        &mut conn,
        services.secrets(),
        &secret_key("game"),
        Some(&signing::new_secret()),
        None,
    )
    .await
    .unwrap();
    let mut config = UpdateConfig::new(BuildInfo::new(None, None));
    config.modules_host = Some("127.0.0.1".into());
    services.set_update_config(Some(config));
}

fn module_port(server: &MockServer) -> u16 {
    server.address().port()
}

fn web_apps(body: &serde_json::Value) -> Vec<String> {
    body["reply_markup"]["inline_keyboard"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["web_app"]["url"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn кнопка_модуля_в_меню_открывает_его_экран() {
    let Some((_db, services, server, ctx)) = setup().await else {
        return;
    };
    let module = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot/screen"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "🎮 <b>Игра</b>\n\n👉 Ваши попытки: 5",
            "buttons": [[{ "text": "🕹️ Играть", "web_app": "https://game.example/?v=1" }]]
        })))
        .mount(&module)
        .await;
    install_module(&services, module_port(&module)).await;
    handle_update(&ctx, text_update(1, 7101, "/start")).await;
    assert!(
        last_buttons(&server).await.contains(&"xm:game".to_string()),
        "{:?}",
        last_buttons(&server).await
    );
    assert!(last_labels(&server).await.contains(&"🎮 Игра".to_string()));
    handle_update(&ctx, callback_update(2, 7101, "xm:game")).await;
    let shown = sent(&server).await.last().cloned().unwrap();
    assert_eq!(shown["text"], "🎮 <b>Игра</b>\n\n👉 Ваши попытки: 5");
    assert_eq!(
        web_apps(&shown),
        vec!["https://game.example/?v=1".to_string()]
    );
    assert!(last_buttons(&server).await.contains(&"m".to_string()));
    let requests = module.received_requests().await.unwrap();
    let call = requests.last().unwrap();
    assert!(call.headers.get("webhook-signature").is_some());
    let body: serde_json::Value = serde_json::from_slice(&call.body).unwrap();
    assert_eq!(body["type"], "bot_screen");
    assert_eq!(body["data"]["telegram_id"], 7101);
    assert_eq!(body["data"]["locale"], "ru");
}

#[tokio::test]
async fn недоступный_модуль_показывает_вежливый_экран() {
    let Some((_db, services, server, ctx)) = setup().await else {
        return;
    };
    let module = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot/screen"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&module)
        .await;
    install_module(&services, module_port(&module)).await;
    handle_update(&ctx, text_update(1, 7102, "/start")).await;
    handle_update(&ctx, callback_update(2, 7102, "xm:game")).await;
    assert_eq!(
        last_text(&server).await,
        "Раздел сейчас недоступен. Попробуйте через минуту."
    );
    assert!(last_buttons(&server).await.contains(&"m".to_string()));
    handle_update(&ctx, callback_update(3, 7102, "xm:nothing")).await;
    assert!(
        last_buttons(&server).await.contains(&"xm:game".to_string()),
        "незнакомый модуль ведёт в главное меню"
    );
}
