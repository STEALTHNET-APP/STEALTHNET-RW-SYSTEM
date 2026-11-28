use super::*;
use crate::broadcast::TelegramSender;
use sn_modules::catalog::broadcast::{
    self, Audience, BotSection, ButtonSpec, Channel, Draft, Engine, EngineConfig, Sender, Status,
};
use sn_modules::{BotSender, Delivery, Media, MediaFile, OutButton, Outgoing};
use std::time::Instant;
use tokio::sync::oneshot;
use wiremock::matchers::{body_partial_json, body_string_contains};

fn fail(code: u16, description: &str, retry_after: Option<u64>) -> ResponseTemplate {
    let mut body =
        serde_json::json!({ "ok": false, "error_code": code, "description": description });
    if let Some(seconds) = retry_after {
        body["parameters"] = serde_json::json!({ "retry_after": seconds });
    }
    ResponseTemplate::new(code).set_body_json(body)
}

async fn person(services: &Services, telegram_id: i64) -> ClientId {
    let (client, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: telegram_id,
                username: None,
                first_name: Some("Алексей".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap();
    client.id
}

async fn requests(server: &MockServer, name: &str) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with(&format!("/{name}")))
        .collect()
}

async fn bodies(server: &MockServer, name: &str) -> Vec<serde_json::Value> {
    requests(server, name)
        .await
        .into_iter()
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

fn engine_config() -> EngineConfig {
    EngineConfig {
        per_second: 100,
        idle: std::time::Duration::from_millis(40),
        retry_pause: std::time::Duration::from_millis(40),
        lease: std::time::Duration::from_secs(2),
        grace: std::time::Duration::from_secs(2),
        ..EngineConfig::default()
    }
}

async fn send_all(services: &Services, server: &MockServer, draft: &Draft) -> broadcast::Broadcast {
    let ctx = services.module_context();
    let created = broadcast::create(&ctx, draft, None).await.unwrap();
    broadcast::start(&ctx, created.id, None, None)
        .await
        .unwrap();
    let sender: Arc<dyn BotSender> = Arc::new(TelegramSender::new(services.clone(), &server.uri()));
    let (stop, stopped) = oneshot::channel::<()>();
    let handle = tokio::spawn(
        Engine::new(ctx.clone(), Sender::Bot(sender), || true, engine_config()).run(async move {
            let _ = stopped.await;
        }),
    );
    let started = Instant::now();
    let done = loop {
        let view = broadcast::view(&ctx, created.id).await.unwrap();
        if view.status == Status::Done {
            break view;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "рассылка не закончилась: {view:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let _ = stop.send(());
    handle.await.unwrap();
    done
}

#[tokio::test]
async fn рассылка_уходит_через_bot_api_и_отмечает_заблокировавших() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_partial_json(serde_json::json!({ "chat_id": 802 })))
        .respond_with(fail(403, "Forbidden: bot was blocked by the user", None))
        .with_priority(1)
        .mount(&server)
        .await;
    for id in [801, 802, 803] {
        person(&services, id).await;
    }
    let blocked = client_of(&services, 802).await.unwrap().id;
    let done = send_all(
        &services,
        &server,
        &Draft {
            channel: Channel::Bot,
            subject: None,
            text: "<b>Новые серверы</b> уже работают".into(),
            image_id: None,
            buttons: vec![
                ButtonSpec {
                    text: "Продлить".into(),
                    url: None,
                    section: Some(BotSection::Buy),
                    style: None,
                    emoji_id: None,
                },
                ButtonSpec {
                    text: "Новости".into(),
                    url: Some("t.me/stealth_news".into()),
                    section: None,
                    style: None,
                    emoji_id: None,
                },
            ],
            audience: Audience::All,
        },
    )
    .await;
    assert_eq!(
        (
            done.progress.sent,
            done.progress.blocked,
            done.progress.failed
        ),
        (2, 1, 0)
    );
    let sent = bodies(&server, "sendMessage").await;
    assert_eq!(sent.len(), 3);
    let mut chats: Vec<i64> = sent.iter().filter_map(|b| b["chat_id"].as_i64()).collect();
    chats.sort_unstable();
    assert_eq!(chats, [801, 802, 803]);
    for message in &sent {
        assert_eq!(message["parse_mode"], "HTML");
        assert_eq!(message["text"], "<b>Новые серверы</b> уже работают");
        assert_eq!(
            message["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
            "n:buy"
        );
        assert_eq!(
            message["reply_markup"]["inline_keyboard"][1][0]["url"],
            "https://t.me/stealth_news"
        );
    }
    let mut conn = db.store.conn().await.unwrap();
    assert!(conn.clients().get(blocked).await.unwrap().bot_unreachable);
    drop(conn);
    let reach = broadcast::reach(&services.module_context(), Audience::All)
        .await
        .unwrap();
    assert_eq!((reach.recipients, reach.stopped), (2, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn ответ_429_выдерживает_паузу_и_не_дублирует() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_partial_json(serde_json::json!({ "chat_id": 902 })))
        .respond_with(fail(429, "Too Many Requests: retry after 1", Some(1)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    for id in [901, 902, 903, 904] {
        person(&services, id).await;
    }
    let started = Instant::now();
    let done = send_all(
        &services,
        &server,
        &Draft {
            channel: Channel::Bot,
            subject: None,
            text: "Плановые работы ночью".into(),
            image_id: None,
            buttons: Vec::new(),
            audience: Audience::All,
        },
    )
    .await;
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1),
        "пауза retry_after не выдержана"
    );
    assert_eq!(done.progress.sent, 4);
    let sent = bodies(&server, "sendMessage").await;
    let mut chats: Vec<i64> = sent.iter().filter_map(|b| b["chat_id"].as_i64()).collect();
    assert_eq!(chats.len(), 5);
    chats.sort_unstable();
    assert_eq!(chats, [901, 902, 902, 903, 904]);
    db.cleanup().await;
}

#[tokio::test]
async fn картинка_уходит_файлом_один_раз_а_дальше_по_коду() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendPhoto$"))
        .respond_with(ok(serde_json::json!({
            "message_id": 300, "chat": {"id": 1, "type": "private"}, "date": 0,
            "photo": [
                {"file_id": "small-id", "file_unique_id": "u1", "width": 90, "height": 90},
                {"file_id": "big-id", "file_unique_id": "u2", "width": 800, "height": 600}
            ]
        })))
        .mount(&server)
        .await;
    for id in [1001, 1002, 1003] {
        person(&services, id).await;
    }
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&800u32.to_be_bytes());
    png.extend_from_slice(&600u32.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    let image = broadcast::upload_image(&services.module_context(), &png, None)
        .await
        .unwrap();
    let done = send_all(
        &services,
        &server,
        &Draft {
            channel: Channel::Bot,
            subject: None,
            text: "Смотрите, что нового".into(),
            image_id: Some(image.id),
            buttons: Vec::new(),
            audience: Audience::All,
        },
    )
    .await;
    assert_eq!(done.progress.sent, 3);
    let photos = requests(&server, "sendPhoto").await;
    assert_eq!(photos.len(), 3);
    let kind = photos[0]
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(kind.starts_with("multipart/form-data"), "{kind}");
    for later in &photos[1..] {
        let json: serde_json::Value = serde_json::from_slice(&later.body).unwrap();
        assert_eq!(json["photo"], "big-id");
        assert_eq!(json["caption"], "Смотрите, что нового");
    }
    assert!(bodies(&server, "sendMessage").await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn видео_уходит_через_send_video_файлом_один_раз_а_дальше_по_коду() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendVideo$"))
        .respond_with(ok(serde_json::json!({
            "message_id": 310, "chat": {"id": 1, "type": "private"}, "date": 0,
            "video": {
                "file_id": "video-id", "file_unique_id": "v1",
                "width": 1280, "height": 720, "duration": 34
            }
        })))
        .mount(&server)
        .await;
    for id in [1101, 1102, 1103] {
        person(&services, id).await;
    }
    let bytes = sn_domain::video::sample::mp4(1280, 720, 34, true);
    let video = broadcast::upload_media(&services.module_context(), &bytes, None)
        .await
        .unwrap();
    let done = send_all(
        &services,
        &server,
        &Draft {
            channel: Channel::Bot,
            subject: None,
            text: "<b>Новый ролик</b> о сервисе".into(),
            image_id: Some(video.id),
            buttons: vec![ButtonSpec {
                text: "Продлить".into(),
                url: None,
                section: Some(BotSection::Buy),
                style: None,
                emoji_id: None,
            }],
            audience: Audience::All,
        },
    )
    .await;
    assert_eq!(done.progress.sent, 3);
    let videos = requests(&server, "sendVideo").await;
    assert_eq!(videos.len(), 3);
    let kind = videos[0]
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(kind.starts_with("multipart/form-data"), "{kind}");
    let form = String::from_utf8_lossy(&videos[0].body).into_owned();
    for part in [
        "name=\"video\"; filename=\"video.mp4\"",
        "Content-Type: video/mp4",
        "name=\"width\"\r\n\r\n1280\r\n",
        "name=\"height\"\r\n\r\n720\r\n",
        "name=\"duration\"\r\n\r\n34\r\n",
        "name=\"supports_streaming\"\r\n\r\ntrue\r\n",
        "name=\"parse_mode\"\r\n\r\nHTML\r\n",
        "<b>Новый ролик</b> о сервисе",
        "\"callback_data\":\"n:buy\"",
    ] {
        assert!(form.contains(part), "нет {part:?} в первой отправке");
    }
    assert!(
        videos[0]
            .body
            .windows(bytes.len())
            .any(|window| window == bytes.as_slice()),
        "файл ушёл не целиком"
    );
    for later in &videos[1..] {
        let json: serde_json::Value = serde_json::from_slice(&later.body).unwrap();
        assert_eq!(json["video"], "video-id");
        assert_eq!(json["caption"], "<b>Новый ролик</b> о сервисе");
        assert_eq!(json["parse_mode"], "HTML");
        assert_eq!(
            json["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
            "n:buy"
        );
    }
    assert!(requests(&server, "sendPhoto").await.is_empty());
    assert!(requests(&server, "sendDocument").await.is_empty());
    assert!(bodies(&server, "sendMessage").await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn устаревший_код_видео_отправляется_файлом_заново() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendVideo$"))
        .and(body_partial_json(
            serde_json::json!({ "video": "old-video" }),
        ))
        .respond_with(fail(
            400,
            "Bad Request: wrong file identifier/HTTP URL specified",
            None,
        ))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendVideo$"))
        .respond_with(ok(serde_json::json!({
            "message_id": 311, "chat": {"id": 1, "type": "private"}, "date": 0,
            "video": {"file_id": "fresh-video", "file_unique_id": "v2", "width": 640, "height": 360, "duration": 5}
        })))
        .mount(&server)
        .await;
    let sender = TelegramSender::new(services.clone(), &server.uri());
    let info = sn_domain::video::VideoInfo {
        width: 640,
        height: 360,
        duration: 5,
        streaming: false,
    };
    let stale = sender
        .send(
            1,
            &Outgoing {
                html: "Ролик",
                media: Some(Media::Video(MediaFile::Cached("old-video"), info)),
                buttons: &[],
            },
        )
        .await;
    assert_eq!(stale, Delivery::StaleFile);
    let bytes = sn_domain::video::sample::mp4(640, 360, 5, false);
    let fresh = sender
        .send(
            1,
            &Outgoing {
                html: "Ролик",
                media: Some(Media::Video(
                    MediaFile::Upload {
                        bytes: &bytes,
                        content_type: "video/mp4",
                    },
                    info,
                )),
                buttons: &[],
            },
        )
        .await;
    assert_eq!(
        fresh,
        Delivery::Sent {
            file_id: Some("fresh-video".into())
        }
    );
    let form = String::from_utf8_lossy(&requests(&server, "sendVideo").await[1].body).into_owned();
    assert!(!form.contains("supports_streaming"), "{form}");
    db.cleanup().await;
}

#[tokio::test]
async fn премиум_эмодзи_снимаются_если_telegram_их_не_принял() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_string_contains("tg-emoji"))
        .respond_with(fail(400, "Bad Request: DOCUMENT_INVALID", None))
        .with_priority(1)
        .mount(&server)
        .await;
    let sender = TelegramSender::new(services.clone(), &server.uri());
    assert_eq!(sender.bot_id(), Some(123_456));
    let buttons = [OutButton {
        text: "Меню".into(),
        url: None,
        callback: Some("n:m".into()),
        style: None,
        emoji_id: None,
    }];
    let delivery = sender
        .send(
            42,
            &Outgoing {
                html: "Огонь <tg-emoji emoji-id=\"5368324170671202286\">🔥</tg-emoji>",
                media: None,
                buttons: &buttons,
            },
        )
        .await;
    assert!(delivery.is_sent(), "{delivery:?}");
    let sent = bodies(&server, "sendMessage").await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["text"], "Огонь 🔥");
    db.cleanup().await;
}

#[tokio::test]
async fn цвет_и_значок_кнопки_уходят_в_telegram_а_при_отказе_значок_снимается() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_string_contains("icon_custom_emoji_id"))
        .and(body_partial_json(serde_json::json!({ "chat_id": 43 })))
        .respond_with(fail(400, "Bad Request: DOCUMENT_INVALID", None))
        .with_priority(1)
        .mount(&server)
        .await;
    let sender = TelegramSender::new(services.clone(), &server.uri());
    let buttons = [
        OutButton {
            text: "Продлить".into(),
            url: None,
            callback: Some("n:buy".into()),
            style: Some("success".into()),
            emoji_id: Some("5368324170671202286".into()),
        },
        OutButton {
            text: "Канал".into(),
            url: Some("https://t.me/demo".into()),
            callback: None,
            style: Some("primary".into()),
            emoji_id: None,
        },
    ];
    for chat in [42, 43] {
        let delivery = sender
            .send(
                chat,
                &Outgoing {
                    html: "Скидка",
                    media: None,
                    buttons: &buttons,
                },
            )
            .await;
        assert!(delivery.is_sent(), "{delivery:?}");
    }
    let sent = bodies(&server, "sendMessage").await;
    assert_eq!(sent.len(), 3);
    let first = &sent[0]["reply_markup"]["inline_keyboard"];
    assert_eq!(first[0][0]["style"], "success");
    assert_eq!(first[0][0]["icon_custom_emoji_id"], "5368324170671202286");
    assert_eq!(first[0][0]["callback_data"], "n:buy");
    assert_eq!(first[1][0]["style"], "primary");
    assert!(first[1][0].get("icon_custom_emoji_id").is_none());
    let retried = &sent[2]["reply_markup"]["inline_keyboard"];
    assert_eq!(sent[2]["chat_id"], 43);
    assert_eq!(retried[0][0]["style"], "success");
    assert!(retried[0][0].get("icon_custom_emoji_id").is_none());
    db.cleanup().await;
}

#[tokio::test]
async fn устаревший_код_картинки_и_ошибки_разбираются() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/sendPhoto$"))
        .respond_with(fail(
            400,
            "Bad Request: wrong file identifier/HTTP URL specified",
            None,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_partial_json(serde_json::json!({ "chat_id": 5 })))
        .respond_with(fail(400, "Bad Request: chat not found", None))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_partial_json(serde_json::json!({ "chat_id": 6 })))
        .respond_with(fail(400, "Bad Request: BUTTON_URL_INVALID", None))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_partial_json(serde_json::json!({ "chat_id": 7 })))
        .respond_with(ResponseTemplate::new(502))
        .with_priority(1)
        .mount(&server)
        .await;
    let sender = TelegramSender::new(services.clone(), &server.uri());
    let text = |chat: i64| {
        let sender = &sender;
        async move {
            sender
                .send(
                    chat,
                    &Outgoing {
                        html: "Текст",
                        media: None,
                        buttons: &[],
                    },
                )
                .await
        }
    };
    let stale = sender
        .send(
            1,
            &Outgoing {
                html: "x",
                media: Some(Media::Photo(MediaFile::Cached("old-id"))),
                buttons: &[],
            },
        )
        .await;
    assert_eq!(stale, Delivery::StaleFile);
    assert_eq!(
        text(5).await,
        Delivery::Blocked("Чат с клиентом не найден".into())
    );
    assert_eq!(
        text(6).await,
        Delivery::Failed {
            error: "Telegram не принял адрес ссылки в кнопке".into(),
            retry: false,
        }
    );
    assert_eq!(
        text(7).await,
        Delivery::Failed {
            error: "Telegram не ответил: ошибка 502".into(),
            retry: true,
        }
    );
    db.cleanup().await;
}

#[tokio::test]
async fn кнопка_раздела_из_рассылки_открывает_экран_новым_сообщением() {
    let Some((db, _services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 1101, "/start")).await;
    let before_edits = requests(&server, "editMessageText").await.len();
    let before_sends = requests(&server, "sendMessage").await.len();
    handle_update(&ctx, callback_update(2, 1101, "n:bal")).await;
    assert_eq!(
        requests(&server, "editMessageText").await.len(),
        before_edits
    );
    assert_eq!(
        requests(&server, "sendMessage").await.len(),
        before_sends + 1
    );
    handle_update(&ctx, callback_update(3, 1101, "bal")).await;
    assert_eq!(
        requests(&server, "editMessageText").await.len(),
        before_edits + 1
    );
    db.cleanup().await;
}
