use super::*;
use crate::handler::parse_command;
use crate::session::{Awaiting, Session};
use crate::telegram::Update;
use sn_domain::client::{ClientStatus, IdentityKind, RegistrationSource, TelegramProfile};
use sn_domain::ids::ClientId;
use sn_domain::jobs::JobPayload;
use sn_domain::money::Stars;
use sn_domain::tariff::{CurrencyPrices, DevicePolicy};
use sn_domain::traffic::TrafficPolicy;
use sn_modules::ModuleRegistry;
use sn_modules::catalog::promo::{NewPromo, NewPromoValue};
use sn_modules::catalog::{self, referral, utm};
use sn_payments::ProviderRegistry;
use sn_services::{EnvFallbacks, ServiceDeps, SystemClock};
use sn_store::testing::{TestDb, test_store};
use sn_store::{NewCategory, NewPeriod, NewTariff, SecretBox};
use sn_worker::{JobHandler, JobResult};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod addons;
mod alerts;
mod balance_off;
mod broadcast;
mod captcha;
mod client_letters;
mod content;
mod contests;
mod email_link;
mod flood;
mod gifts;
mod gram_wallet;
mod languages;
mod layout;
mod maintenance;
mod menu_mode;
mod miniapp;
mod module_screen;
mod photos;
mod promotions;
mod receipt_email;
mod referral_login;
mod referral_payouts;
mod renewal_devices;
mod runtime;
mod single_subscription;
mod site_limits;
mod stars;
mod stars_review;
mod support_files;
mod support_followup;
mod support_mode;
mod tariff_description;
mod telegram_link;
mod trial_variants;
mod utm_clicks;
mod webhook;

const TOKEN: &str = "123456:TEST-secret-token";

fn ok(result: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true, "result": result }))
}

async fn telegram() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .respond_with(ok(serde_json::json!({
            "message_id": 100, "chat": {"id": 1, "type": "private"}, "date": 0
        })))
        .mount(&server)
        .await;
    for name in [
        "editMessageText",
        "answerCallbackQuery",
        "answerPreCheckoutQuery",
        "deleteWebhook",
        "setMyCommands",
        "deleteMyCommands",
    ] {
        Mock::given(method("POST"))
            .and(path_regex(format!("/{name}$")))
            .respond_with(ok(serde_json::json!(true)))
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path_regex(r"/getWebhookInfo$"))
        .respond_with(ok(
            serde_json::json!({ "url": "", "pending_update_count": 0 }),
        ))
        .with_priority(5)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/getMe$"))
        .respond_with(ok(serde_json::json!({
            "id": 1, "is_bot": true, "first_name": "Бот", "username": "stealth_test_bot"
        })))
        .mount(&server)
        .await;
    server
}

async fn setup() -> Option<(TestDb, Services, MockServer, BotCtx)> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(catalog::all()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks {
            bot_token: Some(TOKEN.into()),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    let server = telegram().await;
    let ctx = BotCtx {
        services: services.clone(),
        api: Arc::new(TelegramApi::new(&server.uri(), TOKEN, None)),
        username: Some("stealth_test_bot".into()),
    };
    Some((db, services, server, ctx))
}

fn user(id: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id, "is_bot": false, "first_name": "Алексей", "username": "morozov_a", "language_code": "ru"
    })
}

fn text_update(update_id: i64, from: i64, text: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id, "from": user(from),
            "chat": {"id": from, "type": "private"}, "date": 1, "text": text
        }
    }))
    .unwrap()
}

fn callback_update(update_id: i64, from: i64, data: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "callback_query": {
            "id": format!("cb{update_id}"), "from": user(from), "data": data,
            "message": {"message_id": 55, "chat": {"id": from, "type": "private"}, "date": 1}
        }
    }))
    .unwrap()
}

async fn sent(server: &MockServer) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| {
            let path = r.url.path();
            path.ends_with("/sendMessage") || path.ends_with("/editMessageText")
        })
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

async fn last_text(server: &MockServer) -> String {
    sent(server)
        .await
        .last()
        .and_then(|b| b["text"].as_str().map(str::to_string))
        .unwrap_or_default()
}

async fn last_buttons(server: &MockServer) -> Vec<String> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["callback_data"].as_str().map(str::to_string))
        .collect()
}

async fn copy_texts(server: &MockServer) -> Vec<String> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["copy_text"]["text"].as_str().map(str::to_string))
        .collect()
}

async fn last_labels(server: &MockServer) -> Vec<String> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["text"].as_str().map(str::to_string))
        .collect()
}

async fn antibot_mode(services: &Services, mode: &str) {
    services
        .modules()
        .set_enabled(
            services.store(),
            sn_domain::module::ModuleId::Antibot,
            true,
            None,
        )
        .await
        .unwrap();
    services
        .modules()
        .set_config(
            services.store(),
            sn_domain::module::ModuleId::Antibot,
            sn_modules::ModuleConfig::from_value(&serde_json::json!({
                "mode": mode,
                "burst_count": 0,
                "check_username": false
            })),
            None,
        )
        .await
        .unwrap();
}

async fn client_of(services: &Services, telegram_id: i64) -> Option<sn_domain::client::Client> {
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .find_by_identity(IdentityKind::Telegram, &telegram_id.to_string())
        .await
        .unwrap()
}

#[test]
fn команда_с_именем_бота_и_параметром() {
    assert_eq!(
        parse_command("/start@stealth_bot ref_ABC"),
        Some(("start".into(), "ref_ABC".into()))
    );
    assert_eq!(
        parse_command("/START"),
        Some(("start".into(), String::new()))
    );
    assert_eq!(parse_command("привет"), None);
    assert_eq!(parse_command("/"), None);
}

#[tokio::test]
async fn старт_по_ссылке_регистрирует_клиента_с_меткой_и_пригласившим() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let (referrer, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 700,
                username: None,
                first_name: Some("Ксения".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap();
    let code = {
        let mut conn = db.store.conn().await.unwrap();
        referral::ensure_code(&mut conn, referrer.id).await.unwrap()
    };
    handle_update(
        &ctx,
        text_update(1, 501, &format!("/start ref_{code}__s_tiktok")),
    )
    .await;
    let client = client_of(&services, 501).await.unwrap();
    let text = last_text(&server).await;
    assert!(text.contains("Подписки пока нет"), "{text}");
    let buttons = last_buttons(&server).await;
    for expected in ["buy", "bal", "sup", "x:promo", "x:referral", "x:options"] {
        assert!(
            buttons.contains(&expected.to_string()),
            "{expected}: {buttons:?}"
        );
    }
    services.events().dispatch_pending(50).await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    let origin = utm::for_client(&mut conn, client.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(origin.utm.source.as_deref(), Some("tiktok"));
    drop(conn);
    let overview = referral::overview(&services.module_context(), referrer.id)
        .await
        .unwrap();
    assert_eq!(overview.invited, 1);
    assert_eq!(
        overview.bot_link.as_deref(),
        None,
        "имя бота сохраняется только при запуске"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn согласие_с_документами_до_первого_действия() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                consent::DOCUMENTS_KEY,
                &serde_json::json!([{"title": "Оферта", "url": "https://example.com/offer"}]),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    handle_update(&ctx, text_update(1, 502, "/start s_vk")).await;
    assert!(client_of(&services, 502).await.is_none());
    assert!(last_text(&server).await.contains("Оферта"));
    assert_eq!(last_buttons(&server).await, vec!["terms:ok".to_string()]);
    let pending = Session::load(&services, 502).await.unwrap();
    assert_eq!(pending.pending_start.as_deref(), Some("s_vk"));
    handle_update(&ctx, text_update(2, 502, "/menu")).await;
    assert!(client_of(&services, 502).await.is_none());
    handle_update(&ctx, callback_update(3, 502, "terms:ok")).await;
    let client = client_of(&services, 502).await.unwrap();
    services.events().dispatch_pending(50).await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    let origin = utm::for_client(&mut conn, client.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(origin.utm.source.as_deref(), Some("vk"));
    drop(conn);
    assert!(last_text(&server).await.contains("Подписки пока нет"));
    assert_eq!(
        Session::load(&services, 502).await.unwrap(),
        Session::default()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn промокод_вводится_диалогом_и_запоминается_для_оплаты() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    sn_modules::catalog::promo::create(
        &services.module_context(),
        &NewPromo {
            code: "ZIMA".into(),
            value: NewPromoValue::Percent { percent: 10 },
            max_uses: None,
            per_client: 1,
            valid_from: None,
            valid_until: None,
            first_purchase_only: false,
            tariff_ids: Vec::new(),
            note: None,
        },
        None,
    )
    .await
    .unwrap();
    handle_update(&ctx, text_update(1, 503, "/start")).await;
    handle_update(&ctx, callback_update(2, 503, "x:promo")).await;
    let waiting = Session::load(&services, 503).await.unwrap();
    assert_eq!(waiting.awaiting, Some(Awaiting::PromoCode));
    handle_update(&ctx, text_update(3, 503, "nope")).await;
    assert!(last_text(&server).await.contains("Такого промокода нет"));
    handle_update(&ctx, callback_update(4, 503, "x:promo")).await;
    handle_update(&ctx, text_update(5, 503, " zima ")).await;
    assert!(last_text(&server).await.contains("сохранён"));
    let saved = Session::load(&services, 503).await.unwrap();
    assert_eq!(saved.promo.as_deref(), Some("ZIMA"));
    assert_eq!(saved.awaiting, None);
    db.cleanup().await;
}

#[tokio::test]
async fn обращение_в_поддержку_из_бота() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 504, "/start")).await;
    handle_update(&ctx, callback_update(2, 504, "sup")).await;
    handle_update(&ctx, text_update(3, 504, "Не подключается на iPhone")).await;
    assert!(last_text(&server).await.contains("отправлено"));
    let client = client_of(&services, 504).await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(conn.tickets().count_open(client.id).await.unwrap(), 1);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn покупка_тарифа_доходит_до_выставления_счёта() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let tariff = {
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
                    stars: Some(Stars(350)),
                    sort: 0,
                    active: true,
                }],
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        tariff
    };
    handle_update(&ctx, text_update(1, 505, "/start")).await;
    handle_update(&ctx, callback_update(2, 505, "buy")).await;
    assert!(last_text(&server).await.contains("Стандарт"));
    handle_update(&ctx, callback_update(3, 505, &format!("t:{}", tariff.id))).await;
    let labels = last_labels(&server).await;
    assert!(
        labels.iter().any(|l| l.starts_with("90\u{a0}дней — $6.99")),
        "{labels:?}"
    );
    let period = tariff.periods[0].id;
    handle_update(&ctx, callback_update(4, 505, &format!("p:{period}"))).await;
    let summary = last_text(&server).await;
    assert!(summary.contains("К оплате: <b>$6.99</b>"), "{summary}");
    assert!(summary.contains("350\u{a0}звёзд"), "{summary}");
    assert!(summary.contains("С баланса: не хватает $6.99"), "{summary}");
    assert!(last_buttons(&server).await.contains(&"promo".to_string()));
    services.set_remna(Some(Arc::new(sn_remnawave::fake::FakeRemnawave::new())));
    handle_update(&ctx, callback_update(5, 505, "pay:balance")).await;
    let refused = last_text(&server).await;
    assert!(
        refused.contains("Не хватает: на балансе $0.00"),
        "{refused}"
    );
    let cart = Session::load(&services, 505).await.unwrap().cart.unwrap();
    assert_eq!(cart.tariff_id, Some(tariff.id));
    assert_eq!(cart.period_id, Some(period));
    let client = client_of(&services, 505).await.unwrap();
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.ledger()
            .append(&sn_domain::ledger::NewLedgerEntry {
                client_id: client.id,
                wallet: sn_domain::ledger::Wallet::Main,
                amount: sn_domain::money::Money::new(1_000, sn_domain::money::Currency::Usd),
                kind: sn_domain::ledger::LedgerKind::TopUp,
                order_id: None,
                payment_id: None,
                admin_id: None,
                comment: None,
                idempotency_key: Some("test:topup:505".into()),
            })
            .await
            .unwrap();
    }
    handle_update(&ctx, callback_update(6, 505, "pay:balance")).await;
    let paid = last_text(&server).await;
    assert!(paid.contains("Оплачено с баланса"), "{paid}");
    assert!(Session::load(&services, 505).await.unwrap().cart.is_none());
    db.cleanup().await;
}

#[tokio::test]
async fn заблокированный_клиент_видит_только_причину() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 506, "/start")).await;
    let client = client_of(&services, 506).await.unwrap();
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.clients()
            .set_status(client.id, ClientStatus::Blocked, Some("спам"))
            .await
            .unwrap();
    }
    handle_update(&ctx, callback_update(2, 506, "bal")).await;
    assert!(last_text(&server).await.contains("заблокирован"));
    assert!(last_buttons(&server).await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn неверный_счёт_звёздами_отклоняется_до_списания() {
    let Some((db, _services, server, ctx)) = setup().await else {
        return;
    };
    let update: Update = serde_json::from_value(serde_json::json!({
        "update_id": 1,
        "pre_checkout_query": {
            "id": "pcq1", "from": user(507), "currency": "XTR", "total_amount": 350,
            "invoice_payload": "00000000-0000-0000-0000-000000000000"
        }
    }))
    .unwrap();
    handle_update(&ctx, update).await;
    let answers: Vec<serde_json::Value> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/answerPreCheckoutQuery"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["ok"], false);
    assert!(answers[0]["error_message"].as_str().unwrap().len() > 3);
    db.cleanup().await;
}

async fn notify(
    services: &Services,
    handler: &NotifyHandler,
    client_id: ClientId,
    template: &str,
    params: serde_json::Value,
) -> JobResult {
    handler
        .handle(
            services,
            &JobPayload::Notify {
                client_id,
                template: template.into(),
                params,
            },
            1,
        )
        .await
}

#[tokio::test]
async fn уведомление_уходит_в_телеграм_и_учитывает_блокировку_бота() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 508, "/start")).await;
    let client = client_of(&services, 508).await.unwrap();
    let handler = NotifyHandler::new(&BotConfig {
        api_base: server.uri(),
        ..BotConfig::default()
    });
    let done = notify(
        &services,
        &handler,
        client.id,
        notify::templates::SUPPORT_REPLY,
        serde_json::json!({"body": "Отправили чек повторно"}),
    )
    .await;
    assert_eq!(done, JobResult::Done);
    assert!(last_text(&server).await.contains("Отправили чек повторно"));
    assert!(matches!(
        notify(
            &services,
            &handler,
            client.id,
            "unknown",
            serde_json::json!({})
        )
        .await,
        JobResult::Fail(_)
    ));
    let blocked = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "ok": false, "error_code": 403, "description": "Forbidden: bot was blocked by the user"
        })))
        .mount(&blocked)
        .await;
    let to_blocked = NotifyHandler::new(&BotConfig {
        api_base: blocked.uri(),
        ..BotConfig::default()
    });
    let result = notify(
        &services,
        &to_blocked,
        client.id,
        "text",
        serde_json::json!({"text": "Привет"}),
    )
    .await;
    assert_eq!(result, JobResult::Done);
    assert!(client_of(&services, 508).await.unwrap().bot_unreachable);
    let again = notify(
        &services,
        &to_blocked,
        client.id,
        "text",
        serde_json::json!({"text": "Ещё"}),
    )
    .await;
    assert_eq!(again, JobResult::Done);
    assert_eq!(blocked.received_requests().await.unwrap().len(), 1);
    handle_update(&ctx, text_update(2, 508, "/start")).await;
    assert!(!client_of(&services, 508).await.unwrap().bot_unreachable);
    db.cleanup().await;
}

#[tokio::test]
async fn лимит_телеграма_откладывает_уведомление_без_токена_в_ошибке() {
    let Some((db, services, _server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 509, "/start")).await;
    let client = client_of(&services, 509).await.unwrap();
    let limited = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "ok": false, "error_code": 429, "description": "Too Many Requests: retry after 7",
            "parameters": {"retry_after": 7}
        })))
        .mount(&limited)
        .await;
    let handler = NotifyHandler::new(&BotConfig {
        api_base: limited.uri(),
        ..BotConfig::default()
    });
    match notify(
        &services,
        &handler,
        client.id,
        "text",
        serde_json::json!({"text": "x"}),
    )
    .await
    {
        JobResult::Retry { after, error } => {
            assert_eq!(after, chrono::Duration::seconds(7));
            assert!(!error.contains("secret"));
        }
        other => panic!("ожидался повтор, получено {other:?}"),
    }
    let offline = NotifyHandler::new(&BotConfig {
        api_base: "http://127.0.0.1:9".into(),
        ..BotConfig::default()
    });
    match notify(
        &services,
        &offline,
        client.id,
        "text",
        serde_json::json!({"text": "x"}),
    )
    .await
    {
        JobResult::Retry { error, .. } => assert!(!error.contains(TOKEN)),
        other => panic!("ожидался повтор, получено {other:?}"),
    }
    let egress = services.egress();
    egress.set_plan(sn_egress::Plan::from_panel(
        &sn_egress::ProxySettings {
            enabled: true,
            server: Some("socks5h://127.0.0.1:9".into()),
            routes: [sn_egress::Route::Telegram].into_iter().collect(),
            ..sn_egress::ProxySettings::default()
        },
        None,
    ));
    egress.set_down(true);
    match notify(
        &services,
        &offline,
        client.id,
        "text",
        serde_json::json!({"text": "x"}),
    )
    .await
    {
        JobResult::Wait { after, error } => {
            assert_eq!(after, chrono::Duration::seconds(60));
            assert!(!error.contains(TOKEN));
        }
        other => panic!("ожидалось ожидание прокси, получено {other:?}"),
    }
    egress.set_down(false);
    assert!(matches!(
        notify(
            &services,
            &offline,
            client.id,
            "text",
            serde_json::json!({"text": "x"}),
        )
        .await,
        JobResult::Retry { .. }
    ));
    db.cleanup().await;
}

#[test]
fn шаблоны_уведомлений_с_параметрами() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let renewed = notify::render(
        &sn_domain::bot_content::Content::default(),
        notify::templates::SUBSCRIPTION_RENEWED,
        &serde_json::json!({"expire_at": "2027-02-14T10:00:00Z"}),
        utc,
    )
    .unwrap();
    assert_eq!(
        renewed.text,
        "Подписка продлена до 14\u{a0}февраля\u{a0}2027."
    );
    let credit = notify::render(
        &sn_domain::bot_content::Content::default(),
        referral::TEMPLATE_CREDIT,
        &serde_json::json!({"amount": "$1.50", "level": 1}),
        utc,
    )
    .unwrap();
    assert!(credit.text.contains("$1.50"));
    let fulfilled = notify::render(
        &sn_domain::bot_content::Content::default(),
        notify::templates::ORDER_FULFILLED,
        &serde_json::json!({
            "order_kind": "renewal",
            "tariff": "Стандарт",
            "expire_at": "2027-02-14T10:00:00Z"
        }),
        utc,
    )
    .unwrap();
    assert_eq!(fulfilled.text, renewed.text);
    let ready = notify::render(
        &sn_domain::bot_content::Content::default(),
        notify::templates::ORDER_FULFILLED,
        &serde_json::json!({"order_kind": "new", "expire_at": "2027-02-14T10:00:00Z"}),
        utc,
    )
    .unwrap();
    assert!(ready.text.starts_with("Подписка готова"), "{}", ready.text);
    let reminder = notify::render(
        &sn_domain::bot_content::Content::default(),
        notify::templates::AUTORENEW_REMINDER,
        &serde_json::json!({
            "amount": {"minor": 699, "currency": "USD"},
            "missing": {"minor": 120, "currency": "USD"},
            "expire_at": "2027-02-14T10:00:00Z"
        }),
        utc,
    )
    .unwrap();
    assert!(
        reminder.text.contains("не хватает $1.20"),
        "{}",
        reminder.text
    );
    assert!(
        notify::render(
            &sn_domain::bot_content::Content::default(),
            notify::templates::AUTORENEW_CHARGE_FAILED,
            &serde_json::json!({"amount": {"minor": 699, "currency": "USD"}}),
            utc
        )
        .is_some()
    );
    assert!(
        notify::render(
            &sn_domain::bot_content::Content::default(),
            notify::templates::AUTORENEW_TURNED_OFF,
            &serde_json::json!({"expire_at": "2027-02-14T10:00:00Z"}),
            utc
        )
        .unwrap()
        .text
        .starts_with("Автопродление выключено")
    );
    let escaped = notify::render(
        &sn_domain::bot_content::Content::default(),
        "text",
        &serde_json::json!({"text": "<b>x</b>"}),
        utc,
    )
    .unwrap();
    assert_eq!(escaped.text, "&lt;b&gt;x&lt;/b&gt;");
    let credited = notify::render(&sn_domain::bot_content::Content::default(),
        notify::templates::REPEAT_CREDITED,
        &serde_json::json!({"text": "Повторная оплата $6.99 по уже оплаченному счёту зачислена на баланс."}),
        utc,
    )
    .unwrap();
    assert!(credited.text.contains("зачислена на баланс"));
    assert_eq!(
        credited.keyboard,
        crate::telegram::Keyboard::new().button("Баланс", "bal")
    );
    let review = notify::render(
        &sn_domain::bot_content::Content::default(),
        notify::templates::REPEAT_REVIEW,
        &serde_json::json!({"text": "Проверим её вручную и свяжемся с вами."}),
        utc,
    )
    .unwrap();
    assert_eq!(
        review.keyboard,
        crate::telegram::Keyboard::new().button("Поддержка", "sup")
    );
    assert!(
        notify::render(
            &sn_domain::bot_content::Content::default(),
            notify::templates::SUPPORT_REPLY,
            &serde_json::json!({}),
            utc
        )
        .is_none()
    );
}

#[test]
fn уведомления_о_возврате_заблокированному_и_о_продлении_после_выключения_автопродления() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let content = sn_domain::bot_content::Content::default();
    let refunded = notify::render(
        &content,
        notify::templates::REFUNDED_BLOCKED,
        &serde_json::json!({"amount": "$6.99"}),
        utc,
    )
    .unwrap();
    assert_eq!(
        refunded.text,
        "Оплата $6.99 возвращена: аккаунт заблокирован, заказ не выдан."
    );
    assert_eq!(refunded.keyboard, crate::telegram::Keyboard::new());
    assert_eq!(
        notify::render(
            &content,
            notify::templates::REFUNDED_BLOCKED,
            &serde_json::json!({}),
            utc
        )
        .unwrap()
        .text,
        "Оплата возвращена: аккаунт заблокирован, заказ не выдан."
    );
    let renewed = notify::render(
        &content,
        notify::templates::AUTORENEW_AFTER_OFF,
        &serde_json::json!({"amount": "$6.99", "expire_at": "2027-02-14T10:00:00Z"}),
        utc,
    )
    .unwrap();
    assert_eq!(
        renewed.text,
        "Списание $6.99 за продление ушло до того, как автопродление выключили, поэтому \
         подписка продлена до 14\u{a0}февраля\u{a0}2027. Если продление не нужно, напишите в \
         поддержку — вернём деньги."
    );
    assert_eq!(
        renewed.keyboard,
        crate::telegram::Keyboard::new().button("Поддержка", "sup")
    );
    assert!(
        notify::render(
            &content,
            notify::templates::AUTORENEW_AFTER_OFF,
            &serde_json::json!({"expire_at": "2027-02-14T10:00:00Z"}),
            utc
        )
        .unwrap()
        .text
        .starts_with("Списание за продление ушло до того")
    );
    let (letter, values) = notify::letter_of(
        notify::templates::AUTORENEW_AFTER_OFF,
        &serde_json::json!({"amount": "$6.99", "expire_at": "2027-02-14T10:00:00Z"}),
        utc,
    )
    .unwrap();
    assert_eq!(letter, sn_domain::mail_letters::LetterId::ChargedAfterOff);
    assert_eq!(
        letter.spec().group,
        sn_domain::mail_letters::Group::Payments
    );
    assert_eq!(letter.spec().path, ["/cabinet/help"]);
    assert_eq!(values.get("amount"), Some("$6.99"));
    assert_eq!(values.get("date"), Some("14\u{a0}февраля\u{a0}2027"));
    let (letter, _) = notify::letter_of(
        notify::templates::REFUNDED_BLOCKED,
        &serde_json::json!({}),
        utc,
    )
    .unwrap();
    assert_eq!(
        letter.spec().group,
        sn_domain::mail_letters::Group::Payments
    );
    assert_eq!(letter.spec().ru.subject, ["Оплата возвращена"]);
}

#[test]
fn уведомление_о_возврате_звёзд_называет_покупку_и_сумму() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let content = sn_domain::bot_content::Content::default();
    let params = serde_json::json!({
        "stars": 350,
        "label": "Стандарт <VPN> · 90 дней",
        "label_en": "Standard <VPN> · 90 days",
        "charge_id": "stx-1",
    });
    let refunded =
        notify::render(&content, notify::templates::STARS_REFUNDED, &params, utc).unwrap();
    assert_eq!(
        refunded.text,
        "Звёзды за покупку «Стандарт &lt;VPN&gt; · 90 дней» возвращены: 350\u{a0}звёзд."
    );
    assert_eq!(refunded.keyboard, crate::telegram::Keyboard::new());
    let english_content =
        sn_domain::bot_content::Content::default().in_locale(sn_domain::client::Locale::En);
    let english = sn_domain::lang::sync_scope(sn_domain::client::Locale::En, || {
        notify::render(
            &english_content,
            notify::templates::STARS_REFUNDED,
            &params,
            utc,
        )
        .unwrap()
    });
    assert_eq!(
        english.text,
        "Stars for “Standard &lt;VPN&gt; · 90 days” refunded: 350\u{a0}stars."
    );
    assert_eq!(
        notify::render(
            &content,
            notify::templates::STARS_REFUNDED,
            &serde_json::json!({"stars": 1, "label": null}),
            utc
        )
        .unwrap()
        .text,
        "Звёзды возвращены: 1\u{a0}звезда."
    );
    for broken in [
        serde_json::json!({}),
        serde_json::json!({"stars": 0, "label": "Стандарт"}),
        serde_json::json!({"stars": "350"}),
    ] {
        assert!(
            notify::render(&content, notify::templates::STARS_REFUNDED, &broken, utc).is_none(),
            "{broken}"
        );
    }
    let (letter, values) =
        notify::letter_of(notify::templates::STARS_REFUNDED, &params, utc).unwrap();
    assert_eq!(
        letter.spec().group,
        sn_domain::mail_letters::Group::Payments
    );
    assert_eq!(letter.spec().ru.subject, ["Звёзды возвращены"]);
    assert_eq!(letter.spec().path, ["/cabinet/payments"]);
    assert_eq!(values.get("purchase"), Some("Стандарт <VPN> · 90 дней"));
    assert_eq!(values.get("stars"), Some("350\u{a0}звёзд"));
}

#[test]
fn уведомление_о_привязке_входа_через_google_или_apple() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let content = sn_domain::bot_content::Content::default();
    let linked = notify::render(
        &content,
        sn_modules::catalog::oauth::TEMPLATE_LINKED,
        &serde_json::json!({"provider": "Google", "account": "anna@gmail.com"}),
        utc,
    )
    .unwrap();
    assert_eq!(
        linked.text,
        "К вашему аккаунту привязан вход через Google: anna@gmail.com. Если это сделали не вы, сразу напишите в поддержку."
    );
    assert_eq!(
        linked.keyboard,
        crate::telegram::Keyboard::new().button("Поддержка", "sup")
    );
    assert!(
        notify::render(
            &content,
            sn_modules::catalog::oauth::TEMPLATE_LINKED,
            &serde_json::json!({"provider": "Apple"}),
            utc
        )
        .is_none()
    );
    let (letter, values) = notify::letter_of(
        sn_modules::catalog::oauth::TEMPLATE_LINKED,
        &serde_json::json!({"provider": "Apple", "account": "почта скрыта"}),
        utc,
    )
    .unwrap();
    assert_eq!(letter.spec().group, sn_domain::mail_letters::Group::Service);
    assert_eq!(
        sn_domain::mail_letters::first_plain(letter.spec().ru.subject.iter().copied(), &values),
        Some("Привязан вход через Apple".to_string())
    );
    assert_eq!(letter.spec().path, ["/cabinet/security"]);
}

#[tokio::test]
async fn бот_читает_апдейты_и_хранит_смещение() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    let first = serde_json::json!([{
        "update_id": 41,
        "message": {
            "message_id": 1, "from": user(510), "chat": {"id": 510, "type": "private"},
            "date": 1, "text": "/start"
        }
    }]);
    Mock::given(method("POST"))
        .and(path_regex(r"/getUpdates$"))
        .respond_with(ok(first))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/getUpdates$"))
        .respond_with(ok(serde_json::json!([])).set_delay(Duration::from_millis(100)))
        .with_priority(2)
        .mount(&server)
        .await;
    let bot = Bot::new(
        services.clone(),
        BotConfig {
            api_base: server.uri(),
            poll_timeout: Duration::from_secs(1),
            workers: 2,
            ..BotConfig::default()
        },
    );
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let running = tokio::spawn(bot.run(cancel));
    let mut registered = false;
    for _ in 0..50 {
        if client_of(&services, 510).await.is_some() {
            registered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop.cancel();
    running.await.unwrap().unwrap();
    assert!(registered);
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.bot_state().get(OFFSET_KEY).await.unwrap(),
        Some(serde_json::json!({"bot": 1, "offset": 42}))
    );
    assert_eq!(
        conn.settings().get(keys::BOT_USERNAME).await.unwrap(),
        Some(serde_json::json!("stealth_test_bot"))
    );
    drop(conn);
    let polls: Vec<serde_json::Value> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/getUpdates"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert!(polls.iter().skip(1).all(|body| body["offset"] == 42));
    db.cleanup().await;
}

#[tokio::test]
async fn смещение_сохраняется_и_когда_апдейтов_больше_нет() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    let first = serde_json::json!([{
        "update_id": 41,
        "message": {
            "message_id": 1, "from": user(511), "chat": {"id": 511, "type": "private"},
            "date": 1, "text": "/start"
        }
    }]);
    Mock::given(method("POST"))
        .and(path_regex(r"/getUpdates$"))
        .respond_with(ok(first))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/getUpdates$"))
        .respond_with(ok(serde_json::json!([])).set_delay(Duration::from_millis(100)))
        .with_priority(2)
        .mount(&server)
        .await;
    let bot = Bot::new(
        services.clone(),
        BotConfig {
            api_base: server.uri(),
            poll_timeout: Duration::from_secs(1),
            workers: 2,
            offset_every: Duration::from_secs(1),
            ..BotConfig::default()
        },
    );
    let cancel = CancellationToken::new();
    let running = tokio::spawn(bot.run(cancel.clone()));
    let mut saved = None;
    for _ in 0..60 {
        let mut conn = db.store.conn().await.unwrap();
        saved = conn.bot_state().get(OFFSET_KEY).await.unwrap();
        drop(conn);
        if saved.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let polls_before_stop = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/getUpdates"))
        .count();
    cancel.cancel();
    running.await.unwrap().unwrap();
    assert_eq!(
        saved,
        Some(serde_json::json!({"bot": 1, "offset": 42})),
        "смещение записано, пока бот работает, хотя новых апдейтов не было"
    );
    assert!(polls_before_stop >= 2, "{polls_before_stop}");
    db.cleanup().await;
}

#[tokio::test]
async fn вход_в_кабинет_подтверждается_кнопкой_в_боте() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                sn_domain::settings::keys::BOT_USERNAME,
                &serde_json::json!("stealth_test_bot"),
                None,
            )
            .await
            .unwrap();
    }
    let meta = sn_services::auth::RequestMeta {
        ip: Some("203.0.113.7".into()),
        user_agent: Some(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36".into(),
        ),
    };
    let start = services.auth().telegram_login_start(&meta).await.unwrap();
    let param = start.link.rsplit_once("?start=").unwrap().1.to_string();
    handle_update(&ctx, text_update(1, 509, &format!("/start {param}"))).await;
    let prompt = last_text(&server).await;
    assert!(prompt.contains("Вход в кабинет"), "{prompt}");
    assert!(prompt.contains("203.0.113.7"), "{prompt}");
    handle_update(&ctx, callback_update(2, 509, "auth:ok")).await;
    assert!(last_text(&server).await.contains("Вход подтверждён"));
    let status = services
        .auth()
        .telegram_login_status(&start.poll_token, &meta)
        .await
        .unwrap();
    let sn_services::login::TelegramLoginStatus::Confirmed { login } = status else {
        panic!("{status:?}");
    };
    let client = client_of(&services, 509).await.unwrap();
    assert_eq!(login.principal.client_id, client.id);
    let rejected = services.auth().telegram_login_start(&meta).await.unwrap();
    let param = rejected.link.rsplit_once("?start=").unwrap().1.to_string();
    handle_update(&ctx, text_update(3, 509, &format!("/start {param}"))).await;
    handle_update(&ctx, callback_update(4, 509, "auth:no")).await;
    assert!(last_text(&server).await.contains("Вход отклонён"));
    assert_eq!(
        services
            .auth()
            .telegram_login_status(&rejected.poll_token, &meta)
            .await
            .unwrap(),
        sn_services::login::TelegramLoginStatus::Rejected
    );
    db.cleanup().await;
}

#[tokio::test]
async fn пригласивший_получает_процент_с_оплаты_через_ядро() {
    let Some((db, services, _server, ctx)) = setup().await else {
        return;
    };
    let (referrer, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 710,
                username: None,
                first_name: Some("Ксения".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap();
    let code = {
        let mut conn = db.store.conn().await.unwrap();
        referral::ensure_code(&mut conn, referrer.id).await.unwrap()
    };
    handle_update(&ctx, text_update(1, 711, &format!("/start ref_{code}"))).await;
    services.events().dispatch_pending(50).await.unwrap();
    let invited = client_of(&services, 711).await.unwrap();
    let amount = sn_domain::money::Money::new(1_000, sn_domain::money::Currency::Usd);
    let payment = {
        let mut tx = db.store.begin().await.unwrap();
        let order = tx
            .orders()
            .create(&sn_domain::order::NewOrder {
                client_id: invited.id,
                subscription_id: None,
                kind: sn_domain::order::OrderKind::TopUp,
                intent: sn_domain::order::PurchaseIntent::TopUp { amount },
                quote: sn_domain::pricing::topup_quote(amount, services.settings().topup_limits())
                    .unwrap(),
                snapshot: None,
            })
            .await
            .unwrap();
        let payment = tx
            .payments()
            .create(&sn_domain::payment::NewPayment {
                order_id: order.id,
                client_id: invited.id,
                provider: sn_domain::payment::ProviderId::from(
                    sn_domain::payment::providers::CRYPTOPAY,
                ),
                method: None,
                amount: amount.into(),
                expires_at: None,
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        payment
    };
    let paid = sn_payments::WebhookOutcome {
        event_key: "ref-e2e-paid".into(),
        payment: Some(sn_payments::PaymentRef::Payment(payment.id)),
        provider_txid: Some("ref-e2e".into()),
        status: sn_payments::ProviderStatus::Paid,
        amount: Some(payment.amount),
        saved_method: None,
        reply: sn_payments::WebhookReply::ok(),
        signal: None,
    };
    services
        .inbox()
        .apply_outcome(sn_domain::payment::providers::CRYPTOPAY, &paid)
        .await
        .unwrap();
    services.events().dispatch_pending(50).await.unwrap();
    services.events().dispatch_pending(50).await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    let earned = conn
        .ledger()
        .balance(
            referrer.id,
            sn_domain::ledger::Wallet::Referral,
            sn_domain::money::Currency::Usd,
        )
        .await
        .unwrap();
    assert_eq!(earned.minor, 300);
    let main = conn
        .ledger()
        .balance(
            invited.id,
            sn_domain::ledger::Wallet::Main,
            sn_domain::money::Currency::Usd,
        )
        .await
        .unwrap();
    assert_eq!(main.minor, 1_000);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn устройства_и_новая_ссылка_через_ядро_remnawave() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let fake = Arc::new(sn_remnawave::fake::FakeRemnawave::new());
    services.set_remna(Some(fake.clone()));
    handle_update(&ctx, text_update(1, 720, "/start")).await;
    let client = client_of(&services, 720).await.unwrap();
    let user = sn_remnawave::fake::sample_user(20, "sn_bot_devices");
    fake.insert_user(user.clone());
    let added = chrono::Utc::now() - chrono::Duration::days(3);
    fake.add_device(sn_remnawave::fake::sample_device(user.id, "hw-1", added));
    fake.add_device(sn_remnawave::fake::sample_device(user.id, "hw-2", added));
    let subscription = {
        let mut tx = db.store.begin().await.unwrap();
        let subscription = tx
            .subscriptions()
            .create(&sn_store::NewSubscription {
                client_id: client.id,
                tariff_id: None,
                kind: sn_domain::subscription::SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: sn_domain::subscription::AutoRenewState::default(),
            })
            .await
            .unwrap();
        tx.subscriptions()
            .link_remna(
                subscription.id,
                &sn_domain::subscription::RemnaLink {
                    user_id: user.id,
                    username: user.username.clone(),
                    short_uuid: Some(user.short_uuid.clone()),
                },
            )
            .await
            .unwrap();
        tx.subscriptions()
            .apply_mirror(subscription.id, &user.mirror(Some(2), chrono::Utc::now()))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        subscription
    };
    let sub = subscription.id.0;
    handle_update(&ctx, callback_update(2, 720, &format!("dev:{sub}"))).await;
    let list = last_text(&server).await;
    assert!(list.contains("iPhone 15 Pro"), "{list}");
    assert!(list.contains("Happ 3.2"), "{list}");
    handle_update(&ctx, callback_update(3, 720, "dx:0")).await;
    handle_update(&ctx, callback_update(4, 720, "dxy:0")).await;
    assert!(last_text(&server).await.contains("отключено"));
    assert_eq!(fake.devices(user.id).len(), 1);
    handle_update(&ctx, callback_update(5, 720, &format!("reiy:{sub}"))).await;
    let reissued = last_text(&server).await;
    assert!(reissued.contains("Новая ссылка подписки"), "{reissued}");
    let copied = copy_texts(&server).await;
    assert_eq!(copied.len(), 1, "{reissued}");
    assert!(
        copied[0].starts_with("http") && reissued.contains(copied[0].as_str()),
        "{copied:?}\n{reissued}"
    );
    assert!(fake.devices(user.id).is_empty());
    let stranger = text_update(6, 721, "/start");
    handle_update(&ctx, stranger).await;
    handle_update(&ctx, callback_update(7, 721, &format!("reiy:{sub}"))).await;
    let foreign = last_text(&server).await;
    assert!(foreign.contains("Не найдено"), "{foreign}");
    handle_update(&ctx, callback_update(8, 720, &format!("sub:{sub}"))).await;
    let screen = last_text(&server).await;
    let copied = copy_texts(&server).await;
    assert_eq!(copied.len(), 1, "{screen}");
    assert!(screen.contains(copied[0].as_str()), "{copied:?}\n{screen}");
    db.cleanup().await;
}
