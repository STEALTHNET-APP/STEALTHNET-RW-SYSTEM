use super::*;
use sn_domain::payment::PaymentStatus;
use sn_payments::providers::cryptopay_signature;

const CRYPTO_TOKEN: &str = "4321:TEST";

async fn turn(h: &Http, owner: &str, body: Value) -> Value {
    let (status, view) = h
        .write(Method::PUT, "/api/admin/settings/maintenance", owner, body)
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

async fn on(h: &Http, owner: &str) -> Value {
    turn(
        h,
        owner,
        json!({
            "enabled": true,
            "message_ru": "Переезжаем на новый сервер",
            "message_en": "",
            "until": "2026-11-16T20:00:00+03:00",
        }),
    )
    .await
}

async fn site(h: &Http) -> Value {
    let (status, _, body) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(status, StatusCode::OK);
    body
}

async fn english(h: &Http, owner: &str) {
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            owner,
            json!({ "english": true, "default": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn client_call(
    h: &Http,
    method: Method,
    path: &str,
    token: Option<&str>,
    lang: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(lang) = lang {
        builder = builder.header(crate::lang::HEADER, lang);
    }
    let request = builder
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    (status, headers, body_json(response).await)
}

#[tokio::test]
async fn режим_работ_включается_в_настройках_и_виден_на_сайте_и_в_журнале() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, view) = h.get("/api/admin/settings/maintenance", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["settings"]["enabled"], false);
    assert_eq!(view["telegram_id"], Value::Null);
    assert!(
        view["standard"]["ru"]
            .as_str()
            .unwrap()
            .contains("Обновляем сервис")
    );
    assert_eq!(site(&h).await["maintenance"], Value::Null);
    assert_eq!(h.get("/api/admin/me", &owner).await.1["maintenance"], false);

    let view = on(&h, &owner).await;
    assert_eq!(view["settings"]["enabled"], true);
    assert_eq!(view["settings"]["until"], "2026-11-16T17:00:00Z");
    assert_eq!(view["settings"]["offset_minutes"], 180);
    let shown = site(&h).await["maintenance"].clone();
    assert_eq!(shown["message"]["ru"], "Переезжаем на новый сервер");
    assert!(
        shown["message"]["en"]
            .as_str()
            .unwrap()
            .starts_with("We are updating")
    );
    assert_eq!(shown["until"], "2026-11-16T17:00:00Z");
    assert_eq!(h.get("/api/admin/me", &owner).await.1["maintenance"], true);

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/auth/telegram",
            &owner,
            json!({ "telegram_id": 700_300 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["telegram_id"], 700_300);
    let (_, view) = h.get("/api/admin/settings/maintenance", &owner).await;
    assert_eq!(view["telegram_id"], 700_300);

    let support = h.admin("help@example.com", AdminRole::Support).await;
    assert_eq!(
        h.get("/api/admin/settings/maintenance", &support).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.write(
            Method::PUT,
            "/api/admin/settings/maintenance",
            &support,
            json!({ "enabled": false })
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.get("/api/admin/me", &support).await.1["maintenance"],
        true
    );

    turn(&h, &owner, json!({ "enabled": false })).await;
    assert_eq!(site(&h).await["maintenance"], Value::Null);

    let (status, page) = h.get("/api/admin/audit", &owner).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let titles: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["title"].as_str())
        .collect();
    for title in [
        "Включены технические работы",
        "Выключены технические работы",
        "Изменён Telegram ID администратора",
    ] {
        assert!(titles.contains(&title), "{title}: {titles:?}");
    }
    let (_, page) = h
        .get("/api/admin/audit?action=settings.maintenance_on", &owner)
        .await;
    let labels: Vec<&str> = page["items"][0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|change| change["label"].as_str())
        .collect();
    assert!(labels.contains(&"Сообщение на русском"), "{labels:?}");
    assert!(labels.contains(&"Закончим примерно"), "{labels:?}");
    assert!(!labels.contains(&"Сообщение на английском"), "{labels:?}");
    let (_, page) = h
        .get("/api/admin/audit?action=admin.telegram", &owner)
        .await;
    assert_eq!(page["items"][0]["changes"][0]["label"], "ID в Telegram");
    assert_eq!(page["items"][0]["changes"][0]["after"], "700300");
}

#[tokio::test]
async fn во_время_работ_api_клиента_отказывает_в_изменениях() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    english(&h, &owner).await;
    let client = h.client(5_100_200_300).await;
    let token = h.client_token(client).await;
    on(&h, &owner).await;

    for path in [
        "/api/client/trial",
        "/api/client/topup",
        "/api/client/checkout",
        "/api/client/m/promo/activate",
        "/api/client/m/gifts/buy",
        "/api/client/auth/miniapp",
        "/api/public/verify/pass",
    ] {
        let (status, headers, body) =
            client_call(&h, Method::POST, path, Some(&token), Some("ru")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {body}");
        assert_eq!(body["error"]["code"], "maintenance", "{path}");
        assert_eq!(
            body["error"]["message"],
            "Переезжаем на новый сервер\n\nЗакончим примерно 16\u{a0}ноября в\u{a0}20:00 (UTC+3).",
            "{path}"
        );
        assert_eq!(
            headers
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("300")
        );
    }
    let (status, _, body) = client_call(
        &h,
        Method::PUT,
        "/api/client/subscriptions/1/autorenew",
        Some(&token),
        Some("en"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let english = body["error"]["message"].as_str().unwrap();
    assert!(
        english.starts_with("We are updating the service."),
        "{english}"
    );
    assert!(
        english.ends_with("We expect to finish around 20:00 on\u{a0}16\u{a0}November (UTC+3).")
    );

    let (status, _, me) = client_call(&h, Method::GET, "/api/client/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    let (status, _, _) = client_call(
        &h,
        Method::GET,
        "/api/client/subscriptions",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = client_call(
        &h,
        Method::POST,
        "/api/client/auth/logout",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    turn(&h, &owner, json!({ "enabled": false })).await;
    let token = h.client_token(client).await;
    let (status, _, body) =
        client_call(&h, Method::POST, "/api/client/trial", Some(&token), None).await;
    assert_ne!(body["error"]["code"], "maintenance", "{status} {body}");
}

#[tokio::test]
async fn уведомления_платёжек_проводят_оплату_во_время_работ() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    {
        let mut conn = h.db.store.conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::CRYPTOPAY),
                &json!({ "enabled": true }),
                None,
            )
            .await
            .unwrap();
        sn_services::settings::write_secret(
            &mut conn,
            h.services().secrets(),
            &sn_domain::settings::secret_keys::provider(providers::CRYPTOPAY, "api_token"),
            Some(CRYPTO_TOKEN),
            None,
        )
        .await
        .unwrap();
    }
    h.services().reload().await.unwrap();
    h.services().set_remna(Some(h.remna.clone()));
    let client = h.client(5_100_200_301).await;
    let tariff = h.tariff().await;
    let payment = h.pending_payment(client, &tariff).await;
    on(&h, &owner).await;

    let (status, _, page) = h
        .send(
            Method::POST,
            &format!("/api/public/pay/{}/check", payment.public_id),
            None,
            None,
        )
        .await;
    assert_ne!(page["error"]["code"], "maintenance", "{status} {page}");

    let body = json!({
        "update_id": 41, "update_type": "invoice_paid", "request_date": "2026-11-16T15:00:00.000Z",
        "payload": {"invoice_id": 9041, "status": "paid", "currency_type": "fiat", "fiat": "USD",
                    "amount": "6.99", "payload": payment.public_id.to_string()}
    })
    .to_string();
    let response = router(h.state.clone())
        .oneshot(
            Request::post("/api/webhooks/pay/cryptopay")
                .header(
                    "crypto-pay-api-signature",
                    cryptopay_signature(CRYPTO_TOKEN, body.as_bytes()),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut conn = h.db.store.conn().await.unwrap();
    let paid = conn.payments().get(payment.id).await.unwrap();
    assert_eq!(paid.status, PaymentStatus::Paid);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE payload->>'kind' = 'fulfill_order' AND payload->>'order_id' = $1",
    )
    .bind(paid.order_id.0.to_string())
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(queued, 1, "выдача по оплате не поставлена в очередь");
}
