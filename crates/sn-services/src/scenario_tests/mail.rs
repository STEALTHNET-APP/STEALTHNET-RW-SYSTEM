use super::*;
use crate::settings::ModuleConfigUpdate;
use crate::two_factor::CabinetLogin;
use base64::Engine;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_modules::catalog::mail::MailModule;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

pub(super) type Letters = Arc<StdMutex<Vec<String>>>;

pub(super) async fn fake_smtp() -> (u16, Letters) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let letters: Letters = Arc::new(StdMutex::new(Vec::new()));
    let store = letters.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let store = store.clone();
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                if write.write_all(b"220 fake ESMTP\r\n").await.is_err() {
                    return;
                }
                let mut data = false;
                let mut body = String::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    if data {
                        if line == "." {
                            data = false;
                            store.lock().unwrap().push(std::mem::take(&mut body));
                            let _ = write.write_all(b"250 queued\r\n").await;
                        } else {
                            body.push_str(&line);
                            body.push('\n');
                        }
                        continue;
                    }
                    let upper = line.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") {
                        b"250-fake\r\n250 OK\r\n"
                    } else if upper.starts_with("DATA") {
                        data = true;
                        b"354 go on\r\n"
                    } else if upper.starts_with("QUIT") {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"250 OK\r\n"
                    };
                    if write.write_all(reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, letters)
}

fn boundary_of(head: &str) -> Option<String> {
    let unfolded = head.replace("\n ", " ").replace("\n\t", " ");
    let at = unfolded.find("boundary=")?;
    let rest = unfolded[at + "boundary=".len()..].trim_start_matches('"');
    rest.split(['"', ';', '\n', ' ']).next().map(str::to_string)
}

fn quoted_printable(body: &str) -> String {
    let joined = body.replace("=\n", "");
    let bytes = joined.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'='
            && at + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&joined[at + 1..at + 3], 16)
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(super) fn readable(letter: &str) -> String {
    let (head, body) = letter.split_once("\n\n").unwrap_or((letter, ""));
    let lower = head.to_ascii_lowercase();
    if lower.contains("multipart/")
        && let Some(boundary) = boundary_of(head)
    {
        return body
            .split(&format!("--{boundary}"))
            .map(|part| part.trim_start_matches(['\r', '\n']))
            .filter(|part| !part.trim().is_empty() && !part.starts_with("--"))
            .map(readable)
            .collect::<Vec<_>>()
            .join("\n");
    }
    if lower.contains("content-transfer-encoding: base64") {
        let joined: String = body.split_whitespace().collect();
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(joined)
                .unwrap(),
        )
        .unwrap()
    } else if lower.contains("content-transfer-encoding: quoted-printable") {
        quoted_printable(body)
    } else {
        body.replace("=\n", "")
    }
}

pub(super) fn code_in(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_digit())
        .find(|part| part.len() == crate::login::EMAIL_CODE_DIGITS)
        .unwrap()
        .to_string()
}

async fn configure(services: &Services, admin: AdminId, port: u16) {
    configure_with(services, admin, port, serde_json::json!({})).await;
}

pub(super) async fn configure_with(
    services: &Services,
    admin: AdminId,
    port: u16,
    extra: serde_json::Value,
) {
    let mut config = serde_json::Map::new();
    if let serde_json::Value::Object(extra) = extra {
        config.extend(extra);
    }
    config.insert("transport".into(), serde_json::json!("smtp"));
    config.insert("smtp_host".into(), serde_json::json!("127.0.0.1"));
    config.insert("smtp_port".into(), serde_json::json!(port));
    config.insert("security".into(), serde_json::json!("none"));
    config.insert("from_email".into(), serde_json::json!("robot@example.com"));
    let ops = services.settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Mail, true, admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn код_входа_приходит_письмом_и_открывает_кабинет() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.9");
    assert!(!services.mail().available());
    assert_eq!(
        code(auth.email_login_start("alex@example.com", &who).await),
        "auth.email_password_mode"
    );
    let (port, letters) = fake_smtp().await;
    configure(services, admin, port).await;
    assert!(services.mail().available());
    services.mail().test("owner@example.com").await.unwrap();
    let start = auth
        .email_login_start("Alex@Example.com", &who)
        .await
        .unwrap();
    assert_eq!(start.email, "alex@example.com");
    let received = letters.lock().unwrap().clone();
    assert_eq!(received.len(), 2);
    assert!(readable(&received[0]).contains("проверочное письмо"));
    let letter = readable(&received[1]);
    assert!(letter.contains("Код для входа в кабинет"), "{letter}");
    let sent_code = code_in(&letter);
    auth.email_login_verify("alex@example.com", &sent_code, &who)
        .await
        .unwrap();
    let closed = {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    };
    configure(services, admin, closed).await;
    assert_eq!(
        code(services.mail().test("owner@example.com").await),
        "mail.failed"
    );
    services
        .settings_ops()
        .toggle_module(ModuleId::Mail, false, admin)
        .await
        .unwrap();
    assert!(!services.mail().available());
    env.db.cleanup().await;
}

#[tokio::test]
async fn письмо_через_resend_уходит_с_ключом_и_ошибка_понятна() {
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let mut config = serde_json::Map::new();
    config.insert("transport".into(), serde_json::json!("resend"));
    config.insert("resend_api_key".into(), serde_json::json!("re_test"));
    config.insert("from_email".into(), serde_json::json!("robot@example.com"));
    config.insert("from_name".into(), serde_json::json!("Сервис"));
    let ops = services.settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    assert!(services.mail().available());
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/emails"))
        .and(header("authorization", "Bearer re_test"))
        .and(body_partial_json(serde_json::json!({
            "from": "Сервис <robot@example.com>",
            "to": ["owner@example.com"]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "1" })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/emails"))
        .respond_with(ResponseTemplate::new(422).set_body_json(
            serde_json::json!({ "statusCode": 422, "message": "The example.com domain is not verified" }),
        ))
        .mount(&server)
        .await;
    services
        .mail()
        .with_resend_base(server.uri())
        .test("owner@example.com")
        .await
        .unwrap();
    let failed = services
        .mail()
        .with_resend_base(server.uri())
        .test("owner@example.com")
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "mail.failed");
    assert!(
        failed.to_string().contains("domain is not verified"),
        "{failed}"
    );
    env.db.cleanup().await;
}

pub(super) async fn fake_smtp_rejecting(refused: &'static str) -> (u16, Letters) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let letters: Letters = Arc::new(StdMutex::new(Vec::new()));
    let store = letters.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let store = store.clone();
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                if write.write_all(b"220 fake ESMTP\r\n").await.is_err() {
                    return;
                }
                let mut data = false;
                let mut body = String::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    if data {
                        if line == "." {
                            data = false;
                            store.lock().unwrap().push(std::mem::take(&mut body));
                            let _ = write.write_all(b"250 queued\r\n").await;
                        } else {
                            body.push_str(&line);
                            body.push('\n');
                        }
                        continue;
                    }
                    let upper = line.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") {
                        b"250-fake\r\n250 OK\r\n"
                    } else if upper.starts_with("RCPT") && line.contains(refused) {
                        b"550 5.1.1 No such user here\r\n"
                    } else if upper.starts_with("DATA") {
                        data = true;
                        b"354 go on\r\n"
                    } else if upper.starts_with("QUIT") {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"250 OK\r\n"
                    };
                    if write.write_all(reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, letters)
}

async fn site_ready(services: &Services) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set(
            sn_domain::settings::keys::PUBLIC_URL,
            &serde_json::json!("https://vpn.example.com/"),
            None,
        )
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

const UNSUBSCRIBE: &str = "https://vpn.example.com/api/public/m/broadcast/unsubscribe/abc";

fn rich(to: &str) -> sn_modules::MailOut<'_> {
    sn_modules::MailOut {
        to,
        subject: "Новые серверы",
        text: "Текст письма",
        html: "<p>Текст <b>письма</b></p>",
        unsubscribe: Some(UNSUBSCRIBE),
    }
}

#[tokio::test]
async fn письмо_рассылки_уходит_по_smtp_с_html_и_отпиской() {
    use sn_modules::{MailDelivery, MailSender};
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let bridge = crate::mail::MailBridge::new(services.clone());
    assert!(bridge.site().unwrap_err().contains("способ отправки"));
    let (port, letters) = fake_smtp_rejecting("nobody@example.com").await;
    configure(services, admin, port).await;
    assert!(bridge.site().unwrap_err().contains("адрес сервиса"));
    site_ready(services).await;
    let site = bridge.site().unwrap();
    assert_eq!(site.base_url, "https://vpn.example.com");
    assert_eq!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Sent
    );
    let raw = letters.lock().unwrap()[0].clone();
    let unfolded = raw.replace("\n ", " ").replace("\n\t", " ");
    assert!(unfolded.contains("List-Unsubscribe:"), "{raw}");
    assert!(unfolded.contains(&format!("<{UNSUBSCRIBE}>")), "{raw}");
    assert!(unfolded.contains("List-Unsubscribe-Post: List-Unsubscribe=One-Click"));
    let lower = unfolded.to_ascii_lowercase();
    assert!(lower.contains("multipart/alternative"), "{raw}");
    assert!(lower.contains("text/plain") && lower.contains("text/html"));
    assert_eq!(
        bridge.send(&rich("nobody@example.com")).await,
        MailDelivery::Rejected("Почтовый сервер отказал, код 550: 5.1.1 No such user here".into())
    );
    assert!(matches!(
        bridge.send(&rich("не адрес")).await,
        MailDelivery::Rejected(_)
    ));
    let closed = {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    };
    configure(services, admin, closed).await;
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Off(_)
    ));
    services
        .settings_ops()
        .toggle_module(ModuleId::Mail, false, admin)
        .await
        .unwrap();
    assert!(bridge.site().unwrap_err().contains("выключен"));
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Off(reason) if reason.contains("выключен")
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn письмо_рассылки_через_resend_разбирает_ответы() {
    use sn_modules::{MailDelivery, MailSender};
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let mut config = serde_json::Map::new();
    config.insert("transport".into(), serde_json::json!("resend"));
    config.insert("resend_api_key".into(), serde_json::json!("re_test"));
    config.insert("from_email".into(), serde_json::json!("robot@example.com"));
    let ops = services.settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Mail, true, admin)
        .await
        .unwrap();
    site_ready(services).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/emails"))
        .and(body_partial_json(serde_json::json!({
            "to": ["alex@example.com"],
            "subject": "Новые серверы",
            "text": "Текст письма",
            "html": "<p>Текст <b>письма</b></p>",
            "headers": {
                "List-Unsubscribe": format!("<{UNSUBSCRIBE}>"),
                "List-Unsubscribe-Post": "List-Unsubscribe=One-Click"
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "1" })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let replies = [
        ResponseTemplate::new(429)
            .insert_header("retry-after", "3")
            .set_body_json(serde_json::json!({ "message": "Too many requests" })),
        ResponseTemplate::new(422).set_body_json(serde_json::json!({
            "message": "Invalid `to` field. The email address needs to follow the `email@example.com` format."
        })),
        ResponseTemplate::new(422).set_body_json(
            serde_json::json!({ "message": "The example.com domain is not verified" }),
        ),
        ResponseTemplate::new(500),
    ];
    for reply in replies {
        Mock::given(method("POST"))
            .and(path("/emails"))
            .respond_with(reply)
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    let bridge = crate::mail::MailBridge::new(services.clone()).with_resend_base(server.uri());
    assert_eq!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Sent
    );
    assert_eq!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Wait(std::time::Duration::from_secs(3))
    );
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Rejected(reason) if reason.contains("`to`")
    ));
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Off(reason) if reason.contains("not verified")
    ));
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Failed { retry: true, .. }
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn пароль_восстанавливается_кодом_из_письма_при_входе_по_паролю() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.21");
    auth.email_register("olga@example.com", "пароль-ольги-1", &who)
        .await
        .unwrap();
    assert_eq!(
        code(auth.email_reset_start("olga@example.com", &who).await),
        "auth.reset_unavailable"
    );
    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;
    assert!(services.mail().available());
    assert!(!services.mail().code_login());
    assert_eq!(
        code(auth.email_login_start("olga@example.com", &who).await),
        "auth.email_password_mode"
    );
    auth.email_password_login("olga@example.com", "пароль-ольги-1", &who)
        .await
        .unwrap();
    auth.email_reset_start("nobody@example.com", &who)
        .await
        .unwrap();
    assert!(letters.lock().unwrap().is_empty());
    auth.email_reset_start("Olga@Example.com", &who)
        .await
        .unwrap();
    let letter = readable(&letters.lock().unwrap()[0]);
    assert!(letter.contains("Код для смены пароля"), "{letter}");
    let sent_code = code_in(&letter);
    let wrong = if sent_code == "000000" {
        "111111"
    } else {
        "000000"
    };
    assert_eq!(
        code(
            auth.email_reset_confirm("olga@example.com", wrong, "новый-пароль-оли", &who)
                .await
        ),
        "auth.code_invalid"
    );
    let login = auth
        .email_reset_confirm("olga@example.com", &sent_code, "новый-пароль-оли", &who)
        .await
        .unwrap();
    assert!(matches!(login, CabinetLogin::Session(_)));
    assert_eq!(
        code(
            auth.email_password_login("olga@example.com", "пароль-ольги-1", &who)
                .await
        ),
        "auth.wrong_password"
    );
    auth.email_password_login("olga@example.com", "новый-пароль-оли", &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_reset_confirm("olga@example.com", &sent_code, "ещё-один-пароль", &who)
                .await
        ),
        "auth.code_invalid"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn пароль_меняется_в_кабинете_и_выдаётся_поддержкой() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.22");
    auth.email_register("ivan@example.com", "пароль-ивана-1", &who)
        .await
        .unwrap();
    let client = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(IdentityKind::Email, "ivan@example.com")
            .await
            .unwrap()
            .unwrap()
    };
    assert!(auth.has_password(client.id).await.unwrap());
    let principal = ClientPrincipal {
        client_id: client.id,
        channel: ClientChannel::Cabinet,
        full_access: true,
        session_id: None,
    };
    assert_eq!(
        code(
            auth.change_password(&principal, Some("чужой-пароль-1"), "новый-пароль-ивана")
                .await
        ),
        "auth.wrong_password"
    );
    auth.change_password(&principal, Some("пароль-ивана-1"), "новый-пароль-ивана")
        .await
        .unwrap();
    auth.email_password_login("ivan@example.com", "новый-пароль-ивана", &who)
        .await
        .unwrap();
    let issued = auth
        .issue_client_password(client.id, Actor::Admin(admin))
        .await
        .unwrap();
    assert_eq!(issued.chars().count(), 16);
    assert_eq!(
        code(
            auth.email_password_login("ivan@example.com", "новый-пароль-ивана", &who)
                .await
        ),
        "auth.wrong_password"
    );
    auth.email_password_login("ivan@example.com", &issued, &who)
        .await
        .unwrap();
    let telegram_only = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 7_100_200,
                username: None,
                first_name: Some("Пётр".into()),
                last_name: None,
                language_code: None,
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0;
    assert_eq!(
        code(
            auth.issue_client_password(telegram_only.id, Actor::Admin(admin))
                .await
        ),
        "client.no_email"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn шаблоны_писем_сохраняются_проверяются_и_уходят_на_проверку() {
    use crate::mail_letters::{DraftLetter, DraftTexts, LangDrafts, LettersUpdate, Saved};
    use sn_domain::client::Locale;
    use sn_domain::mail_letters::{Group, LetterId};
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let letters = services.letters();
    let view = letters.view().await.unwrap();
    assert_eq!(view.letters.len(), 29);
    assert!(!view.sends);
    let ready = |view: &crate::mail_letters::LettersView| {
        view.letters
            .iter()
            .find(|l| l.id == LetterId::SubscriptionReady)
            .unwrap()
            .clone()
    };
    assert_eq!(ready(&view).default.ru.subject, "Подписка готова");
    assert_eq!(ready(&view).saved.ru, None);
    let mut typo = LettersUpdate::default();
    typo.letters.insert(
        "subscription_ready".into(),
        LangDrafts {
            ru: Some(DraftTexts {
                subject: "Готово".into(),
                text: "Работает до {datee}".into(),
                button: String::new(),
            }),
            en: None,
        },
    );
    let err = letters.update(&typo, admin).await.unwrap_err();
    assert_eq!(err.code(), "mail_letters.invalid");
    assert_eq!(
        err.to_string(),
        "«Подписка готова», текст: подстановки {datee} в этом письме нет, можно: {tariff}, {date}, {brand}, {email}"
    );
    assert_eq!(ready(&letters.view().await.unwrap()).saved.ru, None);
    let mut good = LettersUpdate::default();
    good.letters.insert(
        "subscription_ready".into(),
        LangDrafts {
            ru: Some(DraftTexts {
                subject: "Подписка «{tariff}» готова".into(),
                text: "Работает до {date}.".into(),
                button: "Открыть кабинет".into(),
            }),
            en: None,
        },
    );
    good.sign.ru = Some("Команда {brand}".into());
    good.sign.en = Some("Cheers, {brand}".into());
    good.sender = Some("Мой VPN".into());
    good.groups.insert(Group::Support, false);
    let saved = letters.update(&good, admin).await.unwrap();
    assert_eq!(
        ready(&saved).saved.ru,
        Some(Saved {
            subject: Some("Подписка «{tariff}» готова".into()),
            text: Some("Работает до {date}.".into()),
            button: None,
        })
    );
    assert_eq!(saved.sign.saved.ru, None);
    assert_eq!(saved.sign.saved.en.as_deref(), Some("Cheers, {brand}"));
    assert_eq!(saved.sender.as_deref(), Some("Мой VPN"));
    assert!(
        !saved
            .groups
            .iter()
            .find(|g| g.key == Group::Support)
            .unwrap()
            .on
    );
    let mut reset = LettersUpdate::default();
    reset.letters.insert(
        "subscription_ready".into(),
        LangDrafts {
            ru: Some(DraftTexts::default()),
            en: None,
        },
    );
    reset.sender = Some(String::new());
    let back = letters.update(&reset, admin).await.unwrap();
    assert_eq!(ready(&back).saved.ru, None);
    assert_eq!(back.sender, None);
    {
        let mut conn = services.store().conn().await.unwrap();
        assert!(
            conn.settings()
                .get("mail.letter.subscription_ready.ru")
                .await
                .unwrap()
                .is_none()
        );
    }
    let draft = DraftLetter {
        letter: "topup_done".into(),
        lang: Locale::En,
        subject: String::new(),
        text: "Thanks! +{amount}".into(),
        button: String::new(),
        sign: None,
    };
    let preview = letters.preview(&draft).await.unwrap();
    assert_eq!(preview.subject, "Balance topped up");
    assert!(preview.html.contains("Thanks! +$10.00"), "{}", preview.html);
    assert!(preview.html.contains("Cheers, STEALTHNET"));
    assert!(
        preview.text.starts_with("Thanks! +$10.00"),
        "{}",
        preview.text
    );
    let broken = DraftLetter {
        text: "<b>не закрыт".into(),
        ..draft.clone()
    };
    assert_eq!(
        letters.preview(&broken).await.unwrap_err().to_string(),
        "«Баланс пополнен», английский, текст: тег <b> не закрыт"
    );
    assert_eq!(
        letters
            .test(&draft, "owner@example.com")
            .await
            .unwrap_err()
            .code(),
        "mail.unavailable"
    );
    let (port, mails) = fake_smtp().await;
    configure(services, admin, port).await;
    assert_eq!(
        letters.test(&draft, "не адрес").await.unwrap_err().code(),
        "mail.address"
    );
    letters.test(&draft, "Owner@Example.com").await.unwrap();
    let sent = mails.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(readable(&sent[0]).contains("Thanks! +$10.00"));
    assert!(sent[0].contains("owner@example.com"));
    env.db.cleanup().await;
}

#[tokio::test]
async fn подтверждение_почты_кодом_отвязывает_google_привязанный_до_него() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.22");
    auth.email_register("nina@example.com", "пароль-нины-1", &who)
        .await
        .unwrap();
    let client_id = {
        let mut conn = services.store().conn().await.unwrap();
        let client = conn
            .clients()
            .find_by_identity(IdentityKind::Email, "nina@example.com")
            .await
            .unwrap()
            .unwrap();
        conn.clients()
            .add_identity(
                client.id,
                &sn_domain::client::NewIdentity::new(IdentityKind::Google, "7001", true).unwrap(),
            )
            .await
            .unwrap();
        client.id
    };
    let CabinetLogin::Session(before) = auth
        .email_password_login("nina@example.com", "пароль-нины-1", &who)
        .await
        .unwrap()
    else {
        panic!("ожидался вход без второго фактора");
    };
    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;
    auth.email_reset_start("nina@example.com", &who)
        .await
        .unwrap();
    let sent_code = code_in(&readable(&letters.lock().unwrap()[0]));
    auth.email_reset_confirm("nina@example.com", &sent_code, "новый-пароль-нины", &who)
        .await
        .unwrap();
    let mut conn = services.store().conn().await.unwrap();
    assert!(
        conn.clients()
            .find_by_identity(IdentityKind::Google, "7001")
            .await
            .unwrap()
            .is_none(),
        "вход через Google, привязанный до подтверждения почты, отвязан"
    );
    let email = conn
        .clients()
        .identities(client_id)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IdentityKind::Email)
        .unwrap();
    assert!(email.verified);
    let notified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE payload->>'template' = 'oauth_unlinked' \
         AND (payload->>'client_id')::bigint = $1",
    )
    .bind(client_id.0)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(notified, 1);
    drop(conn);
    assert!(matches!(
        auth.client_session(&before.token).await,
        Err(ServiceError::Unauthorized)
    ));
    auth.email_reset_start("nina@example.com", &who)
        .await
        .unwrap();
    let second = code_in(&readable(&letters.lock().unwrap()[1]));
    auth.email_reset_confirm("nina@example.com", &second, "ещё-пароль-нины", &who)
        .await
        .unwrap();
    let mut conn = services.store().conn().await.unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'client.oauth_unlinked_on_confirm' AND entity_id = $1",
    )
    .bind(client_id.0)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(audits, 1, "повторное подтверждение ничего не отвязывает");
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn смена_сервера_smtp_без_пароля_не_сохраняется() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    let update = |config: serde_json::Value, clear: &[&str]| ModuleConfigUpdate {
        config: serde_json::from_value(config).unwrap(),
        clear: clear.iter().map(|key| (*key).to_string()).collect(),
    };
    ops.update_module_config(
        ModuleId::Mail,
        &update(
            serde_json::json!({
                "transport": "smtp",
                "smtp_host": "smtp.yandex.ru",
                "username": "robot@example.com",
                "password": "app-password",
                "from_email": "robot@example.com"
            }),
            &[],
        ),
        admin,
    )
    .await
    .unwrap();
    let saved = |key: &str| {
        services
            .modules()
            .config(ModuleId::Mail)
            .text(key)
            .map(str::to_string)
    };
    for moved in [
        serde_json::json!({ "smtp_host": "smtp.attacker.example" }),
        serde_json::json!({ "smtp_port": 2525 }),
        serde_json::json!({ "security": "tls" }),
        serde_json::json!({ "username": "other@example.com" }),
    ] {
        assert_eq!(
            code(
                ops.update_module_config(ModuleId::Mail, &update(moved.clone(), &[]), admin)
                    .await
            ),
            crate::settings::SECRETS_AGAIN,
            "{moved}"
        );
    }
    assert_eq!(saved("smtp_host").as_deref(), Some("smtp.yandex.ru"));
    assert_eq!(saved("password").as_deref(), Some("app-password"));
    ops.update_module_config(
        ModuleId::Mail,
        &update(
            serde_json::json!({ "smtp_host": "smtp.mail.ru", "password": "mail-ru-password" }),
            &[],
        ),
        admin,
    )
    .await
    .unwrap();
    assert_eq!(saved("smtp_host").as_deref(), Some("smtp.mail.ru"));
    assert_eq!(saved("password").as_deref(), Some("mail-ru-password"));
    ops.update_module_config(
        ModuleId::Mail,
        &update(
            serde_json::json!({ "smtp_host": "smtp.gmail.com" }),
            &["password"],
        ),
        admin,
    )
    .await
    .unwrap();
    assert_eq!(saved("smtp_host").as_deref(), Some("smtp.gmail.com"));
    assert!(saved("password").is_none());
    env.db.cleanup().await;
}

#[tokio::test]
async fn первый_вход_по_коду_закрывает_доступ_заданный_до_подтверждения_почты() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let early = meta("203.0.113.31");
    auth.email_register("vera@example.com", "пароль-заранее-1", &early)
        .await
        .unwrap();
    let CabinetLogin::Session(before) = auth
        .email_password_login("vera@example.com", "пароль-заранее-1", &early)
        .await
        .unwrap()
    else {
        panic!("ожидался вход без второго фактора");
    };
    let client_id = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(IdentityKind::Email, "vera@example.com")
            .await
            .unwrap()
            .unwrap()
            .id
    };
    assert!(auth.has_password(client_id).await.unwrap());
    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "code" }),
    )
    .await;
    let owner_meta = meta("198.51.100.40");
    auth.email_login_start("vera@example.com", &owner_meta)
        .await
        .unwrap();
    let first_code = code_in(&readable(&letters.lock().unwrap()[0]));
    let CabinetLogin::Session(proven) = auth
        .email_login_verify("vera@example.com", &first_code, &owner_meta)
        .await
        .unwrap()
    else {
        panic!("ожидался вход без второго фактора");
    };
    assert!(
        matches!(
            auth.client_session(&before.token).await,
            Err(ServiceError::Unauthorized)
        ),
        "сессия, открытая до подтверждения почты, закрыта"
    );
    assert!(
        !auth.has_password(client_id).await.unwrap(),
        "пароль, заданный до подтверждения почты, удалён"
    );
    assert_eq!(
        auth.client_session(&proven.token).await.unwrap().client_id,
        client_id
    );
    auth.email_login_start("vera@example.com", &owner_meta)
        .await
        .unwrap();
    let second_code = code_in(&readable(&letters.lock().unwrap()[1]));
    auth.email_login_verify("vera@example.com", &second_code, &owner_meta)
        .await
        .unwrap();
    assert!(
        auth.client_session(&proven.token).await.is_ok(),
        "вход на уже подтверждённую почту чужие сессии не трогает"
    );
    let mut conn = services.store().conn().await.unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'client.unproven_access_dropped' AND entity_id = $1",
    )
    .bind(client_id.0)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(audits, 1);
    drop(conn);
    env.db.cleanup().await;
}

fn wrong_code(real: &str) -> String {
    real.chars()
        .map(|c| if c == '9' { '0' } else { '9' })
        .collect()
}

#[tokio::test]
async fn неверные_коды_из_письма_ограничены_за_сутки_на_почту() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "code" }),
    )
    .await;
    let rule = crate::limits::rules::EMAIL_CODE_FAILURES_DAY;
    let per_window = crate::login::EMAIL_CODE_MAX_ATTEMPTS as u32;
    let mut failures = 0;
    let mut window = 0;
    while failures < rule.limit {
        env.clock
            .set(start() + chrono::Duration::minutes(16 * window));
        let who = meta(&format!("203.0.113.{}", 40 + window));
        auth.email_login_start("lev@example.com", &who)
            .await
            .unwrap();
        let sent = letters.lock().unwrap().len();
        let real = code_in(&readable(&letters.lock().unwrap()[sent - 1]));
        if window == 0 {
            auth.email_login_verify("lev@example.com", &real, &who)
                .await
                .unwrap();
            auth.email_login_start("lev@example.com", &who)
                .await
                .unwrap();
        }
        let sent = letters.lock().unwrap().len();
        let real = code_in(&readable(&letters.lock().unwrap()[sent - 1]));
        for _ in 0..per_window.min(rule.limit - failures) {
            assert_eq!(
                code(
                    auth.email_login_verify("lev@example.com", &wrong_code(&real), &who)
                        .await
                ),
                "auth.code_invalid"
            );
            failures += 1;
        }
        window += 1;
    }
    env.clock
        .set(start() + chrono::Duration::minutes(16 * window));
    let who = meta("198.51.100.77");
    auth.email_login_start("lev@example.com", &who)
        .await
        .unwrap();
    let sent = letters.lock().unwrap().len();
    let real = code_in(&readable(&letters.lock().unwrap()[sent - 1]));
    assert_eq!(
        code(
            auth.email_login_verify("lev@example.com", &real, &who)
                .await
        ),
        crate::limits::RATE_LIMITED,
        "после {} неверных кодов за сутки почта закрыта даже для верного кода",
        rule.limit
    );
    env.clock
        .set(start() + chrono::Duration::days(2) + chrono::Duration::minutes(16 * window));
    auth.email_login_start("lev@example.com", &who)
        .await
        .unwrap();
    let sent = letters.lock().unwrap().len();
    let real = code_in(&readable(&letters.lock().unwrap()[sent - 1]));
    auth.email_login_verify("lev@example.com", &real, &who)
        .await
        .unwrap();
    env.db.cleanup().await;
}

#[tokio::test]
async fn смена_пароля_в_кабинете_ограничена_по_числу_попыток() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.23");
    auth.email_register("pavel@example.com", "пароль-павла-1", &who)
        .await
        .unwrap();
    let client = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(IdentityKind::Email, "pavel@example.com")
            .await
            .unwrap()
            .unwrap()
    };
    let principal = ClientPrincipal {
        client_id: client.id,
        channel: ClientChannel::Cabinet,
        full_access: true,
        session_id: None,
    };
    let rule = crate::limits::rules::CLIENT_SECURITY;
    for n in 0..rule.limit {
        assert_eq!(
            code(
                auth.change_password(
                    &principal,
                    Some(&format!("не-тот-пароль-{n}")),
                    "новый-пароль-павла"
                )
                .await
            ),
            "auth.wrong_password"
        );
    }
    assert_eq!(
        code(
            auth.change_password(&principal, Some("пароль-павла-1"), "новый-пароль-павла")
                .await
        ),
        crate::limits::RATE_LIMITED
    );
    env.clock
        .set(start() + chrono::Duration::seconds(2 * rule.window_seconds));
    auth.change_password(&principal, Some("пароль-павла-1"), "новый-пароль-павла")
        .await
        .unwrap();
    env.db.cleanup().await;
}

#[tokio::test]
async fn письма_с_кодами_ограничены_на_всю_панель_и_по_сети_ipv6() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let (port, letters) = fake_smtp().await;
    let ops = services.settings_ops();
    let mut config = serde_json::Map::new();
    config.insert("transport".into(), serde_json::json!("smtp"));
    config.insert("smtp_host".into(), serde_json::json!("127.0.0.1"));
    config.insert("smtp_port".into(), serde_json::json!(port));
    config.insert("security".into(), serde_json::json!("none"));
    config.insert("from_email".into(), serde_json::json!("robot@example.com"));
    config.insert("login".into(), serde_json::json!("code"));
    config.insert("codes_per_hour".into(), serde_json::json!(5));
    assert!(
        ops.update_module_config(
            ModuleId::Mail,
            &ModuleConfigUpdate {
                config: config.clone(),
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .is_err(),
        "меньше 10 писем в час не сохраняется"
    );
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "code", "codes_per_hour": 10 }),
    )
    .await;
    for n in 0..10 {
        auth.email_login_start(
            &format!("guest{n}@example.com"),
            &meta(&format!("203.0.113.{}", 60 + n)),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        code(
            auth.email_login_start("guest10@example.com", &meta("198.51.100.90"))
                .await
        ),
        crate::limits::RATE_LIMITED,
        "одиннадцатое письмо с кодом за час не уходит"
    );
    assert_eq!(letters.lock().unwrap().len(), 10);
    env.clock.set(start() + chrono::Duration::hours(2));
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "code", "codes_per_hour": 300 }),
    )
    .await;
    let wide = crate::limits::rules::EMAIL_START_WIDE.limit;
    for n in 0..wide {
        auth.email_login_start(
            &format!("net{n}@example.com"),
            &meta(&format!("2001:db8:77:{:x}::1", n + 1)),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        code(
            auth.email_login_start("net-last@example.com", &meta("2001:db8:77:ff::1"))
                .await
        ),
        crate::limits::RATE_LIMITED,
        "из одной сети /48 писем не больше лимита"
    );
    auth.email_login_start("other-net@example.com", &meta("2001:db8:78::1"))
        .await
        .unwrap();
    env.db.cleanup().await;
}

#[tokio::test]
async fn старые_запросы_входа_удаляются_и_без_входа_через_telegram() {
    let Some(env) = env_with(ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap()).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let (port, _letters) = fake_smtp().await;
    configure(services, admin, port).await;
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "INSERT INTO client_login_requests (method, code_hash, email, created_at, expires_at) \
         SELECT 'email', '\\x00', 'old@example.org', $1 - interval '10 days', \
                $1 - make_interval(days => 5 + n) - interval '1 minute' FROM generate_series(0, 4) n",
    )
    .bind(services.now())
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    services
        .auth()
        .email_login_start("anna@example.org", &RequestMeta::default())
        .await
        .unwrap();
    let mut conn = services.store().conn().await.unwrap();
    let left: Vec<(String, i64)> = sqlx::query_as(
        "SELECT email, count(*) FROM client_login_requests GROUP BY email ORDER BY email",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        left,
        [
            ("anna@example.org".to_string(), 1),
            ("old@example.org".to_string(), 2)
        ]
    );
    env.db.cleanup().await;
}
