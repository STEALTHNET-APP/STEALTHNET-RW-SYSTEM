use super::*;

const ADMIN_PATH: &str = "/api/admin/settings/cabinet";
const CHECK_PATH: &str = "/api/admin/settings/cabinet/css";
const IMAGES_PATH: &str = "/api/admin/settings/cabinet/images";

async fn upload(h: &Http, token: &str, content_type: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(IMAGES_PATH)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn site_css(h: &Http) -> Value {
    let (status, _, body) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["cabinet"]["css"].clone()
}

async fn stored(h: &Http) -> Option<Value> {
    let mut conn = h.state.services.store().conn().await.unwrap();
    conn.settings().get("core.cabinet_css").await.unwrap()
}

fn with_web(h: &mut Http) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sn-cabinet-css-{}", Uuid::new_v4().simple()));
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
    dir
}

#[tokio::test]
async fn css_кабинета_правит_только_право_настроек() {
    let Some(h) = http().await else { return };
    for (email, role) in [
        ("support@example.com", AdminRole::Support),
        ("viewer@example.com", AdminRole::ReadOnly),
    ] {
        let token = h.admin(email, role).await;
        let (status, _) = h.get(ADMIN_PATH, &token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{email}");
        for (method, path) in [(Method::PUT, ADMIN_PATH), (Method::POST, CHECK_PATH)] {
            let (status, _) = h
                .write(method, path, &token, json!({ "css": { "page": ".cb {}" } }))
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{email} {path}");
        }
        let (status, _) = upload(&h, &token, "image/png", logo_png(800, 600, 1)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{email}");
    }
    assert_eq!(stored(&h).await, None);
    h.db.cleanup().await;
}

#[tokio::test]
async fn установка_без_css_кабинета_не_меняется() {
    let Some(mut h) = http().await else { return };
    let owner = h.owner().await;
    let dir = with_web(&mut h);
    let (status, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["css"], json!({}));
    assert_eq!(view["styles"], json!({}));
    assert_eq!(
        view["limits"],
        json!({ "page_css": 50_000, "block_css": 10_000 })
    );
    assert_eq!(view["off"], Value::Null);
    assert_eq!(site_css(&h).await, Value::Null);
    let (_, _, html) = fetch_file(&h, "/cabinet", None).await;
    assert!(!html.contains("preload"), "{html}");
    for body in [
        json!({}),
        json!({ "css": {} }),
        json!({ "css": { "page": "  \n" } }),
    ] {
        let (status, saved) = h.write(Method::PUT, ADMIN_PATH, &owner, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}: {saved}");
        assert_eq!(saved["css"], json!({}));
    }
    assert_eq!(stored(&h).await, None);
    let (_, journal) = h.get("/api/admin/audit?limit=5", &owner).await;
    assert!(
        journal["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["action"] != "settings.cabinet"),
        "{journal}"
    );
    let (status, _, _) = fetch_file(&h, "/api/public/cabinet/0123456789abcdef.css", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(dir).ok();
    h.db.cleanup().await;
}

#[tokio::test]
async fn проверка_css_кабинета_отвечает_по_местам_со_строкой() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, report) = h
        .write(
            Method::POST,
            CHECK_PATH,
            &owner,
            json!({ "css": {
                "pay": ".cb-term-card { border-radius: 20px }",
                "page": ".cb {}\n.cb { background: url(https://evil.example/x.png) }",
                "header": "@import url(/a.css);",
                "hero": ".x {}"
            } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let mut keys: Vec<&String> = report.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["header", "page", "pay"]);
    assert_eq!(report["page"]["line"], 2);
    assert_eq!(report["header"]["line"], 1);
    assert!(
        report["header"]["problem"]
            .as_str()
            .unwrap()
            .contains("@import")
    );
    assert!(
        report["pay"]["style"]
            .as_str()
            .unwrap()
            .starts_with("[data-part=\"pay\"] .cb-term-card"),
        "{report}"
    );
    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "css": { "page": ".cb {}", "stories": ".x {}" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(error["error"]["code"], "cabinet.invalid");
    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "css": { "money": ".a {\n  color: red\n" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("Пополнение и история оплат, CSS раздела, строка"),
        "{message}"
    );
    assert_eq!(stored(&h).await, None);
    h.db.cleanup().await;
}

#[tokio::test]
async fn css_кабинета_отдаётся_файлом_и_только_в_кабинет() {
    let Some(mut h) = http().await else { return };
    let owner = h.owner().await;
    let dir = with_web(&mut h);
    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "css": { "page": "body { --accent: teal }", "subscription": ".cb-days { font-size: 40px }" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["css"]["page"], "body { --accent: teal }");
    assert!(
        saved["styles"]["subscription"]
            .as_str()
            .unwrap()
            .starts_with("[data-part=\"subscription\"] .cb-days")
    );
    assert_eq!(
        stored(&h).await,
        Some(
            json!({ "page": "body { --accent: teal }", "subscription": ".cb-days { font-size: 40px }" })
        )
    );
    let path = site_css(&h).await.as_str().unwrap().to_string();
    assert!(
        path.starts_with("/api/public/cabinet/") && path.ends_with(".css"),
        "{path}"
    );

    let (status, headers, text) = fetch_file(&h, &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        text.starts_with("body { --accent: teal }\n[data-part=\"subscription\"] .cb-days"),
        "{text}"
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

    for page in ["/cabinet", "/cabinet/pay", "/cabinet/help/3"] {
        let (_, headers, html) = fetch_file(&h, page, None).await;
        assert!(
            html.contains(&format!(
                "<link rel=\"preload\" as=\"style\" href=\"{path}\">"
            )),
            "{page}: {html}"
        );
        assert!(!html.contains("<style"));
        let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(policy.contains("style-src 'self';"), "{policy}");
        assert!(!policy.contains("unsafe-inline"), "{policy}");
    }
    for other in [
        "/",
        "/app",
        "/admin",
        "/admin/cabinet-preview",
        "/pay/1",
        "/verify/0123456789abcdef",
        "/open-app",
        "/privacy",
    ] {
        let (_, _, html) = fetch_file(&h, other, None).await;
        assert!(!html.contains(&path), "{other}");
    }

    let (status, kept) = h.write(Method::PUT, ADMIN_PATH, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{kept}");
    assert_eq!(kept["css"]["page"], "body { --accent: teal }");

    let (status, _) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "css": { "page": "body { --accent: navy }" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, headers, text) = fetch_file(&h, &path, Some(&etag)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(text, "body { --accent: navy }\n");
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    for missing in [
        "/api/public/cabinet/style.css",
        "/api/public/cabinet/0123456789ABCDEF.css",
        "/api/public/cabinet/..%2F..%2Fetc%2Fpasswd.css",
        "/api/public/cabinet/.css",
    ] {
        let (status, _, _) = fetch_file(&h, missing, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }

    let (_, journal) = h.get("/api/admin/audit?limit=10", &owner).await;
    let mut texts = Vec::new();
    for item in journal["items"].as_array().unwrap() {
        if item["action"] == "settings.cabinet" {
            let (_, entry) = h
                .get(&format!("/api/admin/audit/{}", item["id"]), &owner)
                .await;
            texts.push(entry.to_string());
        }
    }
    assert_eq!(texts.len(), 2, "{journal}");
    let text = texts.join("\n");
    for part in [
        "Изменён CSS кабинета",
        "общий, подписка",
        "Размер CSS, символов",
        "/admin/settings/cabinet",
    ] {
        assert!(text.contains(part), "{part}: {text}");
    }
    assert!(!text.contains("teal") && !text.contains("navy"), "{text}");

    let (status, reset) = h
        .write(Method::PUT, ADMIN_PATH, &owner, json!({ "css": {} }))
        .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["css"], json!({}));
    assert_eq!(stored(&h).await, None);
    assert_eq!(site_css(&h).await, Value::Null);
    let (status, _, _) = fetch_file(&h, &path, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, _, html) = fetch_file(&h, "/cabinet", None).await;
    assert!(!html.contains("preload"));
    std::fs::remove_dir_all(dir).ok();
    h.db.cleanup().await;
}

#[tokio::test]
async fn сохранённый_css_с_чужим_местом_читается_без_него() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    {
        let mut conn = h.state.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                "core.cabinet_css",
                &json!({ "stories": ".x { color: red }", "help": ".cb-msg { color: red }", "pay": "" }),
                None,
            )
            .await
            .unwrap();
    }
    h.state.services.reload().await.unwrap();
    let (status, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["css"], json!({ "help": ".cb-msg { color: red }" }));
    {
        let mut conn = h.state.services.store().conn().await.unwrap();
        conn.settings()
            .set("core.cabinet_css", &json!(["broken"]), None)
            .await
            .unwrap();
    }
    h.state.services.reload().await.unwrap();
    let (status, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["css"], json!({}));
    assert_eq!(site_css(&h).await, Value::Null);
    h.db.cleanup().await;
}

#[tokio::test]
async fn картинки_кабинета_загружаются_и_проверяются_в_css() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    for (content_type, bytes, status, code) in [
        (
            "text/plain",
            logo_png(800, 600, 1),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "cabinet_image.format",
        ),
        (
            "image/png",
            b"<svg onload=\"alert(1)\"/>".to_vec(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "cabinet_image.format",
        ),
        (
            "image/png",
            logo_png(50, 50, 1),
            StatusCode::UNPROCESSABLE_ENTITY,
            "cabinet_image.too_small",
        ),
    ] {
        let (got, body) = upload(&h, &owner, content_type, bytes).await;
        assert_eq!(got, status, "{body}");
        assert_eq!(body["error"]["code"], code);
    }
    let (status, image) = upload(&h, &owner, "image/png", logo_png(1200, 400, 1)).await;
    assert_eq!(status, StatusCode::OK, "{image}");
    let tag = image["tag"].as_str().unwrap().to_string();
    let url = format!("/api/public/cabinet/{tag}.png");
    assert_eq!(image["url"], url.as_str());
    let (status, headers, bytes) = raw(&h.state, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(bytes, logo_png(1200, 400, 1));
    for missing in [
        format!("/api/public/miniapp/{tag}.png"),
        format!("/api/public/landing/{tag}.png"),
        format!("/api/public/cabinet/{tag}.webp"),
        "/api/public/cabinet/ffffffffffffffff.png".to_string(),
    ] {
        let (status, _, _) = raw(&h.state, &missing).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }
    let css = format!(".cb {{\n  background: url(\"{url}\") center / cover;\n}}");
    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "css": { "page": css } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "css": { "login": ".cb-login {\n  background: url(/api/public/cabinet/ffffffffffffffff.png);\n}" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(
        error["error"]["message"],
        "Вход и регистрация, CSS раздела, строка 2: картинка не найдена, загрузите её заново"
    );
    let (status, _, _) = raw(&h.state, &url).await;
    assert_eq!(status, StatusCode::OK);
    h.db.cleanup().await;
}

#[tokio::test]
async fn превью_кабинета_встраивается_только_в_свой_сайт() {
    let (state, dir) = web_state().await;
    let (status, headers, body) = fetch(&state, Method::GET, "/admin/cabinet-preview").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("noindex"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "SAMEORIGIN");
    assert!(
        headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'self'")
    );
    let (_, headers, _) = fetch(&state, Method::GET, "/admin/settings/cabinet").await;
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let (_, headers, _) = fetch(&state, Method::GET, "/cabinet").await;
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    std::fs::remove_dir_all(dir).ok();
}
