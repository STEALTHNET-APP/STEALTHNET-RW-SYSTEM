use crate::admin::{FeedKind, FeedQuery, MarkPaidRequest, MarkPaidResult, PaymentFilter, Sees};
use crate::auth::{RequestMeta, TOTP_INVALID, TOTP_REQUIRED};
use crate::limits::{RATE_LIMITED, rules};
use crate::login::TelegramLoginStatus;
use crate::settings::{CoreUpdate, ModuleConfigUpdate, ProviderUpdate};
use crate::support::{OpenTicket, ReplyRequest};
use crate::tariffs::{PeriodInput, TariffInput};
use crate::*;
use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use sn_domain::client::{ClientStatus, IdentityKind, RegistrationSource, TelegramProfile};
use sn_domain::ids::{AdminId, CategoryId, ClientId, SquadId};
use sn_domain::jobs::JobPayload;
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money, Stars};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::payment::{NewPayment, PaymentStatus, ProviderId, providers};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::settings::{SettingField, keys, secret_keys};
use sn_domain::support::{TicketChannel, TicketFilter};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, Tariff};
use sn_domain::traffic::TrafficPolicy;
use sn_modules::{Module, ModuleRegistry};
use sn_payments::ProviderRegistry;
use sn_remnawave::fake::FakeRemnawave;
use sn_remnawave::{InternalSquad, SquadInfo};
use sn_store::testing::{TestDb, test_store};
use sn_store::{Db, NewCategory, SecretBox};
use std::sync::Arc;
use uuid::Uuid;

const PASSWORD: &str = "очень-надёжный-пароль";

mod admins;
mod ai_support;
mod alerts;
mod antibot;
mod api_keys;
mod app_links;
mod client_access;
mod digest;
mod email_change;
mod email_link;
mod ext_modules;
mod limits;
mod mail;
mod maintenance;
mod nodes;
mod one_connection;
mod paces;
mod remna_guard;
mod server_clock;
mod signup;
mod site_limits;
pub(crate) mod siteverify;
mod telegram_link;
mod utm;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

struct Env {
    db: TestDb,
    services: Services,
    clock: Arc<FixedClock>,
    remna: Arc<FakeRemnawave>,
}

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap()
}

fn meta(ip: &str) -> RequestMeta {
    RequestMeta {
        ip: Some(ip.to_string()),
        user_agent: Some(UA.to_string()),
    }
}

async fn env_with(modules: ModuleRegistry) -> Option<Env> {
    env_with_fallbacks(modules, EnvFallbacks::default()).await
}

async fn env_with_fallbacks(modules: ModuleRegistry, fallbacks: EnvFallbacks) -> Option<Env> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(modules),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: fallbacks,
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Env {
        db,
        services,
        clock,
        remna,
    })
}

async fn env() -> Option<Env> {
    env_with(ModuleRegistry::catalog_only()).await
}

fn code<T: std::fmt::Debug>(result: ServiceResult<T>) -> &'static str {
    result.unwrap_err().code()
}

fn profile(id: i64) -> TelegramProfile {
    TelegramProfile {
        id,
        username: Some("morozov_a".into()),
        first_name: Some("Алексей".into()),
        last_name: None,
        language_code: Some("ru".into()),
        is_premium: false,
    }
}

async fn owner(services: &Services) -> AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap()
        .0
}

async fn telegram_client(services: &Services, id: i64) -> ClientId {
    services
        .clients()
        .ensure_telegram(&profile(id), RegistrationSource::Bot, None)
        .await
        .unwrap()
        .0
        .id
}

async fn set_bot(services: &Services) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set_as(keys::BOT_USERNAME, &"stealth_vpn_bot", None)
        .await
        .unwrap();
}

fn totp_code(secret: &str, at: DateTime<Utc>) -> String {
    let key = totp::base32_decode(secret).unwrap();
    totp::code_at(
        &key,
        (at.timestamp() / totp::STEP_SECONDS) as u64,
        totp::DIGITS,
    )
    .unwrap()
}

#[tokio::test]
async fn вход_админа_по_коду_totp_без_повтора_кода() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let auth = services.auth();
    let admin = owner(services).await;
    let setup = auth.admin_totp_setup(admin).await.unwrap();
    assert!(
        setup
            .uri
            .starts_with("otpauth://totp/STEALTHNET:owner%40example.com?secret=")
    );
    let now = start();
    assert_eq!(
        code(
            auth.admin_totp_enable(
                admin,
                &setup.secret,
                &totp_code(&setup.secret, now),
                "чужой-пароль-123"
            )
            .await
        ),
        "auth.wrong_password"
    );
    let far = totp_code(&setup.secret, now + Duration::hours(1));
    if far != totp_code(&setup.secret, now) {
        assert_eq!(
            code(
                auth.admin_totp_enable(admin, &setup.secret, &far, PASSWORD)
                    .await
            ),
            "auth.totp_code"
        );
    }
    auth.admin_totp_enable(
        admin,
        &setup.secret,
        &totp_code(&setup.secret, now),
        PASSWORD,
    )
    .await
    .unwrap();
    let stored = {
        let mut conn = services.store().conn().await.unwrap();
        conn.admins().get(admin).await.unwrap()
    };
    let sealed = stored.totp.unwrap();
    assert_ne!(sealed.ciphertext, setup.secret.as_bytes());
    let anon = RequestMeta::default();
    assert_eq!(
        code(
            auth.admin_login("owner@example.com", PASSWORD, None, &anon)
                .await
        ),
        TOTP_REQUIRED
    );
    assert_eq!(
        code(
            auth.admin_login(
                "owner@example.com",
                PASSWORD,
                Some(&totp_code(&setup.secret, now)),
                &anon
            )
            .await
        ),
        TOTP_INVALID
    );
    env.clock.set(now + Duration::seconds(30));
    let fresh = totp_code(&setup.secret, now + Duration::seconds(30));
    let login = auth
        .admin_login("owner@example.com", PASSWORD, Some(&fresh), &anon)
        .await
        .unwrap();
    assert_eq!(login.principal.admin_id, admin);
    assert_eq!(
        code(
            auth.admin_login("owner@example.com", PASSWORD, Some(&fresh), &anon)
                .await
        ),
        TOTP_INVALID
    );
    assert!(auth.admin_security(admin).await.unwrap().totp_enabled);
    env.clock.set(now + Duration::seconds(60));
    auth.admin_totp_disable(
        admin,
        PASSWORD,
        &totp_code(&setup.secret, now + Duration::seconds(60)),
    )
    .await
    .unwrap();
    auth.admin_login("owner@example.com", PASSWORD, None, &anon)
        .await
        .unwrap();
    auth.admin_totp_enable(
        admin,
        &setup.secret,
        &totp_code(&setup.secret, now + Duration::seconds(90)),
        PASSWORD,
    )
    .await
    .unwrap();
    assert!(auth.admin_security(admin).await.unwrap().totp_enabled);
    auth.upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap();
    assert!(!auth.admin_security(admin).await.unwrap().totp_enabled);
    env.db.cleanup().await;
}

#[tokio::test]
async fn смена_пароля_закрывает_все_сессии_админа() {
    let Some(env) = env().await else { return };
    let auth = env.services.auth();
    let admin = owner(&env.services).await;
    let anon = RequestMeta::default();
    let first = auth
        .admin_login("owner@example.com", PASSWORD, None, &anon)
        .await
        .unwrap();
    let second = auth
        .admin_login("owner@example.com", PASSWORD, None, &anon)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.change_admin_password(admin, "неверный-пароль-1", "новый-надёжный-пароль", &anon)
                .await
        ),
        "auth.wrong_password"
    );
    assert_eq!(
        code(
            auth.change_admin_password(admin, PASSWORD, "коротко", &anon)
                .await
        ),
        "auth.weak_password"
    );
    let renewed = auth
        .change_admin_password(admin, PASSWORD, "новый-надёжный-пароль", &anon)
        .await
        .unwrap();
    assert!(!renewed.must_change_password);
    for token in [&first.token, &second.token] {
        assert!(matches!(
            auth.admin_session(token).await,
            Err(ServiceError::Unauthorized)
        ));
    }
    assert_eq!(
        auth.admin_session(&renewed.token).await.unwrap().admin_id,
        admin
    );
    assert!(matches!(
        auth.admin_login("owner@example.com", PASSWORD, None, &anon)
            .await,
        Err(ServiceError::Unauthorized)
    ));
    auth.admin_login("owner@example.com", "новый-надёжный-пароль", None, &anon)
        .await
        .unwrap();
    env.db.cleanup().await;
}

#[tokio::test]
async fn лимит_попыток_считается_окнами_по_адресу() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    set_bot(services).await;
    let auth = services.auth();
    let rule = rules::TELEGRAM_START_IP;
    for _ in 0..rule.limit {
        auth.telegram_login_start(&meta("203.0.113.7"))
            .await
            .unwrap();
    }
    assert_eq!(
        code(auth.telegram_login_start(&meta("203.0.113.7")).await),
        RATE_LIMITED
    );
    auth.telegram_login_start(&meta("198.51.100.1"))
        .await
        .unwrap();
    env.clock
        .set(start() + Duration::seconds(rule.window_seconds));
    auth.telegram_login_start(&meta("203.0.113.7"))
        .await
        .unwrap();
    let limits = services.limits();
    assert_eq!(limits.hit(rules::ADMIN_SECURITY, "x").await.unwrap(), 1);
    assert_eq!(limits.hit(rules::ADMIN_SECURITY, " X ").await.unwrap(), 2);
    env.db.cleanup().await;
}

#[tokio::test]
async fn вход_в_кабинет_через_telegram_выдаёт_сессию_один_раз() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let auth = services.auth();
    assert_eq!(
        code(auth.telegram_login_start(&meta("203.0.113.7")).await),
        "bot.not_configured"
    );
    set_bot(services).await;
    let start_login = auth
        .telegram_login_start(&meta("203.0.113.7"))
        .await
        .unwrap();
    let code_part = start_login
        .link
        .strip_prefix("https://t.me/stealth_vpn_bot?start=")
        .unwrap()
        .to_string();
    assert!(code_part.starts_with("auth_"));
    assert!(matches!(
        auth.telegram_login_status(&start_login.poll_token, &meta("203.0.113.7"))
            .await
            .unwrap(),
        TelegramLoginStatus::Pending { .. }
    ));
    let prompt = auth
        .telegram_login_prompt(&code_part, &profile(5_093_118_422))
        .await
        .unwrap();
    assert_eq!(prompt.ip.as_deref(), Some("203.0.113.7"));
    assert_eq!(prompt.device.as_deref(), Some("Chrome, Windows"));
    assert!(prompt.text().contains("Алексей (@morozov_a)"));
    assert!(matches!(
        auth.confirm_telegram_login("auth_ffffffffffffffffffffffffffffffff", &profile(1))
            .await,
        Err(ServiceError::NotFound(_))
    ));
    let confirmed = auth
        .confirm_telegram_login(&code_part, &profile(5_093_118_422))
        .await
        .unwrap();
    assert!(confirmed.created);
    assert_eq!(
        code(
            auth.confirm_telegram_login(&code_part, &profile(5_093_118_422))
                .await
        ),
        "auth.login_expired"
    );
    let status = auth
        .telegram_login_status(&start_login.poll_token, &meta("203.0.113.7"))
        .await
        .unwrap();
    let TelegramLoginStatus::Confirmed { login } = status else {
        panic!("ожидалась сессия, получено {status:?}");
    };
    assert_eq!(login.principal.client_id, confirmed.client_id);
    let principal = auth.client_session(&login.token).await.unwrap();
    assert_eq!(principal.client_id, confirmed.client_id);
    assert_eq!(principal.channel, sn_domain::access::ClientChannel::Cabinet);
    assert_eq!(
        auth.telegram_login_status(&start_login.poll_token, &meta("203.0.113.7"))
            .await
            .unwrap(),
        TelegramLoginStatus::Expired
    );
    let client = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients().get(confirmed.client_id).await.unwrap()
    };
    assert_eq!(client.registration_source, RegistrationSource::Web);
    auth.client_logout(&login.token).await.unwrap();
    assert!(matches!(
        auth.client_session(&login.token).await,
        Err(ServiceError::Unauthorized)
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn отказ_и_просроченная_ссылка_входа_не_дают_сессию() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    set_bot(services).await;
    let auth = services.auth();
    let rejected = auth
        .telegram_login_start(&meta("203.0.113.7"))
        .await
        .unwrap();
    let rejected_code = rejected.link.rsplit('=').next().unwrap().to_string();
    auth.reject_telegram_login(&rejected_code).await.unwrap();
    assert_eq!(
        auth.telegram_login_status(&rejected.poll_token, &meta("203.0.113.7"))
            .await
            .unwrap(),
        TelegramLoginStatus::Rejected
    );
    assert_eq!(
        code(
            auth.confirm_telegram_login(&rejected_code, &profile(42))
                .await
        ),
        "auth.login_expired"
    );
    let late = auth
        .telegram_login_start(&meta("203.0.113.7"))
        .await
        .unwrap();
    let late_code = late.link.rsplit('=').next().unwrap().to_string();
    env.clock.set(start() + Duration::minutes(6));
    assert_eq!(
        auth.telegram_login_status(&late.poll_token, &meta("203.0.113.7"))
            .await
            .unwrap(),
        TelegramLoginStatus::Expired
    );
    assert_eq!(
        code(auth.confirm_telegram_login(&late_code, &profile(42)).await),
        "auth.login_expired"
    );
    assert!(matches!(
        auth.telegram_login_status("не-тот-токен", &meta("203.0.113.7"))
            .await,
        Err(ServiceError::NotFound(_))
    ));
    let blocked = telegram_client(services, 77).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .set_status(blocked, ClientStatus::Blocked, None)
            .await
            .unwrap();
    }
    let fresh = auth
        .telegram_login_start(&meta("203.0.113.7"))
        .await
        .unwrap();
    let fresh_code = fresh.link.rsplit('=').next().unwrap().to_string();
    assert!(matches!(
        auth.confirm_telegram_login(&fresh_code, &profile(77)).await,
        Err(ServiceError::Forbidden)
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn вход_по_почте_одноразовым_кодом() {
    let Some(env) = env().await else { return };
    let auth = env.services.auth();
    let who = meta("203.0.113.7");
    assert_eq!(
        code(auth.email_login_start("Alex@Example.com", &who).await),
        "auth.email_password_mode"
    );
    assert_eq!(
        code(auth.email_login_start("не почта", &who).await),
        "validation.invalid_value"
    );
    let (first, _) = auth
        .issue_email_code("Alex@Example.com", &who)
        .await
        .unwrap();
    let wrong = if first == "000000" {
        "111111"
    } else {
        "000000"
    };
    for _ in 0..crate::login::EMAIL_CODE_MAX_ATTEMPTS {
        assert_eq!(
            code(
                auth.email_login_verify("alex@example.com", wrong, &who)
                    .await
            ),
            "auth.code_invalid"
        );
    }
    assert_eq!(
        code(
            auth.email_login_verify("alex@example.com", &first, &who)
                .await
        ),
        "auth.code_attempts"
    );
    let (second, issued) = auth
        .issue_email_code("alex@example.com", &who)
        .await
        .unwrap();
    assert_eq!(issued.email, "alex@example.com");
    let two_factor::CabinetLogin::Session(login) = auth
        .email_login_verify(" ALEX@example.com ", &second, &who)
        .await
        .unwrap()
    else {
        panic!("без второго фактора вход по почте не просит код");
    };
    assert!(login.created);
    assert_eq!(
        code(
            auth.email_login_verify("alex@example.com", &second, &who)
                .await
        ),
        "auth.code_invalid"
    );
    let mut conn = env.services.store().conn().await.unwrap();
    let client = conn
        .clients()
        .find_by_identity(IdentityKind::Email, "alex@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(client.id, login.principal.client_id);
    drop(conn);
    let (third, _) = auth
        .issue_email_code("alex@example.com", &who)
        .await
        .unwrap();
    env.clock.set(start() + Duration::minutes(16));
    assert_eq!(
        code(
            auth.email_login_verify("alex@example.com", &third, &who)
                .await
        ),
        "auth.code_invalid"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn второй_фактор_клиента_спрашивается_при_входе_на_сайт_и_сбрасывается_админом() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.7");
    set_bot(services).await;
    let admin = owner(services).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let off = two_factor::TwoFactor {
        enabled: false,
        enabled_at: None,
    };
    assert_eq!(auth.client_two_factor(client).await.unwrap(), off);
    assert_eq!(
        code(auth.client_totp_disable(client, "123456").await),
        "auth.totp_disabled"
    );
    assert_eq!(
        code(
            services
                .clients()
                .reset_two_factor(client, Actor::Admin(admin))
                .await
        ),
        two_factor::TOTP_NOT_ENABLED
    );
    let setup = auth.client_totp_setup(client).await.unwrap();
    assert!(
        setup
            .uri
            .starts_with("otpauth://totp/STEALTHNET:%40morozov_a?secret="),
        "{}",
        setup.uri
    );
    let now = start();
    assert_eq!(
        code(
            auth.client_totp_enable(client, "не-base32", &totp_code(&setup.secret, now))
                .await
        ),
        "auth.totp_secret"
    );
    let far = totp_code(&setup.secret, now + Duration::hours(1));
    if far != totp_code(&setup.secret, now) {
        assert_eq!(
            code(auth.client_totp_enable(client, &setup.secret, &far).await),
            "auth.totp_code"
        );
    }
    let enabled = auth
        .client_totp_enable(
            client,
            &setup.secret.to_lowercase(),
            &totp_code(&setup.secret, now),
        )
        .await
        .unwrap();
    assert_eq!(
        enabled,
        two_factor::TwoFactor {
            enabled: true,
            enabled_at: Some(now),
        }
    );
    assert_eq!(auth.client_two_factor(client).await.unwrap(), enabled);
    assert_eq!(
        code(auth.client_totp_setup(client).await),
        "auth.totp_enabled"
    );

    let started = auth.telegram_login_start(&who).await.unwrap();
    let link = started.link.rsplit('=').next().unwrap().to_string();
    auth.confirm_telegram_login(&link, &profile(5_093_118_422))
        .await
        .unwrap();
    let TelegramLoginStatus::TotpRequired {
        challenge,
        expires_at,
    } = auth
        .telegram_login_status(&started.poll_token, &who)
        .await
        .unwrap()
    else {
        panic!("вход через Telegram должен попросить код");
    };
    assert_eq!(
        expires_at,
        now + Duration::seconds(two_factor::TOTP_CHALLENGE_TTL_SECONDS)
    );
    assert_eq!(
        auth.telegram_login_status(&started.poll_token, &who)
            .await
            .unwrap(),
        TelegramLoginStatus::Expired
    );
    let wrong = |at| {
        if totp_code(&setup.secret, at) == "000000" {
            "111111"
        } else {
            "000000"
        }
    };
    assert_eq!(
        code(auth.cabinet_totp_login(&challenge, wrong(now), &who).await),
        "auth.totp_code"
    );
    assert_eq!(
        code(
            auth.cabinet_totp_login(&challenge, &totp_code(&setup.secret, now), &who)
                .await
        ),
        "auth.totp_code"
    );
    assert_eq!(
        code(auth.cabinet_totp_login("чужой", "000000", &who).await),
        "auth.login_expired"
    );
    let later = now + Duration::seconds(30);
    env.clock.set(later);
    let login = auth
        .cabinet_totp_login(&challenge, &totp_code(&setup.secret, later), &who)
        .await
        .unwrap();
    assert_eq!(login.principal.client_id, client);
    assert_eq!(
        login.principal.channel,
        sn_domain::access::ClientChannel::Cabinet
    );
    let cabinet = login.token.clone();
    assert!(auth.client_session(&cabinet).await.is_ok());
    assert_eq!(
        code(
            auth.cabinet_totp_login(
                &challenge,
                &totp_code(&setup.secret, later + Duration::seconds(30)),
                &who
            )
            .await
        ),
        "auth.login_expired"
    );

    let hour = later + Duration::hours(1);
    env.clock.set(hour);
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .add_identity(
            client,
            &sn_domain::client::NewIdentity::new(IdentityKind::Email, "alex@example.com", true)
                .unwrap(),
        )
        .await
        .unwrap();
    drop(conn);
    let (mail_code, _) = auth
        .issue_email_code("alex@example.com", &who)
        .await
        .unwrap();
    let two_factor::CabinetLogin::TotpRequired(mail) = auth
        .email_login_verify("alex@example.com", &mail_code, &who)
        .await
        .unwrap()
    else {
        panic!("вход по почте должен попросить код");
    };
    for _ in 0..two_factor::TOTP_CHALLENGE_ATTEMPTS {
        assert_eq!(
            code(
                auth.cabinet_totp_login(&mail.challenge, wrong(hour), &who)
                    .await
            ),
            "auth.totp_code"
        );
    }
    assert_eq!(
        code(
            auth.cabinet_totp_login(&mail.challenge, &totp_code(&setup.secret, hour), &who)
                .await
        ),
        "auth.code_attempts"
    );
    let (mail_code, _) = auth
        .issue_email_code("alex@example.com", &who)
        .await
        .unwrap();
    let two_factor::CabinetLogin::TotpRequired(locked) = auth
        .email_login_verify("alex@example.com", &mail_code, &who)
        .await
        .unwrap()
    else {
        panic!("вход по почте должен попросить код");
    };
    assert_eq!(
        code(
            auth.cabinet_totp_login(&locked.challenge, &totp_code(&setup.secret, hour), &who)
                .await
        ),
        RATE_LIMITED
    );

    let next_day = hour + Duration::days(1);
    env.clock.set(next_day);
    assert_eq!(
        code(auth.client_totp_disable(client, wrong(next_day)).await),
        "auth.totp_code"
    );
    assert_eq!(
        auth.client_totp_disable(client, &totp_code(&setup.secret, next_day))
            .await
            .unwrap(),
        off
    );
    let again = auth.client_totp_setup(client).await.unwrap();
    assert_ne!(again.secret, setup.secret);
    auth.client_totp_enable(client, &again.secret, &totp_code(&again.secret, next_day))
        .await
        .unwrap();
    let miniapp = auth
        .client_session_for(
            client,
            sn_store::SessionChannel::Miniapp,
            &RequestMeta::default(),
            false,
        )
        .await
        .unwrap();
    let (mail_code, _) = auth
        .issue_email_code("alex@example.com", &who)
        .await
        .unwrap();
    let two_factor::CabinetLogin::TotpRequired(pending) = auth
        .email_login_verify("alex@example.com", &mail_code, &who)
        .await
        .unwrap()
    else {
        panic!("вход по почте должен попросить код");
    };
    services
        .clients()
        .reset_two_factor(client, Actor::Admin(admin))
        .await
        .unwrap();
    assert_eq!(auth.client_two_factor(client).await.unwrap(), off);
    assert!(auth.client_session(&cabinet).await.is_err());
    assert!(auth.client_session(&miniapp.token).await.is_ok());
    let after = next_day + Duration::seconds(30);
    env.clock.set(after);
    assert_eq!(
        code(
            auth.cabinet_totp_login(&pending.challenge, &totp_code(&again.secret, after), &who)
                .await
        ),
        "auth.login_expired"
    );
    let (mail_code, _) = auth
        .issue_email_code("alex@example.com", &who)
        .await
        .unwrap();
    assert!(matches!(
        auth.email_login_verify("alex@example.com", &mail_code, &who)
            .await
            .unwrap(),
        two_factor::CabinetLogin::Session(_)
    ));
    assert_eq!(
        code(
            services
                .clients()
                .reset_two_factor(client, Actor::Admin(admin))
                .await
        ),
        two_factor::TOTP_NOT_ENABLED
    );
    let mut conn = services.store().conn().await.unwrap();
    let audit = conn
        .audit()
        .for_entity("client", client.0, 20)
        .await
        .unwrap();
    let actions: Vec<&str> = audit.iter().map(|a| a.action.as_str()).collect();
    for action in [
        "client.totp_enabled",
        "client.totp_disabled",
        "client.totp_reset",
    ] {
        assert!(actions.contains(&action), "{actions:?}");
    }
    let reset = audit
        .iter()
        .find(|a| a.action == "client.totp_reset")
        .unwrap();
    assert_eq!(reset.details["revoked_sessions"], 1);
    assert_eq!(reset.details["rejected_logins"], 1);
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn блокировка_клиента_закрывает_сессии_и_пишет_событие_один_раз() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let session = services
        .auth()
        .client_session_for(
            client,
            sn_store::SessionChannel::Miniapp,
            &RequestMeta::default(),
            false,
        )
        .await
        .unwrap();
    for _ in 0..2 {
        let result = services
            .clients()
            .block(client, Some("  спам  "), Actor::Admin(admin))
            .await;
        result.unwrap();
    }
    assert!(
        services
            .auth()
            .client_session(&session.token)
            .await
            .is_err()
    );
    let mut conn = services.store().conn().await.unwrap();
    let stored = conn.clients().get(client).await.unwrap();
    assert!(stored.is_blocked());
    assert_eq!(stored.block_reason.as_deref(), Some("спам"));
    let events = conn.events().for_client(client, 20).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event.kind() == "client_blocked")
            .count(),
        1
    );
    drop(conn);
    assert_eq!(
        code(
            services
                .clients()
                .block(client, Some(&"x".repeat(501)), Actor::Admin(admin))
                .await
        ),
        "client.block_reason"
    );
    let result = services
        .clients()
        .unblock(client, Actor::Admin(admin))
        .await;
    result.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    assert!(!conn.clients().get(client).await.unwrap().is_blocked());
    let audit = conn
        .audit()
        .for_entity("client", client.0, 10)
        .await
        .unwrap();
    assert!(audit.iter().any(|a| a.action == "client.unblock"));
    drop(conn);
    env.db.cleanup().await;
}

async fn tariff_with(services: &Services, prices: CurrencyPrices) -> Tariff {
    let mut tx = services.store().begin().await.unwrap();
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
        .create(&sn_store::NewTariff {
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
            squads: vec![SquadId(Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![sn_store::NewPeriod {
                days: 90,
                prices,
                stars: Some(Stars(350)),
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tariff
}

#[tokio::test]
async fn смена_валюты_требует_цен_и_подтверждения_балансов() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    tariff_with(
        services,
        CurrencyPrices {
            usd: Some(699),
            rub: None,
        },
    )
    .await;
    let to_rub = CoreUpdate {
        currency: Some(Currency::Rub),
        ..CoreUpdate::default()
    };
    let ops = services.settings_ops();
    let err = ops.update_core(&to_rub, admin).await.unwrap_err();
    assert_eq!(err.code(), "settings.currency_prices");
    assert!(err.to_string().contains("«Стандарт» (90 дн.)"));
    assert!(err.to_string().contains("снимите эти тарифы с продажи"));
    {
        let mut conn = services.store().conn().await.unwrap();
        let tariffs = conn.tariffs().list(true).await.unwrap();
        conn.tariffs()
            .set_on_sale(tariffs[0].id, false)
            .await
            .unwrap();
    }
    let client = telegram_client(services, 5_093_118_422).await;
    services
        .clients()
        .adjust_balance(
            client,
            Money::new(420, Currency::Usd),
            "подарок",
            admin,
            "k1",
        )
        .await
        .unwrap();
    assert_eq!(
        code(ops.update_core(&to_rub, admin).await),
        "settings.currency_balances"
    );
    let confirmed = CoreUpdate {
        confirm_currency_change: true,
        brand_name: Some("  Turtle VPN  ".into()),
        ..to_rub.clone()
    };
    let view = ops.update_core(&confirmed, admin).await.unwrap();
    assert_eq!(view.settings.currency, Currency::Rub);
    assert_eq!(view.settings.brand_name, "Turtle VPN");
    assert_eq!(services.settings().currency, Currency::Rub);
    assert_eq!(services.modules().currency(), Currency::Rub);
    assert_eq!(
        code(
            ops.update_core(
                &CoreUpdate {
                    invoice_ttl_minutes: Some(1),
                    ..CoreUpdate::default()
                },
                admin
            )
            .await
        ),
        "settings.invalid_field"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn секреты_платёжной_системы_только_на_запись() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    let enable_without_token = ProviderUpdate {
        enabled: Some(true),
        ..ProviderUpdate::default()
    };
    assert_eq!(
        code(
            ops.update_provider(providers::CRYPTOPAY, &enable_without_token, admin)
                .await
        ),
        "settings.provider_invalid"
    );
    let with_token = ProviderUpdate {
        topup_bonus: None,
        currency: None,
        enabled: Some(true),
        config: serde_json::from_value(serde_json::json!({
            "api_token": "4321:SECRET-TOKEN", "testnet": "true"
        }))
        .unwrap(),
        clear: Vec::new(),
        channels: None,
        methods: None,
        label: None,
    };
    let details = ops
        .update_provider(providers::CRYPTOPAY, &with_token, admin)
        .await
        .unwrap();
    assert!(details.provider.enabled && details.provider.configured);
    assert_eq!(
        details.provider.settings["api_token"],
        serde_json::json!({ "filled": true })
    );
    assert!(
        !serde_json::to_string(&details)
            .unwrap()
            .contains("SECRET-TOKEN")
    );
    let keep = ProviderUpdate {
        config: serde_json::from_value(serde_json::json!({ "api_token": "" })).unwrap(),
        ..ProviderUpdate::default()
    };
    ops.update_provider(providers::CRYPTOPAY, &keep, admin)
        .await
        .unwrap();
    assert_eq!(
        services
            .payments()
            .config(providers::CRYPTOPAY)
            .get("api_token"),
        Some("4321:SECRET-TOKEN")
    );
    let mut conn = services.store().conn().await.unwrap();
    let stored = conn
        .settings()
        .get(&keys::provider(providers::CRYPTOPAY))
        .await
        .unwrap()
        .unwrap();
    assert!(!stored.to_string().contains("SECRET"));
    assert_eq!(stored["config"]["testnet"], true);
    drop(conn);
    let clear = ProviderUpdate {
        enabled: Some(false),
        clear: vec!["api_token".into()],
        ..ProviderUpdate::default()
    };
    let cleared = ops
        .update_provider(providers::CRYPTOPAY, &clear, admin)
        .await
        .unwrap();
    assert!(!cleared.provider.configured);
    assert_eq!(
        code(
            ops.update_provider(
                providers::CRYPTOPAY,
                &ProviderUpdate {
                    config: serde_json::from_value(serde_json::json!({ "nope": 1 })).unwrap(),
                    ..ProviderUpdate::default()
                },
                admin
            )
            .await
        ),
        "settings.invalid_field"
    );
    assert!(matches!(
        ops.update_provider("unknown", &ProviderUpdate::default(), admin)
            .await,
        Err(ServiceError::NotFound(_))
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn overpay_включается_с_сертификатом_из_p12() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    ops.update_core(
        &CoreUpdate {
            currency: Some(Currency::Rub),
            ..CoreUpdate::default()
        },
        admin,
    )
    .await
    .unwrap();
    let pasted = include_str!("../../sn-payments/src/providers/overpay/testdata/merchant.pem")
        .replace(['\n', '\r'], "");
    assert!(pasted.chars().count() > 2_000);
    let update = |certificate: &str| ProviderUpdate {
        enabled: Some(true),
        config: serde_json::from_value(serde_json::json!({
            "project_id": "1017",
            "login": "shop",
            "password": "secret",
            "certificate": certificate
        }))
        .unwrap(),
        methods: Some(vec![
            sn_domain::payment::MethodChoice {
                code: "sbp".into(),
                title: None,
            },
            sn_domain::payment::MethodChoice {
                code: "card".into(),
                title: Some("Картой".into()),
            },
        ]),
        ..ProviderUpdate::default()
    };
    assert_eq!(
        code(
            ops.update_provider(
                providers::OVERPAY,
                &update("-----BEGIN CERTIFICATE-----"),
                admin
            )
            .await
        ),
        "settings.provider_invalid"
    );
    let details = ops
        .update_provider(providers::OVERPAY, &update(&pasted), admin)
        .await
        .unwrap();
    assert!(details.provider.enabled && details.provider.configured);
    assert_eq!(
        details.provider.settings["certificate"],
        serde_json::json!({ "filled": true })
    );
    assert!(
        !serde_json::to_string(&details)
            .unwrap()
            .contains("PRIVATE KEY")
    );
    assert!(
        details
            .schema
            .iter()
            .any(|f| f.key == "certificate" && f.large)
    );
    let chosen: Vec<&str> = details
        .provider
        .methods
        .iter()
        .filter(|m| m.enabled)
        .map(|m| m.code)
        .collect();
    assert_eq!(chosen, ["sbp", "card"]);
    assert!(
        services
            .payments()
            .is_available(providers::OVERPAY, Currency::Rub)
    );
    let mut conn = services.store().conn().await.unwrap();
    let stored = conn
        .settings()
        .get(&keys::provider(providers::OVERPAY))
        .await
        .unwrap()
        .unwrap();
    assert!(!stored.to_string().contains("PRIVATE"));
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn подключение_remnawave_и_секрет_вебхуков() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    let check = ops.check_remnawave(&Default::default()).await.unwrap();
    assert!(check.ok && check.supported);
    assert_eq!(check.sub_public_domain.as_deref(), Some("sub.example.com"));
    assert_eq!(check.webhooks_enabled, Some(true));
    let generated = ops.generate_remna_webhook_secret(admin).await.unwrap();
    assert_eq!(generated.secret.len(), 64);
    assert!(
        generated
            .env
            .contains(&format!("WEBHOOK_SECRET_HEADER={}", generated.secret))
    );
    assert_eq!(
        services.remna_webhook_secret().as_deref(),
        Some(generated.secret.as_str())
    );
    let view = ops.remnawave().await.unwrap();
    assert!(view.webhook_secret_set);
    assert!(!view.env.unwrap_or_default().contains(&generated.secret));
    let updated = ops
        .update_remnawave(
            &crate::settings::RemnaUpdate {
                url: Some("https://panel.example.com/".into()),
                token: Some("eyJhbGciOi.token".into()),
                cookie: Some("caddy:secretvalue".into()),
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(updated.url.as_deref(), Some("https://panel.example.com"));
    assert!(updated.token_set && updated.cookie_set && !updated.token_from_env);
    assert!(services.remna().is_ok());
    let failed = ops
        .check_remnawave(&crate::settings::RemnaCheckRequest {
            url: Some("ftp://panel".into()),
            ..Default::default()
        })
        .await;
    assert_eq!(code(failed), "settings.invalid_field");
    assert_eq!(
        code(
            ops.update_remnawave(
                &crate::settings::RemnaUpdate {
                    webhook_secret: Some("short".into()),
                    ..Default::default()
                },
                admin
            )
            .await
        ),
        "settings.invalid_field"
    );
    env.db.cleanup().await;
}

fn provider_config(value: serde_json::Value) -> ProviderUpdate {
    ProviderUpdate {
        config: serde_json::from_value(value).unwrap(),
        ..ProviderUpdate::default()
    }
}

#[tokio::test]
async fn смена_адреса_кассы_требует_ключи_заново() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    ops.update_provider(
        providers::HELEKET,
        &provider_config(serde_json::json!({ "merchant_id": "m-1", "api_key": "hk-old" })),
        admin,
    )
    .await
    .unwrap();
    let moved = ops
        .update_provider(
            providers::HELEKET,
            &provider_config(serde_json::json!({ "api_url": "https://collector.example.org" })),
            admin,
        )
        .await
        .unwrap_err();
    assert_eq!(moved.code(), crate::settings::SECRETS_AGAIN);
    assert!(
        moved.to_string().contains("«Платёжный API-ключ»"),
        "{moved}"
    );
    assert_eq!(
        services
            .payments()
            .config(providers::HELEKET)
            .get("api_url"),
        None
    );
    ops.update_provider(
        providers::HELEKET,
        &provider_config(serde_json::json!({ "merchant_id": "m-2" })),
        admin,
    )
    .await
    .unwrap();
    ops.update_provider(
        providers::HELEKET,
        &provider_config(serde_json::json!({
            "api_url": "https://collector.example.org", "api_key": "hk-new"
        })),
        admin,
    )
    .await
    .unwrap();
    let config = services.payments().config(providers::HELEKET);
    assert_eq!(config.get("api_url"), Some("https://collector.example.org"));
    assert_eq!(config.get("api_key"), Some("hk-new"));
    ops.update_provider(
        providers::HELEKET,
        &provider_config(serde_json::json!({ "api_url": "https://collector.example.org/" })),
        admin,
    )
    .await
    .unwrap();
    assert_eq!(
        code(
            ops.update_provider(
                providers::HELEKET,
                &ProviderUpdate {
                    clear: vec!["api_url".into()],
                    ..ProviderUpdate::default()
                },
                admin
            )
            .await
        ),
        crate::settings::SECRETS_AGAIN
    );
    ops.update_provider(
        providers::HELEKET,
        &ProviderUpdate {
            clear: vec!["api_url".into(), "api_key".into()],
            ..ProviderUpdate::default()
        },
        admin,
    )
    .await
    .unwrap();
    assert_eq!(
        services
            .payments()
            .config(providers::HELEKET)
            .get("api_key"),
        None
    );

    let pasted = include_str!("../../sn-payments/src/providers/overpay/testdata/merchant.pem")
        .replace(['\n', '\r'], "");
    ops.update_provider(
        providers::OVERPAY,
        &provider_config(serde_json::json!({
            "project_id": "1017", "login": "shop", "password": "secret", "certificate": pasted
        })),
        admin,
    )
    .await
    .unwrap();
    let moved = ops
        .update_provider(
            providers::OVERPAY,
            &provider_config(serde_json::json!({ "api_url": "https://collector.example.org" })),
            admin,
        )
        .await
        .unwrap_err();
    assert!(
        moved
            .to_string()
            .contains("«Адрес API» вставьте заново «Пароль» и «Сертификат клиента»"),
        "{moved}"
    );
    let half = ops
        .update_provider(
            providers::OVERPAY,
            &provider_config(serde_json::json!({
                "api_url": "https://collector.example.org", "password": "secret"
            })),
            admin,
        )
        .await
        .unwrap_err();
    assert!(half.to_string().contains("«Сертификат клиента»"), "{half}");
    assert!(!half.to_string().contains("«Пароль»"), "{half}");
    assert_ne!(
        services
            .payments()
            .config(providers::OVERPAY)
            .get("api_url"),
        Some("https://collector.example.org")
    );
    ops.update_provider(
        providers::OVERPAY,
        &provider_config(serde_json::json!({
            "api_url": "https://collector.example.org", "password": "secret", "certificate": pasted
        })),
        admin,
    )
    .await
    .unwrap();
    assert_eq!(
        services
            .payments()
            .config(providers::OVERPAY)
            .get("api_url"),
        Some("https://collector.example.org")
    );
    env.db.cleanup().await;
}

async fn sent_secrets(server: &wiremock::MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .flat_map(|request| {
            request
                .headers
                .iter()
                .map(|(name, value)| format!("{name}: {}", value.to_str().unwrap_or_default()))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[tokio::test]
async fn смена_адреса_remnawave_не_отправляет_сохранённый_токен() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    let old = wiremock::MockServer::start().await;
    let new = wiremock::MockServer::start().await;
    ops.update_remnawave(
        &crate::settings::RemnaUpdate {
            url: Some(old.uri()),
            token: Some("tok-old".into()),
            cookie: Some("caddy:cookie-old".into()),
            ..Default::default()
        },
        admin,
    )
    .await
    .unwrap();
    let probe = ops
        .check_remnawave(&crate::settings::RemnaCheckRequest {
            url: Some(new.uri()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!probe.ok);
    assert_eq!(
        probe.error.as_deref(),
        Some(
            "При смене адреса панели вставьте заново API-токен и куку: панель не отправляет сохранённые ключи на новый адрес"
        )
    );
    let probe = ops
        .check_remnawave(&crate::settings::RemnaCheckRequest {
            url: Some(new.uri()),
            token: Some("tok-new".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(probe.error.unwrap_or_default().contains("куку"));
    assert!(sent_secrets(&new).await.is_empty());
    ops.check_remnawave(&crate::settings::RemnaCheckRequest {
        url: Some(new.uri()),
        token: Some("tok-new".into()),
        cookie: Some("caddy:cookie-new".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    let sent = sent_secrets(&new).await;
    assert!(
        sent.iter().any(|header| header.contains("tok-new")),
        "{sent:?}"
    );
    assert!(
        sent.iter()
            .all(|header| !header.contains("tok-old") && !header.contains("cookie-old")),
        "{sent:?}"
    );
    let moved = ops
        .update_remnawave(
            &crate::settings::RemnaUpdate {
                url: Some(new.uri()),
                token: Some("tok-new".into()),
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap_err();
    assert_eq!(moved.code(), crate::settings::SECRETS_AGAIN);
    assert!(moved.to_string().contains("куку"), "{moved}");
    assert_eq!(
        ops.remnawave().await.unwrap().url.as_deref(),
        Some(old.uri().as_str())
    );
    ops.update_remnawave(
        &crate::settings::RemnaUpdate {
            url: Some(format!("{}/", old.uri())),
            ..Default::default()
        },
        admin,
    )
    .await
    .unwrap();
    let saved = ops
        .update_remnawave(
            &crate::settings::RemnaUpdate {
                url: Some(new.uri()),
                token: Some("tok-new".into()),
                guard: Some(crate::settings::GuardUpdate::Off),
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(saved.url.as_deref(), Some(new.uri().as_str()));
    assert!(saved.token_set && !saved.cookie_set);
    env.db.cleanup().await;
}

struct SecretModule;

#[async_trait]
impl Module for SecretModule {
    fn id(&self) -> ModuleId {
        ModuleId::Ai
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret("api_key", "API-ключ", ""),
            SettingField::text("model", "Модель", "").optional(),
        ]
    }
}

#[tokio::test]
async fn секреты_модуля_хранятся_зашифрованными_и_доступны_модулю() {
    let registry = ModuleRegistry::new(vec![Arc::new(SecretModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    assert_eq!(
        code(
            ops.update_module_config(ModuleId::Ai, &ModuleConfigUpdate::default(), admin)
                .await
        ),
        "modules.config"
    );
    let update = ModuleConfigUpdate {
        config: serde_json::from_value(serde_json::json!({
            "api_key": "gsk-very-secret", "model": "llama"
        }))
        .unwrap(),
        clear: Vec::new(),
    };
    let view = ops
        .update_module_config(ModuleId::Ai, &update, admin)
        .await
        .unwrap();
    assert_eq!(
        view.config["api_key"],
        serde_json::json!({ "filled": true })
    );
    assert_eq!(view.config["model"], "llama");
    assert_eq!(
        services.modules().config(ModuleId::Ai).text("api_key"),
        Some("gsk-very-secret")
    );
    let mut conn = services.store().conn().await.unwrap();
    let rows = conn.modules().all().await.unwrap();
    let row = rows.iter().find(|r| r.module == "ai").unwrap();
    assert!(!row.config.to_string().contains("gsk"));
    assert!(
        conn.secrets()
            .get(&secret_keys::module("ai", "api_key"))
            .await
            .unwrap()
            .is_some()
    );
    drop(conn);
    ops.toggle_module(ModuleId::Ai, true, admin).await.unwrap();
    assert_eq!(
        services.modules().config(ModuleId::Ai).text("api_key"),
        Some("gsk-very-secret")
    );
    services.reload().await.unwrap();
    assert_eq!(
        services.modules().config(ModuleId::Ai).text("api_key"),
        Some("gsk-very-secret")
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn ответ_поддержки_уходит_туда_откуда_пришло_обращение() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let other = telegram_client(services, 42).await;
    let support = services.support();
    let ticket = support
        .open(
            client,
            &OpenTicket {
                subscription_id: None,
                channel: TicketChannel::Bot,
                subject: Some("Не пришёл чек".into()),
                body: "Не пришёл чек об оплате".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(support.for_client(client).await.unwrap().len(), 1);
    assert!(support.for_client(other).await.unwrap().is_empty());
    assert_eq!(support.waiting_count().await.unwrap(), 1);
    assert!(matches!(
        support.close(ticket, Actor::Client(other)).await,
        Err(ServiceError::NotFound(_))
    ));
    assert_eq!(
        code(
            support
                .reply(
                    ticket,
                    &ReplyRequest {
                        body: "   ".into(),
                        close: false
                    },
                    Actor::Admin(admin)
                )
                .await
        ),
        "support.body"
    );
    let reply = support
        .reply(
            ticket,
            &ReplyRequest {
                body: "Отправили чек повторно на почту".into(),
                close: true,
            },
            Actor::Admin(admin),
        )
        .await;
    match reply {
        Ok(()) => {
            let mut conn = services.store().conn().await.unwrap();
            let jobs = conn
                .jobs()
                .claim(10, std::time::Duration::from_secs(30))
                .await
                .unwrap();
            let notify: Vec<JobPayload> = jobs
                .iter()
                .filter_map(|j| j.typed().ok())
                .filter(|p| matches!(p, JobPayload::Notify { .. }))
                .collect();
            assert_eq!(notify.len(), 1);
            match &notify[0] {
                JobPayload::Notify {
                    client_id,
                    template,
                    params,
                } => {
                    assert_eq!(*client_id, client);
                    assert_eq!(template, crate::support::REPLY_TEMPLATE);
                    assert_eq!(params["closed"], true);
                }
                other => panic!("ожидалось уведомление, получено {other:?}"),
            }
        }
        Err(err) => panic!("ответ поддержки не записан: {err}"),
    }
    support.close(ticket, Actor::Client(client)).await.unwrap();
    assert_eq!(support.waiting_count().await.unwrap(), 0);
    assert!(matches!(
        support
            .open(
                client,
                &OpenTicket {
                    subscription_id: Some(sn_domain::ids::SubscriptionId(999)),
                    channel: TicketChannel::Cabinet,
                    subject: None,
                    body: "привет".into(),
                },
            )
            .await,
        Err(ServiceError::Store(sn_store::StoreError::NotFound(_)))
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn сообщения_из_бота_идут_в_открытое_обращение_а_не_в_новое() {
    use crate::support::Delivered;
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let support = services.support();
    let cabinet = support
        .open(
            client,
            &OpenTicket {
                subscription_id: None,
                channel: TicketChannel::Cabinet,
                subject: Some("Чек".into()),
                body: "Не пришёл чек".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(support.telegram_ticket(client, None).await.unwrap(), None);
    let Delivered::Opened(first) = support
        .from_telegram(client, "Не подключается", None)
        .await
        .unwrap()
    else {
        panic!("первое сообщение из бота должно открыть обращение");
    };
    assert_ne!(first, cabinet);
    support
        .reply(
            first,
            &ReplyRequest {
                body: "Обновите подписку".into(),
                close: false,
            },
            Actor::Admin(admin),
        )
        .await
        .unwrap();
    assert_eq!(support.waiting_count().await.unwrap(), 1);
    let feed = services
        .admin()
        .feed(
            &FeedQuery {
                kind: FeedKind::Support,
                remna_before: None,
                core_before: None,
                limit: Some(10),
            },
            Sees {
                money: false,
                refunds: false,
                infrastructure: true,
            },
        )
        .await
        .unwrap();
    let states: Vec<(&str, i64, TicketFilter)> = feed
        .items
        .iter()
        .filter_map(|i| {
            i.ticket
                .as_ref()
                .map(|t| (i.kind.as_str(), t.id.0, t.state))
        })
        .collect();
    assert!(
        states.contains(&("ticket_opened", first.0, TicketFilter::Answered)),
        "{states:?}"
    );
    assert!(
        states.contains(&("ticket_opened", cabinet.0, TicketFilter::Waiting)),
        "{states:?}"
    );
    assert_eq!(
        support
            .from_telegram(client, "Не помогло", Some(first))
            .await
            .unwrap(),
        Delivered::Added(first)
    );
    assert_eq!(
        support
            .from_telegram(client, "Ещё вопрос", None)
            .await
            .unwrap(),
        Delivered::Added(first)
    );
    let thread = support.thread(first, Actor::Admin(admin)).await.unwrap();
    assert_eq!(thread.messages.len(), 4);
    assert_eq!(thread.ticket.filter(), TicketFilter::Waiting);
    assert_eq!(support.waiting_count().await.unwrap(), 2);
    assert_eq!(support.for_client(client).await.unwrap().len(), 2);
    support.close(first, Actor::Admin(admin)).await.unwrap();
    let Delivered::Opened(second) = support
        .from_telegram(client, "Снова не работает", Some(first))
        .await
        .unwrap()
    else {
        panic!("после закрытия должно открыться новое обращение");
    };
    assert_ne!(second, first);
    assert_eq!(
        support.telegram_ticket(client, None).await.unwrap(),
        Some(second)
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn журнал_клиента_скрывает_деньги_без_права() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let client = telegram_client(services, 5_093_118_422).await;
    services
        .clients()
        .set_note(client, Some("Просил чек на почту"), Actor::Admin(admin))
        .await
        .unwrap();
    services
        .clients()
        .adjust_balance(
            client,
            Money::new(420, Currency::Usd),
            "подарок",
            admin,
            "h1",
        )
        .await
        .unwrap();
    let kinds = |page: &crate::clients::HistoryPage| -> Vec<String> {
        page.items
            .iter()
            .map(|item| match &item.record {
                crate::clients::HistoryRecord::Event { kind, .. } => kind.clone(),
                crate::clients::HistoryRecord::Audit { action, .. } => action.clone(),
                crate::clients::HistoryRecord::Payment { .. } => "payment".into(),
                crate::clients::HistoryRecord::Order { .. } => "order".into(),
            })
            .collect()
    };
    let page = sn_domain::paging::PageRequest::new(Some(50), None);
    let full = services
        .clients()
        .history(client, page, true)
        .await
        .unwrap();
    let seen = kinds(&full);
    assert!(seen.iter().any(|k| k == "client_registered"));
    assert!(seen.iter().any(|k| k == "client.note"));
    assert!(seen.iter().any(|k| k == "balance_changed"));
    assert_eq!(full.total, seen.len() as i64);
    let note = full
        .items
        .iter()
        .find_map(|item| match &item.record {
            crate::clients::HistoryRecord::Audit {
                action,
                actor_email,
                ..
            } if action == "client.note" => Some(actor_email.clone()),
            _ => None,
        })
        .expect("заметка в истории");
    assert!(note.is_some());
    let redacted = services
        .clients()
        .history(client, page, false)
        .await
        .unwrap();
    assert!(kinds(&redacted).iter().all(|k| k != "balance_changed"));
    assert_eq!(redacted.total, full.total - 1);
    assert!(matches!(
        services
            .clients()
            .history(
                ClientId(999_999),
                sn_domain::paging::PageRequest::new(Some(10), None),
                true
            )
            .await,
        Err(ServiceError::Store(sn_store::StoreError::NotFound(_)))
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn лента_собирает_события_ядра_и_remnawave() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let secret = "0123456789abcdef0123456789abcdef";
    services.set_remna_webhook_secret(Some(secret.into()));
    let body = br#"{"scope":"node","event":"node.connection_lost","timestamp":"2026-11-16T14:38:00.000Z","data":{"uuid":"00000000-0000-0000-0000-000000000002","name":"NL-2","address":"198.51.100.22","isConnected":false,"isDisabled":false,"lastStatusMessage":"connect ETIMEDOUT"}}"#;
    services
        .remna_sync()
        .accept_webhook(Some(&sn_remnawave::webhook::sign(secret, body)), body)
        .await
        .unwrap();
    let admin = owner(services).await;
    let client = telegram_client(services, 5_093_118_422).await;
    services
        .clients()
        .adjust_balance(
            client,
            Money::new(420, Currency::Usd),
            "подарок",
            admin,
            "f1",
        )
        .await
        .unwrap();
    let admin_ops = services.admin();
    let all = admin_ops
        .feed(
            &FeedQuery {
                kind: FeedKind::All,
                remna_before: None,
                core_before: None,
                limit: Some(10),
            },
            Sees::ALL,
        )
        .await
        .unwrap();
    let titles: Vec<&str> = all.items.iter().map(|i| i.title.as_str()).collect();
    assert!(titles.contains(&"Нода NL-2 не отвечает"));
    assert!(titles.contains(&"Новый клиент"));
    assert!(titles.iter().any(|t| t.starts_with("Баланс")));
    let node = all
        .items
        .iter()
        .find(|i| i.kind == "node.connection_lost")
        .unwrap();
    assert_eq!(node.data["lastStatusMessage"], "connect ETIMEDOUT");
    assert!(node.data.get("address").is_none());
    let nodes = admin_ops
        .feed(
            &FeedQuery {
                kind: FeedKind::Nodes,
                remna_before: None,
                core_before: None,
                limit: Some(10),
            },
            Sees::ALL,
        )
        .await
        .unwrap();
    assert_eq!(nodes.items.len(), 1);
    let quiet = admin_ops
        .feed(
            &FeedQuery {
                kind: FeedKind::All,
                remna_before: None,
                core_before: None,
                limit: Some(10),
            },
            Sees {
                money: false,
                refunds: false,
                infrastructure: true,
            },
        )
        .await
        .unwrap();
    assert!(quiet.items.iter().all(|i| i.kind != "balance_changed"));
    let first = admin_ops
        .feed(
            &FeedQuery {
                kind: FeedKind::All,
                remna_before: None,
                core_before: None,
                limit: Some(1),
            },
            Sees::ALL,
        )
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.has_more);
    let rest = admin_ops
        .feed(
            &FeedQuery {
                kind: FeedKind::All,
                remna_before: first.remna_before,
                core_before: first.core_before,
                limit: Some(10),
            },
            Sees::ALL,
        )
        .await
        .unwrap();
    assert_eq!(rest.items.len() + 1, all.items.len());
    assert!(!rest.has_more);
    env.db.cleanup().await;
}

async fn pending_payment(
    services: &Services,
    client_id: ClientId,
    tariff: &Tariff,
    provider: &str,
    stars: bool,
) -> sn_domain::payment::Payment {
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
    let amount: sn_domain::money::Amount = if stars {
        q.stars.map(|s| s.total).unwrap().into()
    } else {
        q.total.into()
    };
    let mut tx = services.store().begin().await.unwrap();
    let order = tx
        .orders()
        .create(&NewOrder {
            client_id,
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
            client_id,
            provider: ProviderId::from(provider),
            method: None,
            amount,
            expires_at: None,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    payment
}

#[tokio::test]
async fn ручная_отметка_оплаты_ставит_выдачу_и_запрещена_для_звёзд() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let tariff = tariff_with(
        services,
        CurrencyPrices {
            usd: Some(699),
            rub: Some(49_900),
        },
    )
    .await;
    let card = pending_payment(services, client, &tariff, providers::CRYPTOPAY, false).await;
    let stars = pending_payment(services, client, &tariff, providers::STARS, true).await;
    let ops = services.admin();
    let pending = ops
        .payments(
            &PaymentFilter {
                status: Some(PaymentStatus::Pending),
                provider: Some(providers::CRYPTOPAY.into()),
                client_id: Some(client),
                id: None,
            },
            sn_domain::paging::PageRequest::new(Some(10), None),
        )
        .await
        .unwrap();
    assert_eq!(pending.items.len(), 1);
    assert_eq!(pending.items[0].id, card.id);
    assert_eq!(
        code(
            ops.mark_paid(stars.id, &MarkPaidRequest::default(), admin)
                .await
        ),
        "payment.manual_stars"
    );
    let result = ops
        .mark_paid(
            card.id,
            &MarkPaidRequest {
                provider_txid: Some(" 9001 ".into()),
                comment: Some("пришло на кошелёк".into()),
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        MarkPaidResult::Accepted {
            order_id: card.order_id
        }
    );
    assert_eq!(
        code(
            ops.mark_paid(card.id, &MarkPaidRequest::default(), admin)
                .await
        ),
        "payment.manual_status"
    );
    let mut conn = services.store().conn().await.unwrap();
    let paid = conn.payments().get(card.id).await.unwrap();
    assert_eq!(paid.status, PaymentStatus::Paid);
    assert_eq!(paid.provider_txid.as_deref(), Some("9001"));
    let jobs = conn
        .jobs()
        .claim(10, std::time::Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(
        jobs[0].typed().unwrap(),
        JobPayload::FulfillOrder {
            order_id: card.order_id
        }
    );
    let audit = conn
        .audit()
        .for_entity("payment", card.id.0, 5)
        .await
        .unwrap();
    assert_eq!(audit[0].action, "payment.mark_paid");
    drop(conn);
    let refund = ops
        .refund(
            card.id,
            &crate::admin::RefundRequest {
                amount_minor: Some(10_000),
                method: sn_domain::payment::RefundMethod::Balance,
                subscription_action: sn_domain::payment::RefundSubscriptionAction::Keep,
                reason: None,
                key: None,
                outside: false,
            },
            admin,
        )
        .await;
    assert_eq!(code(refund), "payment.refund_amount");
    env.db.cleanup().await;
}

#[tokio::test]
async fn тариф_из_админки_проверяет_сквады_в_remnawave() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    env.remna.set_squads(vec![InternalSquad {
        uuid: Uuid::from_u128(1),
        name: "Основной".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    let category = services
        .tariffs()
        .create_category(
            &crate::tariffs::CategoryInput {
                name: "Основные".into(),
                sort: 0,
                visible: true,
                subscription_limit: None,
            },
            admin,
        )
        .await
        .unwrap();
    let mut input = TariffInput {
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
        squads: vec![SquadId(Uuid::from_u128(2))],
        external_squad: None,
        periods: vec![PeriodInput {
            days: 30,
            prices: CurrencyPrices {
                usd: Some(299),
                rub: Some(19_900),
            },
            stars: Some(Stars(150)),
            sort: 0,
            active: true,
        }],
        device_discounts: Vec::new(),
    };
    assert_eq!(
        code(services.tariffs().create(&input, admin).await),
        "tariff.invalid"
    );
    input.squads = vec![SquadId(Uuid::from_u128(1))];
    input.category_id = CategoryId(999);
    assert!(matches!(
        services.tariffs().create(&input, admin).await,
        Err(ServiceError::NotFound(_))
    ));
    input.category_id = category.id;
    let tariff = services.tariffs().create(&input, admin).await.unwrap();
    assert_eq!(tariff.periods.len(), 1);
    let hidden = services
        .tariffs()
        .set_on_sale(tariff.id, false, admin)
        .await
        .unwrap();
    assert!(!hidden.on_sale);
    assert!(services.checkout().catalog().await.unwrap().is_empty());
    let squads = services.infra().squads().await.unwrap();
    assert_eq!(squads[0].tariffs[0].name, "Стандарт");
    let updated = services
        .tariffs()
        .update(tariff.id, &input, admin)
        .await
        .unwrap();
    assert_eq!(updated.id, tariff.id);
    env.db.cleanup().await;
}

#[tokio::test]
async fn описание_тарифа_с_оформлением_сохраняется_и_уходит_в_каталог() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    env.remna.set_squads(vec![InternalSquad {
        uuid: Uuid::from_u128(1),
        name: "Основной".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    let category = services
        .tariffs()
        .create_category(
            &crate::tariffs::CategoryInput {
                name: "Unblock".into(),
                sort: 0,
                visible: true,
                subscription_limit: None,
            },
            admin,
        )
        .await
        .unwrap();
    let mut input = TariffInput {
        traffic_addon: None,
        category_id: category.id,
        name: "Unblock".into(),
        description: Some(
            "🎉 <B>Чёрная пятница!</B>\r\n\r\n💰 <b>Цена:</b> <s>от 419 ₽</s> & <i>(доп. устройства)</i>\n"
                .into(),
        ),
        on_sale: true,
        sort: 0,
        devices: DevicePolicy {
            included: 4,
            extra: None,
        },
        traffic: TrafficPolicy::UNLIMITED,
        squads: vec![SquadId(Uuid::from_u128(1))],
        external_squad: None,
        periods: vec![PeriodInput {
            days: 30,
            prices: CurrencyPrices {
                usd: Some(299),
                rub: Some(39_900),
            },
            stars: None,
            sort: 0,
            active: true,
        }],
        device_discounts: Vec::new(),
    };
    let tariff = services.tariffs().create(&input, admin).await.unwrap();
    let stored = "🎉 <b>Чёрная пятница!</b>\n\n💰 <b>Цена:</b> <s>от 419 ₽</s> &amp; <i>(доп. устройства)</i>";
    assert_eq!(tariff.description.as_deref(), Some(stored));
    let catalog = services.checkout().catalog().await.unwrap();
    assert_eq!(catalog[0].tariff.description.as_deref(), Some(stored));
    input.description = Some("<b>Цена: от 419 ₽".into());
    assert_eq!(
        code(services.tariffs().update(tariff.id, &input, admin).await),
        "tariff.invalid"
    );
    input.description = Some("я".repeat(2_001));
    assert_eq!(
        code(services.tariffs().update(tariff.id, &input, admin).await),
        "tariff.invalid"
    );
    input.description = Some("Для мобильного интернета".into());
    let updated = services
        .tariffs()
        .update(tariff.id, &input, admin)
        .await
        .unwrap();
    assert_eq!(
        updated.description.as_deref(),
        Some("Для мобильного интернета")
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn сохранение_настроек_оповещает_другие_процессы() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let mut listener = sqlx::postgres::PgListener::connect_with(services.store().pool())
        .await
        .unwrap();
    listener
        .listen(crate::settings::SETTINGS_CHANNEL)
        .await
        .unwrap();
    let admin = owner(services).await;
    services
        .settings_ops()
        .update_autorenew(
            &sn_domain::settings::AutoRenewSettings {
                charge_days_before: 2,
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(services.settings().autorenew.charge_days_before, 2);
    let own = services.instance_id().to_string();
    let received = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let notification = listener.recv().await.unwrap();
            if notification.payload() == own {
                return notification;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(received.channel(), crate::settings::SETTINGS_CHANNEL);
    assert_eq!(
        code(
            services
                .settings_ops()
                .update_autorenew(
                    &sn_domain::settings::AutoRenewSettings {
                        charge_days_before: 30,
                        ..Default::default()
                    },
                    admin,
                )
                .await
        ),
        "settings.invalid_field"
    );
    drop(listener);
    env.db.cleanup().await;
}
