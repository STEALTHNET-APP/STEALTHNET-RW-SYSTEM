use super::*;
use sn_domain::access::Permission;
use sn_domain::payment::{NoticeKind, PaymentStatus};
use sn_payments::providers::{Balance, Heleket, heleket_signature};
use sn_store::InboxOutcome;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

const MERCHANT: &str = "8b03432e-385b-4670-8d06-064591096795";
const KEY: &str = "heleket-e2e-payment-key";
const STANDARD: &str = "https://api.heleket.hk";

struct Echo;

impl Respond for Echo {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let order = body["order_id"].as_str().unwrap_or_default().to_string();
        let signed = request
            .headers
            .get("sign")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|sign| sign == heleket_signature(&request.body, KEY));
        if !signed {
            return ResponseTemplate::new(401)
                .set_body_json(json!({"state": 1, "message": "Invalid sign"}));
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "state": 0,
            "result": {
                "uuid": format!("hk-{order}"),
                "order_id": order,
                "amount": body["amount"],
                "currency": body["currency"],
                "payer_currency": null,
                "url": format!("https://pay.heleket.hk/pay/hk-{order}"),
                "expired_at": 1_795_000_000,
                "payment_status": "check",
                "status": "check",
                "is_final": false
            }
        }))
    }
}

async fn heleket_http(api: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Heleket::with_api_base(api.uri())),
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

fn event(public: Uuid, status: &str, amount: &str, currency: &str, paid_usd: &str) -> String {
    format!(
        r#"{{"type":"payment","uuid":"hk-{public}","order_id":"{public}","amount":"{amount}","payment_amount":"6.40000000","payment_amount_usd":"{paid_usd}","merchant_amount":"6.27","commission":"0.13","is_final":true,"status":"{status}","from":null,"wallet_address_uuid":null,"network":"tron","currency":"{currency}","payer_currency":"USDT","additional_data":null,"txid":"5b2e\/91"}}"#
    )
}

fn signed(body: &str) -> String {
    let sign = heleket_signature(body.as_bytes(), KEY);
    format!("{},\"sign\":\"{sign}\"}}", &body[..body.len() - 1])
}

async fn notify(h: &Http, raw: String) -> StatusCode {
    let request = Request::post("/api/webhooks/pay/heleket")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(raw))
        .unwrap();
    router(h.state.clone())
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

async fn top_up(h: &Http, token: &str, amount: &str) -> Uuid {
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            token,
            json!({"amount": amount, "provider": "heleket"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "redirect");
    body["payment_id"].as_str().unwrap().parse().unwrap()
}

async fn balance(h: &Http, token: &str) -> Value {
    let (status, options) = h.get("/api/client/topup", token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    options["balance"].clone()
}

async fn connect(h: &Http, owner: &str) {
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/heleket",
            owner,
            json!({"enabled": true, "config": {"merchant_id": MERCHANT, "api_key": KEY}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn listed(h: &Http, owner: &str) -> Value {
    let (status, list) = h.get("/api/admin/payments/providers", owner).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "heleket")
        .unwrap()
        .clone()
}

async fn sent_bodies(api: &MockServer, route: &str) -> Vec<Value> {
    api.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == route)
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

#[tokio::test]
async fn heleket_в_рублёвой_установке_счёт_оплата_недоплата_излишек_и_сверка() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .and(header("merchant", MERCHANT))
        .respond_with(Echo)
        .mount(&api)
        .await;
    let Some(h) = heleket_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "currency": "RUB" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    connect(&h, &owner).await;
    let shown = listed(&h, &owner).await;
    assert_eq!(shown["currencies"], json!(["USD", "RUB"]));
    assert_eq!(shown["configured"], true);
    assert_eq!(shown["own_api_url"], Value::Null);
    assert_eq!(shown["methods"].as_array().unwrap().len(), 7);
    assert!(shown["topup_bonus"].get("RUB").is_some(), "{shown}");

    let client = h.client(5_093_118_901).await;
    let token = h.client_token(client).await;
    let (status, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["methods"],
        json!([{"provider": "heleket", "method": null, "title": "Криптовалюта", "hint": "USDT и другие монеты"}])
    );
    let paid = top_up(&h, &token, "500").await;
    let over = top_up(&h, &token, "700").await;
    let under = top_up(&h, &token, "300").await;
    let foreign = top_up(&h, &token, "400").await;
    let polled = top_up(&h, &token, "250").await;
    let created = sent_bodies(&api, "/v1/payment").await;
    let asked: Vec<(&str, &str)> = created
        .iter()
        .map(|b| {
            (
                b["amount"].as_str().unwrap(),
                b["currency"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        asked,
        [
            ("500.00", "RUB"),
            ("700.00", "RUB"),
            ("300.00", "RUB"),
            ("400.00", "RUB"),
            ("250.00", "RUB")
        ]
    );
    assert_eq!(
        created[0]["url_callback"],
        "https://panel.example.com/api/webhooks/pay/heleket"
    );

    let first = signed(&event(paid, "paid", "500.00", "RUB", "6.40"));
    for _ in 0..2 {
        assert_eq!(notify(&h, first.clone()).await, StatusCode::OK);
    }
    assert_eq!(
        balance(&h, &token).await,
        json!({"minor": 50_000, "currency": "RUB"})
    );
    let surplus = signed(&event(over, "paid_over", "700.00", "RUB", "9.50"));
    assert_eq!(notify(&h, surplus.clone()).await, StatusCode::OK);
    assert_eq!(notify(&h, surplus).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await["minor"], 120_000);
    let short = signed(&event(under, "wrong_amount", "300.00", "RUB", "2.10"));
    assert_eq!(notify(&h, short).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await["minor"], 120_000);
    let dollars = signed(&event(foreign, "paid", "400.00", "USD", "400.00"));
    assert_eq!(notify(&h, dollars).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await["minor"], 120_000);

    Mock::given(method("POST"))
        .and(path("/v1/payment/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "state": 0,
            "result": {
                "uuid": format!("hk-{polled}"), "order_id": polled.to_string(),
                "amount": "250.00", "currency": "RUB", "payment_status": "paid", "is_final": true
            }
        })))
        .mount(&api)
        .await;
    let polled_id = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.payments().by_public_id(polled).await.unwrap().id
    };
    h.services().inbox().poll_payment(polled_id).await.unwrap();
    h.services().inbox().poll_payment(polled_id).await.unwrap();
    assert_eq!(
        balance(&h, &token).await,
        json!({"minor": 145_000, "currency": "RUB"})
    );
    assert_eq!(
        sent_bodies(&api, "/v1/payment/info").await,
        [json!({"uuid": format!("hk-{polled}")})]
    );

    let mut conn = h.services().store().conn().await.unwrap();
    for (public, expected) in [
        (paid, PaymentStatus::Paid),
        (over, PaymentStatus::Paid),
        (under, PaymentStatus::Failed),
        (foreign, PaymentStatus::Pending),
        (polled, PaymentStatus::Paid),
    ] {
        assert_eq!(
            conn.payments().by_public_id(public).await.unwrap().status,
            expected,
            "{public}"
        );
    }
    let over_payment = conn.payments().by_public_id(over).await.unwrap();
    let notices = conn
        .payment_notices()
        .for_payment(over_payment.id)
        .await
        .unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].kind, NoticeKind::ManualCredit);
    assert!(notices[0].text.contains("$9.50"), "{}", notices[0].text);
    let under_payment = conn.payments().by_public_id(under).await.unwrap();
    let notices = conn
        .payment_notices()
        .for_payment(under_payment.id)
        .await
        .unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].kind, NoticeKind::ManualCredit);
    assert!(
        notices[0].text.contains("$2.10") && notices[0].text.contains("RUB"),
        "{}",
        notices[0].text
    );
    let journal: Vec<InboxOutcome> = conn
        .webhooks()
        .recent(20)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .map(|entry| entry.outcome)
        .collect();
    assert_eq!(
        journal,
        vec![
            InboxOutcome::Accepted,
            InboxOutcome::Accepted,
            InboxOutcome::Accepted,
            InboxOutcome::AmountMismatch,
        ]
    );
    drop(conn);
    h.db.cleanup().await;
}

#[tokio::test]
async fn heleket_в_долларах_работает_как_раньше() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .and(header("merchant", MERCHANT))
        .respond_with(Echo)
        .mount(&api)
        .await;
    let Some(h) = heleket_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    connect(&h, &owner).await;
    let client = h.client(5_093_118_902).await;
    let token = h.client_token(client).await;
    let paid = top_up(&h, &token, "5").await;
    let over = top_up(&h, &token, "7").await;
    let created = sent_bodies(&api, "/v1/payment").await;
    assert_eq!(created[0]["amount"], "5.00");
    assert_eq!(created[0]["currency"], "USD");
    assert_eq!(
        notify(&h, signed(&event(paid, "paid", "5.00", "USD", "5.00"))).await,
        StatusCode::OK
    );
    assert_eq!(
        notify(&h, signed(&event(over, "paid_over", "7.00", "USD", "7.80"))).await,
        StatusCode::OK
    );
    assert_eq!(
        balance(&h, &token).await,
        json!({"minor": 1_280, "currency": "USD"})
    );
    let mut conn = h.services().store().conn().await.unwrap();
    let over_payment = conn.payments().by_public_id(over).await.unwrap();
    assert!(
        conn.payment_notices()
            .for_payment(over_payment.id)
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    h.db.cleanup().await;
}

async fn staff_with(h: &Http, email: &str, extra: &[Permission]) -> String {
    let hash = sn_services::auth::hash_password(PASSWORD).unwrap();
    {
        let mut conn = h.services().store().conn().await.unwrap();
        let created = conn
            .admins()
            .create(&NewAdmin {
                email: email.into(),
                password_hash: hash,
                role: AdminRole::ReadOnly,
                must_change_password: false,
            })
            .await
            .unwrap();
        conn.admins()
            .set_role(created.id, AdminRole::ReadOnly, extra)
            .await
            .unwrap();
    }
    h.login(email).await
}

async fn key_with(h: &Http, owner: &str, permissions: Value) -> String {
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/api-keys",
            owner,
            json!({ "name": "Интеграция", "permissions": permissions, "expires_in_days": 30, "password": PASSWORD }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    created["token"].as_str().unwrap().to_string()
}

async fn save(h: &Http, token: &str, body: Value) -> (StatusCode, Value) {
    h.write(
        Method::PUT,
        "/api/admin/payments/providers/heleket",
        token,
        body,
    )
    .await
}

fn stored_address(h: &Http) -> Option<String> {
    h.services()
        .payments()
        .config("heleket")
        .get("api_url")
        .map(str::to_string)
}

#[tokio::test]
async fn адрес_api_heleket_меняют_только_с_правами_настройки_и_деньги() {
    let api = MockServer::start().await;
    let Some(h) = heleket_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    connect(&h, &owner).await;
    let settings_only = staff_with(&h, "setup@example.com", &[Permission::Settings]).await;
    let cashier = staff_with(
        &h,
        "cash@example.com",
        &[Permission::Settings, Permission::Money],
    )
    .await;
    let anna = h.admin("anna@example.com", AdminRole::Admin).await;
    let settings_key = key_with(&h, &owner, json!(["settings"])).await;
    let money_key = key_with(&h, &owner, json!(["settings", "money"])).await;

    for token in [&settings_only, &settings_key] {
        let (status, body) = save(
            &h,
            token,
            json!({"config": {"api_url": "https://relay.example.com", "api_key": "hk-relay"}}),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "settings.field_forbidden", "{body}");
        let text = body["error"]["message"].as_str().unwrap();
        assert!(
            text.contains("«Адрес API»") && text.contains("«Деньги»"),
            "{text}"
        );
    }
    assert_eq!(stored_address(&h), None);
    assert_eq!(
        h.services().payments().config("heleket").get("api_key"),
        Some(KEY)
    );
    let (status, body) = save(
        &h,
        &settings_only,
        json!({"config": {"merchant_id": "6f1a3c1e-2b4d-4e8f-9a0b-1c2d3e4f5a6b"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = save(
        &h,
        &settings_only,
        json!({"config": {"api_url": "https://api.heleket.hk/"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"]["own_api_url"], Value::Null);

    for wrong in [
        "http://relay.example.com",
        "https://relay.example.com/v1",
        "https://user:secret@relay.example.com",
    ] {
        let (status, body) = save(
            &h,
            &owner,
            json!({"config": {"api_url": wrong, "api_key": "hk-relay"}}),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{wrong}: {body}");
        assert_eq!(body["error"]["code"], "settings.invalid_field", "{body}");
    }

    let (status, body) = save(
        &h,
        &cashier,
        json!({"config": {"api_url": "https://Relay.example.com/", "api_key": "hk-relay"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"]["own_api_url"], "https://relay.example.com");
    assert_eq!(
        body["provider"]["settings"]["api_url"],
        "https://relay.example.com"
    );
    assert_eq!(
        listed(&h, &owner).await["own_api_url"],
        "https://relay.example.com"
    );
    assert_eq!(
        h.services()
            .payments()
            .get("heleket")
            .unwrap()
            .api_url(&h.services().payments().config("heleket")),
        Some(api.uri())
    );

    let (status, body) = save(
        &h,
        &settings_only,
        json!({"clear": ["api_url"], "config": {"api_key": "hk-back"}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "settings.field_forbidden");
    let (status, body) = save(
        &h,
        &money_key,
        json!({"clear": ["api_url"], "config": {"api_key": "hk-back"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"]["own_api_url"], Value::Null);
    assert_eq!(stored_address(&h), None);
    let (status, body) = save(
        &h,
        &anna,
        json!({"config": {"api_url": "https://relay2.example.com", "api_key": "hk-anna"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        stored_address(&h).as_deref(),
        Some("https://relay2.example.com")
    );

    let (status, page) = h
        .get("/api/admin/audit?action=settings.provider", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let moves: Vec<(String, String, String)> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| {
            let change = item["changes"]
                .as_array()?
                .iter()
                .find(|c| c["label"] == "Адрес API")?;
            Some((
                item["actor"]["email"]
                    .as_str()
                    .or(item["actor"]["name"].as_str())
                    .unwrap_or_default()
                    .to_string(),
                change["before"].as_str()?.to_string(),
                change["after"].as_str()?.to_string(),
            ))
        })
        .collect();
    assert_eq!(
        moves,
        [
            (
                "anna@example.com".to_string(),
                STANDARD.to_string(),
                "https://relay2.example.com".to_string()
            ),
            (
                "Интеграция".to_string(),
                "https://relay.example.com".to_string(),
                STANDARD.to_string()
            ),
            (
                "cash@example.com".to_string(),
                STANDARD.to_string(),
                "https://relay.example.com".to_string()
            ),
        ]
    );
    let item = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["actor"]["email"] == "cash@example.com")
        .unwrap();
    assert_eq!(item["target"]["label"], "Платёжка «Heleket»");
    h.db.cleanup().await;
}

#[tokio::test]
async fn heleket_встаёт_в_порядок_оплаты_в_обеих_валютах() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let order = |list: Value| json!({ "order": list });
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/payments",
            &owner,
            order(json!(["platega"])),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "settings.invalid_field");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("не работает с USD"),
        "{body}"
    );
    for currency in ["USD", "RUB"] {
        let (status, body) = h
            .write(
                Method::PUT,
                "/api/admin/settings/core",
                &owner,
                json!({ "currency": currency }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = h
            .write(
                Method::PUT,
                "/api/admin/settings/payments",
                &owner,
                order(json!(["heleket", "cryptomus"])),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{currency}: {body}");
        assert_eq!(body["currency"], currency);
        assert_eq!(body["order"], json!(["heleket", "cryptomus"]));
    }
    h.db.cleanup().await;
}
