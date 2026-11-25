use super::*;
use sn_domain::client::{IdentityKind, NewIdentity};
use sn_store::SessionScope;

const LINK: &str = "/api/client/security/logins/telegram";
const PANEL: &str = "https://panel.example.com";

async fn linking() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks {
            public_url: Some(PANEL.into()),
            bot_token: Some("123456:TEST".into()),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    let http = Http {
        db,
        state: AppState::new(
            ServerConfig {
                public_url: Some(PANEL.into()),
                ..ServerConfig::default()
            },
            services,
        ),
        clock,
        remna,
    };
    http.set_bot().await;
    Some(http)
}

async fn site_client(h: &Http, address: &str) -> ClientId {
    let mut tx = h.services().store().begin().await.unwrap();
    let client = tx
        .clients()
        .create(&sn_store::NewClient {
            locale: sn_domain::client::Locale::Ru,
            registration_source: RegistrationSource::Web,
            note: None,
        })
        .await
        .unwrap();
    tx.clients()
        .add_identity(
            client.id,
            &NewIdentity::new(IdentityKind::Email, address, true).unwrap(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    client.id
}

async fn by_cookie(h: &Http, uri: &str, token: &str, origin: &str, body: Value) -> StatusCode {
    let site = if origin == PANEL {
        "same-origin"
    } else {
        "cross-site"
    };
    router(h.state.clone())
        .oneshot(
            Request::post(uri)
                .header(
                    "cookie",
                    format!("{}={token}", crate::extract::CLIENT_COOKIE),
                )
                .header("origin", origin)
                .header("sec-fetch-site", site)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn кабинет_привязывает_telegram_и_видит_итог() {
    let Some(h) = linking().await else { return };
    let client = site_client(&h, "alexey@mail.example").await;
    let token = h.client_token(client).await;
    let (status, logins) = h.get("/api/client/security/logins", &token).await;
    assert_eq!(status, StatusCode::OK, "{logins}");
    assert_eq!(logins["can_link_telegram"], true);
    assert_eq!(logins["can_link"], json!([]));
    let (status, _, start) = h.send(Method::POST, LINK, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{start}");
    let param = start["link"]
        .as_str()
        .unwrap()
        .strip_prefix("https://t.me/stealth_vpn_bot?start=")
        .unwrap()
        .to_string();
    assert!(param.starts_with("tglink_"));
    let poll = json!({ "poll_token": start["poll_token"] });
    let (status, _, pending) = h
        .send(
            Method::POST,
            &format!("{LINK}/status"),
            Some(&token),
            Some(poll.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    assert_eq!(pending["status"], "pending");
    let neighbour = h.client_token(client).await;
    for uri in [format!("{LINK}/status"), format!("{LINK}/cancel")] {
        let (status, _, _) = h
            .send(Method::POST, &uri, Some(&neighbour), Some(poll.clone()))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        let (status, _, _) = h.send(Method::POST, &uri, None, Some(poll.clone())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
    h.services()
        .telegram_links()
        .confirm(
            &param,
            &TelegramProfile {
                id: 9_401,
                username: None,
                first_name: Some("Алексей".into()),
                last_name: Some("Морозов".into()),
                language_code: Some("ru".into()),
                is_premium: false,
            },
        )
        .await
        .unwrap();
    let (_, _, linked) = h
        .send(
            Method::POST,
            &format!("{LINK}/status"),
            Some(&token),
            Some(poll),
        )
        .await;
    assert_eq!(
        linked,
        json!({ "status": "linked", "account": "Алексей Морозов" })
    );
    let (_, logins) = h.get("/api/client/security/logins", &token).await;
    assert_eq!(logins["can_link_telegram"], false);
    let telegram = logins["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "telegram")
        .unwrap();
    assert_eq!(telegram["account"], "Алексей Морозов");
    assert_eq!(telegram["removable"], false);
    let (status, _, again) = h.send(Method::POST, LINK, Some(&token), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(again["error"]["code"], "telegram_link.already_linked");
    let (status, _, _) = h.send(Method::DELETE, LINK, Some(&token), None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    let owner = h.owner().await;
    let (status, card) = h
        .get(&format!("/api/admin/clients/{}", client.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{card}");
    assert!(card.to_string().contains("9401"), "{card}");
    let (status, history) = h
        .get(&format!("/api/admin/clients/{}/history", client.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert!(
        history.to_string().contains("client.telegram_linked"),
        "{history}"
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn чужой_сайт_и_урезанная_сессия_не_начинают_привязку() {
    let Some(h) = linking().await else { return };
    let client = site_client(&h, "alexey@mail.example").await;
    let token = h.client_token(client).await;
    assert_eq!(
        by_cookie(&h, LINK, &token, "https://evil.example", json!({})).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        by_cookie(&h, LINK, &token, PANEL, json!({})).await,
        StatusCode::OK
    );
    let (status, _, _) = h.send(Method::POST, LINK, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for scope in [SessionScope::SubscriptionView, SessionScope::PasswordChange] {
        let token = h.client_token(client).await;
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query(
            "UPDATE client_sessions SET scope = $1 WHERE id = (SELECT max(id) FROM client_sessions)",
        )
        .bind(scope.as_str())
        .execute(conn.conn())
        .await
        .unwrap();
        drop(conn);
        let (status, _, body) = h.send(Method::POST, LINK, Some(&token), None).await;
        assert!(status.is_client_error(), "{status} {body}");
    }
    let from_bot = h.client(9_402).await;
    let token = h.client_token(from_bot).await;
    let (_, logins) = h.get("/api/client/security/logins", &token).await;
    assert_eq!(logins["can_link_telegram"], false);
    let blocked = site_client(&h, "boris@mail.example").await;
    let token = h.client_token(blocked).await;
    let mut conn = h.services().store().conn().await.unwrap();
    conn.clients()
        .set_status(blocked, sn_domain::client::ClientStatus::Blocked, None)
        .await
        .unwrap();
    drop(conn);
    let (status, _, body) = h.send(Method::POST, LINK, Some(&token), None).await;
    assert!(status.is_client_error(), "{status} {body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn отказы_привязки_приходят_на_языке_клиента() {
    let Some(h) = linking().await else { return };
    let from_bot = h.client(9_403).await;
    let token = h.client_token(from_bot).await;
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            &owner,
            json!({ "english": true, "default": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response = router(h.state.clone())
        .oneshot(
            Request::post(LINK)
                .header("authorization", format!("Bearer {token}"))
                .header("x-sn-lang", "en")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "telegram_link.already_linked");
    assert_eq!(
        body["error"]["message"],
        "A Telegram is already linked to this account."
    );
    h.db.cleanup().await;
}
