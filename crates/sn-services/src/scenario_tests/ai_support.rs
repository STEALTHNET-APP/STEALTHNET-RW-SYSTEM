use super::*;
use crate::support::Delivered;
use sn_domain::support::{AiState, HandoffReason};
use sn_modules::catalog::ai::{self, AiModule};

async fn ai_env() -> Option<(Env, AdminId)> {
    let registry = ModuleRegistry::new(vec![Arc::new(AiModule)]).unwrap();
    let env = env_with(registry).await?;
    let admin = owner(&env.services).await;
    let ops = env.services.settings_ops();
    ops.update_module_config(
        ModuleId::Ai,
        &ModuleConfigUpdate {
            config: serde_json::from_value(serde_json::json!({ "api_key": "gsk-scenario" }))
                .unwrap(),
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Ai, true, admin).await.unwrap();
    Some((env, admin))
}

async fn open(
    services: &Services,
    client: ClientId,
    channel: TicketChannel,
    body: &str,
) -> sn_domain::ids::TicketId {
    services
        .support()
        .open(
            client,
            &OpenTicket {
                subscription_id: None,
                channel,
                subject: None,
                body: body.into(),
            },
        )
        .await
        .unwrap()
}

async fn wake_jobs(services: &Services) -> Vec<i64> {
    let mut conn = services.store().conn().await.unwrap();
    let payloads: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT payload FROM jobs WHERE kind = 'module_task' ORDER BY id")
            .fetch_all(conn.conn())
            .await
            .unwrap();
    payloads
        .into_iter()
        .filter(|p| p["module"] == "ai" && p["task"] == ai::ANSWER_TASK)
        .filter_map(|p| p["payload"]["ticket_id"].as_i64())
        .collect()
}

#[tokio::test]
async fn ии_берёт_только_обращения_кабинета_а_ответ_оператора_забирает_обращение() {
    let Some((env, admin)) = ai_env().await else {
        return;
    };
    let services = &env.services;
    let support = services.support();
    let client = telegram_client(services, 8_801).await;
    let cabinet = open(
        services,
        client,
        TicketChannel::Cabinet,
        "Не подключается ноутбук",
    )
    .await;
    let bot = open(services, client, TicketChannel::Bot, "Вопрос из бота").await;
    assert_eq!(wake_jobs(services).await, [cabinet.0]);
    let thread = support.thread(cabinet, Actor::Admin(admin)).await.unwrap();
    assert_eq!(thread.ai.map(|a| a.state), Some(AiState::Active));
    assert!(
        support
            .thread(bot, Actor::Admin(admin))
            .await
            .unwrap()
            .ai
            .is_none()
    );
    support
        .reply(
            cabinet,
            &ReplyRequest {
                body: "И на телефоне тоже".into(),
                close: false,
            },
            Actor::Client(client),
        )
        .await
        .unwrap();
    assert_eq!(wake_jobs(services).await, [cabinet.0, cabinet.0]);
    support
        .reply(
            cabinet,
            &ReplyRequest {
                body: "Здравствуйте! Сейчас посмотрю.".into(),
                close: false,
            },
            Actor::Admin(admin),
        )
        .await
        .unwrap();
    let ai = support
        .thread(cabinet, Actor::Admin(admin))
        .await
        .unwrap()
        .ai
        .unwrap();
    assert_eq!(
        (ai.state, ai.reason),
        (AiState::HandedOff, Some(HandoffReason::Operator))
    );
    support
        .reply(
            cabinet,
            &ReplyRequest {
                body: "Спасибо, жду".into(),
                close: false,
            },
            Actor::Client(client),
        )
        .await
        .unwrap();
    assert_eq!(wake_jobs(services).await.len(), 2);
    let third = open(services, client, TicketChannel::Cabinet, "Ещё вопрос").await;
    support.close(third, Actor::Admin(admin)).await.unwrap();
    support.reopen(third, Actor::Admin(admin)).await.unwrap();
    let ai = support
        .thread(third, Actor::Admin(admin))
        .await
        .unwrap()
        .ai
        .unwrap();
    assert_eq!(
        (ai.state, ai.reason),
        (AiState::HandedOff, Some(HandoffReason::Operator))
    );
    let mut conn = services.store().conn().await.unwrap();
    let logged: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_support_log")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(logged, 0);
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn админы_узнают_о_передаче_оператору_а_не_о_каждом_вопросе_к_ии() {
    let Some((env, admin)) = ai_env().await else {
        return;
    };
    let services = &env.services;
    let support = services.support();
    let alerts = services.alerts();
    let client = telegram_client(services, 8_802).await;
    assert!(alerts.collect().await.unwrap().is_empty());
    let cabinet = open(
        services,
        client,
        TicketChannel::Cabinet,
        "Как подключить телевизор?",
    )
    .await;
    let bot = open(services, client, TicketChannel::Bot, "Пишу из бота").await;
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].html.contains(&format!("Новое обращение №{}", bot.0)));
    support
        .ask_operator(cabinet, Actor::Client(client))
        .await
        .unwrap();
    let client_view = support
        .thread(cabinet, Actor::Client(client))
        .await
        .unwrap();
    let ai = client_view.ai.unwrap();
    assert_eq!(ai.state, AiState::HandedOff);
    assert_eq!(ai.reason, None);
    assert!(ai.handoff_at.is_some());
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0]
            .html
            .contains(&format!("ИИ передал обращение №{} оператору", cabinet.0)),
        "{}",
        got[0].html
    );
    assert!(got[0].html.contains("Причина: клиент попросил оператора"));
    assert!(got[0].html.contains("«Как подключить телевизор?»"));
    assert_eq!(got[0].path, format!("/admin/support?ticket={}", cabinet.0));
    support
        .reply(
            cabinet,
            &ReplyRequest {
                body: "Жду оператора".into(),
                close: false,
            },
            Actor::Client(client),
        )
        .await
        .unwrap();
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0]
            .html
            .contains(&format!("Новое сообщение в обращении №{}", cabinet.0))
    );
    support
        .ask_operator(cabinet, Actor::Client(client))
        .await
        .unwrap();
    assert!(alerts.collect().await.unwrap().is_empty());
    let other = telegram_client(services, 8_803).await;
    assert_eq!(
        code(support.ask_operator(cabinet, Actor::Client(other)).await),
        "not_found"
    );
    let (items, _) = support
        .list_view(TicketFilter::Waiting, Default::default(), None, None)
        .await
        .unwrap();
    let listed = items.items.iter().find(|i| i.ticket.id == cabinet).unwrap();
    assert_eq!(listed.ai.and_then(|a| a.reason), Some(HandoffReason::Human));
    let _ = admin;
    env.db.cleanup().await;
}

#[tokio::test]
async fn после_ответа_ии_просьба_позвать_оператора_возвращает_обращение_в_ожидание() {
    let Some((env, admin)) = ai_env().await else {
        return;
    };
    let services = &env.services;
    let support = services.support();
    let client = telegram_client(services, 8_804).await;
    let id = open(
        services,
        client,
        TicketChannel::Cabinet,
        "Как подключить айфон?",
    )
    .await;
    {
        let mut tx = services.store().begin().await.unwrap();
        let claimed = ai::store::claim(&mut tx, 120).await.unwrap().unwrap();
        ai::store::answer(
            &mut tx,
            &claimed,
            "Установите Happ и нажмите «Добавить в Happ»",
            &ai::store::Spent::NONE,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let waiting = |filter| async move {
        services
            .support()
            .list(filter, Default::default())
            .await
            .unwrap()
            .items
            .iter()
            .any(|t| t.id == id)
    };
    assert!(waiting(TicketFilter::Answered).await);
    support
        .ask_operator(id, Actor::Client(client))
        .await
        .unwrap();
    assert!(waiting(TicketFilter::Waiting).await);
    let thread = support.thread(id, Actor::Client(client)).await.unwrap();
    assert_eq!(
        thread.messages.last().map(|m| m.author),
        Some(sn_domain::support::MessageAuthor::Ai)
    );
    assert_eq!(thread.ticket.filter(), TicketFilter::Waiting);
    support
        .reply(
            id,
            &ReplyRequest {
                body: "Здравствуйте, на связи оператор".into(),
                close: false,
            },
            Actor::Admin(admin),
        )
        .await
        .unwrap();
    assert!(waiting(TicketFilter::Answered).await);
    env.db.cleanup().await;
}

#[tokio::test]
async fn ответ_из_бота_на_уведомление_оператора_продолжает_обращение_из_мини_приложения() {
    let Some((env, admin)) = ai_env().await else {
        return;
    };
    let services = &env.services;
    let support = services.support();
    let client = telegram_client(services, 8_805).await;
    let miniapp = open(
        services,
        client,
        TicketChannel::Miniapp,
        "Не работает на телевизоре",
    )
    .await;
    support
        .reply(
            miniapp,
            &ReplyRequest {
                body: "Какая модель телевизора?".into(),
                close: false,
            },
            Actor::Admin(admin),
        )
        .await
        .unwrap();
    assert_eq!(support.telegram_ticket(client, None).await.unwrap(), None);
    assert_eq!(
        support
            .telegram_ticket(client, Some(miniapp))
            .await
            .unwrap(),
        Some(miniapp)
    );
    assert_eq!(
        support
            .from_telegram(client, "Samsung 2021 года", Some(miniapp))
            .await
            .unwrap(),
        crate::support::Delivered::Added(miniapp)
    );
    let thread = support.thread(miniapp, Actor::Admin(admin)).await.unwrap();
    assert_eq!(thread.messages.len(), 3);
    assert_eq!(thread.ticket.filter(), TicketFilter::Waiting);
    assert_eq!(wake_jobs(services).await, [miniapp.0]);
    let Delivered::Opened(fresh) = support
        .from_telegram(client, "И ещё вопрос", None)
        .await
        .unwrap()
    else {
        panic!("без выбранного обращения сообщение из бота открывает новое");
    };
    assert_ne!(fresh, miniapp);
    env.db.cleanup().await;
}

#[tokio::test]
async fn смена_провайдера_требует_ключ_заново_и_адрес_только_https() {
    let Some((env, admin)) = ai_env().await else {
        return;
    };
    let ops = env.services.settings_ops();
    let update = |config: serde_json::Value| ModuleConfigUpdate {
        config: serde_json::from_value(config).unwrap(),
        clear: Vec::new(),
    };
    let err = ops
        .update_module_config(
            ModuleId::Ai,
            &update(serde_json::json!({ "provider": "gemini" })),
            admin,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("вставьте ключ заново"), "{err}");
    let err = ops
        .update_module_config(
            ModuleId::Ai,
            &update(serde_json::json!({ "api_url": "https://proxy.example.com/v1" })),
            admin,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("вставьте ключ заново"), "{err}");
    let err = ops
        .update_module_config(
            ModuleId::Ai,
            &update(serde_json::json!({
                "provider": "custom",
                "api_key": "sk-custom-scenario",
                "api_url": "http://llm.example.com/v1",
                "model": "llama-4",
            })),
            admin,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("https://"), "{err}");
    ops.update_module_config(
        ModuleId::Ai,
        &update(serde_json::json!({
            "provider": "gemini",
            "api_key": "gemini-scenario-key",
            "model": "models/gemini-3.8-flash",
        })),
        admin,
    )
    .await
    .unwrap();
    let settings = ai::Settings::from_config(&env.services.modules().config(ModuleId::Ai));
    assert_eq!(settings.provider, ai::provider::Provider::Gemini);
    assert_eq!(settings.api_key, "gemini-scenario-key");
    assert_eq!(settings.model, "gemini-3.8-flash");
    assert_eq!(
        settings.api_url,
        "https://generativelanguage.googleapis.com/v1beta/openai"
    );
    ops.update_module_config(
        ModuleId::Ai,
        &update(serde_json::json!({ "model": "gemini-3.6-flash" })),
        admin,
    )
    .await
    .unwrap();
    assert_eq!(
        ai::Settings::from_config(&env.services.modules().config(ModuleId::Ai)).api_key,
        "gemini-scenario-key"
    );
}
