use super::*;
use sn_domain::payment::PaymentStatus;
use sn_payments::providers::{Balance, RollyPay, rollypay_signature};
use sn_store::InboxOutcome;
use wiremock::matchers::{header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

const KEY: &str = "rpk_live_e2e_9c1f";
const SECRET: &str = "rp_e2e_signing_secret";

struct Echo;

impl Respond for Echo {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let order = body["order_id"].as_str().unwrap_or_default().to_string();
        ResponseTemplate::new(200).set_body_json(json!({
            "payment_id": format!("pay_{order}"),
            "order_id": order,
            "status": "created",
            "pay_url": format!("https://pay.rollypay.io/pay/tok_{}", order.replace('-', "")),
            "amount": body["amount"],
            "payment_currency": "RUB",
            "created_at": "2026-11-16T14:52:00Z",
            "expires_at": "2026-11-16T15:22:00Z"
        }))
    }
}

async fn rolly_http(api: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(RollyPay::with_api_base(api.uri())),
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

async fn notify(h: &Http, raw: &str, sent: i64, secret: &str) -> StatusCode {
    let sent = sent.to_string();
    let request = Request::post("/api/webhooks/pay/rollypay")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Timestamp", sent.as_str())
        .header(
            "X-Signature",
            rollypay_signature(secret, &sent, raw.as_bytes()),
        )
        .body(Body::from(raw.to_string()))
        .unwrap();
    router(h.state.clone())
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

fn event(kind: &str, public: Uuid, status: &str, amount: &str) -> String {
    json!({
        "event_type": kind,
        "payment_id": format!("pay_{public}"),
        "order_id": public.to_string(),
        "status": status,
        "amount": amount,
        "currency": "RUB",
        "test": false
    })
    .to_string()
}

async fn top_up(h: &Http, token: &str, amount: &str) -> Uuid {
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            token,
            json!({"amount": amount, "provider": "rollypay"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "redirect");
    let public: Uuid = body["payment_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        body["pay_url"],
        format!("https://panel.example.com/api/pay/{public}")
    );
    public
}

async fn balance(h: &Http, token: &str) -> Value {
    let (status, options) = h.get("/api/client/topup", token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    options["balance"].clone()
}

#[tokio::test]
async fn rollypay_счёт_уведомление_повтор_чужая_сумма_и_возврат() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .and(header("X-API-Key", KEY))
        .and(header_exists("X-Nonce"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "terminal_id": "d290f1ee-6c54-4b01-90e6-d701748f0851",
            "available_usdt": "0.00", "hold_usdt": "0.00"
        })))
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payments"))
        .and(header("X-API-Key", KEY))
        .and(header_exists("X-Nonce"))
        .respond_with(Echo)
        .mount(&api)
        .await;
    let Some(h) = rolly_http(&api).await else {
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
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/rollypay",
            &owner,
            json!({"enabled": true, "config": {"api_key": KEY, "signing_secret": SECRET}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    let listed = list.to_string();
    assert!(
        !listed.contains(KEY) && !listed.contains(SECRET),
        "{listed}"
    );

    let client = h.client(5_093_118_601).await;
    let token = h.client_token(client).await;
    let (status, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["methods"],
        json!([{"provider": "rollypay", "method": null, "title": "СБП", "hint": "Через приложение банка"}])
    );
    let first = top_up(&h, &token, "500").await;
    let second = top_up(&h, &token, "700").await;
    {
        let mut conn = h.services().store().conn().await.unwrap();
        let payment = conn.payments().by_public_id(first).await.unwrap();
        assert_eq!(
            payment.provider_txid.as_deref(),
            Some(format!("pay_{first}").as_str())
        );
        assert_eq!(
            payment.pay_url.as_deref(),
            Some(format!("https://pay.rollypay.io/pay/tok_{}", first.simple()).as_str())
        );
    }
    let created: Vec<Value> = api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/api/v1/payments")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(created.len(), 2);
    assert_eq!(created[0]["amount"], "500.00");
    assert_eq!(created[0]["payment_currency"], "RUB");
    assert_eq!(created[0]["order_id"], first.to_string());
    assert_eq!(created[1]["amount"], "700.00");

    let now = Utc::now().timestamp();
    let paid = event("payment.paid", first, "paid", "500.00");
    assert_eq!(
        notify(&h, &format!("{paid}\n"), now, "guessed-secret").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        notify(&h, &paid, now - 20 * 60, SECRET).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(balance(&h, &token).await["minor"], 0);
    for _ in 0..2 {
        assert_eq!(notify(&h, &paid, now, SECRET).await, StatusCode::OK);
    }
    assert_eq!(
        balance(&h, &token).await,
        json!({"minor": 50_000, "currency": "RUB"})
    );
    let wrong = event("payment.paid", second, "paid", "699.00");
    assert_eq!(notify(&h, &wrong, now, SECRET).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await["minor"], 50_000);
    for kind in ["refund_request.completed", "payment.refunded"] {
        let refunded = event(kind, first, "refunded", "500.00");
        assert_eq!(notify(&h, &refunded, now, SECRET).await, StatusCode::OK);
    }
    assert_eq!(balance(&h, &token).await["minor"], 0);

    let mut conn = h.services().store().conn().await.unwrap();
    let refunded = conn.payments().by_public_id(first).await.unwrap();
    assert_eq!(refunded.status, PaymentStatus::Refunded);
    let refunds = conn.refunds().for_payment(refunded.id).await.unwrap();
    assert_eq!(refunds.len(), 1);
    assert_eq!(refunds[0].amount.minor, 50_000);
    assert_eq!(
        refunds[0].provider_refund_id.as_deref(),
        Some(format!("refund:pay_{first}").as_str())
    );
    assert_eq!(
        conn.payments().by_public_id(second).await.unwrap().status,
        PaymentStatus::Pending
    );
    let journal: Vec<(InboxOutcome, bool)> = conn
        .webhooks()
        .recent(20)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .map(|entry| (entry.outcome, entry.has_body))
        .collect();
    assert_eq!(
        journal,
        vec![
            (InboxOutcome::RejectedSignature, false),
            (InboxOutcome::RejectedPayload, false),
            (InboxOutcome::Accepted, true),
            (InboxOutcome::AmountMismatch, true),
            (InboxOutcome::Accepted, true),
            (InboxOutcome::Duplicate, true),
        ]
    );
    drop(conn);
    h.db.cleanup().await;
}

#[tokio::test]
async fn rollypay_id_кассы_из_настроек_уходит_в_счёт() {
    let terminal = "d290f1ee-6c54-4b01-90e6-d701748f0851";
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/balance"))
        .and(query_param("terminal_id", terminal))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "terminal_id": terminal, "available_usdt": "0.00", "hold_usdt": "0.00"
        })))
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/terminals/{terminal}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": terminal,
            "callback_url": "https://panel.example.com/api/webhooks/pay/rollypay"
        })))
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payments"))
        .and(header("X-API-Key", KEY))
        .respond_with(Echo)
        .mount(&api)
        .await;
    let Some(h) = rolly_http(&api).await else {
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
    let save = |terminal_id: &'static str| {
        json!({"enabled": true, "config": {
            "api_key": KEY, "signing_secret": SECRET, "terminal_id": terminal_id
        }})
    };
    let (status, refused) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/rollypay",
            &owner,
            save("касса-1"),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "settings.provider_invalid");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("UUID")),
        "{refused}"
    );
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/rollypay",
            &owner,
            save(terminal),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    let rolly = list["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "rollypay")
        .unwrap();
    assert_eq!(rolly["settings"]["terminal_id"], terminal);
    let (status, check) = h
        .write(
            Method::POST,
            "/api/admin/payments/providers/rollypay/check",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{check}");
    assert_eq!(check["status"], "ok", "{check}");
    assert_eq!(check["hooks"][0]["state"], "same", "{check}");

    let client = h.client(5_093_118_602).await;
    let token = h.client_token(client).await;
    let public = top_up(&h, &token, "500").await;
    let created: Vec<Value> = api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/api/v1/payments")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["terminal_id"], terminal);
    assert_eq!(created[0]["order_id"], public.to_string());
    h.db.cleanup().await;
}
