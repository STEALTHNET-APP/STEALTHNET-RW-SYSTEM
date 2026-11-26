use super::*;
use sn_payments::providers::{Balance, Gram, GramInvoice};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WALLET: &str = "UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI";

async fn gram_http(chain: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Gram::with_endpoints(chain.uri(), chain.uri(), chain.uri())),
            Arc::new(Balance),
        ])),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks {
            bot_token: Some("123456:TEST-token".into()),
            ..EnvFallbacks::default()
        },
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &json!("https://panel.example.com"), None)
            .await
            .unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"stealth_vpn_bot", None)
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

async fn gram_payment(
    h: &Http,
    client: ClientId,
    tariff: &Tariff,
    comment: &str,
) -> (sn_domain::payment::Payment, GramInvoice) {
    let period = &tariff.periods[0];
    let q = quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff,
        period_id: period.id,
        extra_devices: 0,
        sell_extra_devices: true,
        adjustments: &[],
        promotion: None,
    })
    .unwrap();
    let payment = {
        let mut tx = h.services().store().begin().await.unwrap();
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id: client,
                subscription_id: None,
                kind: OrderKind::New,
                intent: PurchaseIntent::Subscription {
                    tariff_id: tariff.id,
                    period_id: period.id,
                    extra_devices: 0,
                    target: None,
                },
                quote: q.clone(),
                snapshot: Some(tariff.snapshot(period)),
            })
            .await
            .unwrap();
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id: client,
                provider: ProviderId::from("gram"),
                method: None,
                amount: q.total.into(),
                expires_at: Some(start() + Duration::hours(1)),
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        payment
    };
    let invoice = GramInvoice {
        comment: comment.into(),
        nano: 1_400_000_000,
        address: WALLET.into(),
        amount: payment.amount,
    };
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.payments()
            .attach_invoice(
                payment.id,
                &sn_store::InvoiceUpdate {
                    pay_url: Some(format!(
                        "https://panel.example.com/pay/{}",
                        payment.public_id
                    )),
                    provider_txid: Some(invoice.external_id()),
                    expires_at: Some(start() + Duration::hours(1)),
                    payload: json!({}),
                },
            )
            .await
            .unwrap();
    }
    (payment, invoice)
}

#[tokio::test]
async fn страница_оплаты_gram_показывает_счёт_и_итог() {
    let chain = MockServer::start().await;
    let Some(h) = gram_http(&chain).await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": []})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&chain)
        .await;
    let tariff = h.tariff().await;
    let client = h.client(5_093_118_422).await;
    let other = h.pending_payment(client, &tariff).await;
    let (payment, _) = gram_payment(&h, client, &tariff, "SN-ABCDEFGH").await;
    let url = format!("/api/public/pay/{}", payment.public_id);
    let (status, _, body) = h.send(Method::GET, &url, None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["provider"], "gram");
    assert_eq!(body["amount"]["minor"], 699);
    assert_eq!(body["gram"]["amount"], "1.4");
    assert_eq!(body["gram"]["nano"], "1400000000");
    assert_eq!(body["gram"]["address"], WALLET);
    assert_eq!(body["gram"]["comment"], "SN-ABCDEFGH");
    assert_eq!(
        body["gram"]["ton_link"],
        format!("ton://transfer/{WALLET}?amount=1400000000&text=SN-ABCDEFGH")
    );
    assert_eq!(body["back"]["bot"], "https://t.me/stealth_vpn_bot");
    assert_eq!(body["back"]["cabinet"], "https://panel.example.com/cabinet");
    assert_eq!(body["locale"], "ru");
    assert!(body.get("client").is_none());
    let (status, _, again) = h.send(Method::GET, &url, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["status"], "pending");
    assert_eq!(chain.received_requests().await.unwrap().len(), 1);

    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": [{
                "transaction_id": {"lt": "7", "hash": "Z3JhbQ=="},
                "in_msg": {"source": "EQSender", "value": "1400000000", "message": "SN-ABCDEFGH"}
            }]
        })))
        .with_priority(2)
        .mount(&chain)
        .await;
    h.services().inbox().poll_payment(payment.id).await.unwrap();
    let (status, _, paid) = h.send(Method::GET, &url, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(paid["status"], "paid");

    let (status, _, _) = h
        .send(
            Method::GET,
            &format!("/api/public/pay/{}", other.public_id),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = h
        .send(
            Method::GET,
            &format!("/api/public/pay/{}", Uuid::from_u128(9)),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}

#[tokio::test]
async fn счёт_gram_отправляется_в_чат_с_ботом_для_кошелька_telegram() {
    let chain = MockServer::start().await;
    let Some(h) = gram_http(&chain).await else {
        return;
    };
    let tariff = h.tariff().await;
    let client = h.client(5_093_118_422).await;
    let (payment, _) = gram_payment(&h, client, &tariff, "SN-TELEGRAM").await;
    let token = h.client_token(client).await;
    let url = format!("/api/client/payments/{}/telegram", payment.public_id);
    let (status, body) = h.write(Method::POST, &url, &token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["chat_url"], "https://t.me/stealth_vpn_bot");
    let queued: Vec<String> = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.jobs()
            .claim(100, std::time::Duration::from_secs(30))
            .await
            .unwrap()
            .into_iter()
            .filter_map(|job| serde_json::to_string(&job.typed().ok()?).ok())
            .collect()
    };
    assert!(
        queued
            .iter()
            .any(|job| job.contains("gram.widget") && job.contains(&payment.public_id.to_string())),
        "{queued:?}"
    );

    let stranger = h.client(5_093_118_423).await;
    let other = h.client_token(stranger).await;
    let (status, _) = h.write(Method::POST, &url, &other, json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/client/payments/{}/telegram", Uuid::from_u128(9)),
            &token,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider("gram"),
                &json!({"enabled": true, "config": {"wallet": WALLET, "rate": "5", "testnet": true}}),
                None,
            )
            .await
            .unwrap();
    }
    h.services().reload().await.unwrap();
    let (status, refused) = h.write(Method::POST, &url, &token, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "gram.wallet_unavailable");
    h.db.cleanup().await;
}

#[tokio::test]
async fn кнопка_я_оплатил_проверяет_сразу_и_находит_поздний_перевод() {
    let chain = MockServer::start().await;
    let Some(h) = gram_http(&chain).await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": []})))
        .with_priority(5)
        .mount(&chain)
        .await;
    let tariff = h.tariff().await;
    let client = h.client(5_093_118_424).await;
    let (waiting, _) = gram_payment(&h, client, &tariff, "SN-WAITING").await;
    let check = |payment: &sn_domain::payment::Payment| {
        format!("/api/public/pay/{}/check", payment.public_id)
    };
    let (status, _, body) = h.send(Method::POST, &check(&waiting), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["checked"], "not_found");
    let (_, _, again) = h.send(Method::POST, &check(&waiting), None, None).await;
    assert_eq!(again["checked"], "too_often");

    let (late, invoice) = gram_payment(&h, client, &tariff, "SN-WLPDQ6GT").await;
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.payments()
            .attach_invoice(
                late.id,
                &sn_store::InvoiceUpdate {
                    pay_url: None,
                    provider_txid: None,
                    expires_at: Some(start() - Duration::minutes(1)),
                    payload: json!({}),
                },
            )
            .await
            .unwrap();
        conn.payments()
            .expire_stale(h.services().now(), 10)
            .await
            .unwrap();
        assert_eq!(
            conn.payments().get(late.id).await.unwrap().status,
            sn_domain::payment::PaymentStatus::Canceled
        );
    }
    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": [{
                "transaction_id": {"lt": "11", "hash": "bGF0ZQ=="},
                "in_msg": {"source": "EQDGrR9gCY0r", "value": "1400000000", "message": invoice.comment}
            }]
        })))
        .with_priority(1)
        .mount(&chain)
        .await;
    let (status, _, found) = h.send(Method::POST, &check(&late), None, None).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["checked"], "found");
    assert_eq!(found["status"], "paid");
    h.db.cleanup().await;
}
