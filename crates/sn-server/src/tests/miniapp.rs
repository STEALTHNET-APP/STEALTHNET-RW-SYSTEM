use super::*;

const ADMIN_PATH: &str = "/api/admin/settings/miniapp";
const SITE_PATH: &str = "/api/public/site";

async fn upload(h: &Http, token: &str, content_type: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/settings/miniapp/images")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn site(h: &Http) -> Value {
    let (status, _, body) = h.send(Method::GET, SITE_PATH, None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn keys_of(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

fn kinds(blocks: &Value) -> Vec<&str> {
    blocks
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["kind"].as_str().unwrap())
        .collect()
}

fn tabs(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|tab| tab["std"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn мини_приложение_правится_в_настройках_и_сразу_отдаётся_клиенту() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;

    let before = site(&h).await;
    assert_eq!(before["miniapp"], Value::Null);

    for (email, role) in [
        ("support@example.com", AdminRole::Support),
        ("viewer@example.com", AdminRole::ReadOnly),
    ] {
        let token = h.admin(email, role).await;
        let (status, _) = h.get(ADMIN_PATH, &token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{email}");
        let (status, _) = h.write(Method::PUT, ADMIN_PATH, &token, json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{email}");
        let (status, _) = upload(&h, &token, "image/png", logo_png(800, 600, 1)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{email}");
    }
    let (status, _, _) = h.send(Method::GET, ADMIN_PATH, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(
        kinds(&view["home"]),
        ["banners", "greeting", "subscription", "actions", "sections"]
    );
    assert_eq!(
        kinds(&view["profile"]),
        [
            "person",
            "balance",
            "friends",
            "help",
            "links",
            "documents",
            "account"
        ]
    );
    assert_eq!(tabs(&view["tabs"]), ["home", "plans", "connect", "profile"]);
    assert_eq!(view["limits"]["banners"], 5);
    assert_eq!(view["limits"]["tab"], 12);
    assert_eq!(view["image"]["max_bytes"], 1024 * 1024);
    assert_eq!(view["english"], false);
    assert!(view["off"].as_str().unwrap().contains("https"), "{view}");
    assert_eq!(view["friends_percent"], 30);
    assert_eq!(view["friends_levels"], serde_json::json!([30, 10, 10]));
    let gifts = view["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["key"] == "gifts")
        .unwrap()
        .clone();
    assert_eq!(gifts["title"], "Подарить подписку");

    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({
                "header": { "balance": false },
                "home": [
                    { "kind": "greeting", "on": true, "title": { "ru": "Привет, {name}!", "en": "Hi, {name}!" }, "for": ["new", "trial"] },
                    { "kind": "banners", "items": [
                        { "title": { "ru": "Новости сервиса" }, "to": "t.me/quiet_vpn" }
                    ] },
                    { "kind": "sections", "items": [
                        { "std": "support", "label": { "ru": "Написать нам" } },
                        { "std": "guide", "on": false },
                        { "label": { "ru": "Статус" }, "to": "https://status.example.com", "icon": "zap" }
                    ] }
                ],
                "tabs": [
                    { "std": "home" },
                    { "std": "plans", "label": { "ru": "Купить", "en": "Buy" } },
                    { "std": "support" },
                    { "std": "profile" }
                ],
                "profile": [{ "kind": "friends", "on": false }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        kinds(&saved["home"]),
        ["greeting", "subscription", "actions", "banners", "sections"]
    );
    assert_eq!(saved["home"][3]["items"][0]["to"], "https://t.me/quiet_vpn");
    let shown = site(&h).await["miniapp"].clone();
    assert_eq!(keys_of(&shown), ["header", "home", "profile", "tabs"]);
    for part in ["header", "home", "tabs", "profile"] {
        assert_eq!(shown[part], saved[part], "{part}");
    }
    assert_eq!(shown["header"], json!({ "balance": false }));
    let after = site(&h).await;
    for key in keys_of(&before) {
        if key != "miniapp" {
            assert_eq!(after[&key], before[&key], "{key}");
        }
    }

    let (status, journal) = h.get("/api/admin/audit?limit=5", &owner).await;
    assert_eq!(status, StatusCode::OK, "{journal}");
    let text = journal.to_string();
    assert!(text.contains("Изменено мини-приложение"), "{text}");
    assert!(!text.contains("Новости сервиса"), "{text}");

    let (status, reset) = h.write(Method::PUT, ADMIN_PATH, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["home"], view["home"]);
    assert_eq!(site(&h).await, before);
    h.db.cleanup().await;
}

#[tokio::test]
async fn журнал_показывает_правку_мини_приложения_по_русски_и_без_текстов() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({
                "home": [{ "kind": "actions", "on": false }],
                "tabs": [{ "std": "home" }, { "std": "plans" }, { "std": "profile" }],
                "profile": [{ "kind": "links", "items": [
                    { "label": { "ru": "Секретный текст оператора" }, "to": "https://example.com" }
                ] }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, journal) = h.get("/api/admin/audit?limit=5", &owner).await;
    assert_eq!(status, StatusCode::OK, "{journal}");
    let id = journal["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["action"] == "settings.miniapp")
        .unwrap()["id"]
        .clone();
    let (status, entry) = h.get(&format!("/api/admin/audit/{id}"), &owner).await;
    assert_eq!(status, StatusCode::OK, "{entry}");
    let text = entry.to_string();
    for part in [
        "Изменено мини-приложение",
        "Главная, Меню внизу, Профиль",
        "Кнопки под подпиской, Свои ссылки",
        "/admin/settings/miniapp",
    ] {
        assert!(text.contains(part), "{part}: {text}");
    }
    assert!(!text.contains("Секретный"), "{text}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn опасные_ссылки_и_пропажа_обязательного_не_сохраняются() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    for (body, message) in [
        (
            json!({ "home": [{ "kind": "banners", "items": [
                { "title": { "ru": "Скидка" }, "to": "javascript:alert(document.cookie)" }
            ] }] }),
            "Главная, баннер 1, ссылка: нужна ссылка вида https://… или t.me/…",
        ),
        (
            json!({ "home": [{ "kind": "sections", "items": [
                { "label": { "ru": "Статус" }, "to": "http://status.example.com" }
            ] }] }),
            "Главная, разделы, пункт 1, ссылка: нужна ссылка вида https://… или t.me/…",
        ),
        (
            json!({ "profile": [{ "kind": "links", "items": [
                { "label": { "ru": "Сайт" }, "to": "https://example.com/\" onclick=\"x" }
            ] }] }),
            "Профиль, свои ссылки, ссылка 1, ссылка: в ссылке не должно быть пробелов, кавычек и скобок",
        ),
        (
            json!({ "tabs": [{ "std": "home" }, { "std": "support" }, { "std": "profile" }] }),
            "Меню внизу: вкладку «Тарифы» убрать нельзя: без неё клиент не оплатит подписку",
        ),
        (
            json!({ "tabs": [{ "std": "home" }, { "std": "plans" }, { "std": "support" }] }),
            "Меню внизу: вкладку «Профиль» убрать нельзя: без неё клиент не найдёт поддержку и свой аккаунт",
        ),
        (
            json!({ "home": [{ "kind": "html" }] }),
            "Главная, блок 1: неизвестный блок «html»",
        ),
        (json!([]), "Настройки заполнены неверно"),
    ] {
        let (status, error) = h.write(Method::PUT, ADMIN_PATH, &owner, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
        assert_eq!(error["error"]["code"], "miniapp.invalid");
        assert_eq!(error["error"]["message"], message);
    }
    assert_eq!(site(&h).await["miniapp"], Value::Null);
    let (status, kept) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({
                "home": [{ "kind": "subscription", "on": false }],
                "profile": [{ "kind": "help", "on": false }, { "kind": "account", "on": false }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{kept}");
    assert_eq!(kept["home"][2], json!({ "kind": "subscription" }));
    assert_eq!(kept["profile"][0], json!({ "kind": "person" }));
    assert_eq!(kept["profile"][2], json!({ "kind": "friends" }));
    assert_eq!(kept["profile"][3], json!({ "kind": "help" }));
    h.db.cleanup().await;
}

#[tokio::test]
async fn картинки_баннеров_загружаются_и_отдаются_с_вечным_кешем() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    for (content_type, bytes, status, code) in [
        (
            "text/plain",
            logo_png(800, 600, 1),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "miniapp_image.format",
        ),
        (
            "image/svg+xml",
            b"<svg onload=\"alert(1)\"/>".to_vec(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "miniapp_image.format",
        ),
        (
            "image/png",
            b"<html><script>alert(1)</script></html>".to_vec(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "miniapp_image.format",
        ),
        (
            "image/png",
            logo_png(50, 50, 1),
            StatusCode::UNPROCESSABLE_ENTITY,
            "miniapp_image.too_small",
        ),
    ] {
        let (got, body) = upload(&h, &owner, content_type, bytes).await;
        assert_eq!(got, status, "{body}");
        assert_eq!(body["error"]["code"], code);
    }
    let (status, image) = upload(&h, &owner, "image/png", logo_png(1432, 536, 1)).await;
    assert_eq!(status, StatusCode::OK, "{image}");
    let tag = image["tag"].as_str().unwrap().to_string();
    let url = format!("/api/public/miniapp/{tag}.png");
    assert_eq!(image["url"], url.as_str());
    let (status, headers, bytes) = raw(&h.state, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'none'; style-src 'unsafe-inline'"
    );
    assert_eq!(bytes, logo_png(1432, 536, 1));
    for missing in [
        format!("/api/public/miniapp/{tag}.webp"),
        format!("/api/public/miniapp/{tag}"),
        format!("/api/public/landing/{tag}.png"),
        "/api/public/miniapp/ffffffffffffffff.png".to_string(),
        "/api/public/miniapp/..%2F..%2Fetc.png".to_string(),
    ] {
        let (status, _, _) = raw(&h.state, &missing).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }
    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "home": [{ "kind": "banners", "items": [
                { "image": { "tag": tag }, "to": "plans" }
            ] }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["home"][0]["items"][0]["image"],
        json!({ "tag": tag, "kind": "png", "width": 1432, "height": 536, "url": url })
    );
    assert_eq!(
        site(&h).await["miniapp"]["home"][0]["items"][0]["image"]["url"],
        url.as_str()
    );
    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "home": [{ "kind": "banners", "items": [
                { "image": { "tag": "ffffffffffffffff" } }
            ] }] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(
        error["error"]["message"],
        "Главная, баннер 1, картинка: не найдена, загрузите её заново"
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn вкладка_выключенного_модуля_клиенту_не_отдаётся() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let body = json!({ "tabs": [
        { "std": "home" }, { "std": "plans" }, { "std": "gifts" }, { "std": "profile" }
    ] });
    let (status, saved) = h.write(Method::PUT, ADMIN_PATH, &owner, body).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let toggle = |enabled: bool| {
        let h = &h;
        let owner = owner.clone();
        async move {
            let (status, body) = h
                .write(
                    Method::PATCH,
                    "/api/admin/modules/gifts",
                    &owner,
                    json!({ "enabled": enabled }),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    };
    toggle(true).await;
    assert_eq!(
        tabs(&site(&h).await["miniapp"]["tabs"]),
        ["home", "plans", "gifts", "profile"]
    );
    toggle(false).await;
    assert_eq!(
        tabs(&site(&h).await["miniapp"]["tabs"]),
        ["home", "plans", "profile"]
    );
    let (_, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(tabs(&view["tabs"]), ["home", "plans", "gifts", "profile"]);
    let gifts = view["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["key"] == "gifts")
        .unwrap()
        .clone();
    assert_eq!(
        gifts["off"], "Модуль «Подарочные подписки» выключен",
        "{gifts}"
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn превью_мини_приложения_встраивается_только_в_свой_сайт() {
    let (state, dir) = web_state().await;
    let (status, headers, body) = fetch(&state, Method::GET, "/admin/mini-preview").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("noindex"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "SAMEORIGIN");
    assert!(
        headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'self'")
    );
    let (_, headers, _) = fetch(&state, Method::GET, "/admin/settings/miniapp").await;
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn свой_css_мини_приложения_отдаётся_файлом_и_только_в_приложение() {
    let Some(mut h) = http().await else { return };
    let owner = h.owner().await;
    let check = format!("{ADMIN_PATH}/css");
    for (email, role) in [
        ("support@example.com", AdminRole::Support),
        ("viewer@example.com", AdminRole::ReadOnly),
    ] {
        let token = h.admin(email, role).await;
        let (status, _) = h
            .write(Method::POST, &check, &token, json!({ "css": {} }))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{email}");
    }
    let dir = std::env::temp_dir().join(format!("sn-mini-css-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html lang=\"ru\"><head><title>S</title></head><body></body></html>",
    )
    .unwrap();
    h.state.config = Arc::new(ServerConfig {
        web_dir: Some(dir.clone()),
        csp: crate::security::CspMode::All,
        ..ServerConfig::default()
    });
    let (_, _, plain) = fetch_file(&h, "/app", None).await;
    assert!(!plain.contains("preload"));

    let (status, report) = h
        .write(
            Method::POST,
            &check,
            &owner,
            json!({ "css": {
                "tabs": ".mn-tab { color: red }",
                "page": ".mn {}\n.mn { background: url(https://evil.example/x.png) }",
                "hero": "x"
            } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(keys_of(&report), ["page", "tabs"]);
    assert_eq!(report["page"]["line"], 2);
    assert!(
        report["tabs"]["style"]
            .as_str()
            .unwrap()
            .starts_with("[data-part=\"tabs\"] .mn-tab")
    );
    assert_eq!(site(&h).await["miniapp"], Value::Null);

    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "css": { "page": ".mn { --accent: teal }", "tabs": "gap: 0" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["css"]["page"], ".mn { --accent: teal }");
    assert_eq!(saved["limits"]["page_css"], 50_000);
    assert_eq!(saved["limits"]["block_css"], 10_000);

    let shown = site(&h).await;
    let path = shown["miniapp"]["css"].as_str().unwrap().to_string();
    assert!(path.starts_with("/api/public/miniapp/"), "{path}");
    assert!(shown["miniapp"].get("styles").is_none());
    assert_eq!(
        tabs(&shown["miniapp"]["tabs"]),
        ["home", "plans", "connect", "profile"]
    );

    let (status, headers, text) = fetch_file(&h, &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        text,
        ".mn { --accent: teal }\n[data-part=\"tabs\"] {\n  gap: 0;\n}\n"
    );
    assert_eq!(headers[header::CONTENT_TYPE], "text/css; charset=utf-8");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'none'; frame-ancestors 'none'; sandbox"
    );
    let etag = headers[header::ETAG].to_str().unwrap().to_string();
    let (status, _, text) = fetch_file(&h, &path, Some(&etag)).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(text.is_empty());

    for page in ["/app", "/app/plans"] {
        let (_, headers, html) = fetch_file(&h, page, None).await;
        assert!(
            html.contains(&format!(
                "<link rel=\"preload\" as=\"style\" href=\"{path}\">"
            )),
            "{html}"
        );
        assert!(!html.contains("<style"));
        let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(policy.contains("style-src 'self';"), "{policy}");
        assert!(!policy.contains("unsafe-inline"), "{policy}");
    }
    for other in [
        "/",
        "/cabinet",
        "/admin",
        "/admin/mini-preview",
        "/pay/1",
        "/verify/0123456789abcdef",
    ] {
        let (_, _, html) = fetch_file(&h, other, None).await;
        assert!(!html.contains(&path), "{other}");
    }

    let (status, _) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "css": { "page": ".mn { --accent: navy }" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, headers, text) = fetch_file(&h, &path, Some(&etag)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(text, ".mn { --accent: navy }\n");
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    for missing in [
        "/api/public/miniapp/style.css",
        "/api/public/miniapp/0123456789ABCDEF.css",
        "/api/public/miniapp/..%2F..%2Fetc%2Fpasswd.css",
        "/api/public/miniapp/.css",
    ] {
        let (status, _, _) = fetch_file(&h, missing, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }

    let (_, journal) = h.get("/api/admin/audit?limit=5", &owner).await;
    let mut texts = Vec::new();
    for item in journal["items"].as_array().unwrap() {
        if item["action"] == "settings.miniapp" {
            let (_, entry) = h
                .get(&format!("/api/admin/audit/{}", item["id"]), &owner)
                .await;
            texts.push(entry.to_string());
        }
    }
    let text = texts.join("\n");
    for part in ["Свой CSS", "общий, меню внизу", "Размер CSS, символов"]
    {
        assert!(text.contains(part), "{part}: {text}");
    }
    assert!(!text.contains("teal"), "{text}");

    let (status, reset) = h
        .write(Method::PUT, ADMIN_PATH, &owner, json!({ "css": {} }))
        .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert!(reset.get("css").is_none());
    assert_eq!(site(&h).await["miniapp"], Value::Null);
    let (status, _, _) = fetch_file(&h, &path, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, _, html) = fetch_file(&h, "/app", None).await;
    assert!(!html.contains("preload"));
    std::fs::remove_dir_all(dir).ok();
    h.db.cleanup().await;
}
