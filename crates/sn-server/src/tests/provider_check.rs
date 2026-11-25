use super::*;
use sn_payments::providers::{Balance, CryptoPay, TelegramStars, cryptopay_signature};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn check_http(api: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(CryptoPay::with_api_base(api.uri())),
            Arc::new(TelegramStars::with_api_base(api.uri())),
            Arc::new(Balance),
        ])),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &json!("https://panel.example.com"), None)
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

async fn crypto_api() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/getMe"))
        .and(header("Crypto-Pay-API-Token", "1234:AAA"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": {"app_id": 7, "name": "VPN Shop", "payment_processing_bot_username": "CryptoBot"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/getMe"))
        .and(header("Crypto-Pay-API-Token", "5678:LEAK"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "ok": false, "error": {"code": 400, "name": "TOKEN 5678:LEAK IS DISABLED"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/getMe"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "ok": false, "error": {"code": 401, "name": "UNAUTHORIZED"}
        })))
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

fn provider<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap()
}

async fn set_token(h: &Http, owner: &str, token: &str) {
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/cryptopay",
            owner,
            json!({"enabled": true, "config": {"api_token": token}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn check(h: &Http, owner: &str, id: &str) -> (StatusCode, Value) {
    h.write(
        Method::POST,
        &format!("/api/admin/payments/providers/{id}/check"),
        owner,
        json!({}),
    )
    .await
}

#[tokio::test]
async fn проверка_подключения_из_админки_помнит_итог_и_не_показывает_ключи() {
    let api = crypto_api().await;
    let Some(h) = check_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = check(&h, &owner, "cryptopay").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["code"],
        "settings.provider_check_unconfigured"
    );
    assert!(api.received_requests().await.unwrap().is_empty());

    set_token(&h, &owner, "1234:AAA").await;
    let (status, body) = check(&h, &owner, "cryptopay").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    assert!(body["summary"].as_str().unwrap().contains("VPN Shop"));
    assert_eq!(body["at"], json!(start()));
    assert!(!body.to_string().contains("1234:AAA"));

    h.services().reload().await.unwrap();
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    let crypto = provider(&list, "cryptopay");
    assert_eq!(crypto["check"]["status"], "ok");
    assert_eq!(crypto["check"]["at"], json!(start()));
    assert!(!list.to_string().contains("1234:AAA"));

    set_token(&h, &owner, "9999:BBB").await;
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    assert_eq!(provider(&list, "cryptopay")["check"], Value::Null);
    let (status, body) = check(&h, &owner, "cryptopay").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "keys_rejected");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    assert_eq!(
        provider(&list, "cryptopay")["check"]["status"],
        "keys_rejected"
    );

    set_token(&h, &owner, "5678:LEAK").await;
    let (status, body) = check(&h, &owner, "cryptopay").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "keys_rejected");
    assert!(!body.to_string().contains("5678:LEAK"), "{body}");
    assert!(body["summary"].as_str().unwrap().contains("•••"), "{body}");
    let mut conn = h.services().store().conn().await.unwrap();
    let stored = conn
        .settings()
        .get(&keys::provider_check("cryptopay"))
        .await
        .unwrap()
        .unwrap();
    assert!(!stored.to_string().contains("5678:LEAK"));
    drop(conn);

    let (status, _) = check(&h, &owner, "balance").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = check(&h, &owner, "unknown").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = check(&h, &support, "cryptopay").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn проверки_подключения_ограничены_по_частоте() {
    let api = crypto_api().await;
    let Some(h) = check_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    set_token(&h, &owner, "1234:AAA").await;
    for _ in 0..10 {
        let (status, body) = check(&h, &owner, "cryptopay").await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let calls = api.received_requests().await.unwrap().len();
    let (status, body) = check(&h, &owner, "cryptopay").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(api.received_requests().await.unwrap().len(), calls);
    h.clock.set(start() + Duration::minutes(10));
    let (status, body) = check(&h, &owner, "cryptopay").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn проверка_звёзд_сбрасывается_при_смене_токена_бота() {
    let api = MockServer::start().await;
    for token in [
        "123456:TEST-token-for-stars-check",
        "654321:NEW-token-for-stars-check-2",
    ] {
        Mock::given(method("POST"))
            .and(path(format!("/bot{token}/getMe")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true, "result": {"id": 123456, "is_bot": true, "first_name": "VPN", "username": "vpn_shop_bot"}
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{token}/getWebhookInfo")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true, "result": {"url": "", "pending_update_count": 0}
            })))
            .mount(&api)
            .await;
    }
    let Some(h) = check_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = check(&h, &owner, "stars").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/bot",
            &owner,
            json!({"token": "123456:TEST-token-for-stars-check"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = check(&h, &owner, "stars").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    assert!(body["summary"].as_str().unwrap().contains("@vpn_shop_bot"));
    assert!(!body.to_string().contains("TEST-token-for-stars-check"));
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    assert_eq!(provider(&list, "stars")["check"]["status"], "ok");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/bot",
            &owner,
            json!({"token": "654321:NEW-token-for-stars-check-2"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    assert_eq!(provider(&list, "stars")["check"], Value::Null);
}

async fn crypto_hook(h: &Http, token: &str, invoice: i64) -> StatusCode {
    let body = json!({
        "update_id": invoice, "update_type": "invoice_paid", "request_date": "2026-11-16T14:50:00.000Z",
        "payload": {"invoice_id": invoice, "status": "paid", "currency_type": "fiat", "fiat": "USD",
                    "amount": "5.00", "payload": Uuid::from_u128(77).to_string()}
    })
    .to_string();
    router(h.state.clone())
        .oneshot(
            Request::post("/api/webhooks/pay/cryptopay")
                .header(
                    "crypto-pay-api-signature",
                    cryptopay_signature(token, body.as_bytes()),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn проверка_показывает_последние_уведомления_и_неверную_подпись() {
    let api = crypto_api().await;
    let Some(h) = check_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    set_token(&h, &owner, "1234:AAA").await;
    let (_, body) = check(&h, &owner, "cryptopay").await;
    assert_eq!(body["inbox"], Value::Null);
    assert_eq!(crypto_hook(&h, "1234:AAA", 501).await, StatusCode::OK);
    let (_, body) = check(&h, &owner, "cryptopay").await;
    assert!(body["inbox"]["verified_at"].is_string(), "{body}");
    assert_eq!(body["inbox"]["rejected_at"], Value::Null);
    assert_eq!(
        crypto_hook(&h, "old-token", 502).await,
        StatusCode::UNAUTHORIZED
    );
    let (_, body) = check(&h, &owner, "cryptopay").await;
    assert!(body["inbox"]["verified_at"].is_string(), "{body}");
    assert!(body["inbox"]["rejected_at"].is_string(), "{body}");
}

#[tokio::test]
async fn бонус_к_пополнению_задаётся_у_платёжки_и_виден_клиенту() {
    let api = crypto_api().await;
    let Some(h) = check_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    set_token(&h, &owner, "1234:AAA").await;
    let path = "/api/admin/payments/providers/cryptopay";
    let (_, before) = h.get(path, &owner).await;
    assert_eq!(
        before["provider"]["topup_bonus"],
        json!({"USD": {"enabled": false, "percent": 0}, "RUB": {"enabled": false, "percent": 0}})
    );
    let terms =
        json!({"enabled": true, "percent": 5, "min_minor": 1500, "max_minor": 300, "methods": []});
    let (status, body) = h
        .write(Method::PUT, path, &owner, json!({"topup_bonus": terms}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["provider"]["topup_bonus"]["USD"],
        json!({
            "enabled": true, "percent": 5,
            "min": {"minor": 1500, "currency": "USD"},
            "max": {"minor": 300, "currency": "USD"}
        })
    );
    assert_eq!(body["provider"]["enabled"], true);
    for (wrong, code) in [
        (
            json!({"enabled": true, "percent": 101}),
            "settings.topup_bonus_percent",
        ),
        (
            json!({"enabled": true, "percent": 0}),
            "settings.topup_bonus_percent",
        ),
        (
            json!({"enabled": true, "percent": 5, "min_minor": 0}),
            "settings.topup_bonus_min",
        ),
        (
            json!({"enabled": true, "percent": 5, "max_minor": -1}),
            "settings.topup_bonus_max",
        ),
        (
            json!({"enabled": true, "percent": 5, "methods": ["sbp"]}),
            "settings.topup_bonus_methods",
        ),
    ] {
        let (status, body) = h
            .write(Method::PUT, path, &owner, json!({"topup_bonus": wrong}))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], code);
    }
    let (status, _) = h
        .write(
            Method::PUT,
            path,
            &owner,
            json!({"topup_bonus": {"enabled": true, "percent": 5.5}}),
        )
        .await;
    assert!(status.is_client_error());
    for id in ["stars", "balance"] {
        let (status, body) = h
            .write(
                Method::PUT,
                &format!("/api/admin/payments/providers/{id}"),
                &owner,
                json!({"topup_bonus": {"enabled": true, "percent": 5}}),
            )
            .await;
        assert!(status.is_client_error(), "{body}");
        assert_eq!(body["error"]["code"], "settings.topup_bonus_kind");
    }

    let client = h.client(5_093_118_777).await;
    let token = h.client_token(client).await;
    let (status, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["methods"][0]["provider"], "cryptopay");
    assert_eq!(
        options["methods"][0]["bonus"],
        json!({
            "percent": 5,
            "min": {"minor": 1500, "currency": "USD"},
            "max": {"minor": 300, "currency": "USD"}
        })
    );
    assert!(
        options["methods"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["provider"] != "stars")
    );
    let (status, refused) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({"amount": "15", "provider": "stars"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");

    let (_, body) = h
        .write(
            Method::PUT,
            path,
            &owner,
            json!({"topup_bonus": {"enabled": false, "percent": 5, "min_minor": 1500}}),
        )
        .await;
    assert_eq!(body["provider"]["topup_bonus"]["USD"]["enabled"], false);
    assert_eq!(body["provider"]["topup_bonus"]["USD"]["percent"], 5);
    let (status, body) = h
        .write(
            Method::PUT,
            path,
            &owner,
            json!({"currency": "RUB", "topup_bonus": {"enabled": true, "percent": 9, "max_minor": 50000}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["provider"]["topup_bonus"]["RUB"]["max"],
        json!({"minor": 50000, "currency": "RUB"})
    );
    assert_eq!(body["provider"]["topup_bonus"]["USD"]["percent"], 5);
    let (status, rub) = h.get(&format!("{path}?currency=RUB"), &owner).await;
    assert_eq!(status, StatusCode::OK, "{rub}");
    assert_eq!(rub["provider"]["topup_bonus"]["RUB"]["percent"], 9);
    let (_, options) = h.get("/api/client/topup", &token).await;
    assert!(options["methods"][0].get("bonus").is_none());
    h.db.cleanup().await;
}
