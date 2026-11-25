use super::mail::{Letters, configure_with, fake_smtp};
use super::*;
use crate::login::issue_session_scoped;
use crate::telegram_link::{
    ALREADY_LINKED, BLOCKED, EMAIL_UNCONFIRMABLE, EMAIL_UNVERIFIED, EXPIRED, LINKED_AUDIT,
    SESSION_GONE, TAKEN, TEMPLATE_LINKED, TelegramLinkPrompt, TelegramLinkStart,
    TelegramLinkStatus, UNAVAILABLE,
};
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::client::NewIdentity;
use sn_domain::settings::ClientAccess;
use sn_modules::catalog::mail::MailModule;
use sn_modules::catalog::oauth::OauthModule;
use sn_store::SessionScope;

const TG: i64 = 5_093_118_422;

async fn env() -> Option<Env> {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule), Arc::new(OauthModule)]).unwrap();
    let env = env_with_fallbacks(
        registry,
        EnvFallbacks {
            bot_token: Some("123456:TEST".into()),
            public_url: Some("https://vpn.example.com/".into()),
            ..EnvFallbacks::default()
        },
    )
    .await?;
    set_bot(&env.services).await;
    Some(env)
}

async fn site_client(services: &Services, identities: &[NewIdentity]) -> ClientId {
    let mut tx = services.store().begin().await.unwrap();
    let client = tx
        .clients()
        .create(&sn_store::NewClient {
            locale: sn_domain::client::Locale::Ru,
            registration_source: RegistrationSource::Web,
            note: None,
        })
        .await
        .unwrap();
    for identity in identities {
        tx.clients()
            .add_identity(client.id, identity)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    client.id
}

fn email(address: &str, verified: bool) -> NewIdentity {
    NewIdentity::new(IdentityKind::Email, address, verified).unwrap()
}

async fn by_email(services: &Services, address: &str) -> ClientId {
    site_client(services, &[email(address, true)]).await
}

async fn cabinet_as(
    services: &Services,
    client: ClientId,
    scope: SessionScope,
) -> (String, ClientPrincipal) {
    let mut tx = services.store().begin().await.unwrap();
    let login = issue_session_scoped(
        &mut tx,
        client,
        services.now(),
        &meta("203.0.113.7"),
        false,
        scope,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (login.token, login.principal)
}

async fn cabinet(services: &Services, client: ClientId) -> (String, ClientPrincipal) {
    cabinet_as(services, client, SessionScope::Full).await
}

async fn begin(services: &Services, principal: &ClientPrincipal) -> TelegramLinkStart {
    services
        .telegram_links()
        .start(principal, &meta("203.0.113.7"))
        .await
        .unwrap()
}

fn param(start: &TelegramLinkStart) -> String {
    start
        .link
        .strip_prefix("https://t.me/stealth_vpn_bot?start=")
        .unwrap()
        .to_string()
}

async fn status(
    services: &Services,
    principal: &ClientPrincipal,
    start: &TelegramLinkStart,
) -> TelegramLinkStatus {
    services
        .telegram_links()
        .status(principal, &start.poll_token, &meta("203.0.113.7"))
        .await
        .unwrap()
}

async fn owner_of(services: &Services, telegram: i64) -> Option<ClientId> {
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .find_by_identity(IdentityKind::Telegram, &telegram.to_string())
        .await
        .unwrap()
        .map(|c| c.id)
}

async fn clients(services: &Services) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM clients")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn notices(services: &Services) -> Vec<serde_json::Value> {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE payload->>'kind' = 'notify' AND payload->>'template' = $1 ORDER BY id",
    )
    .bind(TEMPLATE_LINKED)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

#[tokio::test]
async fn telegram_привязывается_к_аккаунту_кабинета_после_подтверждения_в_боте() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let links = services.telegram_links();
    let client = by_email(services, "alexey@mail.example").await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .set_bot_unreachable(client, true)
            .await
            .unwrap();
    }
    let (_, me) = cabinet(services, client).await;
    let before = services.oauth().logins(client).await.unwrap();
    assert!(before.can_link_telegram);
    assert!(before.can_link.is_empty());
    let start = begin(services, &me).await;
    let code_part = param(&start);
    assert!(code_part.starts_with("tglink_"));
    assert!(matches!(
        status(services, &me, &start).await,
        TelegramLinkStatus::Pending { .. }
    ));
    assert_eq!(
        links.prompt(&code_part, &profile(TG)).await.unwrap(),
        TelegramLinkPrompt::Ask {
            account: "a***@mail.example".into(),
            site: "vpn.example.com".into(),
        }
    );
    assert_eq!(owner_of(services, TG).await, None);
    assert_eq!(clients(services).await, 1);
    let linked = links.confirm(&code_part, &profile(TG)).await.unwrap();
    assert_eq!((linked.client_id, linked.repeated), (client, false));
    assert_eq!(owner_of(services, TG).await, Some(client));
    assert_eq!(
        status(services, &me, &start).await,
        TelegramLinkStatus::Linked {
            account: "@morozov_a".into()
        }
    );
    let after = services.oauth().logins(client).await.unwrap();
    assert!(!after.can_link_telegram);
    assert_eq!(
        after.methods.iter().map(|m| m.kind).collect::<Vec<_>>(),
        [IdentityKind::Email, IdentityKind::Telegram]
    );
    let jobs = notices(services).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["params"]["provider"], "Telegram");
    assert_eq!(jobs[0]["params"]["account"], "@morozov_a");
    let mut conn = services.store().conn().await.unwrap();
    assert!(!conn.clients().get(client).await.unwrap().bot_unreachable);
    let (actor, details, ip): (Option<i64>, serde_json::Value, Option<String>) =
        sqlx::query_as("SELECT actor_id, details, ip FROM audit_log WHERE action = $1")
            .bind(LINKED_AUDIT)
            .fetch_one(conn.conn())
            .await
            .unwrap();
    assert_eq!(actor, Some(client.0));
    assert_eq!(details["telegram_id"], TG);
    assert_eq!(ip.as_deref(), Some("203.0.113.7"));
    drop(conn);
    let again = links.confirm(&code_part, &profile(TG)).await.unwrap();
    assert!(again.repeated);
    assert_eq!(
        links.prompt(&code_part, &profile(TG)).await.unwrap(),
        TelegramLinkPrompt::Linked
    );
    assert_eq!(notices(services).await.len(), 1);
    assert_eq!(
        code(links.start(&me, &meta("203.0.113.7")).await),
        ALREADY_LINKED
    );
    let (entered, created) = services
        .clients()
        .ensure_telegram(&profile(TG), RegistrationSource::Miniapp, None)
        .await
        .unwrap();
    assert_eq!((entered.id, created), (client, false));
    env.db.cleanup().await;
}

#[tokio::test]
async fn код_привязки_не_подходит_для_входа_а_код_входа_для_привязки() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let links = services.telegram_links();
    let auth = services.auth();
    let client = by_email(services, "alexey@mail.example").await;
    let (_, me) = cabinet(services, client).await;
    let start = begin(services, &me).await;
    let raw = param(&start).trim_start_matches("tglink_").to_string();
    for code_part in [format!("auth_{raw}"), raw.clone()] {
        assert!(matches!(
            auth.telegram_login_prompt(&code_part, &profile(TG)).await,
            Err(ServiceError::NotFound(_))
        ));
        assert!(matches!(
            auth.confirm_telegram_login(&code_part, &profile(TG)).await,
            Err(ServiceError::NotFound(_))
        ));
    }
    assert!(matches!(
        auth.telegram_login_status(&start.poll_token, &meta("203.0.113.7"))
            .await,
        Err(ServiceError::NotFound(_))
    ));
    let login = auth
        .telegram_login_start(&meta("203.0.113.7"))
        .await
        .unwrap();
    let login_code = login.link.rsplit('_').next().unwrap().to_string();
    for wrong in [
        login_code.as_str(),
        &format!("tglink_{login_code}"),
        "tglink_ffffffffffffffffffffffffffffffff",
        "tglink_",
        "",
        "tglink_0123456789abcdef!!",
    ] {
        assert_eq!(code(links.prompt(wrong, &profile(TG)).await), EXPIRED);
        assert_eq!(code(links.confirm(wrong, &profile(TG)).await), EXPIRED);
    }
    assert!(matches!(
        links
            .status(&me, &login.poll_token, &meta("203.0.113.7"))
            .await,
        Err(ServiceError::NotFound(_))
    ));
    assert_eq!(owner_of(services, TG).await, None);
    assert_eq!(clients(services).await, 1);
    env.db.cleanup().await;
}

#[tokio::test]
async fn просроченная_отменённая_и_заменённая_ссылка_не_привязывает() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let links = services.telegram_links();
    let client = by_email(services, "alexey@mail.example").await;
    let (_, me) = cabinet(services, client).await;
    let late = begin(services, &me).await;
    env.clock.set(super::start() + Duration::minutes(6));
    for result in [
        links.prompt(&param(&late), &profile(TG)).await.map(|_| ()),
        links.confirm(&param(&late), &profile(TG)).await.map(|_| ()),
    ] {
        assert_eq!(code(result), EXPIRED);
    }
    assert_eq!(
        status(services, &me, &late).await,
        TelegramLinkStatus::Expired
    );
    let rejected = begin(services, &me).await;
    links.reject(&param(&rejected), &profile(TG)).await.unwrap();
    assert_eq!(
        status(services, &me, &rejected).await,
        TelegramLinkStatus::Rejected
    );
    assert_eq!(
        code(links.confirm(&param(&rejected), &profile(TG)).await),
        EXPIRED
    );
    let stopped = begin(services, &me).await;
    links.stop(&me, &stopped.poll_token).await.unwrap();
    assert_eq!(
        code(links.confirm(&param(&stopped), &profile(TG)).await),
        EXPIRED
    );
    let old = begin(services, &me).await;
    let fresh = begin(services, &me).await;
    assert_eq!(
        status(services, &me, &old).await,
        TelegramLinkStatus::Expired
    );
    assert_eq!(
        code(links.confirm(&param(&old), &profile(TG)).await),
        EXPIRED
    );
    assert_eq!(owner_of(services, TG).await, None);
    links.confirm(&param(&fresh), &profile(TG)).await.unwrap();
    links.reject(&param(&fresh), &profile(TG)).await.unwrap();
    links.stop(&me, &fresh.poll_token).await.unwrap();
    assert!(matches!(
        status(services, &me, &fresh).await,
        TelegramLinkStatus::Linked { .. }
    ));
    assert_eq!(
        code(links.confirm(&param(&fresh), &profile(TG + 1)).await),
        EXPIRED
    );
    assert_eq!(owner_of(services, TG + 1).await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn параллельные_подтверждения_привязывают_один_telegram() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let client = by_email(services, "alexey@mail.example").await;
    let (_, me) = cabinet(services, client).await;
    let start = begin(services, &me).await;
    let tasks: Vec<_> = (0..8)
        .map(|n| {
            let services = services.clone();
            let code_part = param(&start);
            tokio::spawn(async move {
                services
                    .telegram_links()
                    .confirm(&code_part, &profile(TG + n % 4))
                    .await
            })
        })
        .collect();
    let mut fresh = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(linked) if linked.repeated => {}
            Ok(_) => fresh += 1,
            Err(err) => assert_eq!(err.code(), EXPIRED),
        }
    }
    assert_eq!(fresh, 1);
    assert_eq!(notices(services).await.len(), 1);
    assert_eq!(clients(services).await, 1);

    let other = by_email(services, "boris@mail.example").await;
    let third = by_email(services, "vera@mail.example").await;
    let mut tasks = Vec::new();
    for id in [other, third] {
        let (_, principal) = cabinet(services, id).await;
        let code_part = param(&begin(services, &principal).await);
        let services = services.clone();
        tasks.push(tokio::spawn(async move {
            services
                .telegram_links()
                .confirm(&code_part, &profile(TG + 100))
                .await
        }));
    }
    let mut codes = Vec::new();
    for task in tasks {
        codes.push(match task.await.unwrap() {
            Ok(_) => "ok",
            Err(err) => err.code(),
        });
    }
    codes.sort_unstable();
    assert_eq!(codes, ["ok", TAKEN]);

    let racer = by_email(services, "gleb@mail.example").await;
    let (_, principal) = cabinet(services, racer).await;
    let code_part = param(&begin(services, &principal).await);
    let linking = {
        let services = services.clone();
        tokio::spawn(async move {
            services
                .telegram_links()
                .confirm(&code_part, &profile(TG + 200))
                .await
        })
    };
    let starting = {
        let services = services.clone();
        tokio::spawn(async move {
            services
                .clients()
                .ensure_telegram(&profile(TG + 200), RegistrationSource::Bot, None)
                .await
        })
    };
    let linked = linking.await.unwrap();
    let (entered, created) = starting.await.unwrap().unwrap();
    match linked {
        Ok(linked) => assert_eq!(
            (linked.client_id, entered.id, created),
            (racer, racer, false)
        ),
        Err(err) => {
            assert_eq!(err.code(), TAKEN);
            assert!(created);
        }
    }
    env.db.cleanup().await;
}

#[tokio::test]
async fn привязка_держится_за_сессию_которая_её_начала() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let links = services.telegram_links();
    let client = by_email(services, "alexey@mail.example").await;
    let (token, me) = cabinet(services, client).await;
    let (_, neighbour) = cabinet(services, client).await;
    let stranger = by_email(services, "boris@mail.example").await;
    let (_, other) = cabinet(services, stranger).await;
    let start = begin(services, &me).await;
    for principal in [&neighbour, &other] {
        assert!(matches!(
            links
                .status(principal, &start.poll_token, &meta("203.0.113.7"))
                .await,
            Err(ServiceError::NotFound(_))
        ));
        assert!(matches!(
            links.stop(principal, &start.poll_token).await,
            Err(ServiceError::NotFound(_))
        ));
    }
    services.auth().client_logout(&token).await.unwrap();
    assert_eq!(
        code(links.prompt(&param(&start), &profile(TG)).await),
        SESSION_GONE
    );
    assert_eq!(
        code(links.confirm(&param(&start), &profile(TG)).await),
        SESSION_GONE
    );
    assert_eq!(owner_of(services, TG).await, None);

    let start = begin(services, &neighbour).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE client_sessions SET scope = 'password_change' WHERE id = $1")
            .bind(neighbour.session_id)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    assert_eq!(
        code(links.confirm(&param(&start), &profile(TG)).await),
        SESSION_GONE
    );
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE client_sessions SET scope = 'full', expires_at = $2 WHERE id = $1")
            .bind(neighbour.session_id)
            .bind(services.now())
            .execute(conn.conn())
            .await
            .unwrap();
    }
    assert_eq!(
        code(links.confirm(&param(&start), &profile(TG)).await),
        SESSION_GONE
    );
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("DELETE FROM client_sessions WHERE id = $1")
            .bind(neighbour.session_id)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    assert_eq!(
        code(links.confirm(&param(&start), &profile(TG)).await),
        EXPIRED
    );

    let view = ClientPrincipal {
        full_access: false,
        ..other.clone()
    };
    let mini = ClientPrincipal {
        channel: ClientChannel::Miniapp,
        ..other.clone()
    };
    let no_session = ClientPrincipal {
        session_id: None,
        ..other.clone()
    };
    for principal in [view, mini, no_session] {
        assert!(matches!(
            links.start(&principal, &meta("203.0.113.7")).await,
            Err(ServiceError::Forbidden)
        ));
    }
    env.db.cleanup().await;
}

#[tokio::test]
async fn занятый_telegram_и_другой_telegram_у_аккаунта_дают_отказ() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let links = services.telegram_links();
    let busy = telegram_client(services, 42).await;
    let client = by_email(services, "alexey@mail.example").await;
    let (_, me) = cabinet(services, client).await;
    let start = begin(services, &me).await;
    for result in [
        links.prompt(&param(&start), &profile(42)).await.map(|_| ()),
        links
            .confirm(&param(&start), &profile(42))
            .await
            .map(|_| ()),
    ] {
        let err = result.unwrap_err();
        assert_eq!(err.code(), TAKEN);
        assert_eq!(
            err.to_string(),
            "У этого Telegram уже есть свой аккаунт. Войдите в него или напишите в поддержку."
        );
    }
    assert_eq!(owner_of(services, 42).await, Some(busy));
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .set_status(busy, ClientStatus::Blocked, None)
            .await
            .unwrap();
    }
    assert_eq!(
        code(links.confirm(&param(&start), &profile(42)).await),
        TAKEN
    );
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .add_identity(client, &NewIdentity::telegram(&profile(77)))
            .await
            .unwrap();
    }
    assert_eq!(
        code(links.prompt(&param(&start), &profile(TG)).await),
        ALREADY_LINKED
    );
    assert_eq!(
        code(links.confirm(&param(&start), &profile(TG)).await),
        ALREADY_LINKED
    );
    assert_eq!(
        links.prompt(&param(&start), &profile(77)).await.unwrap(),
        TelegramLinkPrompt::Linked
    );
    assert!(
        links
            .confirm(&param(&start), &profile(77))
            .await
            .unwrap()
            .repeated
    );
    assert!(notices(services).await.is_empty());
    assert_eq!(owner_of(services, TG).await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn заблокированный_удалённый_клиент_и_закрытый_вход_не_привязывают_telegram() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let links = services.telegram_links();
    let admin = owner(services).await;
    let client = by_email(services, "alexey@mail.example").await;
    let (_, me) = cabinet(services, client).await;
    let start = begin(services, &me).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .set_status(client, ClientStatus::Blocked, None)
            .await
            .unwrap();
    }
    for result in [
        links.prompt(&param(&start), &profile(TG)).await.map(|_| ()),
        links
            .confirm(&param(&start), &profile(TG))
            .await
            .map(|_| ()),
    ] {
        assert_eq!(code(result), BLOCKED);
    }
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .set_status(client, ClientStatus::Active, None)
            .await
            .unwrap();
    }
    let access = |site_login| ClientAccess {
        site_login,
        ..ClientAccess::default()
    };
    services
        .settings_ops()
        .update_client_access(&access(false), admin)
        .await
        .unwrap();
    assert_eq!(
        code(links.confirm(&param(&start), &profile(TG)).await),
        UNAVAILABLE
    );
    assert_eq!(
        code(links.start(&me, &meta("203.0.113.7")).await),
        UNAVAILABLE
    );
    assert!(
        !services
            .oauth()
            .logins(client)
            .await
            .unwrap()
            .can_link_telegram
    );
    services
        .settings_ops()
        .update_client_access(&access(true), admin)
        .await
        .unwrap();
    services
        .clients()
        .delete(client, Actor::Admin(admin))
        .await
        .unwrap();
    assert!(links.confirm(&param(&start), &profile(TG)).await.is_err());
    assert_eq!(owner_of(services, TG).await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn без_бота_привязка_не_предлагается() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let client = by_email(services, "alexey@mail.example").await;
    let (_, me) = cabinet(services, client).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"", None)
            .await
            .unwrap();
    }
    assert!(
        !services
            .oauth()
            .logins(client)
            .await
            .unwrap()
            .can_link_telegram
    );
    assert_eq!(
        code(
            services
                .telegram_links()
                .start(&me, &meta("203.0.113.7"))
                .await
        ),
        UNAVAILABLE
    );
    let from_bot = telegram_client(services, 42).await;
    set_bot(services).await;
    assert!(
        !services
            .oauth()
            .logins(from_bot)
            .await
            .unwrap()
            .can_link_telegram
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn неподтверждённая_почта_сначала_подтверждается() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let links = services.telegram_links();
    let admin = owner(services).await;
    let google = |sub: &str| NewIdentity {
        kind: IdentityKind::Google,
        value: sub.into(),
        verified: true,
        meta: serde_json::json!({ "email": "boris@gmail.example" }),
    };
    let only_email = site_client(services, &[email("alexey@mail.example", false)]).await;
    let with_google = site_client(
        services,
        &[google("g-1"), email("boris@mail.example", false)],
    )
    .await;
    for id in [only_email, with_google] {
        let (_, me) = cabinet(services, id).await;
        assert_eq!(
            code(links.start(&me, &meta("203.0.113.7")).await),
            EMAIL_UNCONFIRMABLE
        );
    }
    let (port, _letters): (u16, Letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;
    let (_, me) = cabinet(services, only_email).await;
    assert_eq!(
        code(links.start(&me, &meta("203.0.113.7")).await),
        EMAIL_UNVERIFIED
    );
    let no_code = site_client(
        services,
        &[
            google("g-2"),
            NewIdentity::email_without_code("vera@mail.example").unwrap(),
        ],
    )
    .await;
    let (_, me) = cabinet(services, no_code).await;
    let start = begin(services, &me).await;
    assert_eq!(
        links.prompt(&param(&start), &profile(TG)).await.unwrap(),
        TelegramLinkPrompt::Ask {
            account: "v***@mail.example".into(),
            site: "vpn.example.com".into(),
        }
    );
    let plain_google = site_client(services, &[google("g-3")]).await;
    let (_, me) = cabinet(services, plain_google).await;
    let start = begin(services, &me).await;
    assert_eq!(
        links.prompt(&param(&start), &profile(TG)).await.unwrap(),
        TelegramLinkPrompt::Ask {
            account: "b***@gmail.example".into(),
            site: "vpn.example.com".into(),
        }
    );
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .add_identity(plain_google, &email("grisha@mail.example", false))
            .await
            .unwrap();
    }
    assert_eq!(
        code(links.confirm(&param(&start), &profile(TG)).await),
        EMAIL_UNVERIFIED
    );
    assert_eq!(owner_of(services, TG).await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn попытки_привязки_ограничены() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let links = services.telegram_links();
    let client = by_email(services, "alexey@mail.example").await;
    let (_, me) = cabinet(services, client).await;
    for n in 0..rules::TELEGRAM_LINK_CLIENT.limit {
        links
            .start(&me, &meta(&format!("203.0.113.{n}")))
            .await
            .unwrap();
    }
    assert_eq!(
        code(links.start(&me, &meta("203.0.113.200")).await),
        RATE_LIMITED
    );
    for n in 0..rules::TELEGRAM_LINK_USER.limit {
        let wrong = format!("tglink_{n:032x}");
        assert_eq!(code(links.confirm(&wrong, &profile(TG)).await), EXPIRED);
    }
    for result in [
        links
            .prompt("tglink_ffffffffffffffffffffffffffffffff", &profile(TG))
            .await
            .map(|_| ()),
        links
            .confirm("tglink_ffffffffffffffffffffffffffffffff", &profile(TG))
            .await
            .map(|_| ()),
        links
            .reject("tglink_ffffffffffffffffffffffffffffffff", &profile(TG))
            .await,
    ] {
        assert_eq!(code(result), RATE_LIMITED);
    }
    assert_eq!(
        code(
            links
                .confirm("tglink_ffffffffffffffffffffffffffffffff", &profile(TG + 1))
                .await
        ),
        EXPIRED
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn старые_запросы_привязки_убираются() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let client = by_email(services, "alexey@mail.example").await;
    let (_, me) = cabinet(services, client).await;
    begin(services, &me).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE client_login_requests SET expires_at = $1")
            .bind(services.now() - Duration::days(8))
            .execute(conn.conn())
            .await
            .unwrap();
    }
    begin(services, &me).await;
    let mut conn = services.store().conn().await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM client_login_requests")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(left, 1);
    drop(conn);
    env.db.cleanup().await;
}
