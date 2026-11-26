use super::store::{self, Claimed, Saved, Spent};
use super::*;
use crate::context::{BalanceChange, CoreApi, Notification};
use chrono::{Duration as ChronoDuration, TimeZone};
use provider::Provider;
use sn_domain::client::{IdentityKind, Locale, NewIdentity, RegistrationSource};
use sn_domain::ids::{CategoryId, ClientId, SquadId, SubscriptionId};
use sn_domain::money::{Currency, Money};
use sn_domain::settings::{CoreSettings, TrialSettings};
use sn_domain::subscription::{AutoRenewState, RemnaMirror, SubscriptionKind, SubscriptionStatus};
use sn_domain::support::{AiState, MessageAuthor, TicketAi};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices};
use sn_domain::traffic::TrafficPolicy;
use sn_store::testing::{DAY, TestDb, wait_for_window_room};
use sn_store::{NewCategory, NewClient, NewPeriod, NewSubscription, NewTariff, NewTicket, Store};
use std::sync::Arc;
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

struct AiCore {
    enabled: bool,
    config: ModuleConfig,
    settings: CoreSettings,
}

#[async_trait]
impl CoreApi for AiCore {
    async fn change_balance(&self, _change: BalanceChange) -> Result<Money, ModuleError> {
        Err(ModuleError::NotImplemented("change_balance"))
    }

    async fn add_days(
        &self,
        _id: SubscriptionId,
        _days: u32,
        _reason: &str,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::NotImplemented("add_days"))
    }

    async fn notify(
        &self,
        _client_id: ClientId,
        _notification: Notification,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::NotImplemented("notify"))
    }

    async fn enqueue(&self, _job: JobPayload) -> Result<(), ModuleError> {
        Err(ModuleError::NotImplemented("enqueue"))
    }

    fn module_enabled(&self, module: ModuleId) -> bool {
        module == ModuleId::Ai && self.enabled
    }

    fn module_config(&self, module: ModuleId) -> ModuleConfig {
        if module == ModuleId::Ai {
            self.config.clone()
        } else {
            ModuleConfig::default()
        }
    }

    fn core_settings(&self) -> Arc<CoreSettings> {
        Arc::new(self.settings.clone())
    }
}

fn settings() -> CoreSettings {
    CoreSettings {
        currency: Currency::Rub,
        brand_name: "Тестовый VPN".into(),
        public_url: Some("https://vpn.example".into()),
        trial: TrialSettings::single(
            true,
            3,
            vec![SquadId(Uuid::from_u128(7))],
            TrafficPolicy::UNLIMITED,
            1,
        ),
        ..CoreSettings::default()
    }
}

fn config(url: &str, extra: serde_json::Value) -> ModuleConfig {
    let mut base = serde_json::json!({
        "api_key": "gsk_test_key",
        "api_url": url,
        "instructions": "Обращайтесь к клиенту на «вы».",
    });
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    ModuleConfig::from_value(&base)
}

fn context(store: &Store, config: ModuleConfig) -> ModuleContext {
    ModuleContext {
        store: store.clone(),
        core: Arc::new(AiCore {
            enabled: true,
            config,
            settings: settings(),
        }),
        currency: Currency::Rub,
    }
}

fn completion(content: serde_json::Value, prompt: u32, answer: u32) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "openai/gpt-oss-120b",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content.to_string() },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": prompt,
            "completion_tokens": answer,
            "total_tokens": prompt + answer
        }
    }))
}

fn reply(text: &str) -> serde_json::Value {
    serde_json::json!({ "reply": text, "handoff": false, "reason": "" })
}

async fn groq_mock(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn client_with_email(store: &Store, email: &str) -> ClientId {
    let mut conn = store.conn().await.unwrap();
    let id = conn
        .clients()
        .create(&NewClient {
            locale: Locale::Ru,
            registration_source: RegistrationSource::Web,
            note: None,
        })
        .await
        .unwrap()
        .id;
    conn.clients()
        .add_identity(
            id,
            &NewIdentity::new(IdentityKind::Email, email, true).unwrap(),
        )
        .await
        .unwrap();
    id
}

async fn tariff(store: &Store) {
    let mut conn = store.conn().await.unwrap();
    let category = conn
        .tariffs()
        .create_category(&NewCategory {
            name: "Основные".into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    let period = |days: u32, rub: i64| NewPeriod {
        days,
        prices: CurrencyPrices {
            usd: None,
            rub: Some(rub),
        },
        stars: None,
        sort: 0,
        active: true,
    };
    conn.tariffs()
        .create(&NewTariff {
            category_id: category.id,
            name: "Стандарт".into(),
            description: Some("Для телефона и ноутбука".into()),
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: Some(ExtraDevices {
                    max: 2,
                    monthly: CurrencyPrices {
                        usd: None,
                        rub: Some(5_000),
                    },
                    monthly_stars: None,
                }),
            },
            traffic: TrafficPolicy::UNLIMITED,
            traffic_addon: None,
            squads: vec![SquadId(Uuid::from_u128(7))],
            external_squad: None,
            periods: vec![period(30, 19_900), period(90, 49_900)],
        })
        .await
        .unwrap();
    let hidden = conn
        .tariffs()
        .create(&NewTariff {
            category_id: CategoryId(category.id.0),
            name: "Только бот".into(),
            description: None,
            on_sale: true,
            sort: 1,
            devices: DevicePolicy {
                included: 1,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            traffic_addon: None,
            squads: vec![SquadId(Uuid::from_u128(7))],
            external_squad: None,
            periods: vec![period(30, 9_900)],
        })
        .await
        .unwrap();
    conn.tariffs()
        .set_channels(
            hidden.id,
            sn_domain::tariff::TariffChannels {
                bot: true,
                cabinet: false,
                miniapp: false,
            },
        )
        .await
        .unwrap();
}

async fn ticket(store: &Store, client: ClientId, body: &str) -> TicketId {
    let mut conn = store.conn().await.unwrap();
    conn.tickets()
        .open(&NewTicket {
            client_id: client,
            subscription_id: None,
            channel: TicketChannel::Cabinet,
            subject: None,
            body: body.into(),
            ai: true,
        })
        .await
        .unwrap()
}

async fn client_says(store: &Store, ticket: TicketId, body: &str) {
    let mut conn = store.conn().await.unwrap();
    conn.tickets()
        .reply(&sn_store::NewReply {
            ticket_id: ticket,
            author: MessageAuthor::Client,
            admin_id: None,
            body: body.into(),
            attachments: Vec::new(),
            close: false,
        })
        .await
        .unwrap();
}

async fn state(
    store: &Store,
    ticket: TicketId,
) -> (Option<TicketAi>, Vec<(MessageAuthor, String)>) {
    let mut conn = store.conn().await.unwrap();
    let ai = conn.tickets().ai(ticket).await.unwrap();
    let messages = conn
        .tickets()
        .messages(ticket)
        .await
        .unwrap()
        .into_iter()
        .map(|m| (m.author, m.body))
        .collect();
    (ai, messages)
}

async fn log_rows(store: &Store) -> Vec<(String, Option<String>, i32, i32, Option<String>)> {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_as(
        "SELECT outcome, reason, prompt_tokens, completion_tokens, error FROM ai_support_log ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn setup() -> Option<TestDb> {
    sn_store::testing::test_store().await
}

fn body_of(request: &Request) -> serde_json::Value {
    serde_json::from_slice(&request.body).unwrap()
}

#[tokio::test]
async fn ии_отвечает_по_данным_сервиса_и_ответ_помечен_как_ответ_ассистента() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    groq_mock(
        &server,
        completion(
            reply("Месяц на тарифе «Стандарт» стоит 199 ₽, три месяца — 499 ₽."),
            1_200,
            80,
        ),
    )
    .await;
    tariff(&db.store).await;
    let client = client_with_email(&db.store, "private.person@example.com").await;
    {
        let mut conn = db.store.conn().await.unwrap();
        let sub = conn
            .subscriptions()
            .create(&NewSubscription {
                client_id: client,
                tariff_id: None,
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState::default(),
            })
            .await
            .unwrap();
        conn.subscriptions()
            .apply_mirror(
                sub.id,
                &RemnaMirror {
                    status: Some(SubscriptionStatus::Active),
                    expire_at: Some(Utc.with_ymd_and_hms(2099, 12, 25, 10, 0, 0).unwrap()),
                    ..RemnaMirror::default()
                },
            )
            .await
            .unwrap();
    }
    let id = ticket(&db.store, client, "Сколько стоит подписка?").await;
    let ctx = context(&db.store, config(&server.uri(), serde_json::json!({})));
    let report = answer_pending(&ctx).await.unwrap();
    assert_eq!(report.answered, 1);
    let (ai, messages) = state(&db.store, id).await;
    assert_eq!(ai.map(|a| a.state), Some(AiState::Active));
    assert_eq!(
        messages.last(),
        Some(&(
            MessageAuthor::Ai,
            "Месяц на тарифе «Стандарт» стоит 199 ₽, три месяца — 499 ₽.".to_string()
        ))
    );
    let rows = log_rows(&db.store).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].0.as_str(), rows[0].2, rows[0].3),
        ("answered", 1_200, 80)
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer gsk_test_key")
    );
    let body = body_of(&requests[0]);
    assert_eq!(body["model"], "openai/gpt-oss-120b");
    assert_eq!(body["max_completion_tokens"], 1_000);
    assert_eq!(body["response_format"]["type"], "json_object");
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("Тарифы в продаже:"));
    assert!(system.contains("«Стандарт»"));
    assert!(system.contains("199"));
    assert!(system.contains("499"));
    assert!(system.contains("Можно докупить до 2 устройств сверх тарифа"));
    assert!(!system.contains("Только бот"));
    assert!(system.contains("Пробный период: 3"));
    assert!(system.contains("активна до 25.12.2099"));
    assert!(system.contains("Обращайтесь к клиенту на «вы»."));
    let everything = body.to_string();
    assert!(!everything.contains("private.person@example.com"));
    assert!(!everything.contains("gsk_test_key"));
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"], "Сколько стоит подписка?");
    db.cleanup().await;
}

#[tokio::test]
async fn просьба_позвать_человека_и_вопрос_о_деньгах_сразу_уходят_оператору() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(completion(reply("не должно уйти"), 10, 10))
        .expect(0)
        .mount(&server)
        .await;
    let client = client_with_email(&db.store, "a@example.com").await;
    let human = ticket(&db.store, client, "Позовите оператора, пожалуйста").await;
    let money = ticket(
        &db.store,
        client,
        "Оплатил, а подписка не продлилась. Верните деньги",
    )
    .await;
    let ctx = context(&db.store, config(&server.uri(), serde_json::json!({})));
    let report = answer_pending(&ctx).await.unwrap();
    assert_eq!(report.handed_off, 2);
    for (id, reason) in [(human, HandoffReason::Human), (money, HandoffReason::Money)] {
        let (ai, messages) = state(&db.store, id).await;
        let ai = ai.unwrap();
        assert_eq!(ai.state, AiState::HandedOff);
        assert_eq!(ai.reason, Some(reason));
        assert!(ai.handoff_at.is_some());
        assert_eq!(messages.last().map(|m| m.0), Some(MessageAuthor::Client));
    }
    let mut conn = db.store.conn().await.unwrap();
    let waiting = conn
        .tickets()
        .list_view(
            sn_domain::support::TicketFilter::Waiting,
            Default::default(),
            Default::default(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(waiting.items.len(), 2);
    drop(conn);
    client_says(&db.store, human, "Ау?").await;
    assert_eq!(answer_pending(&ctx).await.unwrap().total(), 0);
    db.cleanup().await;
}

#[tokio::test]
async fn модель_передаёт_оператору_когда_не_уверена_и_когда_называет_чужую_цену() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(completion(
            serde_json::json!({ "reply": "", "handoff": true, "reason": "unsure" }),
            900,
            30,
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(completion(reply("Тариф стоит 149 ₽ в месяц"), 900, 40))
        .mount(&server)
        .await;
    tariff(&db.store).await;
    let client = client_with_email(&db.store, "b@example.com").await;
    let unsure = ticket(
        &db.store,
        client,
        "Почему у меня медленно работает сервер в Нидерландах?",
    )
    .await;
    let ctx = context(&db.store, config(&server.uri(), serde_json::json!({})));
    assert_eq!(answer_pending(&ctx).await.unwrap().handed_off, 1);
    let price = ticket(&db.store, client, "Сколько стоит месяц?").await;
    assert_eq!(answer_pending(&ctx).await.unwrap().handed_off, 1);
    assert_eq!(
        state(&db.store, unsure).await.0.and_then(|a| a.reason),
        Some(HandoffReason::Unsure)
    );
    let (ai, messages) = state(&db.store, price).await;
    assert_eq!(ai.and_then(|a| a.reason), Some(HandoffReason::Unsure));
    assert!(messages.iter().all(|m| m.0 != MessageAuthor::Ai));
    let rows = log_rows(&db.store).await;
    assert_eq!(rows[0].2, 900);
    assert!(rows[1].4.as_deref().unwrap().contains("149"));
    db.cleanup().await;
}

#[tokio::test]
async fn сбой_groq_передаёт_обращения_оператору_с_ошибкой() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
        .expect(2)
        .mount(&server)
        .await;
    let first = client_with_email(&db.store, "c@example.com").await;
    let second = client_with_email(&db.store, "d@example.com").await;
    let a = ticket(&db.store, first, "Не подключается телефон").await;
    let b = ticket(&db.store, second, "Не работает на ноутбуке").await;
    let ctx = context(&db.store, config(&server.uri(), serde_json::json!({})));
    let report = answer_pending(&ctx).await.unwrap();
    assert_eq!(report.handed_off, 2);
    for id in [a, b] {
        let ai = state(&db.store, id).await.0.unwrap();
        assert_eq!(ai.state, AiState::HandedOff);
        assert_eq!(ai.reason, Some(HandoffReason::NoAnswer));
    }
    let rows = log_rows(&db.store).await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.1.as_deref() == Some("no_answer")
        && r.4.as_deref().is_some_and(|e| e.contains("503"))));
    let mut conn = db.store.conn().await.unwrap();
    assert!(
        store::last_error(&mut conn)
            .await
            .unwrap()
            .unwrap()
            .text
            .contains("Groq")
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn недоступная_модель_и_неверный_ключ_тоже_уводят_к_оператору() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": { "message": "Invalid API Key", "type": "invalid_request_error", "code": "invalid_api_key" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_with_email(&db.store, "e@example.com").await;
    let id = ticket(&db.store, client, "Как подключить айфон?").await;
    let ctx = context(&db.store, config(&server.uri(), serde_json::json!({})));
    answer_pending(&ctx).await.unwrap();
    assert_eq!(
        state(&db.store, id).await.0.and_then(|a| a.reason),
        Some(HandoffReason::NoAnswer)
    );
    assert!(
        log_rows(&db.store).await[0]
            .4
            .as_deref()
            .unwrap()
            .contains("Groq не принял ключ: проверьте его на console.groq.com/keys")
    );
    db.cleanup().await;
}

#[tokio::test]
async fn лимит_ответов_клиенту_и_дневной_бюджет_переводят_на_оператора() {
    let Some(db) = setup().await else { return };
    wait_for_window_room(&db.store, DAY).await;
    let server = MockServer::start().await;
    groq_mock(
        &server,
        completion(reply("Обновите подписку в приложении"), 1_500, 100),
    )
    .await;
    let first = client_with_email(&db.store, "f@example.com").await;
    let id = ticket(&db.store, first, "Не работает").await;
    let ctx = context(
        &db.store,
        config(
            &server.uri(),
            serde_json::json!({ "client_daily": "1", "daily_tokens": "1000" }),
        ),
    );
    assert_eq!(answer_pending(&ctx).await.unwrap().answered, 1);
    client_says(&db.store, id, "Обновил, не помогло").await;
    assert_eq!(answer_pending(&ctx).await.unwrap().handed_off, 1);
    assert_eq!(
        state(&db.store, id).await.0.and_then(|a| a.reason),
        Some(HandoffReason::ClientLimit)
    );
    let second = client_with_email(&db.store, "g@example.com").await;
    let other = ticket(&db.store, second, "Не работает на телевизоре").await;
    assert_eq!(answer_pending(&ctx).await.unwrap().handed_off, 1);
    assert_eq!(
        state(&db.store, other).await.0.and_then(|a| a.reason),
        Some(HandoffReason::Budget)
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    let mut conn = db.store.conn().await.unwrap();
    let usage = store::usage(&mut conn, Utc::now() - ChronoDuration::hours(1))
        .await
        .unwrap();
    assert_eq!((usage.answers, usage.handoffs, usage.tokens), (1, 2, 1_600));
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn после_ответа_оператора_ии_молчит() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(completion(reply("не должно уйти"), 10, 10))
        .expect(0)
        .mount(&server)
        .await;
    let client = client_with_email(&db.store, "h@example.com").await;
    let id = ticket(&db.store, client, "Не подключается").await;
    let mut conn = db.store.conn().await.unwrap();
    assert!(conn.tickets().take_over(id).await.unwrap());
    assert!(!conn.tickets().take_over(id).await.unwrap());
    drop(conn);
    client_says(&db.store, id, "Жду ответа").await;
    let ctx = context(&db.store, config(&server.uri(), serde_json::json!({})));
    assert_eq!(answer_pending(&ctx).await.unwrap().total(), 0);
    let ai = state(&db.store, id).await.0.unwrap();
    assert_eq!(
        (ai.state, ai.reason),
        (AiState::HandedOff, Some(HandoffReason::Operator))
    );
    assert!(log_rows(&db.store).await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn groq_без_режима_json_и_без_рассуждений_получает_второй_запрос() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": { "message": "reasoning_effort is not supported with this model", "type": "invalid_request_error" }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": { "message": "Failed to generate JSON", "code": "json_validate_failed" }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(completion(reply("Перезапустите приложение"), 300, 20))
        .mount(&server)
        .await;
    let client = client_with_email(&db.store, "i@example.com").await;
    let id = ticket(&db.store, client, "Висит подключение").await;
    let ctx = context(&db.store, config(&server.uri(), serde_json::json!({})));
    assert_eq!(answer_pending(&ctx).await.unwrap().answered, 1);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(body_of(&requests[0]).get("reasoning_effort").is_some());
    assert!(body_of(&requests[1]).get("reasoning_effort").is_none());
    assert!(body_of(&requests[1]).get("response_format").is_some());
    assert!(body_of(&requests[2]).get("response_format").is_none());
    assert_eq!(
        state(&db.store, id).await.1.last().map(|m| m.1.clone()),
        Some("Перезапустите приложение".to_string())
    );
    db.cleanup().await;
}

#[tokio::test]
async fn ответ_на_устаревший_снимок_не_публикуется() {
    let Some(db) = setup().await else { return };
    let client = client_with_email(&db.store, "j@example.com").await;
    let id = ticket(&db.store, client, "Первое").await;
    let mut conn = db.store.conn().await.unwrap();
    let claimed = store::claim(&mut conn, 120).await.unwrap().unwrap();
    assert_eq!(claimed.ticket_id, id);
    assert!(store::claim(&mut conn, 120).await.unwrap().is_none());
    drop(conn);
    client_says(&db.store, id, "Второе").await;
    let mut tx = db.store.begin().await.unwrap();
    let saved = store::answer(
        &mut tx,
        &claimed,
        "Ответ на первое",
        &Spent {
            model: "openai/gpt-oss-20b",
            prompt_tokens: 50,
            completion_tokens: 5,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(saved, Saved::Stale);
    let (_, messages) = state(&db.store, id).await;
    assert!(messages.iter().all(|m| m.0 == MessageAuthor::Client));
    let mut conn = db.store.conn().await.unwrap();
    let again: Claimed = store::claim(&mut conn, 120).await.unwrap().unwrap();
    assert!(again.last_message_id > claimed.last_message_id);
    let usage = store::usage(&mut conn, Utc::now() - ChronoDuration::hours(1))
        .await
        .unwrap();
    assert_eq!((usage.answers, usage.tokens), (0, 55));
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn выключенный_модуль_отдаёт_свои_обращения_операторам() {
    let Some(db) = setup().await else { return };
    let client = client_with_email(&db.store, "k@example.com").await;
    let id = ticket(&db.store, client, "Помогите").await;
    let ctx = ModuleContext {
        store: db.store.clone(),
        core: Arc::new(AiCore {
            enabled: false,
            config: config("http://127.0.0.1:1", serde_json::json!({})),
            settings: settings(),
        }),
        currency: Currency::Rub,
    };
    assert_eq!(answer_pending(&ctx).await.unwrap().total(), 0);
    AiModule.run_task(&ctx, RELEASE_TASK).await.unwrap();
    let ai = state(&db.store, id).await.0.unwrap();
    assert_eq!(
        (ai.state, ai.reason),
        (AiState::HandedOff, Some(HandoffReason::Disabled))
    );
    assert!(log_rows(&db.store).await.is_empty());
    db.cleanup().await;
}

#[test]
fn настройки_модуля_проверяются() {
    let module = AiModule;
    let ok = config("https://api.groq.com/openai/v1", serde_json::json!({}));
    assert!(module.validate_config(&ok).is_ok());
    let no_key = ModuleConfig::from_value(&serde_json::json!({}));
    let err = module.validate_config(&no_key).unwrap_err().to_string();
    assert!(err.contains("«API-ключ»"), "{err}");
    for (key, value) in [
        ("max_tokens", "50"),
        ("max_tokens", "1.5"),
        ("client_daily", "0"),
        ("daily_tokens", "abc"),
    ] {
        let bad = config("https://x", serde_json::json!({ key: value }));
        assert!(module.validate_config(&bad).is_err(), "{key}={value}");
    }
    let long = config(
        "https://x",
        serde_json::json!({ "instructions": "а".repeat(INSTRUCTIONS_MAX + 1) }),
    );
    assert!(module.validate_config(&long).is_err());
    let parsed = Settings::from_config(&config(
        "https://x/",
        serde_json::json!({ "max_tokens": "600", "miniapp": false }),
    ));
    assert_eq!(parsed.max_tokens, 600);
    assert!(parsed.takes(TicketChannel::Cabinet));
    assert!(!parsed.takes(TicketChannel::Miniapp));
    assert!(!parsed.takes(TicketChannel::Bot));
    assert_eq!(parsed.model, "openai/gpt-oss-120b");
    assert_eq!(
        module
            .settings_schema()
            .iter()
            .filter(|f| f.is_secret())
            .map(|f| f.key)
            .collect::<Vec<_>>(),
        ["api_key"]
    );
}

#[test]
fn сутки_считаются_по_часам_панели() {
    let offset = FixedOffset::east_opt(3 * 3600).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 11, 18, 22, 30, 0).unwrap();
    assert_eq!(
        day_start(now, offset),
        Utc.with_ymd_and_hms(2026, 11, 18, 21, 0, 0).unwrap()
    );
    let morning = Utc.with_ymd_and_hms(2026, 11, 18, 5, 0, 0).unwrap();
    assert_eq!(
        day_start(morning, offset),
        Utc.with_ymd_and_hms(2026, 11, 17, 21, 0, 0).unwrap()
    );
}

#[test]
fn реестр_показывает_ии_чат_только_с_ключом_и_в_нужных_каналах() {
    let registry = ModuleRegistry::new(vec![Arc::new(AiModule)]).unwrap();
    let row = |enabled: bool, config: serde_json::Value| sn_store::ModuleStateRow {
        module: "ai".into(),
        enabled,
        config,
        updated_at: Utc::now(),
    };
    registry.apply_rows(&[row(true, serde_json::json!({ "cabinet": true }))]);
    assert!(!takes_ticket(&registry, TicketChannel::Cabinet));
    registry.apply_rows(&[row(
        true,
        serde_json::json!({ "api_key": "gsk", "miniapp": false }),
    )]);
    assert!(takes_ticket(&registry, TicketChannel::Cabinet));
    assert!(!takes_ticket(&registry, TicketChannel::Miniapp));
    assert!(!takes_ticket(&registry, TicketChannel::Bot));
    assert_eq!(
        channels(&registry),
        Channels {
            cabinet: true,
            miniapp: false
        }
    );
    registry.apply_rows(&[row(false, serde_json::json!({ "api_key": "gsk" }))]);
    assert_eq!(channels(&registry), Channels::default());
    let job = wake_job(TicketId(5));
    assert!(matches!(
        job,
        JobPayload::ModuleTask { module: ModuleId::Ai, ref task, .. } if task == ANSWER_TASK
    ));
}

#[test]
fn старые_настройки_без_провайдера_работают_как_раньше() {
    let old = ModuleConfig::from_value(&serde_json::json!({
        "api_key": "gsk_old_key",
        "model": "openai/gpt-oss-20b",
        "api_url": "https://proxy.example.com/openai/v1/",
        "max_tokens": "800",
    }));
    assert!(AiModule.validate_config(&old).is_ok());
    let settings = Settings::from_config(&old);
    assert_eq!(settings.provider, Provider::Groq);
    assert_eq!(settings.model, "openai/gpt-oss-20b");
    assert_eq!(settings.api_url, "https://proxy.example.com/openai/v1");
    assert_eq!(settings.max_tokens, 800);
    assert!(settings.ready());
    let bare = Settings::from_config(&ModuleConfig::from_value(
        &serde_json::json!({ "api_key": "gsk_old_key" }),
    ));
    assert_eq!(
        (bare.provider, bare.model.as_str(), bare.api_url.as_str()),
        (
            Provider::Groq,
            "openai/gpt-oss-120b",
            "https://api.groq.com/openai/v1"
        )
    );
    let gemini = Settings::from_config(&ModuleConfig::from_value(&serde_json::json!({
        "api_key": "k",
        "provider": "gemini",
        "model": "models/gemini-3.8-flash",
    })));
    assert_eq!(
        (gemini.model.as_str(), gemini.api_url.as_str()),
        (
            "gemini-3.8-flash",
            "https://generativelanguage.googleapis.com/v1beta/openai"
        )
    );
    for provider in Provider::ALL.into_iter().filter(|p| *p != Provider::Custom) {
        let settings = Settings::from_config(&ModuleConfig::from_value(
            &serde_json::json!({ "api_key": "k", "provider": provider.code() }),
        ));
        assert_eq!(Some(settings.model.as_str()), provider.default_model());
        assert_eq!(Some(settings.api_url.as_str()), provider.base_url());
        assert!(settings.ready());
    }
    let schema = AiModule.settings_schema();
    let field = |key: &str| schema.iter().find(|f| f.key == key).unwrap();
    assert_eq!(field("provider").default, Some("groq"));
    assert_eq!(field("provider").labels.map(<[&str]>::len), Some(7));
    let model = field("model").depends.unwrap();
    assert_eq!(
        model.case("anthropic").and_then(|c| c.placeholder),
        Some("claude-haiku-5-5")
    );
    assert!(
        model
            .case("openrouter")
            .and_then(|c| c.hint)
            .is_some_and(|h| h.contains(":free") && h.contains("обучать"))
    );
    assert!(
        model
            .case("gemini")
            .and_then(|c| c.hint)
            .is_some_and(|h| h.contains("бесплатном уровне"))
    );
    for provider in Provider::ALL {
        if let Some(default) = provider.default_model() {
            assert_eq!(
                model.case(provider.code()).and_then(|c| c.placeholder),
                Some(default)
            );
        }
        let url = field("api_url").depends.unwrap();
        assert_eq!(
            url.case(provider.code()).and_then(|c| c.placeholder),
            provider.base_url().or(Some("https://llm.example.com/v1"))
        );
        let key = field("api_key").depends.unwrap();
        if let Some(page) = provider.keys_page() {
            assert!(
                key.case(provider.code())
                    .and_then(|c| c.hint)
                    .is_some_and(|h| h.contains(page))
            );
        }
    }
    let texts: String = schema
        .iter()
        .map(|f| format!("{} {}", f.label, f.hint))
        .collect();
    assert!(!texts.contains("Groq"), "{texts}");
}

#[test]
fn свой_адрес_только_https_и_ключ_не_переезжает_на_новый_адрес() {
    let module = AiModule;
    let custom = |extra: serde_json::Value| {
        let mut base = serde_json::json!({ "api_key": "k", "provider": "custom" });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        ModuleConfig::from_value(&base)
    };
    let err = module
        .validate_config(&custom(serde_json::json!({})))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("«Адрес API»") && err.contains("«Модель»"),
        "{err}"
    );
    assert!(!Settings::from_config(&custom(serde_json::json!({}))).ready());
    let good = custom(serde_json::json!({
        "api_url": "https://llm.example.com/v1",
        "model": "llama-4",
    }));
    assert!(module.validate_config(&good).is_ok());
    assert!(Settings::from_config(&good).ready());
    let http = module
        .validate_config(&custom(serde_json::json!({
            "api_url": "http://llm.example.com/v1",
            "model": "llama-4",
        })))
        .unwrap_err()
        .to_string();
    assert!(http.contains("https://"), "{http}");
    assert!(
        module
            .validate_config(&custom(serde_json::json!({
                "api_url": "https://llm.example.com/v1",
                "model": "llama 4",
            })))
            .is_err()
    );
    assert!(
        module
            .validate_config(&ModuleConfig::from_value(
                &serde_json::json!({ "api_key": "k", "provider": "mistral" })
            ))
            .is_err()
    );

    let groq = ModuleConfig::from_value(&serde_json::json!({}));
    let moved = ModuleConfig::from_value(
        &serde_json::json!({ "api_key": "gsk_old_key", "provider": "gemini" }),
    );
    let err = module
        .validate_change(&groq, &moved, &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("вставьте ключ заново"), "{err}");
    assert!(module.validate_change(&groq, &moved, &["api_key"]).is_ok());
    let proxied = ModuleConfig::from_value(&serde_json::json!({
        "api_key": "gsk_old_key",
        "api_url": "https://proxy.example.com/v1",
    }));
    assert!(module.validate_change(&groq, &proxied, &[]).is_err());
    let same = ModuleConfig::from_value(&serde_json::json!({
        "api_key": "gsk_old_key",
        "model": "openai/gpt-oss-20b",
        "api_url": "https://api.groq.com/openai/v1/",
    }));
    assert!(module.validate_change(&groq, &same, &[]).is_ok());

    let saved_config = ModuleConfig::from_value(&serde_json::json!({ "api_key": "gsk_saved_key" }));
    let saved = Settings::from_config(&saved_config);
    let request = |provider: Option<&str>, url: Option<&str>| routes::ModelsRequest {
        provider: provider.map(str::to_string),
        api_url: url.map(str::to_string),
        api_key: None,
        model: None,
        refresh: false,
    };
    let same_form = routes::form_settings(&saved_config, &request(Some("groq"), Some("")));
    assert_eq!(
        routes::check_key(&saved, &same_form, None).unwrap(),
        "gsk_saved_key"
    );
    let other = routes::form_settings(&saved_config, &request(Some("openai"), None));
    assert_eq!(other.provider, Provider::OpenAi);
    assert_eq!(other.model, "gpt-6-luna");
    assert_eq!(
        routes::check_key(&saved, &other, None).unwrap_err().0,
        "ai.key_needed"
    );
    assert_eq!(
        routes::check_key(&saved, &other, Some("sk-new-key")).unwrap(),
        "sk-new-key"
    );
    let proxy = routes::form_settings(
        &saved_config,
        &request(None, Some("https://proxy.example.com/v1")),
    );
    assert_eq!(
        routes::check_key(&saved, &proxy, None).unwrap_err().0,
        "ai.key_needed"
    );
    let empty = Settings::from_config(&ModuleConfig::default());
    assert_eq!(
        routes::check_key(&empty, &empty, None).unwrap_err().0,
        "ai.no_key"
    );
}

#[tokio::test]
async fn claude_отвечает_через_messages_api() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .and(header("x-api-key", "sk-ant-test-key"))
        .and(header("anthropic-version", "2026-10-21"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-haiku-5-5",
            "content": [
                { "type": "thinking", "thinking": "", "signature": "sig" },
                { "type": "text", "text": reply("Откройте приложение и нажмите «Подключить».").to_string() }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 900, "output_tokens": 40 }
        })))
        .expect(1)
        .mount(&server)
        .await;
    tariff(&db.store).await;
    let client = client_with_email(&db.store, "claude@example.com").await;
    let id = ticket(&db.store, client, "Как подключиться с телефона?").await;
    let ctx = context(
        &db.store,
        config(
            &server.uri(),
            serde_json::json!({ "provider": "anthropic", "api_key": "sk-ant-test-key" }),
        ),
    );
    assert_eq!(answer_pending(&ctx).await.unwrap().answered, 1);
    let requests = server.received_requests().await.unwrap();
    let body = body_of(&requests[0]);
    assert_eq!(body["model"], "claude-haiku-5-5");
    assert!(
        body["system"]
            .as_str()
            .is_some_and(|s| s.contains("Данные сервиса"))
    );
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(
        body["messages"][0]["content"],
        "Как подключиться с телефона?"
    );
    assert_eq!(body["output_config"]["effort"], "low");
    assert_eq!(body["max_tokens"], 1000);
    assert!(body.get("temperature").is_none());
    assert!(requests[0].headers.get("authorization").is_none());
    let (ai, messages) = state(&db.store, id).await;
    assert_eq!(
        messages.last().map(|m| m.1.as_str()),
        Some("Откройте приложение и нажмите «Подключить».")
    );
    assert!(ai.is_some_and(|a| a.answering()));
    let rows = log_rows(&db.store).await;
    assert_eq!(
        (rows[0].0.as_str(), rows[0].2, rows[0].3),
        ("answered", 900, 40)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn openai_повторяет_запрос_без_рассуждения_по_своей_ошибке() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {
                "message": "Unsupported parameter: 'reasoning_effort' is not supported with this model.",
                "type": "invalid_request_error",
                "param": "reasoning_effort",
                "code": "unsupported_parameter"
            }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(completion(reply("Смените сервер в приложении."), 400, 30))
        .mount(&server)
        .await;
    let client = client_with_email(&db.store, "openai@example.com").await;
    let id = ticket(&db.store, client, "Медленно работает").await;
    let ctx = context(
        &db.store,
        config(
            &server.uri(),
            serde_json::json!({ "provider": "openai", "model": "gpt-4.1" }),
        ),
    );
    assert_eq!(answer_pending(&ctx).await.unwrap().answered, 1);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let first = body_of(&requests[0]);
    assert_eq!(first["model"], "gpt-4.1");
    assert_eq!(first["reasoning_effort"], "low");
    assert_eq!(first["max_completion_tokens"], 1000);
    assert!(first.get("temperature").is_none());
    assert_eq!(first["response_format"]["type"], "json_schema");
    let second = body_of(&requests[1]);
    assert!(second.get("reasoning_effort").is_none());
    assert_eq!(second["response_format"]["type"], "json_schema");
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer gsk_test_key")
    );
    assert_eq!(
        state(&db.store, id).await.1.last().map(|m| m.1.clone()),
        Some("Смените сервер в приложении.".to_string())
    );
    db.cleanup().await;
}

#[tokio::test]
async fn deepseek_с_пустым_ответом_в_режиме_json_спрашивают_без_него() {
    let Some(db) = setup().await else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "   " },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 500, "completion_tokens": 10, "total_tokens": 510 }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(completion(
            reply("Обновите подписку в приложении."),
            500,
            25,
        ))
        .mount(&server)
        .await;
    let client = client_with_email(&db.store, "deepseek@example.com").await;
    ticket(&db.store, client, "Не обновляется список серверов").await;
    let ctx = context(
        &db.store,
        config(&server.uri(), serde_json::json!({ "provider": "deepseek" })),
    );
    assert_eq!(answer_pending(&ctx).await.unwrap().answered, 1);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let first = body_of(&requests[0]);
    assert_eq!(first["model"], "deepseek-flash");
    assert_eq!(first["thinking"], serde_json::json!({ "type": "disabled" }));
    assert_eq!(first["response_format"]["type"], "json_object");
    assert_eq!(first["max_tokens"], 1000);
    assert!(body_of(&requests[1]).get("response_format").is_none());
    let rows = log_rows(&db.store).await;
    assert_eq!((rows[0].2, rows[0].3), (1_000, 35));
    db.cleanup().await;
}

#[tokio::test]
async fn ключ_не_попадает_в_ошибки_и_не_уходит_по_перенаправлению() {
    let Some(db) = setup().await else { return };
    let elsewhere = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completion(reply("лишний ответ"), 1, 1))
        .expect(0)
        .mount(&elsewhere)
        .await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/chat/completions", elsewhere.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    let first = client_with_email(&db.store, "redirect@example.com").await;
    ticket(&db.store, first, "Не работает").await;
    let ctx = context(&db.store, config(&server.uri(), serde_json::json!({})));
    assert_eq!(answer_pending(&ctx).await.unwrap().handed_off, 1);
    let rows = log_rows(&db.store).await;
    assert!(
        rows[0]
            .4
            .as_deref()
            .is_some_and(|e| e.contains("перенаправляет")),
        "{rows:?}"
    );
    assert!(elsewhere.received_requests().await.unwrap().is_empty());

    let echo = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": { "message": "Bad key gsk_test_key in request" }
        })))
        .mount(&echo)
        .await;
    let second = client_with_email(&db.store, "echo@example.com").await;
    ticket(&db.store, second, "Тоже не работает").await;
    let ctx = context(&db.store, config(&echo.uri(), serde_json::json!({})));
    assert_eq!(answer_pending(&ctx).await.unwrap().handed_off, 1);
    let rows = log_rows(&db.store).await;
    assert!(
        rows.iter()
            .all(|r| r.4.as_deref().is_none_or(|e| !e.contains("gsk_test_key"))),
        "{rows:?}"
    );
    assert!(
        rows[1]
            .4
            .as_deref()
            .is_some_and(|e| e.contains("Bad key *** in request")),
        "{rows:?}"
    );
    let mut conn = db.store.conn().await.unwrap();
    assert!(
        !store::last_error(&mut conn)
            .await
            .unwrap()
            .unwrap()
            .text
            .contains("gsk_test_key")
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn в_подсказке_перечислены_включённые_варианты_пробного_периода() {
    let Some(db) = sn_store::testing::test_store().await else {
        return;
    };
    let variant = |id: u32, name: &str, enabled: bool, days: u32, devices: u32| {
        sn_domain::settings::TrialVariant {
            id,
            name: name.into(),
            enabled,
            days,
            squads: vec![SquadId(Uuid::from_u128(u128::from(id)))],
            traffic: TrafficPolicy::UNLIMITED,
            devices,
        }
    };
    let mut core = settings();
    core.trial = TrialSettings {
        enabled: true,
        variants: vec![
            variant(1, "Россия", true, 3, 1),
            variant(2, "Европа", true, 1, 2),
            variant(3, "Черновик", false, 9, 1),
        ],
    };
    let mut conn = db.store.conn().await.unwrap();
    let several = prompt::facts(
        &mut conn,
        &core,
        sn_domain::access::ClientChannel::Bot,
        None,
    )
    .await
    .unwrap()
    .text;
    assert!(
        several.contains("Клиент выбирает один из вариантов"),
        "{several}"
    );
    assert!(several.contains("- Россия: 3"), "{several}");
    assert!(several.contains("- Европа: 1"), "{several}");
    assert!(several.contains("2 устройства"), "{several}");
    assert!(!several.contains("Черновик"), "{several}");
    core.trial.enabled = false;
    let none = prompt::facts(
        &mut conn,
        &core,
        sn_domain::access::ClientChannel::Bot,
        None,
    )
    .await
    .unwrap()
    .text;
    assert!(none.contains("Пробного периода нет."), "{none}");
    drop(conn);
    db.cleanup().await;
}
