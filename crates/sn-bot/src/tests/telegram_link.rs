use super::client_letters::{decoded, fake_smtp, mail_on};
use super::*;
use sn_domain::client::NewIdentity;
use sn_domain::settings::keys;
use sn_services::auth::RequestMeta;
use sn_services::telegram_link::{TelegramLinkStart, TelegramLinkStatus};

const USER: i64 = 9_301;

struct Account {
    client: ClientId,
    principal: sn_domain::access::ClientPrincipal,
}

async fn site(services: &Services) {
    {
        let mut conn = services.store().conn().await.unwrap();
        for (key, value) in [
            (keys::BOT_USERNAME, serde_json::json!("stealth_test_bot")),
            (
                keys::PUBLIC_URL,
                serde_json::json!("https://vpn.example.com"),
            ),
            (
                keys::LANGUAGES,
                serde_json::json!({ "english": true, "default": "ru" }),
            ),
        ] {
            conn.settings().set(key, &value, None).await.unwrap();
        }
    }
    services.reload().await.unwrap();
}

async fn account(services: &Services, address: &str) -> Account {
    let mut tx = services.store().begin().await.unwrap();
    let client = tx
        .clients()
        .create(&sn_store::NewClient {
            locale: sn_domain::client::Locale::Ru,
            registration_source: RegistrationSource::Web,
            note: None,
        })
        .await
        .unwrap()
        .id;
    tx.clients()
        .add_identity(
            client,
            &NewIdentity::new(IdentityKind::Email, address, true).unwrap(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let login = services
        .auth()
        .client_session_for(
            client,
            sn_store::SessionChannel::Cabinet,
            &RequestMeta::default(),
            false,
        )
        .await
        .unwrap();
    Account {
        client,
        principal: services.auth().client_session(&login.token).await.unwrap(),
    }
}

async fn begin(services: &Services, account: &Account) -> (String, TelegramLinkStart) {
    let start = services
        .telegram_links()
        .start(&account.principal, &RequestMeta::default())
        .await
        .unwrap();
    let param = start.link.rsplit_once("?start=").unwrap().1.to_string();
    (param, start)
}

async fn state(
    services: &Services,
    account: &Account,
    start: &TelegramLinkStart,
) -> TelegramLinkStatus {
    services
        .telegram_links()
        .status(
            &account.principal,
            &start.poll_token,
            &RequestMeta::default(),
        )
        .await
        .unwrap()
}

async fn clients(services: &Services) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM clients")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn незнакомый_telegram_привязывается_без_нового_аккаунта() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    let owner = account(&services, "alexey@mail.example").await;
    let (param, start) = begin(&services, &owner).await;
    handle_update(&ctx, text_update(1, USER, &format!("/start {param}"))).await;
    let text = last_text(&server).await;
    for part in [
        "Привязка Telegram",
        "Аккаунт на vpn.example.com: a***@mail.example",
        "Ваш Telegram: Алексей (@morozov_a, Telegram 9301)",
        "Это не вход на сайт",
        "Если привязку начинали не вы, нажмите «Отменить»",
    ] {
        assert!(text.contains(part), "{text}");
    }
    assert!(!text.contains("alexey@"), "{text}");
    assert_eq!(last_labels(&server).await, ["Привязать", "Отменить"]);
    let buttons = last_buttons(&server).await;
    assert!(buttons.iter().all(|b| b.len() <= 64));
    assert!(client_of(&services, USER).await.is_none());
    assert_eq!(clients(&services).await, 1);
    handle_update(&ctx, callback_update(2, USER, &buttons[0])).await;
    let text = last_text(&server).await;
    assert!(text.contains("Telegram привязан."), "{text}");
    assert_eq!(client_of(&services, USER).await.unwrap().id, owner.client);
    assert_eq!(clients(&services).await, 1);
    assert!(matches!(
        state(&services, &owner, &start).await,
        TelegramLinkStatus::Linked { account } if account == "@morozov_a"
    ));
    handle_update(&ctx, callback_update(3, USER, &buttons[0])).await;
    assert!(last_text(&server).await.contains("уже привязан к вашему"));
    handle_update(&ctx, callback_update(4, USER, &buttons[1])).await;
    assert_eq!(client_of(&services, USER).await.unwrap().id, owner.client);
    handle_update(&ctx, text_update(5, USER, "/start")).await;
    assert!(last_text(&server).await.contains("Подписки пока нет"));
    assert_eq!(clients(&services).await, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn отказ_и_негодная_ссылка_не_заводят_клиента() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    let owner = account(&services, "alexey@mail.example").await;
    let (param, start) = begin(&services, &owner).await;
    handle_update(&ctx, text_update(1, USER, &format!("/start {param}"))).await;
    let buttons = last_buttons(&server).await;
    handle_update(&ctx, callback_update(2, USER, &buttons[1])).await;
    assert!(last_text(&server).await.contains("Привязка отменена"));
    assert!(last_buttons(&server).await.is_empty());
    assert_eq!(
        state(&services, &owner, &start).await,
        TelegramLinkStatus::Rejected
    );
    handle_update(&ctx, callback_update(3, USER, &buttons[0])).await;
    assert!(
        last_text(&server)
            .await
            .contains("Ссылка для привязки устарела")
    );
    for (id, wrong) in [
        "tglink_ffffffffffffffffffffffffffffffff",
        "tglink_",
        "tglink_мусор",
    ]
    .into_iter()
    .enumerate()
    {
        let id = i64::try_from(id).unwrap() + 10;
        handle_update(&ctx, text_update(id, USER, &format!("/start {wrong}"))).await;
        assert!(
            last_text(&server)
                .await
                .contains("Ссылка для привязки устарела")
        );
        assert!(last_buttons(&server).await.is_empty());
    }
    handle_update(&ctx, callback_update(20, USER, "tl:")).await;
    assert!(client_of(&services, USER).await.is_none());
    assert_eq!(clients(&services).await, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn кнопка_из_сообщения_относится_к_своему_запросу() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    let mine = account(&services, "alexey@mail.example").await;
    let foreign = account(&services, "boris@mail.example").await;
    let (param, _) = begin(&services, &mine).await;
    handle_update(&ctx, text_update(1, USER, &format!("/start {param}"))).await;
    let buttons = last_buttons(&server).await;
    let (other, _) = begin(&services, &foreign).await;
    handle_update(&ctx, text_update(2, USER, &format!("/start {other}"))).await;
    assert!(last_text(&server).await.contains("b***@mail.example"));
    handle_update(&ctx, callback_update(3, USER, &buttons[0])).await;
    assert_eq!(client_of(&services, USER).await.unwrap().id, mine.client);
    db.cleanup().await;
}

#[tokio::test]
async fn занятый_telegram_получает_понятный_отказ() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    handle_update(&ctx, text_update(1, USER, "/start")).await;
    let before = client_of(&services, USER).await.unwrap().id;
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO trial_claims (kind, value, client_id) VALUES ('telegram', '9301', $1)",
        )
        .bind(before.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let owner = account(&services, "alexey@mail.example").await;
    let (param, start) = begin(&services, &owner).await;
    handle_update(&ctx, text_update(2, USER, &format!("/start {param}"))).await;
    let text = last_text(&server).await;
    assert!(
        text.contains(
            "У этого Telegram уже есть свой аккаунт. Войдите в него или напишите в поддержку."
        ),
        "{text}"
    );
    assert!(!text.contains("a***"), "{text}");
    let code = param.trim_start_matches("tglink_");
    handle_update(&ctx, callback_update(3, USER, &format!("tl:ok:{code}"))).await;
    assert!(last_text(&server).await.contains("уже есть свой аккаунт"));
    assert_eq!(client_of(&services, USER).await.unwrap().id, before);
    assert!(matches!(
        state(&services, &owner, &start).await,
        TelegramLinkStatus::Pending { .. }
    ));
    db.cleanup().await;
}

#[tokio::test]
async fn документы_приглашения_и_метки_не_мешают_привязке() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                consent::DOCUMENTS_KEY,
                &serde_json::json!([{"title": "Оферта", "url": "https://example.com/offer"}]),
                None,
            )
            .await
            .unwrap();
    }
    site(&services).await;
    let owner = account(&services, "alexey@mail.example").await;
    let (param, _) = begin(&services, &owner).await;
    handle_update(
        &ctx,
        text_update(1, USER, &format!("/start {param}__ref_FRIEND42__s_vk")),
    )
    .await;
    assert_eq!(last_labels(&server).await, ["Привязать", "Отменить"]);
    let buttons = last_buttons(&server).await;
    handle_update(&ctx, callback_update(2, USER, &buttons[0])).await;
    assert!(last_text(&server).await.contains("Telegram привязан."));
    handle_update(&ctx, text_update(3, USER, "/start")).await;
    assert!(last_text(&server).await.contains("Оферта"));
    services.events().dispatch_pending(50).await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    assert!(
        utm::for_client(&mut conn, owner.client)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn привязка_по_английски() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    let owner = account(&services, "alexey@mail.example").await;
    let (param, _) = begin(&services, &owner).await;
    let english = |id: i64, text: String| -> Update {
        serde_json::from_value(serde_json::json!({
            "update_id": id,
            "message": {
                "message_id": id,
                "from": { "id": USER, "is_bot": false, "first_name": "Ann", "language_code": "en" },
                "chat": { "id": USER, "type": "private" }, "date": 1, "text": text
            }
        }))
        .unwrap()
    };
    handle_update(&ctx, english(1, format!("/start {param}"))).await;
    let text = last_text(&server).await;
    assert!(
        text.contains("Account on vpn.example.com: a***@mail.example"),
        "{text}"
    );
    assert!(
        text.contains("Your Telegram: Ann (Telegram 9301)"),
        "{text}"
    );
    assert_eq!(last_labels(&server).await, ["Link", "Cancel"]);
    handle_update(&ctx, english(2, "/start tglink_0".into())).await;
    let text = last_text(&server).await;
    assert!(text.contains("This link has expired"), "{text}");
    db.cleanup().await;
}

#[tokio::test]
async fn о_привязке_telegram_приходит_письмо_а_не_сообщение() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    let (port, letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({})).await;
    let owner = account(&services, "alexey@mail.example").await;
    let (param, _) = begin(&services, &owner).await;
    handle_update(&ctx, text_update(1, USER, &format!("/start {param}"))).await;
    let buttons = last_buttons(&server).await;
    handle_update(&ctx, callback_update(2, USER, &buttons[0])).await;
    let before = sent(&server).await.len();
    let payload: serde_json::Value = {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT payload FROM jobs WHERE payload->>'template' = $1")
            .bind(notify::templates::TELEGRAM_LINKED)
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    let handler = NotifyHandler::new(&BotConfig {
        api_base: server.uri(),
        ..BotConfig::default()
    });
    let job: JobPayload = serde_json::from_value(payload).unwrap();
    assert_eq!(handler.handle(&services, &job, 1).await, JobResult::Done);
    assert_eq!(sent(&server).await.len(), before);
    let all = letters.lock().unwrap().clone();
    assert_eq!(all.len(), 1);
    assert!(all[0].contains("alexey@mail.example"));
    let letter = decoded(&all[0]);
    assert!(
        letter.contains("привязан вход через Telegram: @morozov_a"),
        "{letter}"
    );
    assert!(
        letter.contains("https://vpn.example.com/cabinet/security"),
        "{letter}"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn пустой_аккаунт_бота_закрывается_и_бот_открывает_аккаунт_с_сайта() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    handle_update(&ctx, text_update(1, USER, "/start")).await;
    services.events().dispatch_pending(100).await.unwrap();
    let empty = client_of(&services, USER).await.unwrap().id;
    let owner = account(&services, "alexey@mail.example").await;
    let (param, _) = begin(&services, &owner).await;
    handle_update(&ctx, text_update(2, USER, &format!("/start {param}"))).await;
    let text = last_text(&server).await;
    assert!(
        text.contains("Telegram перейдёт к аккаунту на сайте, а пустой аккаунт бота закроется"),
        "{text}"
    );
    let buttons = last_buttons(&server).await;
    handle_update(&ctx, callback_update(3, USER, &buttons[0])).await;
    assert!(last_text(&server).await.contains("Telegram привязан."));
    assert_eq!(client_of(&services, USER).await.unwrap().id, owner.client);
    let mut conn = services.store().conn().await.unwrap();
    let left: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM clients WHERE id = $1)")
        .bind(empty.0)
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert!(!left);
    drop(conn);
    handle_update(&ctx, text_update(4, USER, "/start")).await;
    assert!(last_text(&server).await.contains("Подписки пока нет"));
    assert_eq!(clients(&services).await, 1);
    db.cleanup().await;
}
