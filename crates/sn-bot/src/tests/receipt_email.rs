use super::*;
use sn_domain::client::NewIdentity;
use sn_domain::payment::providers;
use sn_domain::settings::{keys, secret_keys};
use sn_domain::tariff::Tariff;
use sn_payments::providers::{Balance, Lava};
use std::sync::atomic::{AtomicU32, Ordering};
use wiremock::matchers::path;
use wiremock::{Request, Respond};

const OFFER: &str = "836b9fc5-7ae9-4a27-9642-592bc44072b7";

struct Invoices(AtomicU32);

impl Respond for Invoices {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let n = self.0.fetch_add(1, Ordering::SeqCst);
        ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": format!("7ea82675-4ded-4133-95a7-{n:012}"),
            "status": "in-progress",
            "amountTotal": {"currency": "USD", "amount": 2.99},
            "paymentUrl": format!("https://app.lava.top/payment/{n}")
        }))
    }
}

async fn lava_api() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/invoice"))
        .respond_with(Invoices(AtomicU32::new(1)))
        .mount(&server)
        .await;
    server
}

async fn invoices(lava: &MockServer) -> Vec<serde_json::Value> {
    lava.received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path() == "/api/v3/invoice")
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

async fn lava_bot(lava: &MockServer) -> Option<(TestDb, Services, MockServer, BotCtx)> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Lava::with_api_base(lava.uri())),
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
        let mut conn = services.store().conn().await.unwrap();
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
                &keys::provider(providers::LAVA),
                &serde_json::json!({
                    "enabled": true,
                    "config": {"offer_id": OFFER},
                    "methods": [{"code": "card"}]
                }),
                None,
            )
            .await
            .unwrap();
        for (field, value) in [
            ("api_key", "lava-api-key"),
            ("webhook_key", "lava-webhook-key-123"),
        ] {
            sn_services::settings::write_secret(
                &mut conn,
                services.secrets(),
                &secret_keys::provider(providers::LAVA, field),
                Some(value),
                None,
            )
            .await
            .unwrap();
        }
    }
    services.reload().await.unwrap();
    let remna = Arc::new(sn_remnawave::fake::FakeRemnawave::new());
    remna.set_squads(vec![sn_remnawave::InternalSquad {
        uuid: uuid::Uuid::from_u128(1),
        name: "Германия".into(),
        view_position: 0,
        info: sn_remnawave::SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    services.set_remna(Some(remna));
    let server = telegram().await;
    let ctx = BotCtx {
        services: services.clone(),
        api: Arc::new(TelegramApi::new(&server.uri(), TOKEN, None)),
        username: Some("stealth_test_bot".into()),
    };
    Some((db, services, server, ctx))
}

async fn standard(db: &TestDb) -> Tariff {
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: "Обычный".into(),
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
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(19_900),
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

async fn to_summary(ctx: &BotCtx, chat: i64, tariff: &Tariff, from: i64) {
    handle_update(ctx, text_update(from, chat, "/start")).await;
    handle_update(ctx, callback_update(from + 1, chat, "buy")).await;
    handle_update(
        ctx,
        callback_update(from + 2, chat, &format!("t:{}", tariff.id)),
    )
    .await;
    handle_update(
        ctx,
        callback_update(from + 3, chat, &format!("p:{}", tariff.periods[0].id)),
    )
    .await;
}

async fn waiting(services: &Services, chat: i64) -> Option<Awaiting> {
    Session::load(services, chat).await.unwrap().awaiting
}

async fn receipt_of(services: &Services, chat: i64) -> Option<String> {
    let client = client_of(services, chat).await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    conn.clients().receipt_email(client.id).await.unwrap()
}

#[tokio::test]
async fn бот_спрашивает_почту_для_чека_и_продолжает_ту_же_оплату() {
    let lava = lava_api().await;
    let Some((db, services, server, ctx)) = lava_bot(&lava).await else {
        return;
    };
    let tariff = standard(&db).await;
    let chat = 7101;
    to_summary(&ctx, chat, &tariff, 1).await;
    assert!(
        last_buttons(&server)
            .await
            .contains(&"pay:lava:card".to_string())
    );
    assert!(!last_text(&server).await.contains("Чек придёт"));

    handle_update(&ctx, callback_update(5, chat, "pay:lava:card")).await;
    let ask = last_text(&server).await;
    assert!(
        ask.contains("Для оплаты через Lava.top нужна почта"),
        "{ask}"
    );
    assert_eq!(last_buttons(&server).await, vec!["sum".to_string()]);
    assert_eq!(waiting(&services, chat).await, Some(Awaiting::ReceiptEmail));
    assert!(
        invoices(&lava).await.is_empty(),
        "счёт до почты не выставляется"
    );

    handle_update(&ctx, text_update(6, chat, "почта@")).await;
    assert!(last_text(&server).await.contains("опечатка"));
    assert_eq!(waiting(&services, chat).await, Some(Awaiting::ReceiptEmail));
    assert!(invoices(&lava).await.is_empty());

    handle_update(&ctx, text_update(7, chat, " Anna.K@Example.com ")).await;
    let sent = invoices(&lava).await;
    assert_eq!(sent.len(), 1, "после почты выставлен ровно один счёт");
    assert_eq!(sent[0]["email"], "anna.k@example.com");
    assert_eq!(sent[0]["amount"], 2.99, "та же оплата: тот же тариф и срок");
    assert_eq!(
        receipt_of(&services, chat).await.as_deref(),
        Some("anna.k@example.com")
    );
    assert_eq!(waiting(&services, chat).await, None);
    let client = client_of(&services, chat).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        let orders = conn
            .orders()
            .for_client(client.id, Default::default())
            .await
            .unwrap();
        assert_eq!(orders.items.len(), 1);
        let audit: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT details FROM audit_log WHERE action = 'client.receipt_email' AND entity_id = $1",
        )
        .bind(client.id.0)
        .fetch_all(conn.conn())
        .await
        .unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["email"], "a***@example.com", "в журнале — маской");
    }

    let card = services.clients().card(client.id).await.unwrap();
    assert_eq!(card.receipt_email.as_deref(), Some("anna.k@example.com"));

    handle_update(&ctx, callback_update(8, chat, "buy")).await;
    handle_update(&ctx, callback_update(9, chat, &format!("t:{}", tariff.id))).await;
    handle_update(
        &ctx,
        callback_update(10, chat, &format!("p:{}", tariff.periods[0].id)),
    )
    .await;
    let summary = last_text(&server).await;
    assert!(
        summary.contains("Чек придёт на a***@example.com"),
        "{summary}"
    );
    assert!(
        last_buttons(&server)
            .await
            .contains(&"rcpt:sum".to_string())
    );

    handle_update(&ctx, callback_update(11, chat, "rcpt:sum")).await;
    assert!(last_text(&server).await.contains("новую почту"));
    handle_update(&ctx, text_update(12, chat, "new@example.com")).await;
    assert!(
        last_text(&server)
            .await
            .contains("Чек придёт на n***@example.com")
    );
    assert_eq!(
        receipt_of(&services, chat).await.as_deref(),
        Some("new@example.com")
    );
    assert_eq!(invoices(&lava).await.len(), 1);

    handle_update(&ctx, callback_update(13, chat, "pay:lava:card")).await;
    let sent = invoices(&lava).await;
    assert_eq!(sent.len(), 2, "второй раз почту не спрашивает");
    assert_eq!(sent[1]["email"], "new@example.com");
    assert_eq!(waiting(&services, chat).await, None);
    db.cleanup().await;
}

#[tokio::test]
async fn отмена_и_команда_вместо_почты_сбрасывают_ожидание() {
    let lava = lava_api().await;
    let Some((db, services, server, ctx)) = lava_bot(&lava).await else {
        return;
    };
    let tariff = standard(&db).await;
    let chat = 7102;
    to_summary(&ctx, chat, &tariff, 1).await;
    handle_update(&ctx, callback_update(5, chat, "pay:lava:card")).await;
    assert_eq!(waiting(&services, chat).await, Some(Awaiting::ReceiptEmail));
    handle_update(&ctx, callback_update(6, chat, "sum")).await;
    assert_eq!(waiting(&services, chat).await, None);
    assert!(last_text(&server).await.contains("К оплате"));

    handle_update(&ctx, callback_update(7, chat, "pay:lava:card")).await;
    assert_eq!(waiting(&services, chat).await, Some(Awaiting::ReceiptEmail));
    handle_update(&ctx, text_update(8, chat, "/language")).await;
    assert_eq!(waiting(&services, chat).await, None);
    handle_update(&ctx, text_update(9, chat, "after@example.com")).await;
    assert_eq!(receipt_of(&services, chat).await, None);
    assert!(invoices(&lava).await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn клиент_с_почтой_входа_платит_без_вопроса() {
    let lava = lava_api().await;
    let Some((db, services, server, ctx)) = lava_bot(&lava).await else {
        return;
    };
    let tariff = standard(&db).await;
    let chat = 7103;
    handle_update(&ctx, text_update(1, chat, "/start")).await;
    let client = client_of(&services, chat).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .add_identity(
                client.id,
                &NewIdentity::new(IdentityKind::Email, "login@example.com", true).unwrap(),
            )
            .await
            .unwrap();
    }
    handle_update(&ctx, callback_update(2, chat, "buy")).await;
    handle_update(&ctx, callback_update(3, chat, &format!("t:{}", tariff.id))).await;
    handle_update(
        &ctx,
        callback_update(4, chat, &format!("p:{}", tariff.periods[0].id)),
    )
    .await;
    assert!(!last_text(&server).await.contains("Чек придёт"));
    handle_update(&ctx, callback_update(5, chat, "pay:lava:card")).await;
    let sent = invoices(&lava).await;
    assert_eq!(sent.len(), 1, "{}", last_text(&server).await);
    assert_eq!(sent[0]["email"], "login@example.com");
    assert_eq!(waiting(&services, chat).await, None);
    assert_eq!(receipt_of(&services, chat).await, None);
    db.cleanup().await;
}

#[tokio::test]
async fn юkassa_спрашивает_почту_только_с_чеками() {
    let Some(db) = test_store().await else {
        return;
    };
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(sn_payments::providers::YooKassa::new()),
            Arc::new(Balance),
        ])),
        modules: Arc::new(ModuleRegistry::new(catalog::all()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    let set = |config: serde_json::Value| {
        let services = services.clone();
        async move {
            let mut conn = services.store().conn().await.unwrap();
            conn.settings()
                .set(
                    &keys::provider(providers::YOOKASSA),
                    &serde_json::json!({"enabled": true, "config": config}),
                    None,
                )
                .await
                .unwrap();
            drop(conn);
            services.reload().await.unwrap();
        }
    };
    let (client, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 7104,
                username: None,
                first_name: Some("Аня".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap();
    let needed = |services: &Services| {
        let services = services.clone();
        async move {
            services
                .checkout()
                .email_needed(client.id, providers::YOOKASSA, Some("bank_card"))
                .await
                .unwrap()
        }
    };
    set(serde_json::json!({"shop_id": "504367"})).await;
    assert!(!needed(&services).await, "без чеков почта не нужна");
    set(serde_json::json!({"shop_id": "504367", "vat_code": "1"})).await;
    assert!(needed(&services).await, "с чеками без почты спрашиваем");
    services
        .clients()
        .set_receipt_email(client.id, "kassa@example.com", sn_services::Actor::System)
        .await
        .unwrap();
    assert!(!needed(&services).await, "почта уже есть — не спрашиваем");
    db.cleanup().await;
}
