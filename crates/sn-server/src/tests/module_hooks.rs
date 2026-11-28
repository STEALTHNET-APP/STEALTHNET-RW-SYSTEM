use super::module_packages::{
    Folder, build, clear_request, configure, helper_dir, page_get, ready, temp, upload,
};
use super::*;
use axum::extract::State;
use axum::response::IntoResponse;
use sn_domain::ids::{RemnaUserId, SubscriptionId};
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_remnawave::fake::sample_user;
use sn_services::ext_modules::bot::{ScreenAction, ScreenFailure, ScreenRequest};
use sn_store::NewSubscription;
use std::path::Path;
use std::sync::Mutex;

type Hit = (String, HeaderMap, Vec<u8>);

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Hit>>>);

impl Seen {
    fn last(&self) -> Hit {
        self.0.lock().unwrap().last().cloned().unwrap()
    }
}

async fn game_module(
    State(seen): State<Seen>,
    method: Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    seen.0
        .lock()
        .unwrap()
        .push((format!("{method} {uri}"), headers, body.to_vec()));
    match uri.path() {
        "/in/api/internal/game/fulfill" => (
            StatusCode::ACCEPTED,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::SET_COOKIE, "sn_admin=stolen; Path=/api"),
            ],
            r#"{"ok":true}"#,
        )
            .into_response(),
        "/bot/screen" => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json!({
                "text": "🎮 <b>Игра</b>\n\n👉 Ваши попытки: 5",
                "buttons": [[{ "text": "🕹️ Играть", "web_app": "https://game.example/?v=1" }]]
            })
            .to_string(),
        )
            .into_response(),
        _ => (StatusCode::OK, "раздел модуля").into_response(),
    }
}

async fn spawn_game() -> (u16, Seen) {
    let seen = Seen::default();
    let app = axum::Router::new()
        .fallback(game_module)
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, seen)
}

const RIGHTS: [&str; 3] = ["clients_read", "clients_write", "marketing"];

fn manifest(port: u16) -> Value {
    json!({
        "format": 1,
        "id": "game",
        "name": "Бонус-игра",
        "version": "1.0.0",
        "author": "STEALTHNET",
        "permissions": RIGHTS,
        "page": { "path": "/", "title": "Бонус-игра", "permission": "marketing" },
        "client_card": { "path": "/card", "title": "Бонус-игра" },
        "bot": { "button": "🎮 Игра", "path": "/bot/screen" },
        "run": {
            "port": port,
            "health_path": "/healthz",
            "inbound_path": "/in",
            "memory_mb": 128,
            "cpus": 0.5,
            "internet": true
        }
    })
}

struct Installed {
    _dir: Folder,
    _work: Folder,
    key: String,
}

fn issued_key(dir: &Path) -> String {
    std::fs::read_dir(dir.join("request"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "cred"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| text.lines().next().map(str::to_string))
        .unwrap()
}

async fn install_game(h: &Http, owner: &str, port: u16) -> Installed {
    let dir = helper_dir(Some(ready()));
    configure(h, Some(&dir.0), None, Some("127.0.0.1"));
    let work = temp("sn-modules-game");
    let bytes = build(&work.0, &manifest(port), b"image");
    let upload = upload(h, owner, &bytes).await;
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/ext-modules/packages/{upload}/install"),
            owner,
            json!({
                "password": PASSWORD,
                "permissions": RIGHTS,
                "events": [],
                "trust_unsigned": true
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let key = issued_key(&dir.0);
    clear_request(&dir.0);
    Installed {
        _dir: dir,
        _work: work,
        key,
    }
}

async fn inbound(
    h: &Http,
    method: Method,
    path: &str,
    body: &str,
) -> (StatusCode, HeaderMap, String) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-timestamp", "1764320000")
        .header("x-signature", "abc123")
        .header(header::COOKIE, "sn_admin=секрет")
        .header("x-forwarded-for", "6.6.6.6")
        .header("x-stealthnet-admin", "forged")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn входящий_адрес_пересылает_запрос_модулю_без_кук_и_чужих_заголовков() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (port, seen) = spawn_game().await;
    let _installed = install_game(&h, &owner, port).await;
    let (status, card) = h.get("/api/admin/ext-modules/game", &owner).await;
    assert_eq!(status, StatusCode::OK, "{card}");
    assert_eq!(card["inbound"], "/api/ext/game/");
    assert_eq!(card["bot"]["button"], "🎮 Игра");
    assert_eq!(card["client_card"]["title"], "Бонус-игра");
    let (status, headers, body) = inbound(
        &h,
        Method::POST,
        "/api/ext/game/api/internal/game/fulfill?x=1",
        r#"{"userId":5093118422,"winId":"w1"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body, r#"{"ok":true}"#);
    assert!(headers.get(header::SET_COOKIE).is_none());
    let (line, sent, received) = seen.last();
    assert_eq!(line, "POST /in/api/internal/game/fulfill?x=1");
    assert_eq!(received, br#"{"userId":5093118422,"winId":"w1"}"#);
    assert_eq!(sent.get("x-timestamp").unwrap(), "1764320000");
    assert_eq!(sent.get("x-signature").unwrap(), "abc123");
    assert_eq!(sent.get("x-forwarded-prefix").unwrap(), "/api/ext/game");
    assert!(sent.get(header::COOKIE).is_none(), "{sent:?}");
    assert!(sent.get("x-stealthnet-admin").is_none(), "{sent:?}");
    assert!(sent.get("x-forwarded-for").is_none(), "{sent:?}");
    let (status, _, _) = inbound(&h, Method::GET, "/api/ext/game", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen.last().0, "GET /in");
    let calls = seen.0.lock().unwrap().len();
    inbound(&h, Method::OPTIONS, "/api/ext/game/x", "").await;
    inbound(&h, Method::TRACE, "/api/ext/game/x", "").await;
    assert_eq!(seen.0.lock().unwrap().len(), calls);
    let (status, _, _) = inbound(&h, Method::POST, "/api/ext/other/x", "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let calls = seen.0.lock().unwrap().len();
    let (status, _, _) = inbound(&h, Method::GET, "/api/ext/game/../healthz", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(seen.0.lock().unwrap().len(), calls);
    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/ext-modules/game",
            &owner,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, _) = inbound(&h, Method::POST, "/api/ext/game/x", "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}

#[tokio::test]
async fn раздел_модуля_в_карточке_клиента_открывается_только_по_своему_пути() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (port, seen) = spawn_game().await;
    let _installed = install_game(&h, &owner, port).await;
    let client = h.client(5_093_118_422).await;
    let (status, frames) = h
        .get(
            &format!("/api/admin/clients/{}/module-cards", client.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{frames}");
    let frames = frames.as_array().unwrap().clone();
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0]["module"], "game");
    assert_eq!(frames[0]["title"], "Бонус-игра");
    let url = frames[0]["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/m/game/"), "{url}");
    assert!(
        url.contains(&format!("/card?client_id={}&sn_token=", client.0)),
        "{url}"
    );
    let (status, _, body) = page_get(&h, Method::GET, &url, Some("sn_admin=секрет")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, "раздел модуля");
    let (line, sent, _) = seen.last();
    assert!(
        line.starts_with(&format!("GET /card?client_id={}&sn_token=", client.0)),
        "{line}"
    );
    assert!(sent.get("x-stealthnet-admin").is_some());
    assert!(sent.get(header::COOKIE).is_none());
    let prefix = url.split("/card").next().unwrap().to_string();
    let (status, _, _) = page_get(&h, Method::GET, &format!("{prefix}/card/app.js"), None).await;
    assert_eq!(status, StatusCode::OK);
    for outside in ["/", "/cardx", "/api/state", "/card/../api"] {
        let (status, _, _) = page_get(&h, Method::GET, &format!("{prefix}{outside}"), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{outside}");
    }
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, frames) = h
        .get(
            &format!("/api/admin/clients/{}/module-cards", client.0),
            &support,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{frames}");
    assert_eq!(frames.as_array().unwrap().len(), 1);
    let (status, menu) = h.get("/api/admin/ext-modules/game", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{menu}");
    let (status, _) = h
        .get("/api/admin/clients/999999/module-cards", &owner)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}

async fn live_subscription(h: &Http, client: ClientId, remna: i64) -> SubscriptionId {
    let tariff = h.tariff().await;
    let mut user = sample_user(remna, &format!("sn_{remna}"));
    user.expire_at = Utc::now() + Duration::days(365);
    user.hwid_device_limit = Some(3);
    h.remna.insert_user(user.clone());
    let mut conn = h.services().store().conn().await.unwrap();
    let subscription = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: Some(tariff.id),
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    conn.subscriptions()
        .link_remna(
            subscription.id,
            &RemnaLink {
                user_id: RemnaUserId(remna),
                username: user.username,
                short_uuid: Some(user.short_uuid),
            },
        )
        .await
        .unwrap();
    conn.subscriptions()
        .apply_mirror(
            subscription.id,
            &RemnaMirror {
                status: Some(SubscriptionStatus::Active),
                expire_at: Some(start() + Duration::days(20)),
                hwid_limit: Some(3),
                ..RemnaMirror::default()
            },
        )
        .await
        .unwrap();
    subscription.id
}

async fn reward(h: &Http, key: &str, body: Value) -> (StatusCode, Value) {
    h.write(Method::POST, "/api/admin/ext-modules/me/rewards", key, body)
        .await
}

#[tokio::test]
async fn модуль_выдаёт_дни_устройства_и_личный_промокод_один_раз() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (port, _) = spawn_game().await;
    let installed = install_game(&h, &owner, port).await;
    let key = installed.key.clone();
    let client = h.client(5_093_118_422).await;
    let subscription = live_subscription(&h, client, 71).await;
    let days = json!({
        "reference": "win:a1",
        "client_id": client.0,
        "reward": { "kind": "days", "days": 3 },
        "label": "Приз в игре"
    });
    let before = h.remna.user(RemnaUserId(71)).unwrap().expire_at;
    let (status, first) = reward(&h, &key, days.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["status"], "applied");
    assert_eq!(first["subscription_id"], subscription.0);
    let expire = h.remna.user(RemnaUserId(71)).unwrap().expire_at;
    assert_eq!(expire, before + Duration::days(3));
    let (status, again) = reward(&h, &key, days.clone()).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["status"], "duplicate");
    assert_eq!(again["expire_at"], first["expire_at"]);
    assert_eq!(h.remna.user(RemnaUserId(71)).unwrap().expire_at, expire);
    let mut other = days.clone();
    other["reward"] = json!({ "kind": "days", "days": 5 });
    let (status, body) = reward(&h, &key, other).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "reward.mismatch");
    let (status, devices) = reward(
        &h,
        &key,
        json!({
            "reference": "win:a2",
            "client_id": client.0,
            "subscription_id": subscription.0,
            "reward": { "kind": "devices", "devices": 1 }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{devices}");
    assert_eq!(devices["device_limit"], 4, "{devices}");
    assert_eq!(
        h.remna.user(RemnaUserId(71)).unwrap().hwid_device_limit,
        Some(4)
    );
    let (status, promo) = reward(
        &h,
        &key,
        json!({
            "reference": "win:a3",
            "client_id": client.0,
            "reward": { "kind": "promo", "percent": 5, "valid_days": 30 },
            "label": "Приз в игре"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{promo}");
    let code = promo["code"].as_str().unwrap().to_string();
    assert!(code.starts_with("WIN-"), "{code}");
    let owned: Option<i64> =
        sqlx::query_scalar("SELECT owner_client_id FROM promo_codes WHERE code = $1")
            .bind(&code)
            .fetch_one(h.services().store().conn().await.unwrap().conn())
            .await
            .unwrap();
    assert_eq!(owned, Some(client.0));
    let (status, state) = h
        .get("/api/admin/ext-modules/me/rewards/win:a3", &key)
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["state"], "applied");
    assert_eq!(state["granted"]["code"], code.as_str());
    let (status, history) = h
        .get(
            &format!("/api/admin/clients/{}/history?limit=50", client.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let items = history["items"].as_array().unwrap();
    let by_module: Vec<&str> = items
        .iter()
        .filter(|item| item["module"] == "Бонус-игра")
        .filter_map(|item| item["action"].as_str())
        .collect();
    assert!(by_module.contains(&"subscription.add_days"), "{history}");
    assert!(
        by_module.contains(&"subscription.change_limits"),
        "{history}"
    );
    assert!(by_module.contains(&"ext_module.reward"), "{history}");
    let added = items
        .iter()
        .find(|item| item["action"] == "subscription.add_days")
        .unwrap();
    assert_eq!(added["details"]["label"], "Приз в игре");
    assert_eq!(added["details"]["reference"], "win:a1");
    let lonely = h.client(5_093_118_423).await;
    let (status, body) = reward(
        &h,
        &key,
        json!({
            "reference": "win:b1",
            "client_id": lonely.0,
            "reward": { "kind": "days", "days": 1 }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "reward.no_subscription");
    let (status, state) = h
        .get("/api/admin/ext-modules/me/rewards/win:b1", &key)
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["state"], "failed");
    let (status, body) = reward(
        &h,
        &key,
        json!({
            "reference": "win:c1",
            "client_id": client.0,
            "subscription_id": subscription.0 + 1000,
            "reward": { "kind": "days", "days": 1 }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "reward.subscription");
    for (bad, code) in [
        (json!({ "kind": "days", "days": 0 }), "reward.invalid"),
        (
            json!({ "kind": "devices", "devices": 11 }),
            "reward.invalid",
        ),
        (
            json!({ "kind": "promo", "percent": 101, "valid_days": 3 }),
            "reward.invalid",
        ),
    ] {
        let (status, body) = reward(
            &h,
            &key,
            json!({ "reference": "win:bad", "client_id": client.0, "reward": bad }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], code);
    }
    let (status, body) = reward(
        &h,
        &key,
        json!({ "reference": "bad ref", "client_id": client.0, "reward": { "kind": "days", "days": 1 } }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "reward.reference");
    let (status, body) = reward(&h, &owner, days).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "ext_modules.not_module");
    h.db.cleanup().await;
}

#[tokio::test]
async fn экран_бота_от_модуля_подписан_и_проверен() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (port, seen) = spawn_game().await;
    let installed = install_game(&h, &owner, port).await;
    let client = h.client(5_093_118_422).await;
    let buttons = h.services().ext_modules().bot_buttons().await.unwrap();
    assert_eq!(buttons.len(), 1);
    assert_eq!(buttons[0].module, "game");
    assert_eq!(buttons[0].button, "🎮 Игра");
    let screen = h
        .services()
        .ext_modules()
        .bot_screen(
            "game",
            ScreenRequest {
                client_id: client,
                telegram_id: 5_093_118_422,
                locale: sn_domain::client::Locale::Ru,
                first_name: "Алексей",
            },
        )
        .await
        .unwrap();
    assert_eq!(screen.html, "🎮 <b>Игра</b>\n\n👉 Ваши попытки: 5");
    assert_eq!(screen.rows.len(), 1);
    assert_eq!(screen.rows[0][0].text, "🕹️ Играть");
    assert_eq!(
        screen.rows[0][0].action,
        ScreenAction::WebApp("https://game.example/?v=1".into())
    );
    let (line, sent, body) = seen.last();
    assert_eq!(line, "POST /bot/screen");
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["type"], "bot_screen");
    assert_eq!(body["data"]["telegram_id"], 5_093_118_422_i64);
    assert_eq!(body["data"]["client_id"], client.0);
    let id = sent
        .get("webhook-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let stamp: i64 = sent
        .get("webhook-timestamp")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let secret = {
        let mut conn = h.services().store().conn().await.unwrap();
        sn_services::settings::read_secret(
            &mut conn,
            h.services().secrets(),
            &sn_services::ext_modules::secret_key("game"),
        )
        .await
        .unwrap()
        .unwrap()
    };
    let raw = serde_json::to_vec(&body).unwrap();
    let expected = sn_services::ext_modules::signing::signature(
        &sn_services::ext_modules::signing::key(&secret).unwrap(),
        &id,
        stamp,
        &seen.last().2,
    )
    .unwrap();
    assert_eq!(sent.get("webhook-signature").unwrap(), expected.as_str());
    assert!(!raw.is_empty());
    let missing = h
        .services()
        .ext_modules()
        .bot_screen(
            "nothing",
            ScreenRequest {
                client_id: client,
                telegram_id: 1,
                locale: sn_domain::client::Locale::Ru,
                first_name: "",
            },
        )
        .await;
    assert_eq!(missing, Err(ScreenFailure::Missing));
    drop(installed);
    h.db.cleanup().await;
}
