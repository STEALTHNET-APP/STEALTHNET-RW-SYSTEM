use super::model::Source;
use super::runs::{self, Outcome, Preview, RunState};
use super::*;
use crate::{EnvFallbacks, FixedClock, ServiceDeps, Services};
use chrono::{TimeZone, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::AdminId;
use sn_domain::money::Currency;
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_remnawave::fake::{FakeRemnawave, sample_user};
use sn_remnawave::{InternalSquad, SquadInfo};
use sn_store::SecretBox;
use sn_store::testing::{TestDb, test_store};
use std::io::Write;
use std::sync::Arc;

pub(super) const S1: &str = "11111111-1111-4111-8111-111111111111";
pub(super) const S2: &str = "22222222-2222-4222-8222-222222222222";

pub(super) struct Env {
    _db: TestDb,
    pub(super) services: Services,
    pub(super) remna: Arc<FakeRemnawave>,
}

pub(super) async fn env() -> Option<Env> {
    env_with(ModuleRegistry::catalog_only()).await
}

pub(super) async fn env_with(modules: ModuleRegistry) -> Option<Env> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 11, 17, 10, 0, 0).unwrap(),
    ));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(modules),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock,
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    remna.set_squads(vec![squad(S1, "NL")]);
    services.set_remna(Some(remna.clone()));
    Some(Env {
        _db: db,
        services,
        remna,
    })
}

pub(super) fn squad(uuid: &str, name: &str) -> InternalSquad {
    InternalSquad {
        uuid: uuid.parse().unwrap(),
        name: name.into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }
}

pub(super) fn user(
    id: i64,
    short: &str,
    name: &str,
    tg: Option<i64>,
    email: Option<&str>,
) -> sn_remnawave::RemnaUser {
    let mut user = sample_user(id, name);
    user.short_uuid = short.into();
    user.telegram_id = tg;
    user.email = email.map(Into::into);
    user.traffic_limit_bytes = 50 * 1024 * 1024 * 1024;
    user.hwid_device_limit = Some(3);
    user
}

fn copy(table: &str, columns: &[&str], rows: &[&[Option<&str>]]) -> String {
    let mut out = format!("COPY public.{table} ({}) FROM stdin;\n", columns.join(", "));
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .map(|cell| match cell {
                None => "\\N".to_string(),
                Some(v) => v
                    .replace('\\', "\\\\")
                    .replace('\t', "\\t")
                    .replace('\n', "\\n"),
            })
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    out.push_str("\\.\n\n");
    out
}

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap()
}

fn stealthnet_dump(fresh: bool) -> Vec<u8> {
    let mut sql = String::from(
        "--\n-- PostgreSQL database dump\n--\n-- StealthNet v7.0.1 Database Dump\n-- Created at: 2026-11-17T09:12:44.000Z\n\nSET statement_timeout = 0;\n\n",
    );
    sql += &copy(
        "system_settings",
        &["id", "key", "value"],
        &[&[Some("ss1"), Some("default_currency"), Some("rub")]],
    );
    sql += &copy(
        "tariff_categories",
        &[
            "id",
            "name",
            "emoji_key",
            "sort_order",
            "single_subscription_mode",
        ],
        &[
            &[Some("cat1"), Some("Обычные"), None, Some("0"), Some("f")],
            &[Some("cat2"), Some("Широкие"), None, Some("1"), Some("f")],
        ],
    );
    let squads1 = format!("{{{S1}}}");
    let squads2 = format!("{{{S1},{S2}}}");
    sql += &copy(
        "tariffs",
        &[
            "id",
            "category_id",
            "name",
            "description",
            "duration_days",
            "internal_squad_uuids",
            "traffic_limit_bytes",
            "traffic_reset_mode",
            "device_limit",
            "included_devices",
            "price_per_extra_device",
            "max_extra_devices",
            "price",
            "currency",
            "sort_order",
        ],
        &[
            &[
                Some("t1"),
                Some("cat1"),
                Some("Базовый"),
                Some("Для одного"),
                Some("30"),
                Some(&squads1),
                Some("107374182400"),
                Some("monthly"),
                None,
                Some("1"),
                Some("50"),
                Some("2"),
                Some("199"),
                Some("rub"),
                Some("0"),
            ],
            &[
                Some("t2"),
                Some("cat2"),
                Some("Широкий"),
                None,
                Some("30"),
                Some(&squads2),
                None,
                Some("carry_over"),
                None,
                Some("3"),
                Some("0"),
                Some("0"),
                Some("4.99"),
                Some("usd"),
                Some("1"),
            ],
        ],
    );
    sql += &copy(
        "tariff_price_options",
        &["id", "tariff_id", "duration_days", "price", "sort_order"],
        &[
            &[Some("o1"), Some("t1"), Some("30"), Some("199"), Some("0")],
            &[Some("o2"), Some("t1"), Some("90"), Some("499"), Some("1")],
            &[Some("o3"), Some("t2"), Some("30"), Some("4.99"), Some("0")],
        ],
    );
    let anna_balance = if fresh { "170.5" } else { "150.5" };
    let boris_balance = if fresh { "5" } else { "20" };
    let mut clients: Vec<[Option<&str>; 16]> = vec![
        [
            Some("c1"),
            Some("anna@example.com"),
            Some("$2b$12$abcdefghijklmnopqrstuuOdGq1xQmB0PZ5d4fN4m0wE1gVyZcH3W"),
            Some("CLIENT"),
            Some("REF-1A2B3C4D"),
            None,
            Some(anna_balance),
            Some("ru"),
            Some("1001"),
            Some("anna"),
            Some("f"),
            None,
            Some("f"),
            Some("t"),
            Some("tiktok"),
            Some("2026-01-10 08:00:00"),
        ],
        [
            Some("c2"),
            None,
            None,
            Some("CLIENT"),
            None,
            Some("c1"),
            Some("0"),
            Some("en"),
            Some("1002"),
            None,
            Some("t"),
            Some("спам"),
            Some("f"),
            Some("f"),
            None,
            Some("2026-02-11 09:00:00"),
        ],
        [
            Some("c3"),
            Some("Boris@Example.com"),
            None,
            Some("CLIENT"),
            None,
            None,
            Some(boris_balance),
            Some("ru"),
            None,
            None,
            Some("f"),
            None,
            Some("t"),
            Some("f"),
            None,
            Some("2026-03-12 10:00:00"),
        ],
        [
            Some("c4"),
            None,
            None,
            Some("CLIENT"),
            None,
            None,
            Some("300"),
            Some("ru"),
            Some("1004"),
            Some("vera"),
            Some("f"),
            None,
            Some("f"),
            Some("f"),
            None,
            Some("2026-04-13 11:00:00"),
        ],
    ];
    if fresh {
        clients.push([
            Some("c6"),
            None,
            None,
            Some("CLIENT"),
            None,
            Some("c1"),
            Some("10"),
            Some("ru"),
            Some("1006"),
            Some("gleb"),
            Some("f"),
            None,
            Some("f"),
            Some("f"),
            None,
            Some("2026-11-17 08:00:00"),
        ]);
    }
    let client_rows: Vec<&[Option<&str>]> = clients.iter().map(|c| c.as_slice()).collect();
    sql += &copy(
        "clients",
        &[
            "id",
            "email",
            "password_hash",
            "role",
            "referral_code",
            "referrer_id",
            "balance",
            "preferred_lang",
            "telegram_id",
            "telegram_username",
            "is_blocked",
            "block_reason",
            "telegram_unreachable",
            "trial_used",
            "utm_source",
            "created_at",
        ],
        &client_rows,
    );
    sql += &copy(
        "subscriptions",
        &[
            "id",
            "owner_id",
            "remnawave_uuid",
            "subscription_index",
            "tariff_id",
            "gift_status",
            "trial_id",
            "auto_renew_enabled",
            "auto_renew_price_option_id",
            "extra_devices",
            "expire_at",
            "short_uuid",
            "promo_granted",
            "purchased_as_gift",
            "custom_price",
            "created_at",
        ],
        &[
            &[
                Some("s1"),
                Some("c1"),
                Some("501"),
                Some("0"),
                Some("t1"),
                None,
                None,
                Some("t"),
                Some("o2"),
                Some("1"),
                Some("2026-12-20 00:00:00"),
                None,
                Some("f"),
                Some("f"),
                None,
                Some("2026-01-10 08:05:00"),
            ],
            &[
                Some("s2"),
                Some("c1"),
                Some("8f2c1d9e-1111-4a4a-9b9b-123456789abc"),
                Some("1"),
                Some("t1"),
                None,
                None,
                Some("f"),
                None,
                Some("0"),
                Some("2026-12-21 00:00:00"),
                Some("shortc1b"),
                Some("f"),
                Some("f"),
                None,
                Some("2026-05-10 08:05:00"),
            ],
            &[
                Some("s3"),
                Some("c3"),
                Some("0a0a0a0a-2222-4b4b-8c8c-abcdefabcdef"),
                Some("0"),
                Some("t1"),
                None,
                None,
                Some("f"),
                None,
                Some("0"),
                Some("2026-12-22 00:00:00"),
                None,
                Some("f"),
                Some("f"),
                None,
                Some("2026-03-12 10:05:00"),
            ],
            &[
                Some("s4"),
                Some("c2"),
                Some("999"),
                Some("0"),
                Some("t1"),
                None,
                None,
                Some("f"),
                None,
                Some("0"),
                Some("2026-02-01 00:00:00"),
                None,
                Some("f"),
                Some("f"),
                None,
                Some("2026-02-11 09:05:00"),
            ],
            &[
                Some("s5"),
                Some("c4"),
                None,
                Some("0"),
                Some("t1"),
                Some("GIFT_RESERVED"),
                None,
                Some("f"),
                None,
                Some("0"),
                None,
                None,
                Some("f"),
                Some("t"),
                None,
                Some("2026-04-13 11:05:00"),
            ],
        ],
    );
    let mut payments: Vec<[Option<&str>; 13]> = vec![
        [
            Some("p1"),
            Some("c1"),
            Some("ord-1"),
            Some("199"),
            Some("RUB"),
            Some("PAID"),
            Some("yookassa"),
            Some("yk-1"),
            Some("t1"),
            Some("o1"),
            Some("s1"),
            None,
            Some("2026-10-01 10:00:00"),
        ],
        [
            Some("p2"),
            Some("c1"),
            Some("ord-2"),
            Some("100"),
            Some("rub"),
            Some("PAID"),
            Some("cryptopay"),
            Some("cp-2"),
            None,
            None,
            None,
            None,
            Some("2026-10-02 10:00:00"),
        ],
        [
            Some("p3"),
            Some("c1"),
            Some("ord-3"),
            Some("199"),
            Some("rub"),
            Some("PENDING"),
            Some("yookassa"),
            None,
            Some("t1"),
            Some("o1"),
            None,
            None,
            None,
        ],
        [
            Some("p4"),
            Some("c4"),
            Some("ord-4"),
            Some("5"),
            Some("usd"),
            Some("PAID"),
            Some("cryptopay"),
            None,
            None,
            None,
            None,
            None,
            Some("2026-10-03 10:00:00"),
        ],
        [
            Some("p5"),
            Some("c4"),
            Some("ord-5"),
            Some("300"),
            Some("rub"),
            Some("PAID"),
            Some("yookassa"),
            None,
            None,
            None,
            None,
            None,
            Some("2026-10-04 10:00:00"),
        ],
        [
            Some("p6"),
            Some("c2"),
            Some("ord-6"),
            Some("199"),
            Some("rub"),
            Some("REFUNDED"),
            Some("yookassa"),
            None,
            Some("t1"),
            Some("o1"),
            Some("s4"),
            None,
            Some("2026-02-11 09:10:00"),
        ],
        [
            Some("p7"),
            Some("c3"),
            Some("ord-7"),
            Some("199"),
            Some("RUB"),
            Some("PAID"),
            Some("telegram_stars"),
            None,
            Some("t1"),
            Some("o1"),
            Some("s3"),
            Some("{\"stars\":{\"total\":150}}"),
            Some("2026-03-12 10:10:00"),
        ],
        [
            Some("p8"),
            Some("c1"),
            Some("ord-8"),
            Some("0"),
            Some("rub"),
            Some("PAID"),
            Some("admin_grant"),
            None,
            Some("t1"),
            None,
            Some("s2"),
            None,
            Some("2026-05-10 08:05:00"),
        ],
    ];
    if fresh {
        payments.push([
            Some("p9"),
            Some("c6"),
            Some("ord-9"),
            Some("10"),
            Some("rub"),
            Some("PAID"),
            Some("yookassa"),
            None,
            None,
            None,
            None,
            None,
            Some("2026-11-17 08:30:00"),
        ]);
    }
    let payment_rows: Vec<&[Option<&str>]> = payments.iter().map(|p| p.as_slice()).collect();
    sql += &copy(
        "payments",
        &[
            "id",
            "client_id",
            "order_id",
            "amount",
            "currency",
            "status",
            "provider",
            "external_id",
            "tariff_id",
            "tariff_price_option_id",
            "subscription_id",
            "metadata",
            "paid_at",
        ],
        &payment_rows,
    );
    sql += &copy(
        "promo_codes",
        &[
            "id",
            "code",
            "name",
            "type",
            "discount_percent",
            "discount_fixed",
            "tariff_id",
            "duration_days",
            "max_uses",
            "max_uses_per_client",
            "is_active",
            "expires_at",
        ],
        &[
            &[
                Some("pr1"),
                Some("welcome10"),
                Some("Скидка новичкам"),
                Some("DISCOUNT"),
                Some("10"),
                None,
                None,
                None,
                Some("100"),
                Some("1"),
                Some("t"),
                None,
            ],
            &[
                Some("pr2"),
                Some("BAD CODE!"),
                Some("Сломанный"),
                Some("DISCOUNT"),
                Some("5"),
                None,
                None,
                None,
                Some("0"),
                Some("1"),
                Some("t"),
                None,
            ],
            &[
                Some("pr3"),
                Some("DAYS7"),
                Some("Неделя"),
                Some("FREE_DAYS"),
                None,
                None,
                Some("t1"),
                Some("7"),
                Some("0"),
                Some("1"),
                Some("t"),
                Some("2027-01-01 00:00:00"),
            ],
        ],
    );
    sql += &copy(
        "promo_code_usages",
        &["id", "promo_code_id", "client_id", "created_at"],
        &[&[
            Some("u1"),
            Some("pr1"),
            Some("c2"),
            Some("2026-02-11 09:09:00"),
        ]],
    );
    sql += &copy(
        "tickets",
        &[
            "id",
            "client_id",
            "subject",
            "status",
            "created_at",
            "closed_at",
        ],
        &[&[
            Some("tk1"),
            Some("c1"),
            Some("Не работает"),
            Some("open"),
            Some("2026-11-01 10:00:00"),
            None,
        ]],
    );
    sql += &copy(
        "ticket_messages",
        &[
            "id",
            "ticket_id",
            "author_type",
            "content",
            "attachments",
            "is_read",
            "created_at",
        ],
        &[
            &[
                Some("m1"),
                Some("tk1"),
                Some("client"),
                Some("Привет\nне работает"),
                None,
                Some("t"),
                Some("2026-11-01 10:00:00"),
            ],
            &[
                Some("m2"),
                Some("tk1"),
                Some("support"),
                Some("Проверьте приложение"),
                None,
                Some("t"),
                Some("2026-11-01 11:00:00"),
            ],
            &[
                Some("m3"),
                Some("tk1"),
                Some("client"),
                Some("Всё ещё"),
                Some("[{\"url\":\"/uploads/x.png\"}]"),
                Some("f"),
                Some("2026-11-01 12:00:00"),
            ],
        ],
    );
    sql += &copy(
        "withdrawal_requests",
        &["id", "client_id", "amount", "status", "wallet_trc20"],
        &[&[
            Some("w1"),
            Some("c1"),
            Some("50"),
            Some("PENDING"),
            Some("TXwallet"),
        ]],
    );
    sql += &copy(
        "proxy_slots",
        &["id", "client_id"],
        &[&[Some("x1"), Some("c1")], &[Some("x2"), Some("c4")]],
    );
    gz(sql.as_bytes())
}

fn seed_stealthnet_remna(remna: &FakeRemnawave) {
    remna.insert_user(user(501, "aaaa501", "anna", Some(1001), None));
    remna.insert_user(user(502, "shortc1b", "anna_2", Some(1001), None));
    remna.insert_user(user(
        503,
        "bbbb503",
        "boris",
        None,
        Some("boris@example.com"),
    ));
    remna.insert_user(user(504, "cccc504", "stranger", Some(9999), None));
}

pub(super) async fn scalar(services: &Services, sql: &str) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(&format!("SELECT ({sql})::bigint"))
        .fetch_one(sn_store::Db::conn(&mut conn))
        .await
        .unwrap()
}

pub(super) async fn import(env: &Env, bytes: &[u8], admin: Option<AdminId>) -> (Preview, Outcome) {
    import_with(env, bytes, admin, runs::StartOptions::default()).await
}

pub(super) async fn import_with(
    env: &Env,
    bytes: &[u8],
    admin: Option<AdminId>,
    options: runs::StartOptions,
) -> (Preview, Outcome) {
    let dump = archive::open_bytes(bytes, &wanted).unwrap();
    let legacy = map_dump(&dump).unwrap();
    let remna = check::fetch_remna(&env.services).await;
    let checked = check::preview(&env.services, &legacy, remna).await.unwrap();
    let mut conn = env.services.store().conn().await.unwrap();
    let (run_id, key): (i64, uuid::Uuid) = sqlx::query_as(
        "INSERT INTO import_runs (source, state, file_name, file_size, started_at, options) \
         VALUES ($1, 'running', 'test', 1, now(), $2) RETURNING id, public_id",
    )
    .bind(legacy.source.as_str())
    .bind(serde_json::to_value(options).unwrap())
    .fetch_one(sn_store::Db::conn(&mut conn))
    .await
    .unwrap();
    runs::add_issues(&mut conn, run_id, &checked.issues)
        .await
        .unwrap();
    drop(conn);
    let outcome = apply_legacy(&env.services, &legacy, run_id, key, admin, None)
        .await
        .unwrap();
    env.services.set_remna(Some(env.remna.clone()));
    let mut conn = env.services.store().conn().await.unwrap();
    runs::finish(&mut conn, run_id, &outcome).await.unwrap();
    (checked.preview, outcome)
}

pub(super) async fn owner(services: &Services) -> AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

#[tokio::test]
async fn перенос_не_сливает_клиента_с_аккаунтом_где_его_почта_привязана_без_кода() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let admin = owner(&env.services).await;
    let s = &env.services;
    use sn_store::Db;
    let stranger = {
        let mut conn = s.store().conn().await.unwrap();
        let client = conn
            .clients()
            .create(&sn_store::NewClient {
                locale: sn_domain::client::Locale::Ru,
                registration_source: sn_domain::client::RegistrationSource::Bot,
                note: None,
            })
            .await
            .unwrap();
        for identity in [
            sn_domain::client::NewIdentity::new(
                sn_domain::client::IdentityKind::Telegram,
                "909090",
                true,
            )
            .unwrap(),
            sn_domain::client::NewIdentity::email_without_code("boris@example.com").unwrap(),
        ] {
            conn.clients()
                .add_identity(client.id, &identity)
                .await
                .unwrap();
        }
        client.id.0
    };
    let (_, outcome) = import(&env, &stealthnet_dump(false), Some(admin)).await;
    assert_eq!(outcome.created, 4);
    assert_eq!(
        scalar(
            s,
            &format!(
                "SELECT count(*) FROM legacy_ids WHERE entity = 'client' AND new_id = {stranger}"
            )
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            s,
            &format!("SELECT count(*) FROM subscriptions WHERE client_id = {stranger}")
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            s,
            &format!("SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances WHERE client_id = {stranger}")
        )
        .await,
        0
    );
}

#[tokio::test]
async fn stealthnet_переносится_целиком_без_изменений_в_remnawave() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let admin = owner(&env.services).await;
    let (preview, outcome) = import(&env, &stealthnet_dump(false), Some(admin)).await;
    assert_eq!(preview.detected, Source::Stealthnet);
    assert_eq!(preview.counts.clients, 4);
    assert_eq!(preview.counts.subscriptions, 4);
    assert_eq!(preview.counts.tariffs, 2);
    assert_eq!(preview.counts.payments, 6);
    assert_eq!(preview.counts.promo_codes, 2);
    assert_eq!(preview.counts.referrals, 1);
    assert_eq!(preview.counts.tickets, 1);
    assert_eq!(preview.remna.found, 3);
    assert_eq!(preview.remna.missing, 1);
    assert_eq!(preview.balances.total_minor, 15_050 + 2_000);
    assert!(preview.currency.switch, "{:?}", preview.currency);
    assert_eq!(preview.currency.source, Currency::Rub);
    let codes: Vec<&str> = preview.skipped.iter().map(|s| s.code.as_str()).collect();
    for code in [
        "balance_mixed",
        "payments_unpaid",
        "payments_free",
        "gift_unclaimed",
        "subscription_missing",
        "products_absent",
        "promo_invalid",
    ] {
        assert!(codes.contains(&code), "нет {code} в {codes:?}");
    }

    assert!(outcome.currency_switched);
    assert_eq!(env.services.settings().currency, Currency::Rub);
    assert_eq!(outcome.counts.clients, 4);
    assert_eq!(outcome.created, 4);
    assert_eq!(outcome.counts.subscriptions, 3);
    assert_eq!(outcome.counts.payments, 6);
    assert_eq!(outcome.counts.promo_codes, 2);
    assert_eq!(outcome.counts.referrals, 1);
    assert_eq!(outcome.counts.tickets, 1);
    assert_eq!(outcome.balance_total_minor, 17_050);
    assert_eq!(outcome.counts.withdrawals, 1);
    let calls = env.remna.calls();
    assert!(
        calls
            .iter()
            .all(|c| matches!(c.as_str(), "stream_users" | "internal_squads")),
        "в Remnawave ничего не меняем: {calls:?}"
    );
    let s = &env.services;
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM clients WHERE registration_source = 'migration'"
        )
        .await,
        4
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE status = 'blocked'").await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM client_identities WHERE kind = 'email' AND value = 'boris@example.com'").await, 1);
    assert_eq!(scalar(s, "SELECT count(*) FROM client_passwords").await, 1);
    assert_eq!(
        scalar(s, "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances WHERE currency = 'RUB'").await,
        17_050
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions WHERE remna_user_id IN (501, 502, 503) AND tariff_id IS NOT NULL AND expire_at IS NOT NULL").await,
        3
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id = 504"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(s, "SELECT p.days FROM subscriptions s JOIN tariff_periods p ON p.id = s.autorenew_period_id WHERE s.remna_user_id = 501 AND s.autorenew_enabled").await,
        90
    );
    assert_eq!(
        scalar(
            s,
            "SELECT extra_devices FROM subscriptions WHERE remna_user_id = 501"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tariffs WHERE on_sale").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tariffs WHERE NOT on_sale").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM payments WHERE provider_payload ? 'imported'"
        )
        .await,
        6
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM payments WHERE currency = 'XTR' AND amount_minor = 150"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM payments WHERE status = 'refunded'").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM orders WHERE status IN ('pending', 'paid', 'review')"
        )
        .await,
        0
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM jobs").await, 0);
    assert_eq!(scalar(s, "SELECT count(*) FROM domain_events").await, 0);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM promo_codes WHERE code IN ('WELCOME10', 'DAYS7')"
        )
        .await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM promo_redemptions WHERE status = 'used'"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 1);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM referral_codes WHERE code = 'REF1A2B3C4D'"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM ticket_messages").await, 3);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tickets t JOIN ticket_messages m ON m.id = t.waiting_message_id WHERE t.status = 'open' AND t.last_author = 'client' AND m.body LIKE 'Всё ещё%'").await,
        1
    );
    assert!(scalar(s, "SELECT count(*) FROM trial_claims").await >= 3);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM client_utm WHERE source = 'tiktok'").await,
        1
    );

    let runs_id: uuid::Uuid = {
        let mut conn = s.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT public_id FROM import_runs ORDER BY id DESC LIMIT 1")
            .fetch_one(sn_store::Db::conn(&mut conn))
            .await
            .unwrap()
    };
    let (name, bytes) = s.legacy().report(runs_id).await.unwrap();
    assert!(name.ends_with(".csv"));
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("не найден"), "{text}");
    assert!(text.contains("и в рублях, и в долларах"), "{text}");
    assert_eq!(
        scalar(s, "SELECT count(*) FROM referral_withdrawals WHERE status = 'pending' AND details = 'TXwallet'").await,
        1
    );
}

#[tokio::test]
async fn повторный_перенос_догружает_свежее_без_дублей() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let admin = owner(&env.services).await;
    import(&env, &stealthnet_dump(false), Some(admin)).await;
    let (preview, outcome) = import(&env, &stealthnet_dump(true), Some(admin)).await;
    assert_eq!(preview.existing.again, 4);
    assert_eq!(outcome.created, 1);
    assert_eq!(outcome.updated, 4);
    let s = &env.services;
    assert_eq!(scalar(s, "SELECT count(*) FROM clients").await, 5);
    assert_eq!(scalar(s, "SELECT count(*) FROM subscriptions").await, 3);
    assert_eq!(scalar(s, "SELECT count(*) FROM payments").await, 7);
    assert_eq!(scalar(s, "SELECT count(*) FROM tariffs").await, 2);
    assert_eq!(scalar(s, "SELECT count(*) FROM promo_codes").await, 2);
    assert_eq!(scalar(s, "SELECT count(*) FROM ticket_messages").await, 3);
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 2);
    assert_eq!(
        scalar(s, "SELECT b.amount_minor FROM client_balances b JOIN client_identities i ON i.client_id = b.client_id WHERE i.kind = 'telegram' AND i.value = '1001'").await,
        17_050
    );
    assert_eq!(
        scalar(s, "SELECT b.amount_minor FROM client_balances b JOIN client_identities i ON i.client_id = b.client_id WHERE i.kind = 'email' AND i.value = 'boris@example.com'").await,
        500
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM balance_entries WHERE kind = 'admin_adjust' AND delta_minor = -1500").await,
        1
    );
    let (_, again) = import(&env, &stealthnet_dump(true), Some(admin)).await;
    assert_eq!(again.created, 0);
    assert_eq!(again.counts.payments, 0);
    assert_eq!(scalar(s, "SELECT count(*) FROM balance_entries").await, 5);
}

#[tokio::test]
async fn клиенты_этой_установки_объединяются_по_telegram_и_почте() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let existing = env
        .services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 1001,
                username: Some("anna_new".into()),
                first_name: Some("Анна".into()),
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
        .id;
    let (preview, outcome) = import(
        &env,
        &stealthnet_dump(false),
        Some(owner(&env.services).await),
    )
    .await;
    assert_eq!(preview.existing.matched, 1);
    assert_eq!(outcome.merged, 1);
    assert_eq!(outcome.created, 3);
    let s = &env.services;
    assert_eq!(scalar(s, "SELECT count(*) FROM clients").await, 4);
    assert_eq!(
        scalar(
            s,
            &format!(
                "SELECT count(*) FROM subscriptions WHERE client_id = {}",
                existing.0
            )
        )
        .await,
        2
    );
    assert_eq!(
        scalar(
            s,
            &format!(
                "SELECT amount_minor FROM client_balances WHERE client_id = {}",
                existing.0
            )
        )
        .await,
        15_050
    );
    assert_eq!(
        scalar(
            s,
            &format!(
                "SELECT count(*) FROM client_identities WHERE client_id = {} AND kind = 'email'",
                existing.0
            )
        )
        .await,
        1
    );
}

fn bedolaga_dump() -> Vec<u8> {
    let data = serde_json::json!({
        "metadata": {"timestamp": "2026-11-16T21:00:00+00:00", "version": "orm-1.0"},
        "data": {
            "users": [
                {"id": 1, "telegram_id": 2001, "username": "petr", "first_name": "Пётр", "status": "active", "language": "ru",
                 "balance_kopeks": 25000, "referral_code": "refPETR1", "referred_by_id": null, "created_at": "2026-05-01T10:00:00+03:00",
                 "has_had_paid_subscription": true},
                {"id": 2, "telegram_id": 2002, "username": null, "status": "blocked", "language": "en", "balance_kopeks": 5000,
                 "referred_by_id": 1, "created_at": "2026-06-01T10:00:00+00:00"},
                {"id": 3, "telegram_id": 2003, "status": "deleted", "balance_kopeks": 100},
                {"id": 4, "telegram_id": null, "email": "olga@example.com", "email_verified": true,
                 "password_hash": "$2b$12$abcdefghijklmnopqrstuuOdGq1xQmB0PZ5d4fN4m0wE1gVyZcH3W", "status": "active", "balance_kopeks": 0}
            ],
            "tariffs": [
                {"id": 7, "name": "Тариф А", "description": "Пример", "display_order": 1, "is_active": true, "traffic_limit_gb": 0,
                 "device_limit": 2, "device_price_kopeks": 5000, "max_device_limit": 4, "allowed_squads": format!("[\"{S1}\"]"),
                 "period_prices": "{\"30\": 19900, \"90\": 49900}", "traffic_reset_mode": null}
            ],
            "subscriptions": [
                {"id": 11, "user_id": 1, "status": "active", "is_trial": false, "end_date": "2026-12-30T00:00:00+00:00",
                 "traffic_limit_gb": 0, "device_limit": 3, "connected_squads": format!("[\"{S1}\"]"), "autopay_enabled": true,
                 "autopay_period_days": 30, "remnawave_short_uuid": "pppp601", "remnawave_id": 601, "tariff_id": 7},
                {"id": 12, "user_id": 2, "status": "active", "is_trial": false, "end_date": "2026-12-31T00:00:00+00:00",
                 "traffic_limit_gb": 50, "device_limit": 1, "connected_squads": format!("[\"{S1}\", \"{S2}\"]"),
                 "remnawave_short_uuid": "classic602", "tariff_id": null},
                {"id": 13, "user_id": 4, "status": "expired", "is_trial": true, "end_date": "2026-10-01T00:00:00+00:00",
                 "traffic_limit_gb": 50, "device_limit": 2, "connected_squads": format!("[\"{S2}\", \"{S1}\"]"),
                 "subscription_url": "https://sub.example.com/olga603", "tariff_id": null},
                {"id": 14, "user_id": 2, "status": "pending", "end_date": "2026-12-31T00:00:00+00:00"}
            ],
            "transactions": [
                {"id": 21, "user_id": 1, "type": "deposit", "amount_kopeks": 25000, "payment_method": "yookassa",
                 "external_id": "yk-21", "is_completed": true, "created_at": "2026-05-02T10:00:00+00:00", "completed_at": "2026-05-02T10:01:00+00:00"},
                {"id": 22, "user_id": 1, "type": "subscription_payment", "amount_kopeks": -19900, "description": "Покупка подписки на 30 дней",
                 "is_completed": true, "created_at": "2026-05-02T10:02:00+00:00"},
                {"id": 23, "user_id": 2, "type": "referral_reward", "amount_kopeks": 5000, "is_completed": true},
                {"id": 24, "user_id": 1, "type": "deposit", "amount_kopeks": 9900, "payment_method": "cryptobot", "is_completed": false}
            ],
            "promocodes": [
                {"id": 31, "code": "free7", "type": "subscription_days", "subscription_days": 7, "max_uses": 50, "is_active": true},
                {"id": 32, "code": "BONUS100", "type": "balance", "balance_bonus_kopeks": 10000, "max_uses": 1, "is_active": true}
            ],
            "promocode_uses": [{"id": 41, "promocode_id": 31, "user_id": 2, "used_at": "2026-06-02T10:00:00+00:00"}],
            "server_squads": [
                {"id": 51, "squad_uuid": S1, "display_name": "Нидерланды"},
                {"id": 52, "squad_uuid": S2, "display_name": "Германия"}
            ],
            "tickets": [{"id": 61, "user_id": 2, "title": "Вопрос", "status": "closed", "created_at": "2026-06-03T10:00:00+00:00",
                         "closed_at": "2026-06-03T12:00:00+00:00"}],
            "ticket_messages": [
                {"id": 71, "ticket_id": 61, "user_id": 2, "message_text": "Как подключить?", "is_from_admin": false, "created_at": "2026-06-03T10:00:00+00:00"},
                {"id": 72, "ticket_id": 61, "user_id": 1, "message_text": "Инструкция в боте", "is_from_admin": true, "has_media": true, "created_at": "2026-06-03T11:00:00+00:00"}
            ],
            "monitoring_logs": [{"id": 1, "message": "шум"}]
        },
        "associations": {}
    });
    let json = serde_json::to_vec(&data).unwrap();
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in [
        (
            "metadata.json",
            br#"{"format_version":"2.0","timestamp":"2026-11-16T21:00:00+00:00"}"#.to_vec(),
        ),
        ("database.json", json),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, bytes.as_slice())
            .unwrap();
    }
    gz(&builder.into_inner().unwrap())
}

#[tokio::test]
async fn бедолага_переносится_вместе_с_режимом_без_тарифов() {
    let Some(env) = env().await else { return };
    env.remna.set_squads(vec![squad(S1, "NL"), squad(S2, "DE")]);
    env.remna
        .insert_user(user(601, "pppp601", "petr", Some(2001), None));
    env.remna
        .insert_user(user(602, "classic602", "u2002", Some(2002), None));
    env.remna
        .insert_user(user(603, "olga603", "olga", None, Some("olga@example.com")));
    let (preview, outcome) = import(&env, &bedolaga_dump(), Some(owner(&env.services).await)).await;
    assert_eq!(preview.detected, Source::Bedolaga);
    assert_eq!(preview.tariff_mode, model::TariffMode::Classic);
    assert_eq!(preview.counts.clients, 3);
    assert_eq!(preview.counts.subscriptions, 3);
    assert_eq!(preview.remna.found, 3);
    assert_eq!(preview.counts.payments, 2);
    assert_eq!(preview.counts.promo_codes, 1);
    assert!(preview.skipped.iter().any(|s| s.code == "users_deleted"));
    assert!(
        preview
            .skipped
            .iter()
            .any(|s| s.code == "promo_unsupported")
    );
    assert!(
        preview
            .skipped
            .iter()
            .any(|s| s.code == "subscriptions_pending")
    );
    assert_eq!(outcome.counts.subscriptions, 3);
    assert_eq!(outcome.counts.tariffs, 2);
    let s = &env.services;
    assert_eq!(env.services.settings().currency, Currency::Rub);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE status = 'blocked'").await,
        0
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE bot_unreachable").await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM tariffs WHERE name = 'Нидерланды, Германия · 50 ГБ' AND NOT on_sale").await, 1);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions s JOIN tariffs t ON t.id = s.tariff_id WHERE t.name LIKE 'Нидерланды%'").await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT extra_devices FROM subscriptions WHERE remna_user_id = 601"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions WHERE kind = 'trial'").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM tariff_periods WHERE price_rub_minor = 19900"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances WHERE currency = 'RUB'").await,
        30_000
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM payments WHERE provider = 'balance'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM promo_codes WHERE code = 'FREE7' AND kind = 'days'"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 1);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM referral_codes WHERE code = 'REFPETR1'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tickets WHERE status = 'closed'").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM ticket_messages WHERE body LIKE '%вложение осталось%'"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM client_passwords").await, 1);
}

#[tokio::test]
async fn без_remnawave_проверка_просит_подключить_панель() {
    let Some(env) = env().await else { return };
    env.services.set_remna(None);
    let dump = archive::open_bytes(&stealthnet_dump(false), &wanted).unwrap();
    let legacy = map_dump(&dump).unwrap();
    let remna = check::fetch_remna(&env.services).await;
    let checked = check::preview(&env.services, &legacy, remna).await.unwrap();
    assert!(!checked.preview.remna.connected);
    assert_eq!(checked.preview.remna.found, 0);
    let err = apply_legacy(&env.services, &legacy, 1, uuid::Uuid::new_v4(), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "import.remna_required");
    assert_eq!(
        scalar(&env.services, "SELECT count(*) FROM clients").await,
        0
    );
}

pub(super) async fn wait(services: &Services, id: uuid::Uuid, want: &[RunState]) -> runs::RunView {
    for _ in 0..200 {
        let run = services.legacy().run(id).await.unwrap();
        if want.contains(&run.state) {
            return run;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("перенос не дошёл до {want:?}");
}

#[tokio::test]
async fn загрузка_проверка_и_перенос_идут_в_фоне() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let admin = owner(&env.services).await;
    let dir =
        std::env::temp_dir().join(format!("sn-import-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("upload.upload");
    std::fs::write(&path, bedolaga_dump()).unwrap();
    let run = env
        .services
        .legacy()
        .uploaded(
            Source::Stealthnet,
            "backup_20261116_210000.tar.gz",
            100,
            &path,
            Some(admin),
        )
        .await
        .unwrap();
    let ready = wait(&env.services, run.id, &[RunState::Ready, RunState::Failed]).await;
    assert_eq!(ready.state, RunState::Ready, "{:?}", ready.error);
    assert_eq!(ready.source, Source::Bedolaga);
    let view = env.services.legacy().view().await.unwrap();
    assert_eq!(view.run.as_ref().map(|r| r.id), Some(run.id));
    env.services
        .legacy()
        .start(run.id, Some(admin))
        .await
        .unwrap();
    let again = env.services.legacy().start(run.id, Some(admin)).await;
    assert!(again.is_err());
    let done = wait(&env.services, run.id, &[RunState::Done, RunState::Failed]).await;
    assert_eq!(done.state, RunState::Done, "{:?}", done.error);
    assert!(done.outcome.is_some());
    assert!(!path.exists(), "файл выгрузки удаляется после переноса");
    let history = env.services.legacy().view().await.unwrap().history;
    assert_eq!(history.len(), 1);
    let fresh = dir.join("fresh.upload");
    std::fs::write(&fresh, bedolaga_dump()).unwrap();
    let second = env
        .services
        .legacy()
        .uploaded(
            Source::Bedolaga,
            "backup_fresh.tar.gz",
            100,
            &fresh,
            Some(admin),
        )
        .await
        .unwrap();
    wait(
        &env.services,
        second.id,
        &[RunState::Ready, RunState::Failed],
    )
    .await;
    env.services.legacy().discard(second.id).await.unwrap();
    let view = env.services.legacy().view().await.unwrap();
    assert!(
        view.run.is_none(),
        "после отказа от файла не показывается прошлый перенос"
    );
    assert_eq!(view.history.len(), 1);
    assert!(!fresh.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn архив_с_паролем_открывается_только_с_верным_паролем() {
    let dir = std::env::temp_dir().join(format!("sn-zip-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("backup.zip");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .with_aes_encryption(zip::AesMode::Aes256, "верный");
        zip.start_file("backup_20261116_210000.tar.gz", options)
            .unwrap();
        zip.write_all(&bedolaga_dump()).unwrap();
        zip.finish().unwrap();
    }
    assert_eq!(
        read_file(&path, None).unwrap_err(),
        archive::OpenError::NeedsPassword
    );
    assert_eq!(
        read_file(&path, Some("неверный")).unwrap_err(),
        archive::OpenError::WrongPassword
    );
    let (legacy, extracted) = read_file(&path, Some("верный")).unwrap();
    assert_eq!(legacy.source, Source::Bedolaga);
    assert_eq!(legacy.clients.len(), 3);
    let inner = extracted.unwrap();
    assert!(inner.exists());
    assert_eq!(read_file(&inner, None).unwrap().0.clients.len(), 3);
    std::fs::remove_dir_all(&dir).ok();
}

fn quirks_dump() -> Vec<u8> {
    let mut sql = String::from("-- Created at: 2026-11-17T09:00:00.000Z\n\n");
    sql += &copy(
        "system_settings",
        &["id", "key", "value"],
        &[&[Some("ss1"), Some("default_currency"), Some("rub")]],
    );
    sql += &copy(
        "tariff_categories",
        &["id", "name", "sort_order"],
        &[&[Some("cat1"), Some("Все"), Some("0")]],
    );
    sql += &copy(
        "tariffs",
        &[
            "id",
            "category_id",
            "name",
            "duration_days",
            "price",
            "currency",
        ],
        &[&[
            Some("t1"),
            Some("cat1"),
            Some("Месяц"),
            Some("30"),
            Some("300"),
            Some("rub"),
        ]],
    );
    sql += &copy(
        "tariff_price_options",
        &["id", "tariff_id", "duration_days", "price", "sort_order"],
        &[&[Some("o1"), Some("t1"), Some("30"), Some("300"), Some("0")]],
    );
    sql += &copy(
        "clients",
        &[
            "id",
            "email",
            "role",
            "balance",
            "telegram_id",
            "telegram_username",
            "referrer_id",
        ],
        &[
            &[
                Some("k1"),
                None,
                Some("CLIENT"),
                Some("120"),
                Some("2001"),
                Some("Lena"),
                Some("k3"),
            ],
            &[
                Some("k2"),
                None,
                Some("CLIENT"),
                Some("50000"),
                Some("2002"),
                None,
                None,
            ],
            &[
                Some("k3"),
                None,
                Some("CLIENT"),
                Some("0"),
                Some("2003"),
                Some("Oleg"),
                Some("k1"),
            ],
            &[
                Some("k4"),
                Some("Mila@Example.com"),
                Some("CLIENT"),
                Some("0"),
                None,
                None,
                Some("gone"),
            ],
            &[
                Some("k5"),
                Some("mila@example.com"),
                Some("CLIENT"),
                Some("0"),
                None,
                None,
                None,
            ],
            &[
                Some("k6"),
                Some("not-an-email"),
                Some("CLIENT"),
                Some("0"),
                Some("2006"),
                None,
                None,
            ],
        ],
    );
    let u = |n: u32| format!("6f1c2f3e-0000-4000-8000-00000000000{n}");
    let (u1, u2, u3, u4, u5, u6, u7) = (u(1), u(2), u(3), u(4), u(5), u(6), u(7));
    let sub = |id: &'static str,
               owner: &'static str,
               uuid: &str,
               index: &'static str,
               expire: &'static str,
               hidden: Option<&'static str>| {
        [
            Some(id.to_string()),
            Some(owner.to_string()),
            Some(uuid.to_string()),
            Some(index.to_string()),
            Some("t1".to_string()),
            Some(expire.to_string()),
            hidden.map(str::to_string),
        ]
    };
    let rows = [
        sub("a0", "k1", &u1, "0", "2026-12-30 00:00:00", None),
        sub("a1", "k1", &u2, "1", "2026-12-30 00:00:00", None),
        sub("a2", "k1", &u3, "2", "2026-12-30 00:00:00", None),
        sub("b0", "k2", &u4, "0", "2026-12-25 00:00:00", None),
        sub("c0", "k3", &u4, "0", "2026-12-25 00:00:00", None),
        sub(
            "h0",
            "k3",
            &u5,
            "1",
            "2026-01-01 00:00:00",
            Some("2026-02-01 00:00:00"),
        ),
        sub("m0", "k4", &u6, "0", "2026-12-28 00:00:00", None),
        sub("n0", "k5", &u7, "0", "2026-12-28 00:00:00", None),
    ];
    let cells: Vec<Vec<Option<&str>>> = rows
        .iter()
        .map(|r| r.iter().map(|c| c.as_deref()).collect())
        .collect();
    let refs: Vec<&[Option<&str>]> = cells.iter().map(Vec::as_slice).collect();
    sql += &copy(
        "subscriptions",
        &[
            "id",
            "owner_id",
            "remnawave_uuid",
            "subscription_index",
            "tariff_id",
            "expire_at",
            "hidden_at",
        ],
        &refs,
    );
    sql.into_bytes()
}

async fn remna_user_of(services: &Services, old_id: &str) -> i64 {
    scalar(
        services,
        &format!(
            "SELECT s.remna_user_id FROM legacy_ids l JOIN subscriptions s ON s.id = l.new_id \
             WHERE l.entity = 'stealthnet:subscription' AND l.old_id = '{old_id}'"
        ),
    )
    .await
}

#[tokio::test]
async fn особые_случаи_старой_панели_переносятся_и_объясняются() {
    let Some(env) = env().await else { return };
    let expire = |day: u32| Utc.with_ymd_and_hms(2026, 12, day, 0, 0, 0).unwrap();
    let add = |id: i64, name: &str, tg: Option<i64>, email: Option<&str>, day: u32| {
        let mut u = user(id, &format!("short{id}"), name, tg, email);
        u.expire_at = expire(day);
        env.remna.insert_user(u);
    };
    add(11, "Lena_2", Some(2001), None, 30);
    add(12, "Lena", Some(2001), None, 30);
    add(13, "Lena_1", Some(2001), None, 30);
    add(14, "Oleg", Some(2003), None, 25);
    add(15, "x15", None, Some("mila@example.com"), 28);
    add(16, "x16", None, Some("Mila@Example.com"), 28);
    let admin = owner(&env.services).await;
    let (preview, outcome) = import(&env, &quirks_dump(), Some(admin)).await;

    assert_eq!(preview.counts.subscriptions, 7);
    assert_eq!(preview.remna.found, 6);
    assert_eq!(preview.remna.shared, 1);
    assert_eq!(preview.remna.missing + preview.remna.ambiguous, 0);
    assert_eq!(preview.balances.large_clients, 1);
    assert_eq!(preview.balances.large_total_minor, 5_000_000);
    assert_eq!(preview.balances.large_from_minor, Some(300_000));
    assert!(preview.balances.large_by_tariff);
    let skip = |code: &str| {
        preview
            .skipped
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.count)
    };
    assert_eq!(skip("subscription_hidden"), Some(1));
    assert_eq!(skip("subscription_shared"), Some(1));

    let s = &env.services;
    assert_eq!(remna_user_of(s, "a0").await, 12);
    assert_eq!(remna_user_of(s, "a1").await, 13);
    assert_eq!(remna_user_of(s, "a2").await, 11);
    assert_eq!(remna_user_of(s, "c0").await, 14);
    assert_eq!(remna_user_of(s, "m0").await, 16);
    assert_eq!(remna_user_of(s, "n0").await, 15);
    assert_eq!(outcome.counts.subscriptions, 6);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM legacy_ids WHERE entity = 'stealthnet:subscription' AND old_id IN ('b0', 'h0')").await,
        0
    );
    assert_eq!(
        scalar(
            s,
            "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances"
        )
        .await,
        5_012_000
    );

    let run = scalar(s, "SELECT max(id) FROM import_runs").await;
    let mut conn = s.store().conn().await.unwrap();
    let issues = runs::issues(&mut conn, run).await.unwrap();
    assert_eq!(issues[0].code, "balance_large", "деньги — первыми в отчёте");
    let shared = issues
        .iter()
        .find(|i| i.code == "subscription_shared")
        .expect("общая подписка в отчёте");
    assert_eq!(shared.old_id.as_deref(), Some("b0"));
    assert!(shared.details.contains("Oleg"), "{}", shared.details);
    let large = issues
        .iter()
        .find(|i| i.code == "balance_large")
        .expect("крупный баланс в отчёте");
    assert_eq!(large.old_id.as_deref(), Some("k2"));
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.code.starts_with("subscription_"))
            .count(),
        1,
        "{issues:?}"
    );
    let bad_email = issues
        .iter()
        .find(|i| i.code == "email_invalid")
        .expect("ошибка в почте в отчёте");
    assert_eq!(bad_email.old_id.as_deref(), Some("k6"));
    let skipped: Vec<&str> = issues
        .iter()
        .filter(|i| i.code == "referral_skipped")
        .filter_map(|i| i.old_id.as_deref())
        .collect();
    assert_eq!(skipped, ["k3", "k4"], "{issues:?}");
    assert_eq!(outcome.counts.referrals, 1);
    let calls = env.remna.calls();
    assert!(
        calls
            .iter()
            .all(|c| matches!(c.as_str(), "stream_users" | "internal_squads")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn пустая_или_чужая_remnawave_даёт_понятное_предупреждение() {
    let Some(env) = env().await else { return };
    let dump = archive::open_bytes(&stealthnet_dump(false), &wanted).unwrap();
    let legacy = map_dump(&dump).unwrap();
    let check = || async {
        let remna = check::fetch_remna(&env.services).await;
        check::preview(&env.services, &legacy, remna)
            .await
            .unwrap()
            .preview
    };
    let empty = check().await;
    assert_eq!(empty.remna.users, 0);
    let text = wrong_panel(&empty).expect("предупреждение о пустой панели");
    assert!(text.contains("нет ни одного пользователя"), "{text}");
    env.remna
        .insert_user(user(900, "zzzz900", "stranger900", Some(424_242), None));
    let foreign = check().await;
    let text = wrong_panel(&foreign).expect("предупреждение о чужой панели");
    assert!(text.contains("пользователей в ней: 1"), "{text}");
    seed_stealthnet_remna(&env.remna);
    assert_eq!(wrong_panel(&check().await), None);
}

fn extras_dump(approved: bool) -> Vec<u8> {
    let mut sql = String::from("-- Created at: 2026-11-17T09:00:00.000Z\n\n");
    let buttons = r#"[{"id":"topup","visible":false,"label":"💳 Пополнить","style":"success"},{"id":"tariffs","visible":true,"label":"🚀 Купить VPN","style":"primary"},{"id":"proxy","visible":true,"label":"Прокси"}]"#;
    let texts = r#"{"welcomeGreeting":"Привет, {{STAR}}!","balancePrefix":"💰 Баланс:"}"#;
    let settings: Vec<[Option<&str>; 3]> = vec![
        [Some("s1"), Some("default_currency"), Some("rub")],
        [Some("s2"), Some("service_name"), Some("Тест VPN")],
        [Some("s3"), Some("support_link"), Some("@help_bot")],
        [Some("s4"), Some("tickets_enabled"), Some("true")],
        [Some("s5"), Some("trial_days"), Some("3")],
        [Some("s6"), Some("trial_squad_uuid"), Some(S1)],
        [Some("s7"), Some("trial_device_limit"), Some("2")],
        [Some("s8"), Some("trial_traffic_limit"), Some("10737418240")],
        [
            Some("s9"),
            Some("offer_link"),
            Some("https://example.com/offer"),
        ],
        [
            Some("s10"),
            Some("agreement_link"),
            Some("https://example.com/terms"),
        ],
        [Some("s11"), Some("bot_buttons"), Some(buttons)],
        [Some("s12"), Some("bot_buttons_per_row"), Some("2")],
        [Some("s13"), Some("bot_menu_texts"), Some(texts)],
        [Some("s14"), Some("bot_emojis"), Some(r#"{"STAR":"⭐"}"#)],
        [Some("s15"), Some("default_referral_percent"), Some("25")],
        [Some("s16"), Some("referral_percent_level_2"), Some("5")],
        [
            Some("s17"),
            Some("cryptopay_api_token"),
            Some("123:TESTTOKEN"),
        ],
        [Some("s18"), Some("cryptopay_testnet"), Some("true")],
        [Some("s19"), Some("yookassa_shop_id"), Some("123456")],
        [
            Some("s20"),
            Some("bot_welcome_text"),
            Some("<b>Добро пожаловать</b>"),
        ],
        [
            Some("s21"),
            Some("gift_subscriptions_enabled"),
            Some("true"),
        ],
        [Some("s22"), Some("paritypay_enabled"), Some("true")],
        [
            Some("s23"),
            Some("paritypay_shop_id"),
            Some("9bee9309-2585-4332-b63d-e1d897f7ce84"),
        ],
        [
            Some("s24"),
            Some("paritypay_api_key"),
            Some("pp_secret_one"),
        ],
        [
            Some("s25"),
            Some("paritypay_signing_secret"),
            Some("pp_secret_two"),
        ],
    ];
    let settings: Vec<&[Option<&str>]> = settings.iter().map(|r| r.as_slice()).collect();
    sql += &copy("system_settings", &["id", "key", "value"], &settings);
    sql += &copy(
        "tariff_categories",
        &["id", "name", "sort_order"],
        &[&[Some("cat1"), Some("Все"), Some("0")]],
    );
    sql += &copy(
        "tariffs",
        &[
            "id",
            "category_id",
            "name",
            "duration_days",
            "price",
            "currency",
        ],
        &[&[
            Some("t1"),
            Some("cat1"),
            Some("Месяц"),
            Some("30"),
            Some("300"),
            Some("rub"),
        ]],
    );
    sql += &copy(
        "tariff_price_options",
        &["id", "tariff_id", "duration_days", "price", "sort_order"],
        &[&[Some("o1"), Some("t1"), Some("30"), Some("300"), Some("0")]],
    );
    sql += &copy(
        "clients",
        &["id", "role", "balance", "telegram_id", "telegram_username"],
        &[
            &[
                Some("g1"),
                Some("CLIENT"),
                Some("0"),
                Some("3001"),
                Some("giver"),
            ],
            &[Some("g2"), Some("CLIENT"), Some("0"), Some("3002"), None],
        ],
    );
    sql += &copy(
        "subscriptions",
        &[
            "id",
            "owner_id",
            "remnawave_uuid",
            "subscription_index",
            "tariff_id",
            "gift_status",
            "expire_at",
        ],
        &[
            &[
                Some("p1"),
                Some("g1"),
                Some("6f1c2f3e-0000-4000-8000-0000000000aa"),
                Some("1"),
                Some("t1"),
                Some("GIFT_CODE_ACTIVE"),
                Some("2026-12-07 09:00:00"),
            ],
            &[
                Some("p2"),
                Some("g1"),
                Some("6f1c2f3e-0000-4000-8000-0000000000bb"),
                Some("2"),
                Some("t1"),
                Some("GIFT_RESERVED"),
                None,
            ],
        ],
    );
    sql += &copy(
        "gift_codes",
        &[
            "id",
            "code",
            "creator_id",
            "secondary_subscription_id",
            "status",
            "expires_at",
            "created_at",
        ],
        &[
            &[
                Some("gc1"),
                Some("ABCD-EFGH-JKLM"),
                Some("g1"),
                Some("p1"),
                Some("ACTIVE"),
                Some("2026-11-27 09:00:00"),
                Some("2026-11-10 09:00:00"),
            ],
            &[
                Some("gc2"),
                Some("MNPQ-RSTU-VWXY"),
                Some("g1"),
                Some("p2"),
                Some("ACTIVE"),
                Some("2026-12-01 09:00:00"),
                Some("2026-11-11 09:00:00"),
            ],
        ],
    );
    let first = if approved { "APPROVED" } else { "PENDING" };
    sql += &copy(
        "withdrawal_requests",
        &[
            "id",
            "client_id",
            "amount",
            "wallet_trc20",
            "status",
            "admin_comment",
            "processed_at",
            "created_at",
        ],
        &[
            &[
                Some("w1"),
                Some("g2"),
                Some("500"),
                Some("TXwallet1"),
                Some(first),
                None,
                approved.then_some("2026-11-17 08:00:00"),
                Some("2026-11-12 09:00:00"),
            ],
            &[
                Some("w2"),
                Some("g2"),
                Some("300"),
                Some("TXwallet1"),
                Some("REJECTED"),
                Some("неверный кошелёк"),
                Some("2026-11-11 10:00:00"),
                Some("2026-11-10 09:00:00"),
            ],
        ],
    );
    let hash = "$2b$10$abcdefghijklmnopqrstuuOdGq1xQmB0PZ5d4fN4m0wE1gVyZcH3W";
    sql += &copy(
        "admins",
        &["id", "email", "password_hash", "role", "allowed_sections"],
        &[
            &[
                Some("a1"),
                Some("boss@example.com"),
                Some(hash),
                Some("ADMIN"),
                None,
            ],
            &[
                Some("a2"),
                Some("man@example.com"),
                Some(hash),
                Some("MANAGER"),
                Some(r#"["clients","tickets"]"#),
            ],
            &[
                Some("a3"),
                Some("owner@example.com"),
                Some(hash),
                Some("ADMIN"),
                None,
            ],
        ],
    );
    sql += &copy(
        "auto_broadcast_rules",
        &[
            "id",
            "name",
            "trigger_type",
            "delay_days",
            "channel",
            "message",
            "button_text",
            "button_url",
            "enabled",
        ],
        &[
            &[
                Some("r1"),
                Some("Скоро конец"),
                Some("subscription_ending_soon"),
                Some("3"),
                Some("telegram"),
                Some("Подписка кончается, промокод {{PROMOCODE}}"),
                Some("Продлить"),
                Some("menu:tariffs"),
                Some("t"),
            ],
            &[
                Some("r2"),
                Some("Привет"),
                Some("welcome"),
                Some("0"),
                Some("telegram"),
                Some("Добро пожаловать"),
                None,
                None,
                Some("t"),
            ],
        ],
    );
    sql.into_bytes()
}

#[tokio::test]
async fn настройки_админы_подарки_выводы_и_авторассылки_переносятся() {
    let Some(env) = env().await else { return };
    let admin = owner(&env.services).await;
    let (preview, outcome) = import(&env, &extras_dump(false), Some(admin)).await;
    assert_eq!(preview.counts.admins, 3);
    assert_eq!(preview.counts.gifts, 2);
    assert_eq!(preview.counts.withdrawals, 2);
    assert_eq!(preview.counts.auto_rules, 2);
    assert_eq!(outcome.counts.admins, 2);
    assert_eq!(outcome.counts.gifts, 2);
    assert_eq!(outcome.counts.withdrawals, 2);
    assert_eq!(outcome.counts.auto_rules, 1);
    assert_eq!(outcome.providers, ["Crypto Pay", "ParityPay"]);

    let s = &env.services;
    let core = s.settings();
    assert_eq!(core.brand_name, "Тест VPN");
    assert_eq!(core.support_url.as_deref(), Some("https://t.me/help_bot"));
    assert_eq!(core.trial.variants.len(), 1);
    let trial = &core.trial.variants[0];
    assert_eq!((trial.days, trial.devices), (3, 2));
    assert_eq!(trial.traffic.limit_bytes, Some(10 * 1024 * 1024 * 1024));
    let setting = |key: &'static str| async move {
        let mut conn = s.store().conn().await.unwrap();
        sqlx::query_scalar::<_, serde_json::Value>("SELECT value FROM settings WHERE key = $1")
            .bind(key)
            .fetch_one(sn_store::Db::conn(&mut conn))
            .await
            .unwrap()
    };
    let docs = setting("bot.documents").await;
    assert_eq!(docs.as_array().map(Vec::len), Some(2), "{docs}");
    let content = setting("bot.content").await;
    assert_eq!(content["menu_columns"], 2, "{content}");
    assert_eq!(content["buttons"]["menu.btn.balance"]["hidden"], true);
    assert_eq!(content["buttons"]["menu.btn.buy"]["text"], "🚀 Купить VPN");
    assert_eq!(content["buttons"]["menu.btn.buy"]["style"], "primary");
    assert_eq!(content["texts"]["menu.greeting"], "Привет, ⭐!");
    assert_eq!(content["texts"]["menu.balance"], "💰 Баланс: {balance}");
    assert!(
        setting("payments.cryptopay").await["enabled"] == true,
        "Crypto Pay включена"
    );
    assert!(setting("payments.yookassa").await["enabled"] == false);
    let parity = setting("payments.paritypay").await;
    assert!(parity["enabled"] == true, "{parity}");
    assert_eq!(
        parity["config"]["shop_id"], "9bee9309-2585-4332-b63d-e1d897f7ce84",
        "{parity}"
    );
    assert!(!parity.to_string().contains("pp_secret_one"), "{parity}");
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM admins WHERE email IN ('boss@example.com', 'man@example.com')"
        )
        .await,
        2
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM admins WHERE email = 'man@example.com' AND role = 'readonly' AND permissions @> ARRAY['clients_write', 'support']").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM gifts WHERE code = 'ABCD-EFGH-JKLM' AND status = 'active' AND days = 20").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM gifts WHERE code = 'MNPQ-RSTU-VWXY' AND days = 30"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM referral_withdrawals WHERE status = 'pending' AND held_at IS NOT NULL AND released_at IS NULL AND amount_minor = 50000").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM referral_withdrawals WHERE status = 'rejected' AND released_at IS NOT NULL").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM balance_entries WHERE idempotency_key LIKE 'withdrawal:%'"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM auto_broadcasts WHERE NOT enabled").await,
        1
    );
    let run = scalar(s, "SELECT max(id) FROM import_runs").await;
    let mut conn = s.store().conn().await.unwrap();
    let issues = runs::issues(&mut conn, run).await.unwrap();
    drop(conn);
    let codes: Vec<&str> = issues.iter().map(|i| i.code.as_str()).collect();
    for code in [
        "admin_exists",
        "provider_off",
        "rule_skipped",
        "rule_changed",
        "setting_text",
    ] {
        assert!(codes.contains(&code), "нет {code} в {codes:?}");
    }
    assert!(
        issues.iter().all(|i| !i.details.contains("TESTTOKEN")),
        "ключи платёжек в отчёт не попадают"
    );

    let (_, again) = import(&env, &extras_dump(true), Some(admin)).await;
    assert_eq!(again.counts.admins, 0);
    assert_eq!(again.counts.gifts, 0);
    assert_eq!(again.counts.auto_rules, 0);
    assert_eq!(scalar(s, "SELECT count(*) FROM gifts").await, 2);
    assert_eq!(scalar(s, "SELECT count(*) FROM auto_broadcasts").await, 1);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM referral_withdrawals").await,
        2
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM referral_withdrawals WHERE status = 'paid' AND decided_at IS NOT NULL").await,
        1
    );
    let run = scalar(s, "SELECT max(id) FROM import_runs").await;
    let mut conn = s.store().conn().await.unwrap();
    let issues = runs::issues(&mut conn, run).await.unwrap();
    assert!(
        issues.iter().any(|i| i.code == "settings_kept"),
        "{issues:?}"
    );
}

#[tokio::test]
async fn настройки_новой_панели_не_затираются_переносом() {
    let Some(env) = env().await else { return };
    let admin = owner(&env.services).await;
    let s = &env.services;
    s.settings_ops()
        .update_core(
            &crate::settings::CoreUpdate {
                brand_name: Some("Новое имя".into()),
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    let mut texts = std::collections::BTreeMap::new();
    texts.insert(
        "menu.greeting".to_string(),
        Some("Своё приветствие".to_string()),
    );
    s.bot_content_ops()
        .update_content(
            &crate::bot_content::ContentUpdate {
                texts,
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    let mut config = serde_json::Map::new();
    config.insert("api_token".into(), serde_json::json!("999:OWN"));
    s.settings_ops()
        .update_provider(
            "cryptopay",
            &crate::settings::ProviderUpdate {
                topup_bonus: None,
                enabled: Some(false),
                config,
                clear: Vec::new(),
                channels: None,
                methods: None,
                label: None,
            },
            admin,
        )
        .await
        .unwrap();
    s.set_remna(Some(env.remna.clone()));
    let (_, outcome) = import(&env, &extras_dump(false), Some(admin)).await;
    assert_eq!(s.settings().brand_name, "Новое имя");
    assert_eq!(
        s.settings().support_url.as_deref(),
        Some("https://t.me/help_bot")
    );
    let content = s.bot_content().content.config().clone();
    assert_eq!(
        content.texts.get("menu.greeting").map(String::as_str),
        Some("Своё приветствие")
    );
    assert!(content.buttons.contains_key("menu.btn.buy"));
    assert!(!outcome.providers.contains(&"Crypto Pay".to_string()));
    let run = scalar(s, "SELECT max(id) FROM import_runs").await;
    let mut conn = s.store().conn().await.unwrap();
    let issues = runs::issues(&mut conn, run).await.unwrap();
    assert!(
        issues.iter().any(|i| i.code == "provider_kept"),
        "{issues:?}"
    );
}

fn invoices_dump(second: bool) -> Vec<u8> {
    let mut sql = String::from("-- Created at: 2026-11-17T09:00:00.000Z\n\n");
    sql += &copy(
        "system_settings",
        &["id", "key", "value"],
        &[&[Some("ss1"), Some("default_currency"), Some("rub")]],
    );
    sql += &copy(
        "tariff_categories",
        &["id", "name", "sort_order"],
        &[&[Some("cat1"), Some("Все"), Some("0")]],
    );
    sql += &copy(
        "tariffs",
        &[
            "id",
            "category_id",
            "name",
            "duration_days",
            "price",
            "currency",
        ],
        &[&[
            Some("t1"),
            Some("cat1"),
            Some("Месяц"),
            Some("30"),
            Some("300"),
            Some("rub"),
        ]],
    );
    sql += &copy(
        "tariff_price_options",
        &["id", "tariff_id", "duration_days", "price", "sort_order"],
        &[&[Some("o1"), Some("t1"), Some("30"), Some("300"), Some("0")]],
    );
    sql += &copy(
        "clients",
        &["id", "role", "balance", "telegram_id", "telegram_username"],
        &[&[
            Some("k1"),
            Some("CLIENT"),
            Some("0"),
            Some("4001"),
            Some("payer"),
        ]],
    );
    sql += &copy(
        "subscriptions",
        &[
            "id",
            "owner_id",
            "remnawave_uuid",
            "subscription_index",
            "tariff_id",
            "expire_at",
        ],
        &[&[
            Some("s1"),
            Some("k1"),
            Some("601"),
            Some("0"),
            Some("t1"),
            Some("2026-12-20 00:00:00"),
        ]],
    );
    let p5 = if second { "PAID" } else { "PENDING" };
    sql += &copy(
        "payments",
        &[
            "id",
            "client_id",
            "order_id",
            "amount",
            "currency",
            "status",
            "provider",
            "external_id",
            "tariff_id",
            "tariff_price_option_id",
            "subscription_id",
            "metadata",
            "created_at",
            "paid_at",
        ],
        &[
            &[
                Some("p1"),
                Some("k1"),
                Some("ord-p1"),
                Some("300"),
                Some("rub"),
                Some("PENDING"),
                Some("cryptopay"),
                None,
                Some("t1"),
                Some("o1"),
                None,
                Some(
                    r#"{"extendsSecondarySubId":"s1","redirectTargetUrl":"https://pay.example.com/p1"}"#,
                ),
                Some("2026-11-16 10:00:00"),
                None,
            ],
            &[
                Some("p2"),
                Some("k1"),
                Some("ord-p2"),
                Some("150"),
                Some("rub"),
                Some("PENDING"),
                Some("cryptopay"),
                None,
                None,
                None,
                None,
                None,
                Some("2026-11-16 11:00:00"),
                None,
            ],
            &[
                Some("p3"),
                Some("k1"),
                Some("ord-p3"),
                Some("150"),
                Some("rub"),
                Some("PENDING"),
                Some("cryptopay"),
                None,
                None,
                None,
                None,
                None,
                Some("2026-08-01 11:00:00"),
                None,
            ],
            &[
                Some("p4"),
                Some("k1"),
                Some("contract-1"),
                Some("300"),
                Some("rub"),
                Some("PAID"),
                Some("lavatop"),
                Some("lava-1"),
                Some("t1"),
                Some("o1"),
                Some("s1"),
                None,
                Some("2026-11-01 10:00:00"),
                Some("2026-11-01 10:05:00"),
            ],
            &[
                Some("p6"),
                Some("k1"),
                Some("lava-new-1"),
                Some("300"),
                Some("rub"),
                Some("PENDING"),
                Some("lavatop"),
                Some("lava-ext-6"),
                Some("t1"),
                Some("o1"),
                None,
                None,
                Some("2026-11-16 13:00:00"),
                None,
            ],
            &[
                Some("p5"),
                Some("k1"),
                Some("ord-p5"),
                Some("200"),
                Some("rub"),
                Some(p5),
                Some("cryptopay"),
                None,
                None,
                None,
                None,
                None,
                Some("2026-11-16 12:00:00"),
                second.then_some("2026-11-17 08:00:00"),
            ],
        ],
    );
    sql.into_bytes()
}

fn crypto_paid(payload: &str, amount: &str, invoice: u64) -> String {
    serde_json::json!({
        "update_id": invoice, "update_type": "invoice_paid", "request_date": "2026-11-17T09:30:00.000Z",
        "payload": {"invoice_id": invoice, "status": "paid", "currency_type": "fiat", "fiat": "RUB",
                    "amount": amount, "payload": payload}
    })
    .to_string()
}

async fn legacy_hook(env: &Env, body: &str, signature: &str) -> sn_payments::WebhookReply {
    let request = sn_payments::WebhookRequest {
        headers: vec![("crypto-pay-api-signature".into(), signature.into())],
        body: body.to_string().into_bytes().into(),
        remote_ip: None,
        route_suffix: None,
        query: None,
    };
    env.services
        .inbox()
        .handle_legacy(crate::inbox::InboundWebhook {
            provider: "cryptopay".into(),
            request,
        })
        .await
        .unwrap()
        .reply
}

#[tokio::test]
async fn счёт_старой_панели_оплачивается_по_старому_адресу_один_раз() {
    let Some(env) = env().await else { return };
    env.remna
        .insert_user(user(601, "short601", "payer", Some(4001), None));
    let admin = owner(&env.services).await;
    let (preview, _) = import(&env, &invoices_dump(false), Some(admin)).await;
    assert_eq!(
        preview
            .skipped
            .iter()
            .find(|s| s.code == "invoices_waiting")
            .map(|s| s.count),
        Some(4)
    );
    let s = &env.services;
    assert_eq!(scalar(s, "SELECT count(*) FROM legacy_invoices").await, 5);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM legacy_invoices WHERE recurring AND order_ref = 'contract-1'"
        )
        .await,
        1
    );
    let token = "4321:TEST";
    {
        use sn_store::Db;
        let mut conn = s.store().conn().await.unwrap();
        conn.settings()
            .set(
                &sn_domain::settings::keys::provider("cryptopay"),
                &serde_json::json!({ "enabled": true }),
                None,
            )
            .await
            .unwrap();
        crate::settings::write_secret(
            &mut conn,
            s.secrets(),
            &sn_domain::settings::secret_keys::provider("cryptopay", "api_token"),
            Some(token),
            None,
        )
        .await
        .unwrap();
    }
    s.reload().await.unwrap();
    s.set_remna(Some(env.remna.clone()));
    let sign = |body: &str| sn_payments::providers::cryptopay_signature(token, body.as_bytes());

    let renew = crypto_paid("p1", "300.00", 7001);
    legacy_hook(&env, &renew, &sign(&renew)).await;
    assert_eq!(
        scalar(s, "SELECT count(*) FROM orders o JOIN legacy_invoices l ON l.order_id = o.id \
                   WHERE l.old_id = 'p1' AND o.kind = 'renewal' AND o.status IN ('paid', 'fulfilled') \
                   AND o.subscription_id = (SELECT s.id FROM subscriptions s WHERE s.remna_user_id = 601)").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM webhook_inbox w JOIN legacy_invoices l ON l.id = w.legacy_invoice_id \
                   WHERE w.legacy_path AND l.old_id = 'p1' AND w.outcome = 'accepted'").await,
        1
    );
    let orders = scalar(s, "SELECT count(*) FROM orders").await;
    legacy_hook(&env, &renew, &sign(&renew)).await;
    let other = crypto_paid("p1", "300.00", 7001).replace("09:30", "09:31");
    legacy_hook(&env, &other, &sign(&other)).await;
    assert_eq!(
        scalar(s, "SELECT count(*) FROM orders").await,
        orders,
        "повтор не создаёт заказ"
    );

    let topup = crypto_paid("p2", "150.00", 7002);
    legacy_hook(&env, &topup, &sign(&topup)).await;
    let balance =
        "SELECT COALESCE(sum(amount_minor), 0) FROM client_balances WHERE currency = 'RUB'";
    assert_eq!(scalar(s, balance).await, 15_000);

    for charge in ["charge-1", "charge-2"] {
        let outcome = sn_payments::WebhookOutcome {
            event_key: charge.into(),
            payment: Some(sn_payments::PaymentRef::ProviderTxid(charge.into())),
            provider_txid: Some(charge.into()),
            status: sn_payments::ProviderStatus::Paid,
            amount: Some(sn_domain::money::Money::new(30_000, Currency::Rub).into()),
            saved_method: None,
            reply: sn_payments::WebhookReply::Empty { status: 200 },
            signal: None,
        };
        let request = sn_payments::WebhookRequest {
            headers: Vec::new(),
            body: format!(r#"{{"contractId":"{charge}","parentContractId":"contract-1"}}"#)
                .into_bytes()
                .into(),
            remote_ip: None,
            route_suffix: None,
            query: None,
        };
        let found = crate::legacy::invoices::settle(s, "lava", &outcome, &request)
            .await
            .unwrap();
        assert!(found.is_some(), "списание {charge} по старому договору");
    }
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM orders o JOIN payments p ON p.order_id = o.id \
                   WHERE p.provider = 'lava' AND o.kind = 'renewal'"
        )
        .await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM legacy_invoices WHERE recurring AND payment_id IS NULL"
        )
        .await,
        2,
        "договоры не расходуются"
    );
    let lava = |contract: &str, parent: Option<&str>| {
        let body = match parent {
            Some(parent) => {
                format!(r#"{{"contractId":"{contract}","parentContractId":"{parent}"}}"#)
            }
            None => format!(r#"{{"contractId":"{contract}"}}"#),
        };
        (
            sn_payments::WebhookOutcome {
                event_key: contract.into(),
                payment: Some(sn_payments::PaymentRef::ProviderTxid(contract.into())),
                provider_txid: Some(contract.into()),
                status: sn_payments::ProviderStatus::Paid,
                amount: Some(sn_domain::money::Money::new(30_000, Currency::Rub).into()),
                saved_method: None,
                reply: sn_payments::WebhookReply::Empty { status: 200 },
                signal: None,
            },
            sn_payments::WebhookRequest {
                headers: Vec::new(),
                body: body.into_bytes().into(),
                remote_ip: None,
                route_suffix: None,
                query: None,
            },
        )
    };
    let (first, request) = lava("lava-new-1", None);
    crate::legacy::invoices::settle(s, "lava", &first, &request)
        .await
        .unwrap()
        .expect("первая оплата счёта Lava.top");
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM orders o JOIN legacy_invoices l ON l.order_id = o.id \
                   WHERE l.old_id = 'p6' AND o.kind = 'new'"
        )
        .await,
        1
    );
    {
        use sn_store::Db;
        let mut conn = s.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE orders SET subscription_id = \
             (SELECT id FROM subscriptions WHERE remna_user_id = 601) \
             WHERE id = (SELECT order_id FROM legacy_invoices WHERE old_id = 'p6')",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let (monthly, request) = lava("charge-p6-1", Some("lava-new-1"));
    crate::legacy::invoices::settle(s, "lava", &monthly, &request)
        .await
        .unwrap()
        .expect("ежемесячное списание по счёту, оплаченному после переезда");
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM orders o JOIN payments p ON p.order_id = o.id \
                   WHERE p.provider = 'lava' AND o.kind = 'renewal' \
                   AND o.subscription_id = (SELECT id FROM subscriptions WHERE remna_user_id = 601)"
        )
        .await,
        3
    );

    let forged = crypto_paid("p5", "200.00", 7005);
    legacy_hook(&env, &forged, "00").await;
    assert_eq!(scalar(s, balance).await, 15_000);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM webhook_inbox WHERE legacy_path AND outcome = 'rejected_signature'").await,
        1
    );

    import(&env, &invoices_dump(true), Some(admin)).await;
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM legacy_invoices WHERE old_id = 'p5'"
        )
        .await,
        0,
        "счёт, оплаченный в старой панели, больше не ждёт оплаты"
    );
    let late = crypto_paid("p5", "200.00", 7006);
    legacy_hook(&env, &late, &sign(&late)).await;
    assert_eq!(scalar(s, balance).await, 15_000);
}

fn mail_dump(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut sql = String::from("-- Created at: 2026-11-17T09:00:00.000Z\n\n");
    let ids: Vec<String> = (0..pairs.len()).map(|i| format!("m{i}")).collect();
    let rows: Vec<[Option<&str>; 3]> = pairs
        .iter()
        .zip(&ids)
        .map(|((key, value), id)| [Some(id.as_str()), Some(*key), Some(*value)])
        .collect();
    let rows: Vec<&[Option<&str>]> = rows.iter().map(|r| r.as_slice()).collect();
    sql += &copy("system_settings", &["id", "key", "value"], &rows);
    sql += &copy(
        "clients",
        &["id", "role", "balance", "telegram_id", "telegram_username"],
        &[&[
            Some("k1"),
            Some("CLIENT"),
            Some("0"),
            Some("4001"),
            Some("payer"),
        ]],
    );
    sql.into_bytes()
}

const OLD_SMTP: &[(&str, &str)] = &[
    ("default_currency", "rub"),
    ("mail_provider", "smtp"),
    ("smtp_host", "smtp.example.com"),
    ("smtp_port", "465"),
    ("smtp_secure", "true"),
    ("smtp_user", "robot@example.com"),
    ("smtp_password", "app-pass-1"),
    ("smtp_from_email", "robot@example.com"),
    ("smtp_from_name", "Сервис"),
];

fn mail_config(services: &Services) -> (bool, sn_modules::ModuleConfig) {
    let modules = services.modules();
    (
        modules.is_enabled(sn_domain::module::ModuleId::Mail),
        modules.config(sn_domain::module::ModuleId::Mail),
    )
}

async fn set_mail(env: &Env, admin: AdminId, pairs: &[(&str, &str)]) {
    let config = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
        .collect();
    env.services
        .settings_ops()
        .update_module_config(
            sn_domain::module::ModuleId::Mail,
            &crate::settings::ModuleConfigUpdate {
                config,
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .unwrap();
    env.services.set_remna(Some(env.remna.clone()));
}

#[tokio::test]
async fn почта_старой_панели_переносится_и_не_затирает_настроенную() {
    use super::mail::MailPlan;
    let modules =
        ModuleRegistry::new(vec![Arc::new(sn_modules::catalog::mail::MailModule)]).unwrap();
    let Some(env) = env_with(modules).await else {
        return;
    };
    let admin = owner(&env.services).await;
    let s = &env.services;

    let (preview, outcome) = import(&env, &mail_dump(OLD_SMTP), Some(admin)).await;
    assert!(
        matches!(preview.mail, Some(MailPlan::Transfer { .. })),
        "{:?}",
        preview.mail
    );
    let issues = {
        let run = scalar(s, "SELECT max(id) FROM import_runs").await;
        let mut conn = s.store().conn().await.unwrap();
        runs::issues(&mut conn, run).await.unwrap()
    };
    assert_eq!(
        outcome.mail.as_deref(),
        Some("SMTP smtp.example.com:465 (TLS), отправитель robot@example.com"),
        "{issues:?}"
    );
    let (enabled, config) = mail_config(s);
    assert!(enabled, "почта работала в старой панели — модуль включён");
    assert_eq!(config.text("transport"), Some("smtp"));
    assert_eq!(config.text("security"), Some("tls"));
    assert_eq!(config.text("login"), Some("password"));
    assert_eq!(config.text("from_name"), Some("Сервис"));
    assert_eq!(config.text("password"), Some("app-pass-1"));
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM module_states WHERE module = 'mail' AND config ? 'password'"
        )
        .await,
        0,
        "пароль лежит только в зашифрованных секретах"
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM secrets WHERE key = 'modules.mail.password' \
             AND position(convert_to('app-pass-1', 'UTF8') in ciphertext) = 0"
        )
        .await,
        1
    );

    let (same, again) = import(&env, &mail_dump(OLD_SMTP), Some(admin)).await;
    assert!(
        matches!(same.mail, Some(MailPlan::Same { .. })),
        "{:?}",
        same.mail
    );
    assert_eq!(again.mail, None);

    set_mail(
        &env,
        admin,
        &[("smtp_host", "mail.other.com"), ("password", "other-pass")],
    )
    .await;
    let (differs, kept) = import(&env, &mail_dump(OLD_SMTP), Some(admin)).await;
    match &differs.mail {
        Some(MailPlan::Differs { current, incoming }) => {
            assert!(current.contains("mail.other.com"), "{current}");
            assert!(incoming.contains("smtp.example.com"), "{incoming}");
        }
        other => panic!("ожидался выбор оператора: {other:?}"),
    }
    assert_eq!(kept.mail, None);
    assert_eq!(
        mail_config(s).1.text("smtp_host"),
        Some("mail.other.com"),
        "без отметки настроенная почта не затирается"
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM import_issues WHERE code = 'mail_kept'"
        )
        .await,
        1
    );

    let resend: Vec<(&str, &str)> = vec![
        ("default_currency", "rub"),
        ("mail_provider", "resend"),
        ("resend_api_key", "re_test_1"),
        ("resend_from_email", "news@example.com"),
    ];
    let (_, replaced) = import_with(
        &env,
        &mail_dump(&resend),
        Some(admin),
        runs::StartOptions {
            replace_mail: true,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        replaced.mail.as_deref(),
        Some("Resend, отправитель news@example.com")
    );
    let (enabled, config) = mail_config(s);
    assert!(enabled);
    assert_eq!(config.text("transport"), Some("resend"));
    assert_eq!(config.text("resend_api_key"), Some("re_test_1"));
    assert_eq!(config.text("smtp_host"), None, "поля SMTP убраны");
    assert_eq!(config.text("password"), None);
    assert_eq!(config.text("from_name"), None);
    assert_eq!(
        config.text("login"),
        Some("password"),
        "выбор входа не тронут"
    );

    set_mail(&env, admin, &[("transport", "none")]).await;
    let (off, untouched) = import(&env, &mail_dump(&resend), Some(admin)).await;
    assert!(
        matches!(&off.mail, Some(MailPlan::Differs { current, .. }) if current == "отправка писем выключена"),
        "{:?}",
        off.mail
    );
    assert_eq!(untouched.mail, None);
    assert_eq!(
        mail_config(s).1.text("transport"),
        Some("none"),
        "выключенную после переноса почту повторный перенос не включает"
    );
}

fn tariff_rules_dump() -> Vec<u8> {
    let mut sql = String::from("-- Created at: 2026-11-17T09:00:00.000Z\n\n");
    sql += &copy(
        "system_settings",
        &["id", "key", "value"],
        &[&[Some("ss1"), Some("default_currency"), Some("rub")]],
    );
    sql += &copy(
        "tariff_categories",
        &["id", "name", "sort_order", "single_subscription_mode"],
        &[
            &[Some("cat1"), Some("Одна подписка"), Some("0"), Some("t")],
            &[Some("cat2"), Some("Сколько угодно"), Some("1"), Some("f")],
        ],
    );
    sql += &copy(
        "tariffs",
        &[
            "id",
            "category_id",
            "name",
            "duration_days",
            "price",
            "currency",
            "max_extra_devices",
            "price_per_extra_device",
            "device_discount_tiers",
        ],
        &[
            &[
                Some("t1"),
                Some("cat1"),
                Some("Месяц"),
                Some("30"),
                Some("300"),
                Some("rub"),
                Some("5"),
                Some("50"),
                Some(
                    r#"[{"minExtraDevices":2,"discountPercent":10},{"minExtraDevices":4,"discountPercent":20},{"minExtraDevices":3,"discountPercent":5},{"minExtraDevices":9,"discountPercent":30},{"minDevices":1,"discountPercent":0}]"#,
                ),
            ],
            &[
                Some("t2"),
                Some("cat2"),
                Some("Год"),
                Some("365"),
                Some("2000"),
                Some("rub"),
                Some("0"),
                None,
                Some(r#"[{"minExtraDevices":1,"discountPercent":10}]"#),
            ],
        ],
    );
    sql += &copy(
        "clients",
        &["id", "role", "balance", "telegram_id", "telegram_username"],
        &[&[
            Some("k1"),
            Some("CLIENT"),
            Some("0"),
            Some("4001"),
            Some("payer"),
        ]],
    );
    sql.into_bytes()
}

#[test]
fn лесенка_скидок_старой_панели_приводится_к_новой() {
    use sn_domain::tariff::DeviceDiscount;
    let step = |from, percent| DeviceDiscount { from, percent };
    let (ladder, dropped) = super::stealthnet::device_ladder(
        Some(serde_json::json!([
            {"minExtraDevices": 4, "discountPercent": 20},
            {"minExtraDevices": 2, "discountPercent": 9.6},
            {"minExtraDevices": 3, "discountPercent": 5},
            {"minDevices": 1, "discountPercent": 0},
            {"minExtraDevices": 9, "discountPercent": 30},
            {"minExtraDevices": "x", "discountPercent": 30}
        ])),
        5,
    );
    assert_eq!(ladder, vec![step(2, 10), step(4, 20)]);
    assert_eq!(dropped.len(), 4, "{dropped:?}");
    for expected in [
        "без числа устройств",
        "от 1 доп. устройств (0 %)",
        "от 3 доп. устройств (5 %)",
        "от 9 доп. устройств (30 %)",
    ] {
        assert!(
            dropped.iter().any(|d| d.contains(expected)),
            "{expected}: {dropped:?}"
        );
    }
    let (same, dropped) = super::stealthnet::device_ladder(
        Some(serde_json::json!([
            {"minExtraDevices": 2, "discountPercent": 10},
            {"minExtraDevices": 2, "discountPercent": 15}
        ])),
        5,
    );
    assert_eq!(same, vec![step(2, 15)]);
    assert!(
        dropped[0].contains("оставлена большая скидка"),
        "{dropped:?}"
    );
    let many: Vec<serde_json::Value> = (1..=12)
        .map(|n| serde_json::json!({"minExtraDevices": n, "discountPercent": n * 5}))
        .collect();
    let (ten, dropped) = super::stealthnet::device_ladder(Some(serde_json::Value::Array(many)), 20);
    assert_eq!(ten.len(), 10);
    assert_eq!(dropped.len(), 2, "{dropped:?}");
    let (text, _) = super::stealthnet::device_ladder(
        Some(serde_json::Value::String(
            r#"[{"minExtraDevices":1,"discountPercent":15}]"#.into(),
        )),
        3,
    );
    assert_eq!(text, vec![step(1, 15)]);
    assert!(super::stealthnet::device_ladder(None, 3).0.is_empty());
}

#[tokio::test]
async fn лимит_подписок_категории_и_скидки_за_устройства_переносятся() {
    use sn_domain::ids::{CategoryId, TariffId};
    use sn_domain::tariff::DeviceDiscount;
    use sn_store::Db;
    let Some(env) = env().await else { return };
    let admin = owner(&env.services).await;
    let s = &env.services;
    let (preview, _) = import(&env, &tariff_rules_dump(), Some(admin)).await;
    assert_eq!(preview.counts.tariffs, 2);
    let category = |old: &'static str| {
        format!(
            "SELECT new_id FROM legacy_ids WHERE entity = 'stealthnet:category' AND old_id = '{old}'"
        )
    };
    let one = CategoryId(scalar(s, &category("cat1")).await);
    let many = CategoryId(scalar(s, &category("cat2")).await);
    let month = TariffId(
        scalar(
            s,
            "SELECT new_id FROM legacy_ids WHERE entity = 'stealthnet:tariff' AND old_id = 't1'",
        )
        .await,
    );
    let year = TariffId(
        scalar(
            s,
            "SELECT new_id FROM legacy_ids WHERE entity = 'stealthnet:tariff' AND old_id = 't2'",
        )
        .await,
    );
    let mut conn = s.store().conn().await.unwrap();
    assert_eq!(
        conn.tariffs()
            .category(one)
            .await
            .unwrap()
            .subscription_limit,
        Some(1)
    );
    assert_eq!(
        conn.tariffs()
            .category(many)
            .await
            .unwrap()
            .subscription_limit,
        None
    );
    assert_eq!(
        conn.tariffs().get(month).await.unwrap().device_discounts,
        vec![
            DeviceDiscount {
                from: 2,
                percent: 10
            },
            DeviceDiscount {
                from: 4,
                percent: 20
            }
        ]
    );
    assert!(
        conn.tariffs()
            .get(year)
            .await
            .unwrap()
            .device_discounts
            .is_empty()
    );
    let run = scalar(s, "SELECT max(id) FROM import_runs").await;
    let issues = runs::issues(&mut conn, run).await.unwrap();
    assert!(
        issues
            .iter()
            .any(|i| i.code == "tariff_note" && i.details.contains("от 3 доп. устройств")),
        "{issues:?}"
    );
    assert!(
        issues
            .iter()
            .any(|i| i.code == "tariff_note" && i.details.contains("у тарифа нет доп. устройств")),
        "{issues:?}"
    );
    let limits: Vec<_> = issues
        .iter()
        .filter(|i| i.code == "category_limit")
        .map(|i| i.who.clone())
        .collect();
    assert_eq!(
        limits,
        vec![Some("Одна подписка".to_string())],
        "{issues:?}"
    );

    conn.tariffs()
        .set_subscription_limit(one, None)
        .await
        .unwrap();
    conn.tariffs()
        .set_device_discounts(month, &[])
        .await
        .unwrap();
    drop(conn);
    import(&env, &tariff_rules_dump(), Some(admin)).await;
    let mut conn = s.store().conn().await.unwrap();
    assert_eq!(
        conn.tariffs()
            .category(one)
            .await
            .unwrap()
            .subscription_limit,
        None,
        "снятый оператором лимит повторный перенос не возвращает"
    );
    assert!(
        conn.tariffs()
            .get(month)
            .await
            .unwrap()
            .device_discounts
            .is_empty()
    );

    sqlx::query("UPDATE legacy_ids SET data = '{}' WHERE entity IN ('stealthnet:category', 'stealthnet:tariff')")
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    import(&env, &tariff_rules_dump(), Some(admin)).await;
    let mut conn = s.store().conn().await.unwrap();
    assert_eq!(
        conn.tariffs()
            .category(one)
            .await
            .unwrap()
            .subscription_limit,
        Some(1),
        "категории, перенесённые до этих правил, получают лимит при повторном переносе"
    );
    assert_eq!(
        conn.tariffs()
            .get(month)
            .await
            .unwrap()
            .device_discounts
            .len(),
        2
    );
}

#[tokio::test]
async fn почта_без_сервера_переносится_частично_и_не_включается() {
    use super::mail::MailPlan;
    let modules =
        ModuleRegistry::new(vec![Arc::new(sn_modules::catalog::mail::MailModule)]).unwrap();
    let Some(env) = env_with(modules).await else {
        return;
    };
    let admin = owner(&env.services).await;
    let s = &env.services;
    let half: &[(&str, &str)] = &[
        ("default_currency", "rub"),
        ("mail_provider", "smtp"),
        ("smtp_port", "587"),
        ("smtp_secure", "false"),
        ("smtp_from_email", "robot@example.com"),
        ("smtp_from_name", "Сервис"),
    ];
    let (preview, outcome) = import(&env, &mail_dump(half), Some(admin)).await;
    match &preview.mail {
        Some(MailPlan::Partial { incoming, reason }) => {
            assert_eq!(incoming, "адрес отправителя, имя отправителя");
            assert_eq!(reason, "не задан сервер SMTP");
        }
        other => panic!("ожидался частичный перенос: {other:?}"),
    }
    assert_eq!(outcome.mail, None);
    let (_, config) = mail_config(s);
    assert_eq!(config.text("from_email"), Some("robot@example.com"));
    assert_eq!(config.text("from_name"), Some("Сервис"));
    assert!(
        !sn_modules::catalog::mail::MailSettings::parse(&config)
            .unwrap()
            .sends(),
        "без сервера отправка не включается"
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM import_issues WHERE code = 'mail_partial'"
        )
        .await,
        1
    );
    let (again, _) = import(&env, &mail_dump(half), Some(admin)).await;
    assert_eq!(again.mail, None, "частичный перенос не повторяется");
}

fn multi_dump(setting: Option<&str>, second: bool) -> Vec<u8> {
    let mut sql = String::from("-- Created at: 2026-11-17T09:00:00.000Z\n\n");
    let mut settings: Vec<[Option<&str>; 3]> = vec![
        [Some("ss1"), Some("default_currency"), Some("rub")],
        [Some("ss3"), Some("max_additional_subscriptions"), Some("5")],
    ];
    if let Some(value) = setting {
        settings.push([
            Some("ss2"),
            Some("multi_subscriptions_enabled"),
            Some(value),
        ]);
    }
    let rows: Vec<&[Option<&str>]> = settings.iter().map(|r| r.as_slice()).collect();
    sql += &copy("system_settings", &["id", "key", "value"], &rows);
    sql += &copy(
        "clients",
        &["id", "role", "balance", "telegram_id", "telegram_username"],
        &[&[
            Some("k1"),
            Some("CLIENT"),
            Some("0"),
            Some("4001"),
            Some("payer"),
        ]],
    );
    let mut subs: Vec<[Option<&str>; 5]> = vec![[
        Some("s1"),
        Some("k1"),
        Some("601"),
        Some("0"),
        Some("2026-12-20 00:00:00"),
    ]];
    if second {
        subs.push([
            Some("s2"),
            Some("k1"),
            Some("602"),
            Some("1"),
            Some("2026-12-21 00:00:00"),
        ]);
    }
    let rows: Vec<&[Option<&str>]> = subs.iter().map(|r| r.as_slice()).collect();
    sql += &copy(
        "subscriptions",
        &[
            "id",
            "owner_id",
            "remnawave_uuid",
            "subscription_index",
            "expire_at",
        ],
        &rows,
    );
    sql.into_bytes()
}

async fn last_codes(services: &Services) -> Vec<String> {
    let run = scalar(services, "SELECT max(id) FROM import_runs").await;
    let mut conn = services.store().conn().await.unwrap();
    runs::issues(&mut conn, run)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.code)
        .collect()
}

async fn set_multi(env: &Env, admin: AdminId, enabled: bool) {
    env.services
        .settings_ops()
        .update_multi_subscriptions(&sn_domain::settings::MultiSubscriptions { enabled }, admin)
        .await
        .unwrap();
    env.services.set_remna(Some(env.remna.clone()));
}

#[tokio::test]
async fn несколько_подписок_старой_панели_включают_режим() {
    use super::multi::MultiPlan;
    let Some(env) = env().await else { return };
    let admin = owner(&env.services).await;
    let s = &env.services;
    let (preview, outcome) = import(&env, &multi_dump(Some("true"), false), Some(admin)).await;
    assert_eq!(
        preview.multi,
        Some(MultiPlan::Enable {
            setting: true,
            clients: 0
        })
    );
    assert!(outcome.multi);
    assert!(s.settings().multi_subscriptions);
    let codes = last_codes(s).await;
    assert!(codes.iter().any(|c| c == "multi_gift_limit"), "{codes:?}");
    let (same, again) = import(&env, &multi_dump(Some("true"), false), Some(admin)).await;
    assert_eq!(same.multi, Some(MultiPlan::Same));
    assert!(!again.multi);

    let Some(env) = super::tests::env().await else {
        return;
    };
    let admin = owner(&env.services).await;
    let (single, _) = import(&env, &multi_dump(Some("false"), false), Some(admin)).await;
    assert_eq!(
        single.multi, None,
        "одна подписка в старой панели — менять нечего"
    );
    assert!(!env.services.settings().multi_subscriptions);
    let (many, done) = import(&env, &multi_dump(None, true), Some(admin)).await;
    assert_eq!(
        many.multi,
        Some(MultiPlan::Enable {
            setting: false,
            clients: 1
        }),
        "у клиента две подписки — режим включается и без настройки"
    );
    assert!(done.multi);
}

#[tokio::test]
async fn выбранную_одну_подписку_перенос_меняет_только_с_отметкой() {
    use super::multi::MultiPlan;
    let Some(env) = env().await else { return };
    let admin = owner(&env.services).await;
    let s = &env.services;
    set_multi(&env, admin, false).await;
    let (preview, outcome) = import(&env, &multi_dump(None, true), Some(admin)).await;
    assert_eq!(
        preview.multi,
        Some(MultiPlan::Differs {
            setting: false,
            clients: 1,
            recommended: true
        })
    );
    assert!(!outcome.multi);
    assert!(
        !s.settings().multi_subscriptions,
        "без отметки не затирается"
    );
    assert!(last_codes(s).await.iter().any(|c| c == "multi_kept"));

    let (_, replaced) = import_with(
        &env,
        &multi_dump(None, true),
        Some(admin),
        runs::StartOptions {
            replace_multi: true,
            ..Default::default()
        },
    )
    .await;
    assert!(replaced.multi);
    assert!(s.settings().multi_subscriptions);

    set_multi(&env, admin, false).await;
    let (off, _) = import(&env, &multi_dump(None, true), Some(admin)).await;
    assert_eq!(
        off.multi,
        Some(MultiPlan::Differs {
            setting: false,
            clients: 1,
            recommended: false
        }),
        "выключенное после переноса оператор выключил сам — по умолчанию не включаем"
    );
    assert!(!s.settings().multi_subscriptions);
}

#[test]
fn при_запуске_из_папки_переноса_удаляются_только_её_временные_файлы() {
    let id = "0123456789abcdef0123456789abcdef";
    for own in [
        format!("{id}.upload"),
        format!("{id}.inner"),
        format!("sqlite-{id}.part"),
    ] {
        assert!(super::own_temp_file(&own), "{own}");
    }
    for foreign in [
        "stealthnet-2026-11-19T04-00-00Z.snbk".to_string(),
        "backup.sql".to_string(),
        format!("{id}.snbk"),
        id.to_string(),
        format!("{}.upload", &id[..31]),
        format!("{}.upload", id.to_uppercase()),
        format!("x{id}.upload"),
        format!("{id}.upload.inner"),
    ] {
        assert!(!super::own_temp_file(&foreign), "{foreign}");
    }
}
