use crate::Services;
use crate::checkout::testkit::{Kit, client, kit_with};
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::ClientId;
use sn_domain::module::ModuleId;
use sn_domain::paging::PageRequest;
use sn_modules::ModuleConfig;
use sn_modules::catalog::antibot::{self, AntibotModule, JournalFilter};
use sn_store::Db;
use sn_store::testing::{DAY, wait_for_window_room};
use std::sync::Arc;

async fn antibot_kit() -> Option<Kit> {
    let kit = kit_with(vec![Arc::new(AntibotModule)]).await?;
    for id in 1..=25 {
        client(&kit.services, 1_000 + id).await;
    }
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query(
        "UPDATE client_identities SET created_at = now() - interval '90 days' WHERE kind = 'telegram'",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    Some(kit)
}

async fn fresh(services: &Services, id: i64, username: Option<&str>) -> ClientId {
    services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id,
                username: username.map(str::to_string),
                first_name: Some("Новый".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0
        .id
}

async fn mode(services: &Services, mode: &str) {
    services
        .modules()
        .set_config(
            services.store(),
            ModuleId::Antibot,
            ModuleConfig::from_value(&serde_json::json!({ "mode": mode })),
            None,
        )
        .await
        .unwrap();
}

async fn actions(services: &Services, client_id: ClientId) -> Vec<&'static str> {
    let mut conn = services.store().conn().await.unwrap();
    let mut page = antibot::store::journal(
        &mut conn,
        JournalFilter::All,
        Some(client_id),
        PageRequest::default(),
    )
    .await
    .unwrap();
    page.items.reverse();
    page.items.into_iter().map(|i| i.action.as_str()).collect()
}

async fn age_challenge(services: &Services, client_id: ClientId) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "UPDATE antibot_marks SET challenge_at = now() - interval '5 seconds' WHERE client_id = $1",
    )
    .bind(client_id.0)
    .execute(conn.conn())
    .await
    .unwrap();
}

#[tokio::test]
async fn новый_аккаунт_без_username_жмёт_кнопку_и_получает_пробник() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let checkout = services.checkout();
    let calm = fresh(services, 1_010_000_000, Some("real_user")).await;
    checkout.start_trial(calm, None).await.unwrap();
    assert!(actions(services, calm).await.is_empty());
    let suspect = fresh(services, 8_100_000_000, None).await;
    let err = checkout.start_trial(suspect, None).await.unwrap_err();
    assert_eq!(err.code(), antibot::CONFIRM_CODE);
    let token = checkout.trial_challenge(suspect).await.unwrap();
    assert_eq!(
        checkout
            .confirm_trial(suspect, &token, None)
            .await
            .unwrap_err()
            .code(),
        "antibot.too_fast"
    );
    let token = checkout.trial_challenge(suspect).await.unwrap();
    age_challenge(services, suspect).await;
    assert_eq!(
        checkout
            .confirm_trial(suspect, "чужой", None)
            .await
            .unwrap_err()
            .code(),
        "antibot.expired"
    );
    checkout.confirm_trial(suspect, &token, None).await.unwrap();
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        "trial.used"
    );
    assert_eq!(
        actions(services, suspect).await,
        ["marked", "asked", "confirmed"]
    );
    let mut conn = services.store().conn().await.unwrap();
    let mark = antibot::store::mark_of(&mut conn, suspect)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mark.reasons,
        [
            antibot::signals::Signal::YoungAccount,
            antibot::signals::Signal::NoUsername
        ]
    );
    assert!(mark.confirmed_at.is_some());
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn в_режиме_запрета_пробника_нет_пока_админ_не_снимет_отметку() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    wait_for_window_room(kit.services.store(), DAY).await;
    let services = &kit.services;
    mode(services, "block").await;
    let checkout = services.checkout();
    let suspect = fresh(services, 8_200_000_000, None).await;
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        antibot::DENIED_CODE
    );
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        antibot::DENIED_CODE
    );
    assert_eq!(actions(services, suspect).await, ["marked", "trial_denied"]);
    let ctx = services.module_context();
    let admin = crate::scenario_tests::owner(services).await;
    assert!(antibot::clear(&ctx, suspect, Some(admin)).await.unwrap());
    assert!(!antibot::clear(&ctx, suspect, Some(admin)).await.unwrap());
    checkout.start_trial(suspect, None).await.unwrap();
    assert_eq!(
        actions(services, suspect).await,
        ["marked", "trial_denied", "cleared"]
    );
    mode(services, "mark").await;
    let quiet = fresh(services, 8_300_000_000, None).await;
    checkout.start_trial(quiet, None).await.unwrap();
    assert_eq!(actions(services, quiet).await, ["marked"]);
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Antibot, false, None)
        .await
        .unwrap();
    let off = fresh(services, 8_400_000_000, None).await;
    checkout.start_trial(off, None).await.unwrap();
    assert!(actions(services, off).await.is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn почта_из_одноразового_домена_и_вариант_чужого_адреса_отмечаются() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let ctx = services.module_context();
    let mut conn = services.store().conn().await.unwrap();
    let mut clients = Vec::new();
    for email in [
        "ivan.petrov@gmail.com",
        "ivanpetrov+vpn@googlemail.com",
        "bot@mailinator.com",
        "anna@example.org",
    ] {
        let id = conn
            .clients()
            .create(&sn_store::NewClient {
                locale: sn_domain::client::Locale::Ru,
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await
            .unwrap()
            .id;
        conn.clients()
            .add_identity(
                id,
                &sn_domain::client::NewIdentity::new(
                    sn_domain::client::IdentityKind::Email,
                    email,
                    true,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        clients.push(id);
    }
    let settings = antibot::AntibotSettings::default();
    let now = sn_modules::catalog::db_now(&mut conn).await.unwrap();
    let first = antibot::assess(&mut conn, clients[0], &settings, now)
        .await
        .unwrap();
    assert!(
        first
            .signals
            .contains(&antibot::signals::Signal::EmailAlias)
    );
    let alias = antibot::assess(&mut conn, clients[1], &settings, now)
        .await
        .unwrap();
    assert!(alias.suspicious());
    let disposable = antibot::assess(&mut conn, clients[2], &settings, now)
        .await
        .unwrap();
    assert_eq!(
        disposable.signals,
        [antibot::signals::Signal::DisposableEmail]
    );
    let clean = antibot::assess(&mut conn, clients[3], &settings, now)
        .await
        .unwrap();
    assert!(clean.signals.is_empty());
    drop(conn);
    assert_eq!(
        antibot::trial_gate(&ctx, clients[2]).await.unwrap(),
        antibot::Gate::Confirm
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn регистрации_в_кабинете_с_одного_адреса_сверх_лимита_отмечаются() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let mut conn = services.store().conn().await.unwrap();
    let mut clients = Vec::new();
    for n in 0..5 {
        let id = conn
            .clients()
            .create(&sn_store::NewClient {
                locale: sn_domain::client::Locale::Ru,
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await
            .unwrap()
            .id;
        let ip = if n == 4 {
            "198.51.100.20"
        } else {
            "203.0.113.7"
        };
        sqlx::query(
            "INSERT INTO client_sessions (client_id, token_hash, channel, ip, expires_at) \
             VALUES ($1, $2, 'cabinet', $3, now() + interval '1 day')",
        )
        .bind(id.0)
        .bind(format!("token-{}", id.0).into_bytes())
        .bind(ip)
        .execute(conn.conn())
        .await
        .unwrap();
        clients.push(id);
    }
    let settings = antibot::AntibotSettings::default();
    let now = sn_modules::catalog::db_now(&mut conn).await.unwrap();
    for (n, id) in clients.iter().enumerate() {
        let found = antibot::assess(&mut conn, *id, &settings, now)
            .await
            .unwrap();
        let limited = found.signals.contains(&antibot::signals::Signal::IpLimit);
        assert_eq!(limited, n == 3, "клиент {n}: {:?}", found.signals);
    }
    drop(conn);
    kit.db.cleanup().await;
}
