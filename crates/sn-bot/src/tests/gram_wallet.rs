use super::*;
use crate::notify::NotifyHandler;
use sn_domain::payment::{PaymentStatus, providers};
use sn_domain::settings::keys;
use sn_payments::providers::{Balance, Gram, GramInvoice, comment_payload};
use sn_services::checkout::{CartRequest, Channel, CheckoutRequest, CheckoutResult};
use wiremock::matchers::path;

const WALLET: &str = "UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI";

async fn gram_setup(chain: &MockServer) -> Option<(TestDb, Services, MockServer, BotCtx)> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Gram::with_endpoints(chain.uri(), chain.uri(), chain.uri())),
            Arc::new(Balance),
        ])),
        modules: Arc::new(ModuleRegistry::new(catalog::all()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks {
            bot_token: Some(TOKEN.into()),
            ..EnvFallbacks::default()
        },
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                keys::PUBLIC_URL,
                &serde_json::json!("https://panel.example.com"),
                None,
            )
            .await
            .unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::GRAM),
                &serde_json::json!({"enabled": true, "config": {"wallet": WALLET, "rate": "5"}}),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let server = telegram().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendWidget$"))
        .respond_with(ok(serde_json::json!({
            "message_id": 101, "chat": {"id": 1, "type": "private"}, "date": 0
        })))
        .mount(&server)
        .await;
    for name in ["answerWidgetQuery", "editMessageWidgetData"] {
        Mock::given(method("POST"))
            .and(path_regex(format!("/{name}$")))
            .respond_with(ok(serde_json::json!(true)))
            .mount(&server)
            .await;
    }
    let ctx = BotCtx {
        services: services.clone(),
        api: Arc::new(TelegramApi::new(&server.uri(), TOKEN, None)),
        username: Some("stealth_test_bot".into()),
    };
    Some((db, services, server, ctx))
}

async fn tariff(db: &TestDb) -> sn_domain::tariff::Tariff {
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: "Основные".into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    let tariff = tx
        .tariffs()
        .create(&NewTariff {
            traffic_addon: None,
            category_id: category.id,
            name: "Стандарт".into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![sn_domain::ids::SquadId(uuid::Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 90,
                prices: CurrencyPrices {
                    usd: Some(699),
                    rub: Some(49_900),
                },
                stars: None,
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tariff
}

async fn calls(server: &MockServer, name: &str) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with(&format!("/{name}")))
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

fn widget_query(update_id: i64, from: i64, kind: serde_json::Value, data: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "widget_query": {
            "id": format!("wq{update_id}"),
            "from": user(from),
            "chat": {"id": from, "type": "private"},
            "message_id": 101,
            "chat_instance": "ci",
            "type": kind,
            "data": data
        }
    }))
    .unwrap()
}

async fn answer(server: &MockServer, id: &str) -> serde_json::Value {
    let body = calls(server, "answerWidgetQuery")
        .await
        .into_iter()
        .rev()
        .find(|b| b["widget_query_id"] == id)
        .unwrap();
    serde_json::from_str(body["data"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn если_telegram_не_принял_виджет_бот_даёт_ссылку_на_оплату() {
    let chain = MockServer::start().await;
    let Some((db, services, server, ctx)) = gram_setup(&chain).await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendWidget$"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: method not found"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    let tariff = tariff(&db).await;
    handle_update(&ctx, text_update(1, 507, "/start")).await;
    handle_update(&ctx, callback_update(2, 507, "buy")).await;
    handle_update(&ctx, callback_update(3, 507, &format!("t:{}", tariff.id))).await;
    let period = tariff.periods[0].id;
    handle_update(&ctx, callback_update(4, 507, &format!("p:{period}"))).await;
    services.set_remna(Some(Arc::new(sn_remnawave::fake::FakeRemnawave::new())));
    handle_update(&ctx, callback_update(5, 507, "pay:gram")).await;
    assert_eq!(calls(&server, "sendWidget").await.len(), 1);
    let ready = last_text(&server).await;
    assert!(ready.contains("Счёт готов: нажмите «Оплатить»"), "{ready}");
    assert!(ready.starts_with("Стандарт, 3\u{a0}месяца.\n"), "{ready}");
    assert!(last_labels(&server).await.contains(&"Оплатить".to_string()));
    db.cleanup().await;
}

#[tokio::test]
async fn оплата_gram_кошельком_telegram_из_бота() {
    let chain = MockServer::start().await;
    let Some((db, services, server, ctx)) = gram_setup(&chain).await else {
        return;
    };
    let tariff = tariff(&db).await;
    handle_update(&ctx, text_update(1, 505, "/start")).await;
    handle_update(&ctx, callback_update(2, 505, "buy")).await;
    handle_update(&ctx, callback_update(3, 505, &format!("t:{}", tariff.id))).await;
    let period = tariff.periods[0].id;
    handle_update(&ctx, callback_update(4, 505, &format!("p:{period}"))).await;
    services.set_remna(Some(Arc::new(sn_remnawave::fake::FakeRemnawave::new())));
    handle_update(&ctx, callback_update(5, 505, "pay:gram")).await;
    let ready = last_text(&server).await;
    assert!(
        ready.contains("оплатите его в сообщении с кнопкой"),
        "{ready}"
    );

    let widgets = calls(&server, "sendWidget").await;
    assert_eq!(widgets.len(), 1);
    let widget = &widgets[0];
    assert_eq!(widget["chat_id"], 505);
    assert_eq!(widget["allow_queries"], true);
    assert!(
        widget["html"]
            .as_str()
            .unwrap()
            .contains("Оплатить в Telegram")
    );
    assert!(
        widget["caption"]
            .as_str()
            .unwrap()
            .contains("К оплате 1.4 GRAM"),
        "{widget}"
    );
    let data: serde_json::Value =
        serde_json::from_str(widget["chat_data"].as_str().unwrap()).unwrap();
    assert_eq!(data["a"], "1.4 GRAM");
    assert_eq!(data["f"], "≈ $6.99");
    let id = data["id"].as_str().unwrap().to_string();
    let other = &widget["reply_markup"]["inline_keyboard"][0][0];
    assert_eq!(other["text"], "Другой кошелёк");
    let public_id = uuid::Uuid::parse_str(&id).unwrap();
    assert_eq!(
        other["url"],
        format!("https://panel.example.com/pay/{public_id}")
    );
    let payment = {
        let mut conn = db.store.conn().await.unwrap();
        conn.payments().by_public_id(public_id).await.unwrap()
    };
    let invoice = GramInvoice::parse(payment.provider_txid.as_deref().unwrap()).unwrap();

    handle_update(
        &ctx,
        widget_query(
            6,
            505,
            serde_json::json!({"type": "data"}),
            &format!("p:{id}"),
        ),
    )
    .await;
    let tx = answer(&server, "wq6").await;
    assert_eq!(tx["messages"][0]["address"], WALLET);
    assert_eq!(tx["messages"][0]["amount"], "1400000000");
    assert_eq!(
        tx["messages"][0]["payload"],
        comment_payload(&invoice.comment).unwrap()
    );
    let valid_until = tx["valid_until"].as_i64().unwrap();
    let now = chrono::Utc::now().timestamp();
    assert!(valid_until > now && valid_until <= now + 300, "{tx}");

    handle_update(
        &ctx,
        widget_query(
            7,
            777,
            serde_json::json!({"type": "data"}),
            &format!("p:{id}"),
        ),
    )
    .await;
    assert_eq!(answer(&server, "wq7").await["error"], "Счёт не найден.");

    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": [{
                "transaction_id": {"lt": "7", "hash": "Z3JhbQ=="},
                "in_msg": {"source": "EQSender", "value": "1400000000", "message": invoice.comment}
            }]
        })))
        .mount(&chain)
        .await;
    handle_update(
        &ctx,
        widget_query(
            8,
            505,
            serde_json::json!({
                "type": "sent_transaction",
                "wallet": {"address": "UQSender", "amount": "5000000000"},
                "msg_hash": "aGFzaA=="
            }),
            &format!("s:{id}"),
        ),
    )
    .await;
    assert_eq!(answer(&server, "wq8").await["status"], "paid");
    let states: Vec<String> = calls(&server, "editMessageWidgetData")
        .await
        .into_iter()
        .map(|b| {
            assert_eq!(b["message_id"], 101);
            let data: serde_json::Value =
                serde_json::from_str(b["chat_data"].as_str().unwrap()).unwrap();
            data["s"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(states, ["sent", "paid"]);
    let paid = {
        let mut conn = db.store.conn().await.unwrap();
        conn.payments().get(payment.id).await.unwrap()
    };
    assert_eq!(paid.status, PaymentStatus::Paid);

    handle_update(
        &ctx,
        widget_query(
            9,
            505,
            serde_json::json!({"type": "data"}),
            &format!("p:{id}"),
        ),
    )
    .await;
    assert_eq!(
        answer(&server, "wq9").await["done"],
        "Этот счёт уже оплачен."
    );
    db.cleanup().await;
}

#[tokio::test]
async fn счёт_gram_из_мини_приложения_приходит_виджетом_в_чат() {
    let chain = MockServer::start().await;
    let Some((db, services, server, ctx)) = gram_setup(&chain).await else {
        return;
    };
    let tariff = tariff(&db).await;
    handle_update(&ctx, text_update(1, 506, "/start")).await;
    let client = client_of(&services, 506).await.unwrap();
    services.set_remna(Some(Arc::new(sn_remnawave::fake::FakeRemnawave::new())));
    let result = services
        .checkout()
        .start(
            client.id,
            &CheckoutRequest {
                cart: CartRequest {
                    tariff_id: tariff.id,
                    period_id: tariff.periods[0].id,
                    extra_devices: 0,
                    subscription_id: None,
                    promo_code: None,
                },
                provider: providers::GRAM.into(),
                method: None,
                channel: Channel::Miniapp,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт: {result:?}");
    };
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"stealth_test_bot", None)
            .await
            .unwrap();
    }
    let url = services
        .checkout()
        .gram_in_telegram(client.id, payment_id)
        .await
        .unwrap();
    assert_eq!(url, "https://t.me/stealth_test_bot");
    let handler = NotifyHandler::new(&BotConfig {
        api_base: server.uri(),
        ..BotConfig::default()
    });
    let done = notify(
        &services,
        &handler,
        client.id,
        crate::gram_wallet::TEMPLATE,
        serde_json::json!({ "payment_id": payment_id }),
    )
    .await;
    assert_eq!(done, JobResult::Done);
    let widgets = calls(&server, "sendWidget").await;
    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0]["chat_id"], 506);
    assert!(
        widgets[0]["caption"]
            .as_str()
            .unwrap()
            .starts_with("Стандарт, 3\u{a0}месяца."),
        "{}",
        widgets[0]
    );
    let stranger = handle_stranger(&services).await;
    let refused = services
        .checkout()
        .gram_in_telegram(stranger, payment_id)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "not_found");
    db.cleanup().await;
}

async fn handle_stranger(services: &Services) -> ClientId {
    let (client, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 909,
                username: None,
                first_name: Some("Гость".into()),
                last_name: None,
                language_code: None,
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap();
    client.id
}
